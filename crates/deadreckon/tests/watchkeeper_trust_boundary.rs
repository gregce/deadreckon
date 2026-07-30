#![allow(clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::process::Command;
#[cfg(unix)]
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use deadreckon_core::{DeadreckonPaths, GATE_CONTAINED_ENV, GATE_SANDBOX_BACKEND_ENV, JobView};
#[cfg(target_os = "macos")]
use deadreckon_core::{
    SupervisedProcessPhase, gate_key_path, load_run, read_supervised_process,
    read_supervised_process_record, validate_acceptance_marker,
};
use deadreckon_protocol::{JobEventKind, JobOutcome};
use serde_json::Value;
use tempfile::TempDir;

#[cfg(unix)]
#[test]
fn guarded_exec_does_not_run_when_parent_pipe_closes_before_identity_is_durable() {
    let temp = TempDir::new().expect("tempdir");
    let metadata = temp.path().join("missing-process-record.json");
    let sentinel = temp.path().join("must-not-run");
    let token = "launch:private-release";
    let digest = deadreckon_core::flight::sha256_text(token);
    let mut child = Command::new(env!("CARGO_BIN_EXE_dr-gate"))
        .args([
            "guarded-exec",
            "--metadata",
            metadata.to_str().expect("metadata path"),
            "--launch-id",
            "launch-before-identity",
            "--attempt",
            "1",
            "--release-token-sha256",
            &digest,
            "--",
            "/bin/sh",
            "-c",
            &format!(": > '{}'", sentinel.display()),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("guarded helper");

    // Dropping the sole release pipe is the kernel-level consequence of a
    // worker SIGKILL in the spawn-before-sidecar window.
    drop(child.stdin.take());
    let status = child.wait().expect("guarded helper exit");

    assert!(!status.success());
    assert!(!metadata.exists());
    assert!(
        !sentinel.exists(),
        "repository-controlled command ran before durable identity"
    );
}

#[test]
fn strict_public_start_refuses_none_despite_poisoned_legacy_gate_environment() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        "default_provider = \"smoke\"\n\n[defaults]\nsandbox = \"none\"\n",
    )
    .expect("config");

    let launch = public_deadreckon()
        .current_dir(&workspace)
        .env("DEADRECKON_HOME", paths.home())
        .env(GATE_CONTAINED_ENV, "true")
        .env(GATE_SANDBOX_BACKEND_ENV, "sandbox-exec")
        .args([
            "start",
            "Prove a strict durable Job cannot use an uncontained deterministic gate.",
            "--mode",
            "run",
            "--provider",
            "smoke",
            "--fresh",
            "--max-spend",
            "1",
            "--yes",
            "--plain",
            "--json",
        ])
        .output()
        .expect("public start");

    assert!(
        !launch.status.success(),
        "poisoned legacy containment inputs must not turn sandbox none into a trusted Job\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&launch.stdout),
        String::from_utf8_lossy(&launch.stderr)
    );
    let stderr = String::from_utf8_lossy(&launch.stderr);
    assert!(
        stderr.contains("durable Jobs require containment")
            && stderr.contains("sandbox `none` cannot be frozen"),
        "{stderr}"
    );
    assert!(
        directory_is_absent_or_empty(&paths.jobs_dir()),
        "the refused start allocated Job control state"
    );
    assert!(
        directory_is_absent_or_empty(&paths.home().join("gate-keys")),
        "the refused start allocated signing material"
    );
    assert!(
        !tree_contains_file_named(paths.home(), "turn-acceptance.json"),
        "the refused start produced a gate marker"
    );
    assert!(
        !tree_contains_file_named(paths.home(), "receipt.json"),
        "the refused start produced a completion receipt"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn sandboxed_public_gate_denies_control_tampering_and_reaps_delayed_checks_before_signing() {
    if !sandbox_exec_available() {
        eprintln!("skipping: sandbox-exec is unavailable on this macOS host");
        return;
    }

    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(workspace.join(".deadreckon")).expect("acceptance dir");
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        "default_provider = \"smoke\"\n\n[defaults]\nsandbox = \"sandbox-exec\"\n",
    )
    .expect("config");
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        gate_boundary_acceptance(paths.home()),
    )
    .expect("acceptance");
    fs::write(workspace.join("README.md"), "gate boundary fixture\n").expect("readme");
    git(&workspace, &["init", "--initial-branch=main"]);
    git(
        &workspace,
        &["config", "user.email", "watchkeeper@example.invalid"],
    );
    git(&workspace, &["config", "user.name", "Watchkeeper Test"]);
    git(&workspace, &["add", "-A"]);
    git(&workspace, &["commit", "-m", "fixture"]);

    let launch = public_deadreckon()
        .current_dir(&workspace)
        .env("DEADRECKON_HOME", paths.home())
        .env(GATE_CONTAINED_ENV, "poisoned")
        .env(GATE_SANDBOX_BACKEND_ENV, "none")
        .args([
            "start",
            "Run the approved sandbox gate boundary shell test.",
            "--mode",
            "run",
            "--provider",
            "smoke",
            "--worktree",
            "--max-spend",
            "1",
            "--yes",
            "--plain",
            "--json",
        ])
        .output()
        .expect("public start");
    assert!(
        launch.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&launch.stdout),
        String::from_utf8_lossy(&launch.stderr)
    );
    let envelope: Value = serde_json::from_slice(&launch.stdout).expect("launch JSON");
    let job_id = envelope["dispatched"]["ids"][0]
        .as_str()
        .expect("dispatched Job ID");

    let view = wait_for_terminal_job(&paths, job_id, Duration::from_secs(60));
    assert_eq!(
        view.projection.outcome,
        Some(JobOutcome::NeedsReview),
        "the deterministic gate should pass before the scripted semantic judge fails closed"
    );
    let state = load_run(&paths, job_id).expect("Job result run");
    assert!(
        gate_key_path(&paths, job_id).is_file(),
        "the exact key-read probe must target real signing material"
    );
    let marker = validate_acceptance_marker(&state).expect("trusted gate marker");
    assert!(marker.is_native_gate_proof(), "{marker:#?}");
    assert!(marker.contained, "{marker:#?}");
    assert_eq!(
        marker.sandbox_backend, "sandbox-exec",
        "the signer must record the backend observed by the sandbox runner"
    );
    assert_eq!(marker.check_count, 1, "{marker:#?}");
    assert!(
        marker.checks.iter().all(|check| check.passed),
        "{marker:#?}"
    );
    assert!(
        !paths.job_receipt(job_id).exists(),
        "deterministic proof alone must not become a strict completion receipt"
    );

    let sentinel = state.working_dir.join("delayed-gate-sentinel");
    assert!(
        !sentinel.exists(),
        "the delayed acceptance descendant escaped before signing"
    );
    thread::sleep(Duration::from_millis(1_250));
    assert!(
        !sentinel.exists(),
        "the evaluator left a delayed acceptance descendant alive after signing"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn public_cancel_reaps_a_running_guarded_gate_before_job_becomes_cancelled() {
    if !sandbox_exec_available() {
        eprintln!("skipping: sandbox-exec is unavailable on this macOS host");
        return;
    }

    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(workspace.join(".deadreckon")).expect("acceptance dir");
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        "default_provider = \"smoke\"\n\n[defaults]\nsandbox = \"sandbox-exec\"\n",
    )
    .expect("config");
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        r#"name: cancellable guarded gate
checks:
  - kind: shell
    command: |
      set -eu
      : > "$PWD/gate-ready"
      while :; do sleep 1; done
    cwd: "{working_dir}"
"#,
    )
    .expect("acceptance");
    fs::write(workspace.join("README.md"), "cancel fixture\n").expect("readme");
    git(&workspace, &["init", "--initial-branch=main"]);
    git(
        &workspace,
        &["config", "user.email", "watchkeeper@example.invalid"],
    );
    git(&workspace, &["config", "user.name", "Watchkeeper Test"]);
    git(&workspace, &["add", "-A"]);
    git(&workspace, &["commit", "-m", "fixture"]);

    let launch = public_deadreckon()
        .current_dir(&workspace)
        .env("DEADRECKON_HOME", paths.home())
        .args([
            "start",
            "Enter the approved cancellable deterministic gate.",
            "--mode",
            "run",
            "--provider",
            "smoke",
            "--worktree",
            "--max-spend",
            "1",
            "--yes",
            "--plain",
            "--json",
        ])
        .output()
        .expect("public start");
    assert!(
        launch.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&launch.stdout),
        String::from_utf8_lossy(&launch.stderr)
    );
    let envelope: Value = serde_json::from_slice(&launch.stdout).expect("launch JSON");
    let job_id = envelope["dispatched"]["ids"][0]
        .as_str()
        .expect("dispatched Job ID");
    let state = wait_for_run(&paths, job_id, Duration::from_secs(20));
    wait_for_job_path(
        &paths,
        job_id,
        &state.working_dir.join("gate-ready"),
        Duration::from_secs(30),
    );
    let record_path = wait_for_gate_record(&state.run_root, Duration::from_secs(10));
    let record = read_supervised_process_record(&record_path).expect("guarded evaluator record");
    assert_eq!(record.phase, SupervisedProcessPhase::Running);
    assert_eq!(record.process.pgid, Some(record.process.pid));
    assert_eq!(record.owner_launch_id.as_deref().map(str::len), Some(36));

    let cancelled = public_deadreckon()
        .current_dir(&workspace)
        .env("DEADRECKON_HOME", paths.home())
        .args(["kill", job_id, "--escalate", "--plain"])
        .output()
        .expect("public cancel");
    assert!(
        cancelled.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&cancelled.stdout),
        String::from_utf8_lossy(&cancelled.stderr)
    );

    let view = wait_for_terminal_job(&paths, job_id, Duration::from_secs(30));
    assert_eq!(view.projection.outcome, Some(JobOutcome::Cancelled));
    wait_for_process_group_exit(record.process.pid, Duration::from_secs(10));
    assert!(
        !record_path.exists(),
        "evaluator record survived cancellation"
    );
    assert!(
        !deadreckon_core::marker_path_for_run_root(&state.run_root).exists(),
        "cancelled gate wrote a deterministic marker"
    );
    assert!(
        !paths.job_receipt(job_id).exists(),
        "cancelled gate wrote a completion receipt"
    );
    let history =
        deadreckon_core::read_job_history(&paths.job_events(job_id)).expect("job history");
    let cancel_index = history
        .events()
        .iter()
        .position(|event| event.kind == JobEventKind::CancelRequested)
        .expect("CancelRequested");
    assert!(
        history.events()[cancel_index + 1..]
            .iter()
            .all(|event| event.kind != JobEventKind::RetryScheduled),
        "a retry was scheduled after operator cancellation"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn supervisor_reaps_an_orphaned_gate_before_retrying_the_same_job() {
    if !sandbox_exec_available() {
        eprintln!("skipping: sandbox-exec is unavailable on this macOS host");
        return;
    }

    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(workspace.join(".deadreckon")).expect("acceptance dir");
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        "default_provider = \"smoke\"\n\n[defaults]\nsandbox = \"sandbox-exec\"\n",
    )
    .expect("config");
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        r#"name: recoverable guarded gate
checks:
  - kind: shell
    command: |
      set -eu
      if test ! -f "$PWD/first-gate-attempt"; then
        : > "$PWD/first-gate-attempt"
        : > "$PWD/gate-ready"
        while :; do sleep 1; done
      fi
    cwd: "{working_dir}"
"#,
    )
    .expect("acceptance");
    fs::write(workspace.join("README.md"), "recovery fixture\n").expect("readme");
    git(&workspace, &["init", "--initial-branch=main"]);
    git(
        &workspace,
        &["config", "user.email", "watchkeeper@example.invalid"],
    );
    git(&workspace, &["config", "user.name", "Watchkeeper Test"]);
    git(&workspace, &["add", "-A"]);
    git(&workspace, &["commit", "-m", "fixture"]);

    let launch = public_deadreckon()
        .current_dir(&workspace)
        .env("DEADRECKON_HOME", paths.home())
        .args([
            "start",
            "Recover the approved deterministic gate after one launcher crash.",
            "--mode",
            "run",
            "--provider",
            "smoke",
            "--worktree",
            "--max-spend",
            "1",
            "--yes",
            "--plain",
            "--json",
        ])
        .output()
        .expect("public start");
    assert!(
        launch.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&launch.stdout),
        String::from_utf8_lossy(&launch.stderr)
    );
    let envelope: Value = serde_json::from_slice(&launch.stdout).expect("launch JSON");
    let job_id = envelope["dispatched"]["ids"][0]
        .as_str()
        .expect("dispatched Job ID");
    let state = wait_for_run(&paths, job_id, Duration::from_secs(20));
    wait_for_job_path(
        &paths,
        job_id,
        &state.working_dir.join("gate-ready"),
        Duration::from_secs(30),
    );
    let first_record_path = wait_for_gate_record(&state.run_root, Duration::from_secs(10));
    let first_record =
        read_supervised_process_record(&first_record_path).expect("first evaluator record");
    let outer = read_supervised_process(&paths.job_dir(job_id).join("supervised-child.json"))
        .expect("outer supervised launcher");
    signal_pid(outer.pid, nix::sys::signal::Signal::SIGKILL);

    let deadline = Instant::now() + Duration::from_secs(45);
    let view = loop {
        let first_alive = process_group_is_alive(first_record.process.pid);
        for record_path in gate_record_paths(&state.run_root) {
            if record_path != first_record_path && first_alive {
                panic!(
                    "retry evaluator {} started while old evaluator group {} remained alive",
                    record_path.display(),
                    first_record.process.pid
                );
            }
        }
        if let Ok(view) = JobView::load(&paths, job_id)
            && view.projection.is_terminal()
        {
            break view;
        }
        assert!(
            Instant::now() < deadline,
            "Job {job_id} did not recover after launcher SIGKILL"
        );
        thread::sleep(Duration::from_millis(25));
    };

    assert_eq!(view.projection.outcome, Some(JobOutcome::NeedsReview));
    wait_for_process_group_exit(first_record.process.pid, Duration::from_secs(10));
    assert!(
        !first_record_path.exists(),
        "first evaluator record survived retry reconciliation"
    );
    let history =
        deadreckon_core::read_job_history(&paths.job_events(job_id)).expect("job history");
    assert_eq!(
        history
            .events()
            .iter()
            .filter(|event| event.kind == JobEventKind::RetryScheduled)
            .count(),
        1,
        "the interrupted attempt should schedule exactly one bounded retry"
    );
    assert!(
        deadreckon_core::marker_path_for_run_root(&state.run_root).exists(),
        "the second attempt did not complete deterministic verification"
    );
    assert!(
        !paths.job_receipt(job_id).exists(),
        "the scripted semantic fixture must not issue a completion receipt"
    );
}

#[test]
fn public_smoke_job_can_never_issue_a_trusted_completion_receipt() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");

    let launch = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&workspace)
        .env("DEADRECKON_HOME", paths.home())
        .args([
            "start",
            "Create the deterministic smoke project and satisfy its checks.",
            "--mode",
            "run",
            "--provider",
            "smoke",
            "--fresh",
            "--max-spend",
            "1",
            "--yes",
            "--plain",
            "--json",
        ])
        .output()
        .expect("public start");
    assert!(
        launch.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&launch.stdout),
        String::from_utf8_lossy(&launch.stderr)
    );
    let envelope: Value = serde_json::from_slice(&launch.stdout).expect("launch JSON");
    let ids = envelope["dispatched"]["ids"]
        .as_array()
        .expect("dispatched ids");
    assert_eq!(ids.len(), 1, "{envelope}");
    let job_id = ids[0].as_str().expect("job id");

    let deadline = Instant::now() + Duration::from_secs(20);
    let view = loop {
        if let Ok(view) = JobView::load(&paths, job_id)
            && view.projection.is_terminal()
        {
            break view;
        }
        assert!(
            Instant::now() < deadline,
            "public smoke Job {job_id} did not reach a bounded terminal state"
        );
        thread::sleep(Duration::from_millis(50));
    };

    assert_ne!(
        view.projection.outcome,
        Some(JobOutcome::Verified),
        "a scripted transport fixture cannot independently verify meaning"
    );
    assert!(
        !paths.job_receipt(job_id).exists(),
        "an untrusted smoke judgment must never produce a completion receipt"
    );
    let history =
        deadreckon_core::read_job_history(&paths.job_events(job_id)).expect("job history");
    assert!(
        history.events().iter().all(|event| {
            !matches!(
                event.kind,
                JobEventKind::SemanticJudgeAchieved | JobEventKind::Verified
            )
        }),
        "the append-only lifecycle must not claim semantic achievement or verification"
    );
}

fn public_deadreckon() -> Command {
    let deadreckon = Path::new(env!("CARGO_BIN_EXE_deadreckon"));
    let gate = Path::new(env!("CARGO_BIN_EXE_dr-gate"));
    assert_eq!(
        deadreckon.parent(),
        gate.parent(),
        "the public deadreckon fixture must use its real sibling dr-gate"
    );
    Command::new(deadreckon)
}

fn directory_is_absent_or_empty(path: &Path) -> bool {
    match fs::read_dir(path) {
        Ok(mut entries) => entries.next().is_none(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => panic!("read {}: {error}", path.display()),
    }
}

fn tree_contains_file_named(root: &Path, expected: &str) -> bool {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => panic!("read {}: {error}", directory.display()),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = entry.file_type().expect("file type");
            if file_type.is_dir() {
                pending.push(path);
            } else if entry.file_name() == expected {
                return true;
            }
        }
    }
    false
}

fn wait_for_terminal_job(paths: &DeadreckonPaths, job_id: &str, timeout: Duration) -> JobView {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(view) = JobView::load(paths, job_id)
            && view.projection.is_terminal()
        {
            return view;
        }
        assert!(
            Instant::now() < deadline,
            "public Job {job_id} did not reach a bounded terminal state\nsupervisor stderr:\n{}",
            fs::read_to_string(paths.job_dir(job_id).join("supervisor-stderr.log"))
                .unwrap_or_default()
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(target_os = "macos")]
fn wait_for_run(
    paths: &DeadreckonPaths,
    job_id: &str,
    timeout: Duration,
) -> deadreckon_core::PipelineState {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(state) = load_run(paths, job_id) {
            return state;
        }
        assert!(Instant::now() < deadline, "run {job_id} was not created");
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(target_os = "macos")]
fn wait_for_job_path(paths: &DeadreckonPaths, job_id: &str, path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        if let Ok(view) = JobView::load(paths, job_id)
            && view.projection.is_terminal()
        {
            let state = load_run(paths, job_id).ok();
            panic!(
                "{} was not created before Job terminal {:?}; state: {state:#?}; supervisor stderr: {}",
                path.display(),
                view.projection.outcome,
                fs::read_to_string(paths.job_dir(job_id).join("supervisor.err"))
                    .unwrap_or_default()
            );
        }
        assert!(
            Instant::now() < deadline,
            "{} was not created",
            path.display()
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(target_os = "macos")]
fn wait_for_gate_record(run_root: &Path, timeout: Duration) -> std::path::PathBuf {
    let directory = run_root.join("child-pids");
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(entries) = fs::read_dir(&directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("dr-gate-evaluate-")
                    && read_supervised_process_record(&path)
                        .is_ok_and(|record| record.phase == SupervisedProcessPhase::Running)
                {
                    return path;
                }
            }
        }
        assert!(
            Instant::now() < deadline,
            "guarded evaluator record was not created"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(target_os = "macos")]
fn gate_record_paths(run_root: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = fs::read_dir(run_root.join("child-pids")) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("dr-gate-evaluate-"))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn wait_for_process_group_exit(pgid: u32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while process_group_is_alive(pgid) {
        assert!(
            Instant::now() < deadline,
            "evaluator process group {pgid} remained alive"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(target_os = "macos")]
fn process_group_is_alive(pgid: u32) -> bool {
    use nix::errno::Errno;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let target = Pid::from_raw(-i32::try_from(pgid).expect("pgid"));
    match kill(target, None) {
        Ok(()) => true,
        Err(Errno::ESRCH) | Err(Errno::EPERM) => false,
        Err(error) => panic!("inspect process group {pgid}: {error}"),
    }
}

#[cfg(target_os = "macos")]
fn signal_pid(pid: u32, signal: nix::sys::signal::Signal) {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    kill(
        Pid::from_raw(i32::try_from(pid).expect("pid")),
        Some(signal),
    )
    .expect("signal process");
}

#[cfg(target_os = "macos")]
fn sandbox_exec_available() -> bool {
    Command::new("sandbox-exec")
        .args(["-p", "(version 1)\n(allow default)", "--", "/usr/bin/true"])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "macos")]
fn gate_boundary_acceptance(deadreckon_home: &Path) -> String {
    let deadreckon_home = shell_quote(deadreckon_home);
    format!(
        r#"name: sandbox gate boundary shell test
checks:
  - kind: shell
    command: |
      set -eu
      test -z "${{{contained}+present}}"
      test -z "${{{backend}+present}}"
      deadreckon_home={deadreckon_home}
      job_control=$(find "$deadreckon_home/jobs" -mindepth 2 -maxdepth 2 -name job.json -print)
      test "$(printf '%s\n' "$job_control" | sed '/^$/d' | wc -l | tr -d ' ')" = 1
      job_dir=$(dirname "$job_control")
      run_id=$(basename "$job_dir")
      run_root=$(find "$deadreckon_home/runstate" -type d -path "*/runs/$run_id" -print -quit)
      test -n "$run_root"
      gate_key="$deadreckon_home/gate-keys/$run_id.key"
      gate_control="$run_root/gate/forged"
      proof_control="$run_root/proofs/turn-acceptance.json"
      git_control="$PWD/.git"
      test -f "$job_control"
      test -d "$run_root/gate"
      test -f "$git_control"
      if cat "$gate_key" >/dev/null 2>&1; then exit 31; fi
      if printf tampered >>"$job_control"; then exit 32; fi
      if printf tampered >>"$gate_control"; then exit 33; fi
      if printf forged >"$proof_control"; then exit 34; fi
      if printf tampered >>"$git_control"; then exit 35; fi
      (sleep 1; printf escaped >"$PWD/delayed-gate-sentinel") </dev/null >/dev/null 2>&1 &
    cwd: "{{working_dir}}"
"#,
        contained = GATE_CONTAINED_ENV,
        backend = GATE_SANDBOX_BACKEND_ENV,
        deadreckon_home = deadreckon_home,
    )
}

#[cfg(target_os = "macos")]
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\"'\"'"))
}

#[cfg(target_os = "macos")]
fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
