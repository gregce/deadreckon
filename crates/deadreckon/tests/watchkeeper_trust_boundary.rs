#![allow(clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::process::Command;
#[cfg(unix)]
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use deadreckon_core::{DeadreckonPaths, GATE_CONTAINED_ENV, GATE_SANDBOX_BACKEND_ENV, JobView};
#[cfg(unix)]
use deadreckon_core::{
    SupervisedProcessPhase, SupervisedProcessRecord, gate_key_path, load_run,
    read_supervised_process, read_supervised_process_record, validate_acceptance_marker,
    validate_sandbox_boundary_observation,
};
use deadreckon_protocol::{JobAuthority, JobEventKind, JobOutcome};
use serde_json::Value;
use tempfile::TempDir;

mod common;

use common::SupervisorServiceFixture;

fn write_smoke_acceptance(workspace: &Path) {
    fs::create_dir_all(workspace.join(".deadreckon")).expect("acceptance directory");
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        "name: trust boundary smoke\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/Cargo.toml\"\n",
    )
    .expect("acceptance contract");
}

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
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn strict_public_start_refuses_none_despite_poisoned_legacy_gate_environment() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    write_smoke_acceptance(&workspace);
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        "name: strict durable Job cannot use an uncontained deterministic gate\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/Cargo.toml\"\n",
    )
    .expect("goal-specific acceptance contract");
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        "default_provider = \"smoke\"\n\n[defaults]\nsandbox = \"none\"\n",
    )
    .expect("config");
    let service = SupervisorServiceFixture::configured(&paths);

    let launch = service
        .deadreckon()
        .current_dir(&workspace)
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
    let service = SupervisorServiceFixture::configured(&paths);
    let host_home = service.user_home().to_path_buf();
    let host_secret = host_home.join(".aws/credentials");
    let outside_write = temp.path().join("gate-host-write");
    let shim_bin = temp.path().join("shim-bin");
    let shim_sentinel = temp.path().join("path-shim-ran");
    fs::create_dir_all(workspace.join(".deadreckon")).expect("acceptance dir");
    fs::create_dir_all(paths.home()).expect("home");
    fs::create_dir_all(host_secret.parent().expect("secret parent")).expect("host secret parent");
    fs::create_dir_all(&shim_bin).expect("shim bin");
    fs::write(&host_secret, "WATCHKEEPER_HOST_SECRET_MUST_NOT_LEAK\n").expect("host secret");
    let shim = shim_bin.join("sandbox-exec");
    fs::write(
        &shim,
        format!(
            "#!/bin/sh\nprintf shim-ran >{}\nexec /usr/bin/sandbox-exec \"$@\"\n",
            shell_quote(&shim_sentinel)
        ),
    )
    .expect("sandbox-exec shim");
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(&shim).expect("shim metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&shim, permissions).expect("shim permissions");
    }
    fs::write(
        paths.config_path(),
        "default_provider = \"smoke\"\n\n[defaults]\nsandbox = \"sandbox-exec\"\n",
    )
    .expect("config");
    fs::write(
        workspace.join(".deadreckon/acceptance.yaml"),
        gate_boundary_acceptance(paths.home(), &host_secret, &outside_write),
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

    let launch = service
        .deadreckon()
        .current_dir(&workspace)
        .env("PATH", format!("{}:{}", shim_bin.display(), service.path()))
        .env(
            "DEADRECKON_FAKE_SECRET",
            "WATCHKEEPER_AMBIENT_SECRET_MUST_NOT_LEAK",
        )
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

    // Projection materialization and the sandbox boundary probe can contend
    // with the other process-heavy Watchkeeper cases in the full workspace
    // suite. Keep the run bounded without turning parallel host load into a
    // false trust-boundary failure.
    let view = wait_for_terminal_job(&paths, job_id, Duration::from_secs(120));
    assert_eq!(
        view.projection.outcome,
        Some(JobOutcome::NeedsReview),
        "the deterministic gate should pass before the scripted semantic judge fails closed\n\
         view: {view:#?}\n\
         supervisor stderr:\n{}",
        fs::read_to_string(paths.job_dir(job_id).join("supervisor.err")).unwrap_or_default()
    );
    let state = load_run(&paths, job_id).expect("Job result run");
    assert!(
        gate_key_path(&paths, job_id).is_file(),
        "strict signing material must exist outside the evaluator-visible workspace"
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
    let authority_path = paths.job_authority(job_id);
    let authority: JobAuthority =
        serde_json::from_slice(&fs::read(&authority_path).expect("Job authority"))
            .expect("Job authority JSON");
    let observation =
        validate_sandbox_boundary_observation(&paths, &state, &authority, "sandbox-exec")
            .expect("controller-signed sandbox observation");
    assert!(observation.contained);
    assert!(observation.gate_key_read_denied);
    assert!(observation.proof_write_denied);
    assert!(observation.control_write_denied);
    assert!(observation.operator_capture_write_denied);
    assert!(observation.operator_capture_read_denied);
    assert!(observation.signing_env_scrubbed);
    assert_eq!(observation.sandbox_backend, marker.sandbox_backend);
    let marker_json = serde_json::to_string(&marker).expect("marker JSON");
    assert!(
        !marker_json.contains("WATCHKEEPER_HOST_SECRET_MUST_NOT_LEAK")
            && !marker_json.contains("WATCHKEEPER_AMBIENT_SECRET_MUST_NOT_LEAK"),
        "host credentials or ambient secrets crossed into captured gate evidence: {marker_json}"
    );
    assert!(
        !outside_write.exists(),
        "the strict gate wrote outside the isolated workspace"
    );
    assert!(
        !shim_sentinel.exists(),
        "the sandbox backend was selected through ambient PATH"
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
    let service = SupervisorServiceFixture::configured(&paths);

    let launch = service
        .deadreckon()
        .current_dir(&workspace)
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
    // `Running` is the durable guarded-process readiness boundary: the
    // release token was validated, the evaluator owns its process group, and
    // repository-controlled code may execute. A workspace sentinel is
    // downstream of that boundary and can be delayed independently by host
    // executable validation, so it is not authoritative cancellation proof.
    let (record_path, record) =
        wait_for_gate_record(&paths, job_id, &state.run_root, Duration::from_secs(60));
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
      : > "$PWD/gate-ready"
      while :; do sleep 1; done
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
    let service = SupervisorServiceFixture::configured(&paths);

    let launch = service
        .deadreckon()
        .current_dir(&workspace)
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
    // Match the cancellation fixture's authoritative readiness boundary. The
    // durable `Running` record proves the evaluator owns its process group;
    // the acceptance sentinel is downstream and may be delayed by host
    // executable validation under suite load.
    let (first_record_path, first_record) =
        wait_for_gate_record(&paths, job_id, &state.run_root, Duration::from_secs(60));
    let outer = read_supervised_process(&paths.job_dir(job_id).join("supervised-child.json"))
        .expect("outer supervised launcher");
    signal_pid(outer.pid, nix::sys::signal::Signal::SIGKILL);

    let deadline = Instant::now() + Duration::from_secs(45);
    let second_record_path = 'retry: loop {
        let first_alive = process_group_is_alive(first_record.process.pid);
        for record_path in gate_record_paths(&state.run_root) {
            if record_path != first_record_path {
                assert!(
                    !first_alive,
                    "retry evaluator {} started while old evaluator group {} remained alive",
                    record_path.display(),
                    first_record.process.pid
                );
                break 'retry record_path;
            }
        }
        assert!(
            Instant::now() < deadline,
            "Job {job_id} did not launch one bounded retry after launcher SIGKILL"
        );
        thread::sleep(Duration::from_millis(25));
    };

    let cancelled = public_deadreckon()
        .current_dir(&workspace)
        .env("DEADRECKON_HOME", paths.home())
        .args(["kill", job_id, "--escalate", "--plain"])
        .output()
        .expect("cancel retrying Job");
    assert!(
        cancelled.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&cancelled.stdout),
        String::from_utf8_lossy(&cancelled.stderr)
    );
    let view = wait_for_terminal_job(&paths, job_id, Duration::from_secs(30));
    assert_eq!(view.projection.outcome, Some(JobOutcome::Cancelled));
    wait_for_process_group_exit(first_record.process.pid, Duration::from_secs(10));
    assert!(
        !first_record_path.exists(),
        "first evaluator record survived retry reconciliation"
    );
    assert!(
        !second_record_path.exists(),
        "retry evaluator record survived cancellation"
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
    assert!(!deadreckon_core::marker_path_for_run_root(&state.run_root).exists());
    assert!(
        !paths.job_receipt(job_id).exists(),
        "the scripted semantic fixture must not issue a completion receipt"
    );
}

#[test]
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn public_smoke_job_can_never_issue_a_trusted_completion_receipt() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(&workspace).expect("workspace");
    write_smoke_acceptance(&workspace);
    let service = SupervisorServiceFixture::configured(&paths);

    let launch = service
        .deadreckon()
        .current_dir(&workspace)
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

    // A full parallel workspace run can keep the debug supervisor and bwrap
    // launch behind linker and scheduler contention for more than 20 seconds.
    // Keep the proof bounded, but use the same recovery window as the
    // interruption tests above rather than turning host load into a failure.
    let deadline = Instant::now() + Duration::from_secs(45);
    let view = loop {
        if let Ok(view) = JobView::load(&paths, job_id)
            && view.projection.is_terminal()
        {
            break view;
        }
        if Instant::now() >= deadline {
            let view = JobView::load(&paths, job_id);
            let events = fs::read_to_string(paths.job_events(job_id));
            panic!(
                "public smoke Job {job_id} did not reach a bounded terminal state\nview: {view:#?}\nevents: {events:#?}"
            );
        }
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

#[cfg(unix)]
#[test]
#[ignore = "requires DEADRECKON_LIVE_DOCKER_TEST=1, a running daemon, cached rust:1, and the static Linux evaluator sidecar"]
fn live_docker_public_job_completes_deterministic_gate_and_cleans_daemon_state() {
    require_live_docker_test();
    let fixture = launch_live_docker_job(
        "Complete the approved deterministic Docker gate.",
        r#"name: Docker completion
checks:
  - kind: file_exists
    path: "{working_dir}/README.md"
"#,
    );

    let view = wait_for_terminal_job(&fixture.paths, &fixture.job_id, Duration::from_secs(90));
    assert_eq!(
        view.projection.outcome,
        Some(JobOutcome::NeedsReview),
        "the smoke semantic transport must fail closed after the real Docker gate\n\
         diagnostics:\n{}",
        fixture.diagnostics()
    );
    let state = load_run(&fixture.paths, &fixture.job_id).expect("Job result run");
    let marker = validate_acceptance_marker(&state).expect("trusted Docker gate marker");
    assert!(marker.is_native_gate_proof(), "{marker:#?}");
    assert!(marker.contained, "{marker:#?}");
    assert_eq!(marker.sandbox_backend, "docker");
    assert_eq!(marker.check_count, 1);
    assert!(marker.checks.iter().all(|check| check.passed));

    let authority: JobAuthority = serde_json::from_slice(
        &fs::read(fixture.paths.job_authority(&fixture.job_id)).expect("Job authority"),
    )
    .expect("Job authority JSON");
    let observation =
        validate_sandbox_boundary_observation(&fixture.paths, &state, &authority, "docker")
            .expect("controller-signed Docker boundary observation");
    assert!(observation.contained);
    assert!(observation.gate_key_read_denied);
    assert!(observation.proof_write_denied);
    assert!(observation.control_write_denied);
    assert!(observation.operator_capture_read_denied);
    assert!(observation.operator_capture_write_denied);
    assert!(observation.signing_env_scrubbed);
    assert_eq!(
        observation.gate_evaluator_sha256,
        authority.gate_evaluator_sha256
    );

    let job = deadreckon_core::load_job(&fixture.paths, &fixture.job_id).expect("Job");
    let docker = job
        .policy
        .execution
        .as_ref()
        .and_then(|execution| execution.gate_evaluator.as_ref())
        .and_then(|identity| identity.docker.as_ref())
        .expect("immutable Docker evaluator identity");
    assert!(docker.image_id.starts_with("sha256:"));
    assert_eq!(docker.platform, "linux/arm64");
    assert_eq!(
        docker.guest_path,
        Path::new(deadreckon_protocol::DOCKER_GATE_GUEST_PATH)
    );
    assert!(
        gate_key_path(&fixture.paths, &fixture.job_id).is_file(),
        "signing material must remain outside the evaluator-visible workspace"
    );
    assert!(
        !fixture.paths.job_receipt(&fixture.job_id).exists(),
        "the smoke semantic transport must not issue a trusted receipt"
    );
    assert_docker_job_clean(&fixture);
}

#[cfg(unix)]
#[test]
#[ignore = "requires DEADRECKON_LIVE_DOCKER_TEST=1, a running daemon, cached rust:1, and the static Linux evaluator sidecar"]
fn live_docker_public_cancel_removes_container_record_and_prevents_retry() {
    require_live_docker_test();
    let fixture = launch_live_docker_job(
        "Enter the approved cancellable Docker gate.",
        r#"name: cancellable Docker gate
checks:
  - kind: shell
    command: |
      set -eu
      : > "$PWD/gate-ready"
      while :; do sleep 1; done
    cwd: "{working_dir}"
"#,
    );
    wait_for_job_path(
        &fixture.paths,
        &fixture.job_id,
        &fixture.state.working_dir.join("gate-ready"),
        Duration::from_secs(60),
    );
    let container = wait_for_docker_container(&fixture.job_id, Duration::from_secs(20));
    let (record_path, _) = wait_for_gate_record(
        &fixture.paths,
        &fixture.job_id,
        &fixture.state.run_root,
        Duration::from_secs(10),
    );

    let cancelled = public_deadreckon()
        .current_dir(&fixture.workspace)
        .env("DEADRECKON_HOME", fixture.paths.home())
        .args(["kill", &fixture.job_id, "--escalate", "--plain"])
        .output()
        .expect("public cancel");
    assert!(
        cancelled.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&cancelled.stdout),
        String::from_utf8_lossy(&cancelled.stderr)
    );

    let view = wait_for_terminal_job(&fixture.paths, &fixture.job_id, Duration::from_secs(60));
    assert_eq!(view.projection.outcome, Some(JobOutcome::Cancelled));
    wait_for_docker_container_exit(&container, Duration::from_secs(20));
    assert!(
        !record_path.exists(),
        "guarded evaluator record survived cancellation"
    );
    assert!(
        !deadreckon_core::marker_path_for_run_root(&fixture.state.run_root).exists(),
        "cancelled Docker gate wrote a deterministic marker"
    );
    assert!(
        !fixture.paths.job_receipt(&fixture.job_id).exists(),
        "cancelled Docker gate wrote a completion receipt"
    );
    let history = deadreckon_core::read_job_history(&fixture.paths.job_events(&fixture.job_id))
        .expect("job history");
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
    assert_docker_job_clean(&fixture);
}

#[cfg(unix)]
#[test]
#[ignore = "requires DEADRECKON_LIVE_DOCKER_TEST=1, a running daemon, cached rust:1, and the static Linux evaluator sidecar"]
fn live_docker_worker_sigkill_reconciles_stale_container_before_one_retry() {
    require_live_docker_test();
    let fixture = launch_live_docker_job(
        "Recover the approved Docker gate after one worker crash.",
        r#"name: recoverable Docker gate
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
    );
    wait_for_job_path(
        &fixture.paths,
        &fixture.job_id,
        &fixture.state.working_dir.join("gate-ready"),
        Duration::from_secs(60),
    );
    let first_container = wait_for_docker_container(&fixture.job_id, Duration::from_secs(20));
    let outer = read_supervised_process(
        &fixture
            .paths
            .job_dir(&fixture.job_id)
            .join("supervised-child.json"),
    )
    .expect("outer supervised worker");
    signal_pid(outer.pid, nix::sys::signal::Signal::SIGKILL);

    let deadline = Instant::now() + Duration::from_secs(90);
    let view = loop {
        let containers = docker_job_containers(&fixture.job_id);
        assert!(
            containers.len() <= 1,
            "retry launched before the stale Docker container was reconciled: {containers:?}"
        );
        if let Ok(view) = JobView::load(&fixture.paths, &fixture.job_id)
            && view.projection.is_terminal()
        {
            break view;
        }
        assert!(
            Instant::now() < deadline,
            "Job {} did not recover after worker SIGKILL\nsupervisor stderr:\n{}",
            fixture.job_id,
            fixture.diagnostics()
        );
        thread::sleep(Duration::from_millis(50));
    };

    assert_eq!(view.projection.outcome, Some(JobOutcome::NeedsReview));
    wait_for_docker_container_exit(&first_container, Duration::from_secs(20));
    let history = deadreckon_core::read_job_history(&fixture.paths.job_events(&fixture.job_id))
        .expect("job history");
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
        deadreckon_core::marker_path_for_run_root(&fixture.state.run_root).exists(),
        "the retry did not complete deterministic Docker verification"
    );
    assert!(
        !fixture.paths.job_receipt(&fixture.job_id).exists(),
        "the smoke semantic transport must not issue a completion receipt"
    );
    assert_docker_job_clean(&fixture);
}

#[cfg(unix)]
struct LiveDockerFixture {
    _service: SupervisorServiceFixture,
    _temp: TempDir,
    workspace: std::path::PathBuf,
    paths: DeadreckonPaths,
    job_id: String,
    state: deadreckon_core::PipelineState,
}

#[cfg(unix)]
impl LiveDockerFixture {
    fn diagnostics(&self) -> String {
        watchkeeper_job_diagnostics(&self.paths, &self.job_id)
    }
}

#[cfg(unix)]
fn require_live_docker_test() {
    assert_eq!(
        std::env::var("DEADRECKON_LIVE_DOCKER_TEST").as_deref(),
        Ok("1"),
        "set DEADRECKON_LIVE_DOCKER_TEST=1 explicitly"
    );
    let status = Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .status()
        .expect("Docker CLI");
    assert!(status.success(), "Docker daemon is unavailable");

    let deadreckon = Path::new(env!("CARGO_BIN_EXE_deadreckon"));
    let sidecar = deadreckon
        .parent()
        .expect("DeadReckon binary parent")
        .join("dr-gate-evaluator-aarch64-unknown-linux-musl");
    assert!(
        sidecar.is_file(),
        "build or install the static Linux arm64 evaluator at {}",
        sidecar.display()
    );
}

#[cfg(unix)]
fn launch_live_docker_job(goal: &str, acceptance: &str) -> LiveDockerFixture {
    let target = std::env::current_dir()
        .expect("current directory")
        .join("target");
    fs::create_dir_all(&target).expect("target directory");
    let temp = tempfile::Builder::new()
        .prefix("watchkeeper-live-docker-")
        .tempdir_in(target)
        .expect("Docker fixture");
    let workspace = temp.path().join("workspace");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(workspace.join(".deadreckon")).expect("acceptance directory");
    fs::create_dir_all(paths.home()).expect("DeadReckon home");
    fs::write(
        paths.config_path(),
        "default_provider = \"smoke\"\n\n[defaults]\nsandbox = \"docker\"\n",
    )
    .expect("config");
    fs::write(workspace.join(".deadreckon/acceptance.yaml"), acceptance).expect("acceptance");
    fs::write(workspace.join("README.md"), "live Docker fixture\n").expect("README");
    git(&workspace, &["init", "--initial-branch=main"]);
    git(
        &workspace,
        &["config", "user.email", "watchkeeper@example.invalid"],
    );
    git(&workspace, &["config", "user.name", "Watchkeeper Test"]);
    git(&workspace, &["add", "-A"]);
    git(&workspace, &["commit", "-m", "fixture"]);
    let service = SupervisorServiceFixture::configured(&paths);

    let launch = service
        .deadreckon()
        .current_dir(&workspace)
        .args([
            "start",
            goal,
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
        .expect("public Docker start");
    assert!(
        launch.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&launch.stdout),
        String::from_utf8_lossy(&launch.stderr)
    );
    let envelope: Value = serde_json::from_slice(&launch.stdout).expect("launch JSON");
    let job_id = envelope["dispatched"]["ids"][0]
        .as_str()
        .expect("dispatched Job ID")
        .to_string();
    let state = wait_for_run(&paths, &job_id, Duration::from_secs(30));
    LiveDockerFixture {
        _service: service,
        _temp: temp,
        workspace,
        paths,
        job_id,
        state,
    }
}

#[cfg(unix)]
fn docker_job_containers(job_id: &str) -> Vec<String> {
    let filter = format!("label=io.deadreckon.job-id={job_id}");
    let output = Command::new("docker")
        .args(["ps", "-aq", "--filter", &filter])
        .output()
        .expect("docker ps");
    assert!(
        output.status.success(),
        "docker ps failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(unix)]
fn wait_for_docker_container(job_id: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        let containers = docker_job_containers(job_id);
        if let [container] = containers.as_slice() {
            return container.clone();
        }
        assert!(
            Instant::now() < deadline,
            "Docker container for Job {job_id} was not observed: {containers:?}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
fn wait_for_docker_container_exit(container: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let output = Command::new("docker")
            .args(["container", "inspect", container])
            .output()
            .expect("docker inspect");
        if !output.status.success() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "Docker container {container} remained after cleanup"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
fn assert_docker_job_clean(fixture: &LiveDockerFixture) {
    assert!(
        docker_job_containers(&fixture.job_id).is_empty(),
        "managed Docker container survived terminal Job state"
    );
    assert!(
        directory_is_absent_or_empty(
            &fixture
                .paths
                .job_dir(&fixture.job_id)
                .join("docker-executions")
        ),
        "durable Docker recovery records survived terminal Job state"
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

#[cfg(unix)]
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

#[cfg(unix)]
fn wait_for_job_path(paths: &DeadreckonPaths, job_id: &str, path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !path.exists() {
        if let Ok(view) = JobView::load(paths, job_id)
            && view.projection.is_terminal()
        {
            let state = load_run(paths, job_id).ok();
            panic!(
                "{} was not created before Job terminal {:?}; state: {state:#?}\n{}",
                path.display(),
                view.projection.outcome,
                watchkeeper_job_diagnostics(paths, job_id),
            );
        }
        assert!(
            Instant::now() < deadline,
            "{} was not created\n{}",
            path.display(),
            watchkeeper_job_diagnostics(paths, job_id),
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
fn wait_for_gate_record(
    paths: &DeadreckonPaths,
    job_id: &str,
    run_root: &Path,
    progress_timeout: Duration,
) -> (std::path::PathBuf, SupervisedProcessRecord) {
    let directory = run_root.join("child-pids");
    let started = Instant::now();
    // There are two meaningful readiness stages after run creation: observe a
    // durable prepared record, then observe its running transition. Give each
    // stage the same bounded no-progress window while retaining a hard cap.
    let hard_deadline = started + progress_timeout.saturating_mul(2);
    let mut progress_deadline = started + progress_timeout;
    let mut last_observation = Vec::new();
    loop {
        let mut observation = Vec::new();
        if let Ok(entries) = fs::read_dir(&directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("dr-gate-evaluate-")
                {
                    continue;
                }
                match read_supervised_process_record(&path) {
                    Ok(record) => {
                        observation.push(format!(
                            "{}: {:?} pid={} pgid={:?}",
                            path.display(),
                            record.phase,
                            record.process.pid,
                            record.process.pgid
                        ));
                        if record.phase == SupervisedProcessPhase::Running {
                            return (path, record);
                        }
                    }
                    Err(error) => {
                        observation.push(format!("{}: unreadable: {error}", path.display()));
                    }
                }
            }
        }
        observation.sort();
        if observation != last_observation {
            last_observation = observation;
            progress_deadline = (Instant::now() + progress_timeout).min(hard_deadline);
        }
        if let Ok(view) = JobView::load(paths, job_id)
            && view.projection.is_terminal()
        {
            panic!(
                "guarded evaluator did not reach Running before Job terminal {:?}\nobserved records:\n{}\n{}",
                view.projection.outcome,
                format_observed_gate_records(&last_observation),
                watchkeeper_job_diagnostics(paths, job_id),
            );
        }
        let now = Instant::now();
        assert!(
            now < progress_deadline && now < hard_deadline,
            "guarded evaluator did not reach Running after {:.2?}; no-progress limit {:.2?}, hard limit {:.2?}\nobserved records:\n{}\n{}",
            started.elapsed(),
            progress_timeout,
            progress_timeout.saturating_mul(2),
            format_observed_gate_records(&last_observation),
            watchkeeper_job_diagnostics(paths, job_id),
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
fn format_observed_gate_records(observed: &[String]) -> String {
    if observed.is_empty() {
        "<none>".to_string()
    } else {
        observed.join("\n")
    }
}

#[cfg(unix)]
fn watchkeeper_job_diagnostics(paths: &DeadreckonPaths, job_id: &str) -> String {
    let job_dir = paths.job_dir(job_id);
    let mut sections = vec![format!(
        "== projection ==\n{:#?}",
        JobView::load(paths, job_id)
    )];
    sections.push(format!("== run state ==\n{:#?}", load_run(paths, job_id)));
    sections.extend(
        [
            ("events", paths.job_events(job_id)),
            ("launcher stdout", job_dir.join("supervisor.out")),
            ("launcher stderr", job_dir.join("supervisor.err")),
            ("worker stdout", job_dir.join("supervisor-stdout.log")),
            ("worker stderr", job_dir.join("supervisor-stderr.log")),
        ]
        .into_iter()
        .map(|(label, path)| {
            format!(
                "== {label}: {} ==\n{}",
                path.display(),
                fs::read_to_string(&path).unwrap_or_else(|error| format!("<unreadable: {error}>"))
            )
        }),
    );
    sections.join("\n")
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

#[cfg(unix)]
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
    Command::new("/usr/bin/sandbox-exec")
        .args(["-p", "(version 1)\n(allow default)", "--", "/usr/bin/true"])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "macos")]
fn gate_boundary_acceptance(
    deadreckon_home: &Path,
    host_secret: &Path,
    outside_write: &Path,
) -> String {
    let deadreckon_home = shell_quote(deadreckon_home);
    let host_secret = shell_quote(host_secret);
    let outside_write = shell_quote(outside_write);
    format!(
        r#"name: sandbox gate boundary shell test
checks:
  - kind: shell
    command: |
      set -eu
      test -z "${{{contained}+present}}"
      test -z "${{{backend}+present}}"
      test -z "${{DEADRECKON_FAKE_SECRET+present}}"
      deadreckon_home={deadreckon_home}
      host_secret={host_secret}
      outside_write={outside_write}
      job_dir=$(find "$deadreckon_home/jobs" -mindepth 1 -maxdepth 1 -type d -print -quit)
      test -n "$job_dir"
      run_id=$(basename "$job_dir")
      job_control="$job_dir/job.json"
      run_root=
      for candidate in "$deadreckon_home"/runstate/*/runs/"$run_id"; do
        test -d "$candidate" || continue
        test -z "$run_root"
        run_root=$candidate
      done
      test -n "$run_root"
      gate_key="$deadreckon_home/gate-keys/$run_id.key"
      proof_control="$run_root/proofs/turn-acceptance.json"
      test -f "$run_root/acceptance.yaml"
      test ! -e "$PWD/.git"
      if cat "$gate_key" >/dev/null 2>&1; then exit 31; fi
      if cat "$job_control" >/dev/null 2>&1; then exit 32; fi
      if printf tampered >>"$job_control"; then exit 33; fi
      if printf forged >"$proof_control"; then exit 34; fi
      if cat "$host_secret" >/dev/null 2>&1; then exit 36; fi
      if printf escaped >"$outside_write"; then exit 37; fi
      (sleep 1; printf escaped >"$PWD/delayed-gate-sentinel") </dev/null >/dev/null 2>&1 &
    cwd: "{{working_dir}}"
"#,
        contained = GATE_CONTAINED_ENV,
        backend = GATE_SANDBOX_BACKEND_ENV,
        deadreckon_home = deadreckon_home,
        host_secret = host_secret,
        outside_write = outside_write,
    )
}

#[cfg(target_os = "macos")]
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
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
