#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::redundant_clone
)]

use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use deadreckon::notify::{NotifyAttempt, NotifyTransition};
use deadreckon_core::{
    DeadreckonPaths, RUN_EVENTS_JSONL, RunStatus, cancel_marker_path, list_runs, load_run,
    write_cancel_marker,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;

mod common;

use common::repo_tempdir;

const IMPORT_FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/import");
const IMPORT_GOLDENS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens/import");

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mock_provider_records_three_turns_and_artifacts_match() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let server = MockServer::start(three_turn_script()).await;
    write_config(temp.path(), &server.base_url());

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("mock three turn task")
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .arg("--max-spend")
        .arg("1")
        .arg("--no-docs")
        .env("DEADRECKON_HOME", temp.path().join("home"))
        .output()
        .expect("run deadreckon");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("completed run"));
    assert!(server.journal().len() >= 3);

    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    assert_eq!(state.status, RunStatus::Completed);
    assert_eq!(state.turn, 5);
    assert_jsonl_count(&state.run_root.join("traces.jsonl"), 6);
    assert_jsonl_count(&state.run_root.join("spend.jsonl"), 5);
    assert_jsonl_count(&state.run_root.join("provenance.jsonl"), 3);
    assert!(state.run_root.join("proofs/turn-acceptance.json").exists());
    assert!(state.working_dir.join("turn1.txt").exists());
    assert!(state.working_dir.join("notes.md").exists());
    assert_provenance_ids_match_traces(&state.run_root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transient_provider_error_retries_once_and_run_completes() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let server = MockServer::start(transient_then_success_script()).await;
    write_config(temp.path(), &server.base_url());

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("mock transient retry task")
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .arg("--max-spend")
        .arg("1")
        .arg("--no-docs")
        .env("DEADRECKON_HOME", temp.path().join("home"))
        .output()
        .expect("run deadreckon");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("completed run"));

    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    assert_eq!(state.status, RunStatus::Completed);
    // The retry is part of the audit trail, not a silent recovery.
    let events = fs::read_to_string(state.run_root.join("events.jsonl")).expect("events");
    assert!(
        events.contains("transient provider error; retrying once"),
        "{events}"
    );
    assert!(events.contains("retry succeeded; continuing"), "{events}");
    // The failed attempt plus the full script: every fixture was consumed.
    assert!(server.journal().len() >= 6, "{}", server.journal().len());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhausted_retries_persist_failed_status_with_reason() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let server = MockServer::start(always_failing_script()).await;
    write_config(temp.path(), &server.base_url());

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("mock always failing task")
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .arg("--max-spend")
        .arg("1")
        .arg("--no-docs")
        .env("DEADRECKON_HOME", temp.path().join("home"))
        .output()
        .expect("run deadreckon");
    assert!(
        !output.status.success(),
        "a run whose provider never recovers must not exit 0"
    );

    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    // No zombie Executing: the failure is durable and explained.
    assert_eq!(state.status, RunStatus::Failed);
    let reason = state.failure_reason.as_deref().expect("failure reason");
    assert!(reason.contains("provider error"), "{reason}");
    // Both fixtures consumed: the single bounded retry really happened.
    assert_eq!(server.journal().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn accepted_run_fires_notification_when_enabled() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let server = MockServer::start(three_turn_script()).await;
    let notify_out = temp.path().join("notify-env.txt");
    let notify_command = format!(
        "env | sort | grep '^DEADRECKON_NOTIFY_' > {}",
        shell_quote(&notify_out)
    );
    write_config_with_extra(
        temp.path(),
        &server.base_url(),
        &format!(
            r#"
[notify]
enabled = true
on = ["accepted", "paused", "failed"]
native = false
command = "{}"
"#,
            toml_string(&notify_command)
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("mock notify accepted")
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .arg("--max-spend")
        .arg("1")
        .arg("--no-docs")
        .env("DEADRECKON_HOME", temp.path().join("home"))
        .output()
        .expect("run deadreckon");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("completed run"));

    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    assert_eq!(state.status, RunStatus::Completed);

    let env_dump = fs::read_to_string(&notify_out).expect("notify env");
    assert!(env_dump.contains("DEADRECKON_NOTIFY_TRANSITION=accepted"));
    assert!(env_dump.contains(&format!("DEADRECKON_NOTIFY_RUN_ID={}", state.run_id)));
    assert!(env_dump.contains("DEADRECKON_NOTIFY_VERDICT=verified run"));
    assert!(env_dump.contains("DEADRECKON_NOTIFY_SPEND="));

    let attempts = jsonl_values(&state.run_root.join("notify.jsonl"));
    assert_eq!(attempts.len(), 1);
    let attempt: NotifyAttempt =
        serde_json::from_value(attempts[0].clone()).expect("notify attempt");
    assert_eq!(attempt.transition, NotifyTransition::Accepted);
    assert_eq!(attempt.channel, "command");
    assert!(attempt.ok, "{attempt:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_notify_fires_nothing() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let server = MockServer::start(three_turn_script()).await;
    let notify_out = temp.path().join("notify-disabled-env.txt");
    let notify_command = format!("env > {}", shell_quote(&notify_out));
    write_config_with_extra(
        temp.path(),
        &server.base_url(),
        &format!(
            r#"
[notify]
enabled = false
native = false
command = "{}"
"#,
            toml_string(&notify_command)
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("mock notify disabled")
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .arg("--max-spend")
        .arg("1")
        .arg("--no-docs")
        .env("DEADRECKON_HOME", temp.path().join("home"))
        .output()
        .expect("run deadreckon");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    assert_eq!(state.status, RunStatus::Completed);
    assert!(!notify_out.exists());
    assert!(!state.run_root.join("notify.jsonl").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_run_across_processes_terminates_in_5s() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let server = MockServer::start(kill_script()).await;
    write_config(temp.path(), &server.base_url());
    let home = temp.path().join("home");
    let mut child = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("mock slow task")
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .arg("--max-spend")
        .arg("1")
        .env("DEADRECKON_HOME", &home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn run");

    let paths = DeadreckonPaths::from_home(&home);
    let run_id = wait_for_run_id(&paths);
    let started = Instant::now();
    let kill = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("kill")
        .arg(&run_id)
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("kill");
    let elapsed = started.elapsed();
    assert!(kill.status.success());
    assert!(
        elapsed < Duration::from_secs(5),
        "kill command returned after {elapsed:?}"
    );
    assert!(
        wait_for_child_exit(&mut child, Duration::from_secs(5)),
        "run did not exit after kill"
    );
    let state = load_run(&paths, &run_id).expect("state");
    assert_eq!(state.status, RunStatus::Killed);
    assert!(cancel_marker_path(&state).exists());
    wait_for_run_events(
        &state.run_root,
        &[r#""kind":"run_completed""#, r#""status":"killed""#],
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kill_during_http_streaming_aborts_request() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let server = MockServer::start(kill_script()).await;
    write_config(temp.path(), &server.base_url());
    let home = temp.path().join("home");
    let mut child = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("mock marker cancel")
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .arg("--max-spend")
        .arg("1")
        .env("DEADRECKON_HOME", &home)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn run");

    let paths = DeadreckonPaths::from_home(&home);
    let run_id = wait_for_run_id(&paths);
    let state = load_run(&paths, &run_id).expect("state");
    write_cancel_marker(&state, "test marker cancel").expect("cancel marker");
    assert!(
        wait_for_child_exit(&mut child, Duration::from_secs(2)),
        "run did not abort after cancel marker"
    );

    let state = load_run(&paths, &run_id).expect("state");
    assert_eq!(state.status, RunStatus::Killed);
    wait_for_run_events(
        &state.run_root,
        &[r#""kind":"run_completed""#, r#""status":"killed""#],
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_preserves_history_file() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let server = MockServer::start(three_turn_script()).await;
    write_config(temp.path(), &server.base_url());
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("mock resume history")
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .arg("--max-spend")
        .arg("1")
        .arg("--no-docs")
        .env("DEADRECKON_HOME", temp.path().join("home"))
        .output()
        .expect("run");
    assert!(output.status.success());
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    let history = fs::read_to_string(state.run_root.join("history.json")).expect("history");
    assert!(history.contains("tool-bash-1"));
    assert!(history.contains("tool-write-2"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_subagent_without_file_changes_fails_run() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let fake_codex = temp.path().join("fake-codex");
    fs::write(&fake_codex, "#!/bin/sh\nprintf 'Changed files: none.\\n'\n").expect("fake codex");
    chmod_exec(&fake_codex);
    write_cli_config(temp.path(), &fake_codex);

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("cli no-op")
        .arg("--provider")
        .arg("cli:codex")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .arg("--max-spend")
        .arg("1")
        .env("DEADRECKON_HOME", temp.path().join("home"))
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("failed run"));
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    assert_eq!(state.status, RunStatus::Failed);
    assert!(
        state
            .failure_reason
            .as_deref()
            .expect("failure reason")
            .contains("without file changes")
    );
    assert!(!state.run_root.join("proofs/turn-acceptance.json").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acceptance_failure_restarts_cli_subagent_until_gate_passes() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&cwd).expect("workspace");
    let fake_codex = temp.path().join("fake-codex-acceptance-retry");
    fs::write(
        &fake_codex,
        r#"#!/bin/sh
if [ "$*" = "exec --help" ]; then
  exit 0
fi
if [ ! -f .provider-count ]; then
  printf 1 > .provider-count
  printf 'first attempt\n' > first.txt
  cat > implementation-notes.html <<'HTML'
<h1>Implementation Notes</h1>
<section id="design-decisions"><h2>Design decisions</h2><p>First acceptance attempt writes a marker file.</p></section>
<section id="deviations"><h2>Deviations</h2><p>None.</p></section>
<section id="tradeoffs"><h2>Tradeoffs</h2><p>Retry fixture keeps notes current on every provider turn.</p></section>
<section id="open-questions"><h2>Open questions</h2><p>None.</p></section>
HTML
  printf 'first turn changed files\n'
else
  printf pass > required.txt
  cat > implementation-notes.html <<'HTML'
<h1>Implementation Notes</h1>
<section id="design-decisions"><h2>Design decisions</h2><p>Second acceptance attempt creates the required file.</p></section>
<section id="deviations"><h2>Deviations</h2><p>None.</p></section>
<section id="tradeoffs"><h2>Tradeoffs</h2><p>The fixture uses a retry rather than weakening acceptance.</p></section>
<section id="open-questions"><h2>Open questions</h2><p>None.</p></section>
HTML
  printf 'second turn fixed acceptance\n'
fi
"#,
    )
    .expect("fake codex");
    chmod_exec(&fake_codex);
    write_cli_config(temp.path(), &fake_codex);
    let acceptance = temp.path().join("acceptance.yaml");
    fs::write(
        &acceptance,
        "checks:\n  - kind: file_exists\n    path: \"{working_dir}/required.txt\"\n",
    )
    .expect("acceptance");

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&cwd)
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("cli acceptance retry")
        .arg("--provider")
        .arg("cli:codex")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .arg("--max-spend")
        .arg("1")
        .arg("--acceptance")
        .arg(&acceptance)
        .arg("--no-docs")
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("completed run"));
    let paths = DeadreckonPaths::from_home(&home);
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    assert_eq!(state.status, RunStatus::Completed);
    assert_eq!(state.turn, 2);
    assert!(state.working_dir.join("required.txt").exists());
    let traces = fs::read_to_string(state.run_root.join("traces.jsonl")).expect("traces");
    assert!(traces.contains("acceptance.failed"));
    assert!(state.run_root.join("proofs/turn-acceptance.json").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acceptance_failure_exhaustion_persists_failed_state() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&cwd).expect("workspace");
    let fake_codex = temp.path().join("fake-codex-acceptance-exhaust");
    fs::write(
        &fake_codex,
        r#"#!/bin/sh
count=$(cat .provider-count 2>/dev/null || printf 0)
count=$((count + 1))
printf '%s' "$count" > .provider-count
printf 'attempt %s\n' "$count" > "attempt-$count.txt"
cat > implementation-notes.html <<HTML
<h1>Implementation Notes</h1>
<section id="design-decisions"><h2>Design decisions</h2><p>Attempt $count writes a tracked file but intentionally misses acceptance.</p></section>
<section id="deviations"><h2>Deviations</h2><p>Acceptance target is intentionally never created.</p></section>
<section id="tradeoffs"><h2>Tradeoffs</h2><p>The exhaustion fixture keeps notes current while proving retry limits.</p></section>
<section id="open-questions"><h2>Open questions</h2><p>None.</p></section>
HTML
printf 'changed attempt %s\n' "$count"
"#,
    )
    .expect("fake codex");
    chmod_exec(&fake_codex);
    write_cli_config(temp.path(), &fake_codex);
    let acceptance = temp.path().join("acceptance.yaml");
    fs::write(
        &acceptance,
        "checks:\n  - kind: file_exists\n    path: \"{working_dir}/never-created.txt\"\n",
    )
    .expect("acceptance");

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&cwd)
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("cli acceptance exhaust")
        .arg("--provider")
        .arg("cli:codex")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .arg("--max-spend")
        .arg("1")
        .arg("--acceptance")
        .arg(&acceptance)
        .arg("--no-docs")
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("failed run"));
    let paths = DeadreckonPaths::from_home(&home);
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    assert_eq!(state.status, RunStatus::Failed);
    assert_eq!(state.turn, 12);
    assert!(state.child_pids.is_empty());
    assert!(
        state
            .failure_reason
            .as_deref()
            .expect("failure reason")
            .contains("acceptance failed after turn 12")
    );
    assert!(!state.run_root.join("proofs/turn-acceptance.json").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn init_config_and_default_spend_work() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let init = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("init")
        .arg("--provider")
        .arg("cli:codex")
        .arg("--max-spend")
        .arg("14")
        .arg("--sandbox")
        .arg("none")
        .arg("--no-confirm")
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("init");
    assert!(
        init.status.success(),
        "{}{}",
        String::from_utf8_lossy(&init.stdout),
        String::from_utf8_lossy(&init.stderr)
    );
    let get = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("config")
        .arg("get")
        .arg("defaults.max_spend")
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("config get");
    assert_eq!(String::from_utf8_lossy(&get.stdout).trim(), "14");
    let set = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("config")
        .arg("set")
        .arg("defaults.max_spend")
        .arg("15")
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("config set");
    assert!(set.status.success());

    let run = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("tiny hello rust")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("run");
    assert!(
        run.status.success(),
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let paths = DeadreckonPaths::from_home(&home);
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    assert_eq!(state.max_spend_usd, Some(15.0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn high_spend_requires_confirmation_flag_in_scripts() {
    let temp = repo_tempdir();
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("too much")
        .arg("--smoke")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .arg("--max-spend")
        .arg("51")
        .env("DEADRECKON_HOME", temp.path().join("home"))
        .output()
        .expect("run");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("max spend above $50"));
    assert!(stderr.contains("hint:"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_wall_clock_budget_enforced() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let fake_codex = temp.path().join("fake-codex-sleep");
    fs::write(
        &fake_codex,
        r#"#!/bin/sh
stamp=$(date +%s%N 2>/dev/null || date +%s)
printf "wall run %s\n" "$stamp" > notes.md
cat > implementation-notes.html <<'HTML'
<h1>Implementation Notes</h1>
<section id="design-decisions"><h2>Design decisions</h2><p>Wall-clock fixture writes notes before sleeping.</p></section>
<section id="deviations"><h2>Deviations</h2><p>None.</p></section>
<section id="tradeoffs"><h2>Tradeoffs</h2><p>The sleep stays in the provider to exercise pause/resume timing.</p></section>
HTML
printf '<section id="open-questions"><h2>Open questions</h2><p>None at %s.</p></section>\n' "$stamp" >> implementation-notes.html
sleep 1
printf 'changed notes\n'
"#,
    )
    .expect("fake codex");
    chmod_exec(&fake_codex);
    write_cli_config(temp.path(), &fake_codex);

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(temp.path())
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("cli wall budget")
        .arg("--provider")
        .arg("cli:codex")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .arg("--max-spend")
        .arg("1")
        .arg("--max-wall-seconds")
        .arg("0.1")
        .arg("--no-docs")
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("run");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("paused run"));
    let paths = DeadreckonPaths::from_home(&home);
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    let pause_reason = state.pause_reason.as_deref().expect("pause reason");
    // The 100 ms Job budget can bind at any bounded phase reached first on a
    // contended machine: before provider launch, during the call, or at the
    // post-turn check. Preserve the canonical cap classification while also
    // requiring contextual phase-boundary reasons to name the authoritative
    // Job cutoff.
    assert!(
        pause_reason == "wall-clock cap reached"
            || pause_reason == "wall-clock cap reached mid-turn"
            || (pause_reason.starts_with("wall-clock cap reached during ")
                && pause_reason.ends_with("(approved Job work cutoff)")),
        "{pause_reason}"
    );
    assert!(
        state.total_wall_seconds >= 0.1,
        "the durable Job clock did not reach its cap: {}",
        state.total_wall_seconds
    );
    let spend_path = state.run_root.join("spend.jsonl");
    if spend_path.exists() {
        let spend = fs::read_to_string(spend_path).expect("provider spend");
        assert!(spend.contains("wall_time_seconds"));
    }
    // The cap pause appends one typed display-only operator-attention row
    // (docs/TAILING.md) even though [notify] delivery is not configured.
    let notify_rows = jsonl_values(&state.run_root.join("notify.jsonl"));
    assert_eq!(notify_rows.len(), 1, "{notify_rows:?}");
    assert_eq!(notify_rows[0]["kind"], "operator_attention");
    assert_eq!(notify_rows[0]["reason"], "paused_at_cap");
    assert_eq!(notify_rows[0]["run_id"], serde_json::json!(state.run_id));
    let state_before = fs::read(state.state_path()).expect("state before refused resume");
    let resume_provider_sentinel = temp.path().join("resume-provider-started");
    fs::write(
        &fake_codex,
        "#!/bin/sh\nprintf started >\"${DEADRECKON_RESUME_SENTINEL:?}\"\nexit 99\n",
    )
    .expect("replace provider with resume sentinel");
    chmod_exec(&fake_codex);

    let resume = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("resume")
        .arg(&run.run_id)
        .arg("--max-wall-seconds")
        .arg("10")
        .arg("--no-docs")
        .env("DEADRECKON_HOME", &home)
        .env("DEADRECKON_RESUME_SENTINEL", &resume_provider_sentinel)
        .output()
        .expect("resume");
    assert!(
        !resume.status.success(),
        "{}{}",
        String::from_utf8_lossy(&resume.stdout),
        String::from_utf8_lossy(&resume.stderr)
    );
    let state = load_run(&paths, &run.run_id).expect("state");
    assert_eq!(
        fs::read(state.state_path()).expect("state after refused resume"),
        state_before
    );
    assert!(
        !resume_provider_sentinel.exists(),
        "the provider launched despite public resume retirement"
    );
    assert_ne!(state.status, RunStatus::Completed);
    let stderr = String::from_utf8_lossy(&resume.stderr);
    assert!(stderr.contains("public resume is retired"), "{stderr}");
    assert!(stderr.contains("no provider was started"), "{stderr}");
    assert!(stderr.contains("deadreckon start"), "{stderr}");
    assert!(stderr.contains("--mode run"), "{stderr}");
    assert!(stderr.contains("--from"), "{stderr}");
    assert!(stderr.contains("--yes"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn kill_storm_no_leaks() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let server = MockServer::start(kill_storm_script(10)).await;
    write_config(temp.path(), &server.base_url());
    let mut children = Vec::new();
    for idx in 0..10 {
        let scope_root = temp.path().join(format!("scope-{idx}"));
        fs::create_dir_all(&scope_root).expect("scope root");
        let child = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
            .arg("run")
            .arg("--fresh")
            .arg("--yes")
            .arg(format!("mock slow task {idx}"))
            .arg("--provider")
            .arg("mock")
            .arg("--sandbox")
            .arg("none")
            .arg("--untrusted")
            .arg("--max-spend")
            .arg("1")
            .env("DEADRECKON_HOME", &home)
            .env("DEADRECKON_SCOPE_ROOT", &scope_root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn run");
        children.push(child);
    }
    let paths = DeadreckonPaths::from_home(&home);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut run_ids = Vec::new();
    while run_ids.len() < 10 {
        run_ids = list_runs(&paths, None)
            .expect("runs")
            .into_iter()
            .filter_map(|run| {
                let state = load_run(&paths, &run.run_id).ok()?;
                if state.child_pids.is_empty() {
                    None
                } else {
                    Some(run.run_id)
                }
            })
            .collect();
        assert!(Instant::now() < deadline, "all run ids did not appear");
        std::thread::sleep(Duration::from_millis(25));
    }
    for run_id in &run_ids {
        let kill = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
            .arg("kill")
            .arg(run_id)
            .env("DEADRECKON_HOME", &home)
            .output()
            .expect("kill");
        assert!(kill.status.success());
    }
    for mut child in children {
        let _ = child.wait();
    }
    for run_id in &run_ids {
        let state = load_run(&paths, run_id).expect("state");
        assert_ne!(state.status, RunStatus::Executing);
        for pid in super_pids(&state) {
            assert!(!deadreckon_core::pid_is_alive(pid), "pid {pid} leaked");
        }
    }
    let lock_count = fs::read_dir(paths.locks_dir())
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(std::result::Result::ok))
        .count();
    assert_eq!(lock_count, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_fails_with_one_verdict_surface() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("doctor")
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("doctor");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("blocked doctor"), "{stdout}");
    assert!(stdout.contains("Explanation\n"), "{stdout}");
    assert!(stdout.contains("Evidence\n"), "{stdout}");
    assert!(stdout.contains("config.toml missing"));
    assert!(stdout.contains("disk space"));
    assert!(stdout.contains("runstate dir"));
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(stdout.contains("Recommended\ndeadreckon init"), "{stdout}");
    assert!(!stdout.contains("try:"), "{stdout}");
    assert!(!stdout.contains("fix:"), "{stdout}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_claude_code_roundtrip() {
    import_jsonl_roundtrip("claude-code", "CLAUDE_PROJECTS_DIR");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_codex_roundtrip() {
    import_jsonl_roundtrip("codex", "CODEX_SESSIONS_DIR");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_cursor_roundtrip() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("cursor");
    fs::create_dir_all(&root).expect("cursor root");
    let db = root.join("chats.db");
    let status = Command::new("sqlite3")
        .arg(&db)
        .arg("create table messages (role text, content text, tool_call_id text, path text); insert into messages values ('assistant','edited first','cursor-tool-1','cursor-one.md'); insert into messages values ('assistant','edited second','cursor-tool-2','cursor-two.md');")
        .status();
    if status.is_err() {
        eprintln!("skipping Cursor import test because sqlite3 is unavailable");
        return;
    }
    assert!(status.expect("sqlite status").success());
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("cursor")
        .env("DEADRECKON_HOME", &home)
        .env("DEADRECKON_IMPORT_CURSOR_ROOT", &root)
        .output()
        .expect("import");
    assert!(output.status.success());
    let run_id = imported_run_id(&output);
    let show = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("show")
        .arg(&run_id)
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("show");
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert!(stdout.contains("import.cursor"));
    assert!(stdout.contains("cursor-one.md"));
    assert!(stdout.contains("cursor-two.md"));
    assert!(stdout.contains("\"source_line\": 1"));
    assert!(stdout.contains("\"source_line\": 2"));
    let paths = DeadreckonPaths::from_home(&home);
    let state = load_run(&paths, &run_id).expect("state");
    assert_eq!(state.turn, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_claude_code_fixture_round_trips_to_golden() {
    import_fixture_roundtrip_golden(
        "claude-code",
        "CLAUDE_PROJECTS_DIR",
        "claude-code.show.golden",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_codex_fixture_round_trips_to_golden() {
    import_fixture_roundtrip_golden("codex", "CODEX_SESSIONS_DIR", "codex.show.golden");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_cursor_fixture_round_trips_to_golden() {
    if !command_available("sqlite3") {
        eprintln!("skipping Cursor golden import test because sqlite3 is unavailable");
        return;
    }
    import_fixture_roundtrip_golden(
        "cursor",
        "DEADRECKON_IMPORT_CURSOR_ROOT",
        "cursor.show.golden",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_jsonl_golden_normalizes_order_and_metadata() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("codex");
    fs::create_dir_all(root.join("nested")).expect("import root");
    fs::write(
        root.join("a.jsonl"),
        r#"{"role":"assistant","tool_call_id":"tool-1","files":["src/a.rs","src/b.rs"]}
"#,
    )
    .expect("a jsonl");
    fs::write(
        root.join("nested/z.jsonl"),
        r#"{"role":"assistant","tool_call_id":"tool-2","path":"src/c.rs"}
{"role":"assistant","tool_call_id":"tool-3","file":"README.md"}
"#,
    )
    .expect("z jsonl");

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("codex")
        .arg("--all")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("import");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let run_id = imported_run_id(&output);
    let paths = DeadreckonPaths::from_home(&home);
    let state = load_run(&paths, &run_id).expect("state");
    assert_eq!(state.turn, 3);
    let traces = jsonl_values(&state.run_root.join("traces.jsonl"));
    assert_eq!(traces.len(), 3);
    assert_eq!(traces[0]["turn"], 1);
    assert_eq!(traces[0]["detail"]["source_line"], 1);
    assert!(
        traces[0]["detail"]["source_path"]
            .as_str()
            .expect("source_path")
            .ends_with("a.jsonl")
    );
    assert_eq!(traces[1]["turn"], 2);
    assert_eq!(traces[1]["detail"]["source_line"], 1);
    assert!(
        traces[1]["detail"]["source_path"]
            .as_str()
            .expect("source_path")
            .ends_with("nested/z.jsonl")
    );
    assert_eq!(traces[2]["detail"]["source_line"], 2);
    let provenance =
        fs::read_to_string(state.run_root.join("provenance.jsonl")).expect("provenance");
    assert!(provenance.contains("src/a.rs"));
    assert!(provenance.contains("src/b.rs"));
    assert!(provenance.contains("src/c.rs"));
    assert!(provenance.contains("README.md"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_renders_provenance_lines_for_each_file_change() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("home");
    let root = temp.path().join("import-root");
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&root).expect("root");
    fs::create_dir_all(&cwd).expect("workspace");
    fs::write(
        root.join("session.jsonl"),
        r#"{"tool_call_id":"multi-file","files":["one.md","two.md","three.md"]}
"#,
    )
    .expect("jsonl");

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&cwd)
        .arg("import")
        .arg("codex")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("import");
    assert!(output.status.success());
    let run_id = imported_run_id(&output);
    let show = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&cwd)
        .arg("show")
        .arg(&run_id)
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("show");
    assert!(show.status.success());
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert!(stdout.contains("\"tool_call_id\": \"multi-file\""));
    assert!(stdout.contains("\"one.md\""));
    assert!(stdout.contains("\"two.md\""));
    assert!(stdout.contains("\"three.md\""));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_normalizes_timestamps_to_rfc3339() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("home");
    let root = temp.path().join("import-root");
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&root).expect("root");
    fs::create_dir_all(&cwd).expect("workspace");
    fs::write(root.join("session.jsonl"), "{\"path\":\"timed.md\"}\n").expect("jsonl");

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&cwd)
        .arg("import")
        .arg("codex")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("import");
    assert!(output.status.success());
    let run_id = imported_run_id(&output);
    let paths = DeadreckonPaths::from_home(&home);
    let state = load_run(&paths, &run_id).expect("state");
    let traces = jsonl_values(&state.run_root.join("traces.jsonl"));
    let timestamp = traces[0]["timestamp"].as_str().expect("timestamp");
    chrono::DateTime::parse_from_rfc3339(timestamp).expect("RFC3339 timestamp");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_jsonl_malformed_line_fails_actionably() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("codex");
    fs::create_dir_all(&root).expect("import root");
    fs::write(root.join("bad.jsonl"), "{\"ok\":true}\n{not-json}\n").expect("bad jsonl");

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("codex")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("import");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("malformed JSONL"), "{stderr}");
    assert!(stderr.contains("bad.jsonl:2"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_current_pointer_uses_imported_run_id() {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("home");
    let root = temp.path().join("codex");
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&root).expect("import root");
    fs::create_dir_all(&cwd).expect("workspace");
    fs::write(root.join("session.jsonl"), "{\"path\":\"current.md\"}\n").expect("jsonl");
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&cwd)
        .arg("import")
        .arg("codex")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("import");
    assert!(output.status.success());
    let run_id = imported_run_id(&output);
    let status = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&cwd)
        .arg("status")
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("status");
    assert!(status.status.success());
    assert!(
        String::from_utf8_lossy(&status.stdout).contains(&run_id[..8]),
        "{}",
        String::from_utf8_lossy(&status.stdout)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reimport_changed_session_requires_replace() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("codex");
    fs::create_dir_all(&root).expect("import root");
    let session = root.join("session.jsonl");
    fs::write(&session, "{\"path\":\"first.md\"}\n").expect("first jsonl");
    let first = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("codex")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("first import");
    assert!(first.status.success());
    let first_id = imported_run_id(&first);

    fs::write(&session, "{\"path\":\"second.md\"}\n").expect("second jsonl");
    let second = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("codex")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("second import");
    assert!(!second.status.success());
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(stderr.contains("changed content"), "{stderr}");
    assert!(stderr.contains("blocked import"), "{stderr}");
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(
        stderr.contains("Recommended\ndeadreckon import codex"),
        "{stderr}"
    );
    assert!(stderr.contains("--replace"), "{stderr}");
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");

    let replaced = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("codex")
        .arg("--replace")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("replace import");
    assert!(
        replaced.status.success(),
        "{}{}",
        String::from_utf8_lossy(&replaced.stdout),
        String::from_utf8_lossy(&replaced.stderr)
    );
    let second_id = imported_run_id(&replaced);
    assert_eq!(first_id, second_id);

    let paths = DeadreckonPaths::from_home(&home);
    let runs = list_runs(&paths, None).expect("runs");
    assert_eq!(runs.len(), 1);
    let state = load_run(&paths, &second_id).expect("state");
    let traces = fs::read_to_string(state.run_root.join("traces.jsonl")).expect("traces");
    assert!(!traces.contains("first.md"));
    assert!(traces.contains("second.md"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_accepts_descriptor_provider_ids_and_legacy_aliases() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let codex = temp.path().join("codex");
    let claude = temp.path().join("claude");
    let gemini = temp.path().join("gemini");
    let opencode = temp.path().join("opencode");
    let copilot = temp.path().join("copilot");
    let pi = temp.path().join("pi");
    let cursor = temp.path().join("cursor");
    for root in [&codex, &claude, &gemini, &opencode, &copilot, &pi, &cursor] {
        fs::create_dir_all(root).expect("provider root");
    }
    for source in [
        "codex",
        "claude-code",
        "gemini",
        "opencode",
        "copilot",
        "pi",
        "cursor",
        "cli:claude-code",
        "cli:codex",
        "cli:gemini",
        "cli:opencode",
        "cli:copilot",
        "cli:pi",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
            .arg("import")
            .arg(source)
            .arg("--list")
            .env("DEADRECKON_HOME", &home)
            .env("CODEX_SESSIONS_DIR", &codex)
            .env("CLAUDE_PROJECTS_DIR", &claude)
            .env("GEMINI_DIR", &gemini)
            .env("OPENCODE_DIR", &opencode)
            .env("COPILOT_DIR", &copilot)
            .env("PI_CODING_AGENT_SESSION_DIR", &pi)
            .env("DEADRECKON_IMPORT_CURSOR_ROOT", &cursor)
            .output()
            .expect("import list");
        assert!(
            output.status.success(),
            "{source}\n{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("source:"),
            "{source}\n{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_preview_does_not_create_run() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("codex");
    fs::create_dir_all(&root).expect("root");
    fs::write(root.join("session.jsonl"), "{\"path\":\"preview.md\"}\n").expect("jsonl");
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("codex")
        .arg("--preview")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("preview");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.starts_with("preview import"), "{stdout}");
    assert!(stdout.contains("Explanation\n"), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(
        stdout.contains("Recommended\ndeadreckon import codex --session"),
        "{stdout}"
    );
    assert!(!stdout.contains("try:"), "{stdout}");
    let paths = DeadreckonPaths::from_home(&home);
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_list_and_complete_surfaces_have_one_verdict_and_primary_action() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("codex");
    fs::create_dir_all(&root).expect("root");
    fs::write(root.join("session.jsonl"), "{\"path\":\"surface.md\"}\n").expect("jsonl");

    let list = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("codex")
        .arg("--list")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("import list");
    assert!(list.status.success());
    let list_stdout = String::from_utf8_lossy(&list.stdout);
    assert!(list_stdout.starts_with("preview import"), "{list_stdout}");
    assert!(list_stdout.contains("Explanation\n"), "{list_stdout}");
    assert!(list_stdout.contains("Evidence\n"), "{list_stdout}");
    assert_eq!(
        list_stdout.matches("\nRecommended\n").count(),
        1,
        "{list_stdout}"
    );
    assert!(
        list_stdout.contains("Recommended\ndeadreckon import codex --session <id-or-path>"),
        "{list_stdout}"
    );
    assert!(!list_stdout.contains("try:"), "{list_stdout}");

    let import = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("codex")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("import");
    assert!(import.status.success());
    let import_stdout = String::from_utf8_lossy(&import.stdout);
    assert!(
        import_stdout.starts_with("completed import"),
        "{import_stdout}"
    );
    assert!(import_stdout.contains("Explanation\n"), "{import_stdout}");
    assert!(import_stdout.contains("Evidence\n"), "{import_stdout}");
    assert_eq!(
        import_stdout.matches("\nRecommended\n").count(),
        1,
        "{import_stdout}"
    );
    let run_id = imported_run_id(&import);
    assert!(
        import_stdout.contains(&format!("Recommended\ndeadreckon show {run_id}")),
        "{import_stdout}"
    );
    assert!(!import_stdout.contains("try:"), "{import_stdout}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_json_adds_verdict_and_primary_action() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("codex");
    fs::create_dir_all(&root).expect("root");
    fs::write(root.join("session.jsonl"), "{\"path\":\"json.md\"}\n").expect("jsonl");

    let preview = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("codex")
        .arg("--preview")
        .arg("--json")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("preview json");
    assert!(preview.status.success());
    let preview_json: Value = serde_json::from_slice(&preview.stdout).expect("preview json");
    assert_eq!(preview_json["kind"], "import_preview");
    assert_eq!(
        preview_json["primary_action"],
        preview_json["verdict"]["recommended_command"]
    );
    assert_eq!(
        preview_json["primary_action"], preview_json["try_lines"][0],
        "{preview_json:#?}"
    );

    let completed = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("codex")
        .arg("--json")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("import json");
    assert!(completed.status.success());
    let completed_json: Value = serde_json::from_slice(&completed.stdout).expect("completed json");
    assert_eq!(completed_json["kind"], "import_completed");
    assert_eq!(
        completed_json["primary_action"],
        completed_json["verdict"]["recommended_command"]
    );
    assert_eq!(
        completed_json["primary_action"], completed_json["try_lines"][0],
        "{completed_json:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_unknown_source_lists_supported_sources_with_verdict_surface() {
    let temp = repo_tempdir();
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("unknown-agent")
        .env("DEADRECKON_HOME", temp.path().join("home"))
        .output()
        .expect("import");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("blocked import"), "{stderr}");
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert!(stderr.contains("accepted sources"), "{stderr}");
    assert!(stderr.contains("cli:copilot"), "{stderr}");
    assert!(
        stderr.contains("Recommended\ndeadreckon import codex --list"),
        "{stderr}"
    );
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(!stderr.contains("try: deadreckon doctor"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_writes_manifest_with_source_schema_hash_and_reimport_command() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("codex");
    fs::create_dir_all(&root).expect("root");
    fs::write(root.join("session.jsonl"), "{\"path\":\"manifest.md\"}\n").expect("jsonl");
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("codex")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("import");
    assert!(output.status.success());
    let run_id = imported_run_id(&output);
    let paths = DeadreckonPaths::from_home(&home);
    let state = load_run(&paths, &run_id).expect("state");
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(state.run_root.join("import.json")).expect("manifest"),
    )
    .expect("manifest json");
    assert_eq!(manifest["source"], "cli:codex");
    assert_eq!(manifest["schema"], "codex-cli");
    assert_eq!(manifest["storage"], "jsonl");
    assert!(
        manifest["content_hash"]
            .as_str()
            .expect("content_hash")
            .starts_with("sha256:")
    );
    assert!(
        manifest["reimport_command"]
            .as_str()
            .expect("reimport")
            .contains("--replace")
    );
    assert_eq!(manifest["events_imported"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_ambiguous_sessions_prints_candidate_table_and_verdict_surface() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("codex");
    fs::create_dir_all(&root).expect("root");
    fs::write(root.join("one.jsonl"), "{\"path\":\"one.md\"}\n").expect("one");
    fs::write(root.join("two.jsonl"), "{\"path\":\"two.md\"}\n").expect("two");
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("codex")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("import");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("two.jsonl") || stderr.contains("codex:two"),
        "{stderr}"
    );
    assert!(
        stderr.contains("one.jsonl") || stderr.contains("codex:one"),
        "{stderr}"
    );
    assert!(
        stderr.contains("Recommended\ndeadreckon import codex --session"),
        "{stderr}"
    );
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(!stderr.contains("try: deadreckon doctor"), "{stderr}");
    let paths = DeadreckonPaths::from_home(&home);
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_stale_sessions_refuse_with_candidate_table_and_verdict_surface() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("codex");
    fs::create_dir_all(&root).expect("root");
    fs::write(root.join("stale.jsonl"), "{\"path\":\"stale.md\"}\n").expect("stale");
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("codex")
        .arg("--since")
        .arg("0s")
        .env("DEADRECKON_HOME", &home)
        .env("CODEX_SESSIONS_DIR", &root)
        .output()
        .expect("import");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("stale candidates"), "{stderr}");
    assert!(
        stderr.contains("stale.jsonl") || stderr.contains("codex:stale"),
        "{stderr}"
    );
    assert!(
        stderr.contains("Recommended\ndeadreckon import codex --since 1d --preview"),
        "{stderr}"
    );
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(
        !stderr.contains("try: deadreckon import codex --session"),
        "{stderr}"
    );
    assert!(!stderr.contains("try: deadreckon doctor"), "{stderr}");
    let paths = DeadreckonPaths::from_home(&home);
    assert!(list_runs(&paths, None).expect("runs").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_gemini_requires_session_when_cwd_match_is_none_and_ambiguous() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("gemini");
    fs::create_dir_all(root.join("tmp")).expect("root");
    fs::write(
        root.join("tmp/one.json"),
        r#"{"sessionId":"one","messages":[]}"#,
    )
    .expect("one");
    fs::write(
        root.join("tmp/two.json"),
        r#"{"sessionId":"two","messages":[]}"#,
    )
    .expect("two");
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("cli:gemini")
        .env("DEADRECKON_HOME", &home)
        .env("GEMINI_DIR", &root)
        .output()
        .expect("import");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("ambiguous import candidates"), "{stderr}");
    assert!(
        stderr.contains("Recommended\ndeadreckon import cli:gemini --session"),
        "{stderr}"
    );
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(!stderr.contains("try: deadreckon doctor"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_gemini_fixture_round_trips_to_show() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("gemini");
    let session = root.join("tmp/session.json");
    fs::create_dir_all(session.parent().expect("parent")).expect("root");
    fs::write(
        &session,
        json!({
            "sessionId": "gemini-session",
            "messages": [{
                "type": "gemini",
                "timestamp": "2026-05-13T18:39:00Z",
                "content": [{"text": "gemini edit"}],
                "toolCalls": [{
                    "id": "gemini-tool-1",
                    "name": "write_file",
                    "args": {"path": "src/gemini.rs"}
                }],
                "tokens": {"input": 10, "output": 2, "cached": 1}
            }]
        })
        .to_string(),
    )
    .expect("gemini");
    let run_id = import_run_with_env(
        "cli:gemini",
        &[("--session", session.as_path())],
        &home,
        &[("GEMINI_DIR", root.as_path())],
    );
    let show = show_import_run(&home, &run_id, temp.path());
    assert!(show.contains("gemini edit"), "{show}");
    assert!(show.contains("src/gemini.rs"), "{show}");
    assert!(show.contains("\"schema\": \"gemini\""), "{show}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_opencode_file_mode_fixture_round_trips_to_show() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("opencode");
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&cwd).expect("cwd");
    let session = root.join("storage/session/project/session.json");
    let message_dir = root.join("storage/message/opencode-session");
    let part_dir = root.join("storage/part/message-1");
    fs::create_dir_all(session.parent().expect("session parent")).expect("session root");
    fs::create_dir_all(&message_dir).expect("message root");
    fs::create_dir_all(&part_dir).expect("part root");
    fs::write(
        &session,
        json!({"id":"opencode-session","directory": cwd, "time":{"created":1770000000000_i64}})
            .to_string(),
    )
    .expect("session");
    fs::write(
        message_dir.join("message-1.json"),
        json!({"id":"message-1","role":"assistant","time":{"created":1770000000100_i64}})
            .to_string(),
    )
    .expect("message");
    fs::write(
        part_dir.join("part-1.json"),
        json!({"id":"part-1","type":"text","content":"opencode edit","time":{"created":1770000000200_i64}})
            .to_string(),
    )
    .expect("text part");
    fs::write(
        part_dir.join("part-2.json"),
        json!({"id":"part-2","type":"tool","tool":"write","state":{"input":{"path":"src/opencode.rs"}},"time":{"created":1770000000300_i64}})
            .to_string(),
    )
    .expect("tool part");
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&cwd)
        .arg("import")
        .arg("cli:opencode")
        .env("DEADRECKON_HOME", &home)
        .env("OPENCODE_DIR", &root)
        .output()
        .expect("import");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let run_id = imported_run_id(&output);
    let show = show_import_run_from(&home, &run_id, &cwd);
    assert!(show.contains("opencode edit"), "{show}");
    assert!(show.contains("src/opencode.rs"), "{show}");
    assert!(show.contains("\"schema\": \"opencode\""), "{show}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_copilot_nested_events_file_is_discovered() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("copilot");
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&cwd).expect("cwd");
    let events = root.join("session-state/nested/events.jsonl");
    fs::create_dir_all(events.parent().expect("events parent")).expect("events root");
    fs::write(
        &events,
        format!(
            "{}\n",
            json!({
                "type": "assistant.message",
                "timestamp": "2026-05-13T18:39:00Z",
                "data": {
                    "context": {"cwd": cwd},
                    "content": "copilot edit",
                    "toolRequests": [{
                        "id": "copilot-tool-1",
                        "name": "write_file",
                        "arguments": {"path": "src/copilot.rs"}
                    }]
                },
                "usage": {"inputTokens": 4, "outputTokens": 2}
            })
        ),
    )
    .expect("events");
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&cwd)
        .arg("import")
        .arg("cli:copilot")
        .env("DEADRECKON_HOME", &home)
        .env("COPILOT_DIR", &root)
        .output()
        .expect("import");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let run_id = imported_run_id(&output);
    let show = show_import_run_from(&home, &run_id, &cwd);
    assert!(show.contains("copilot edit"), "{show}");
    assert!(show.contains("src/copilot.rs"), "{show}");
    assert!(show.contains("\"schema\": \"copilot-cli\""), "{show}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_pi_fixture_round_trips_to_show() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("pi");
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&root).expect("root");
    fs::create_dir_all(&cwd).expect("cwd");
    fs::write(
        root.join("session.jsonl"),
        format!(
            "{}\n{}\n",
            json!({"type":"session","id":"pi-session","cwd": cwd}),
            json!({
                "type": "message",
                "message": {
                    "role": "assistant",
                    "content": [
                        {"type": "text", "text": "pi edit"},
                        {"type": "toolCall", "id": "pi-tool-1", "name": "write", "arguments": {"path": "src/pi.rs"}}
                    ],
                    "usage": {"input_tokens": 5, "output_tokens": 3}
                }
            })
        ),
    )
    .expect("pi");
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&cwd)
        .arg("import")
        .arg("cli:pi")
        .env("DEADRECKON_HOME", &home)
        .env("PI_CODING_AGENT_SESSION_DIR", &root)
        .output()
        .expect("import");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let run_id = imported_run_id(&output);
    let show = show_import_run_from(&home, &run_id, &cwd);
    assert!(show.contains("pi edit"), "{show}");
    assert!(show.contains("src/pi.rs"), "{show}");
    assert!(show.contains("\"schema\": \"pi\""), "{show}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_cursor_writes_manifest_with_sqlite_source() {
    if !command_available("sqlite3") {
        eprintln!("skipping Cursor manifest test because sqlite3 is unavailable");
        return;
    }
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join("cursor");
    fs::create_dir_all(&root).expect("cursor root");
    let db = root.join("chats.db");
    let status = Command::new("sqlite3")
        .arg(&db)
        .arg("create table messages (role text, content text, tool_call_id text, path text); insert into messages values ('assistant','cursor manifest','cursor-tool','cursor.md');")
        .status()
        .expect("sqlite");
    assert!(status.success());
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg("cursor")
        .env("DEADRECKON_HOME", &home)
        .env("DEADRECKON_IMPORT_CURSOR_ROOT", &root)
        .output()
        .expect("import");
    assert!(output.status.success());
    let run_id = imported_run_id(&output);
    let paths = DeadreckonPaths::from_home(&home);
    let state = load_run(&paths, &run_id).expect("state");
    let manifest: Value = serde_json::from_str(
        &fs::read_to_string(state.run_root.join("import.json")).expect("manifest"),
    )
    .expect("manifest");
    assert_eq!(manifest["source"], "cursor");
    assert_eq!(manifest["schema"], "cursor-sqlite");
    assert_eq!(manifest["rows_seen"], 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn stress_5_concurrent_10min() {
    if std::env::var("DEADRECKON_STRESS").ok().as_deref() != Some("1") {
        return;
    }
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let seconds = std::env::var("DEADRECKON_STRESS_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(600);
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let fake_codex = temp.path().join("fake-codex-stress");
    fs::write(
        &fake_codex,
        format!(
            r#"#!/bin/sh
prompt="$*"
case "$prompt" in
  *narrator-overview*)
    printf '%s\n' '{{"why_now":"The stress harness is verifying concurrent run completion.","high_level_approach":"Five concurrent runs wrote notes.md after the configured provider delay.","open_threads":[],"cross_references":["stress harness"]}}'
    exit 0
    ;;
  *narrator-phases*)
    printf '%s\n' '{{"phases_markdown":"Stress runs\n\n- notes.md was written by the fake provider in each concurrent run.\n"}}'
    exit 0
    ;;
  *narrator-as-built*)
    printf '%s\n' '{{"system_overview":"The stress harness exercises concurrent CLI provider runs.","source_layout":"notes.md records the isolated run scope.\n","components":[],"load_bearing_paths":"notes.md","seams":"CLI provider invocation and run-state locking."}}'
    exit 0
    ;;
  *narrator-decisions*)
    printf '%s\n' '{{"decisions":[]}}'
    exit 0
    ;;
esac
printf 'scope:%s\n' "$PWD" > notes.md
sleep {seconds}
printf 'done\n'
"#
        ),
    )
    .expect("fake codex");
    chmod_exec(&fake_codex);
    write_cli_config(temp.path(), &fake_codex);

    let mut children = Vec::new();
    for idx in 0..5 {
        let scope_root = temp.path().join(format!("stress-scope-{idx}"));
        fs::create_dir_all(&scope_root).expect("scope root");
        children.push(
            Command::new(env!("CARGO_BIN_EXE_deadreckon"))
                .arg("run")
                .arg("--fresh")
                .arg("--yes")
                .arg(format!("stress run {idx}"))
                .arg("--provider")
                .arg("cli:codex")
                .arg("--sandbox")
                .arg("none")
                .arg("--untrusted")
                .arg("--max-spend")
                .arg("1")
                .arg("--max-wall-seconds")
                .arg((seconds + 60).to_string())
                .env("DEADRECKON_HOME", &home)
                .env("DEADRECKON_SCOPE_ROOT", &scope_root)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn stress run"),
        );
    }

    for child in children {
        let output = child.wait_with_output().expect("stress wait");
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let paths = DeadreckonPaths::from_home(&home);
    let runs = list_runs(&paths, None).expect("runs");
    assert_eq!(runs.len(), 5);
    let mut seen_scopes = std::collections::BTreeSet::new();
    for run in runs {
        let state = load_run(&paths, &run.run_id).expect("state");
        assert_eq!(state.status, RunStatus::Completed);
        assert!(seen_scopes.insert(state.scope.clone()));
        let provenance =
            fs::read_to_string(state.run_root.join("provenance.jsonl")).expect("provenance");
        assert!(provenance.contains(&state.run_id));
    }
    let lock_count = fs::read_dir(paths.locks_dir())
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(std::result::Result::ok))
        .count();
    assert_eq!(lock_count, 0);
}

fn write_config(temp: &std::path::Path, base_url: &str) {
    write_config_with_extra(temp, base_url, "");
}

fn write_config_with_extra(temp: &std::path::Path, base_url: &str, extra: &str) {
    let home = temp.join("home");
    fs::create_dir_all(&home).expect("home");
    fs::write(
        home.join("config.toml"),
        format!(
            r#"
fallback = ["mock"]

[providers.mock]
kind = "open-ai-compatible"
base_url = "{base_url}"
model = "mock-agent"
api_key = "test"
input_cost_per_million = 1.0
output_cost_per_million = 1.0
"#,
        ) + extra,
    )
    .expect("config");
}

fn toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r#"'\''"#))
}

fn write_cli_config(temp: &std::path::Path, binary: &std::path::Path) {
    let home = temp.join("home");
    fs::create_dir_all(&home).expect("home");
    let providers_dir = home.join("providers.d");
    fs::create_dir_all(&providers_dir).expect("providers");
    fs::write(
        home.join("config.toml"),
        format!(
            r#"
fallback = ["cli:codex"]

[providers."cli:codex"]
kind = "cli-codex"
binary = "{}"
"#,
            binary.display()
        ),
    )
    .expect("config");
    fs::write(
        providers_dir.join("cli-codex.toml"),
        r#"
id = "cli:codex"

[ingest]
default_dirs = []
"#,
    )
    .expect("provider override");
}

fn chmod_exec(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }
}

fn super_pids(state: &deadreckon_core::PipelineState) -> Vec<u32> {
    let mut pids = state.child_pids.clone();
    let pid_dir = state.run_root.join("child-pids");
    if let Ok(entries) = fs::read_dir(pid_dir) {
        for entry in entries.flatten() {
            if let Ok(raw) = fs::read_to_string(entry.path()) {
                for line in raw.lines() {
                    if let Ok(pid) = line.trim().parse() {
                        pids.push(pid);
                    }
                }
            }
        }
    }
    pids
}

fn import_jsonl_roundtrip(source: &str, env_name: &str) {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let root = temp.path().join(source);
    let cwd = std::env::current_dir().expect("cwd");
    let storage_root = import_storage_root(source, &root, &cwd);
    fs::create_dir_all(&storage_root).expect("import root");
    fs::write(
        storage_root.join("session.jsonl"),
        r#"{"role":"assistant","content":"tool call","tool_call_id":"tool-1","path":"notes.md"}
{"role":"assistant","content":"file edit","tool_call_id":"tool-2","file":"src/main.rs"}
"#,
    )
    .expect("jsonl");
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("import")
        .arg(source)
        .env("DEADRECKON_HOME", &home)
        .env(env_name, &root)
        .output()
        .expect("import");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let run_id = imported_run_id(&output);
    let show = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("show")
        .arg(&run_id)
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("show");
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert!(stdout.contains(&format!("import.{source}")));
    assert!(stdout.contains("notes.md"));
    assert!(stdout.contains("src/main.rs"));
}

fn import_fixture_roundtrip_golden(source: &str, env_name: &str, golden_name: &str) {
    let temp = TempDir::new().expect("tempdir");
    let home = temp.path().join("home");
    let root = temp.path().join("import-root");
    let cwd = temp.path().join("workspace");
    let storage_root = import_storage_root(source, &root, &cwd);
    fs::create_dir_all(&cwd).expect("workspace");
    if source == "cursor" {
        fs::create_dir_all(&root).expect("cursor root");
        let sql = fs::read_to_string(Path::new(IMPORT_FIXTURES).join(source).join("messages.sql"))
            .expect("cursor sql fixture");
        let status = Command::new("sqlite3")
            .arg(root.join("chats.db"))
            .arg(sql)
            .status()
            .expect("sqlite3 cursor fixture");
        assert!(status.success());
    } else {
        copy_test_dir(&Path::new(IMPORT_FIXTURES).join(source), &storage_root);
    }

    let mut import = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    import
        .current_dir(&cwd)
        .arg("import")
        .arg(source)
        .env("DEADRECKON_HOME", &home)
        .env(env_name, &root);
    if source != "cursor" {
        import
            .arg("--session")
            .arg(storage_root.join("session.jsonl"));
    }
    let output = import.output().expect("import");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let run_id = imported_run_id(&output);
    let paths = DeadreckonPaths::from_home(&home);
    let state = load_run(&paths, &run_id).expect("state");
    let show = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(&cwd)
        .arg("show")
        .arg(&run_id)
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("show");
    assert!(
        show.status.success(),
        "{}{}",
        String::from_utf8_lossy(&show.stdout),
        String::from_utf8_lossy(&show.stderr)
    );
    let actual = normalize_import_show(
        &String::from_utf8_lossy(&show.stdout),
        temp.path(),
        &home,
        if source == "cursor" {
            &root
        } else {
            &storage_root
        },
        &cwd,
        &run_id,
        &state.scope,
    );
    let golden_path = Path::new(IMPORT_GOLDENS).join(golden_name);
    let expected = fs::read_to_string(&golden_path).unwrap_or_default();
    assert_eq!(
        actual,
        expected,
        "golden mismatch for {}\npath: {}\n--- actual ---\n{}\n--- expected ---\n{}",
        source,
        golden_path.display(),
        actual,
        expected
    );
}

fn import_storage_root(source: &str, root: &Path, cwd: &Path) -> PathBuf {
    if source == "claude-code" {
        return root.join(test_claude_project_name(cwd));
    }
    root.to_path_buf()
}

fn test_claude_project_name(working_dir: &Path) -> String {
    let resolved = fs::canonicalize(working_dir).unwrap_or_else(|_| working_dir.to_path_buf());
    let raw = resolved.to_string_lossy();
    let mut name = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            name.push(ch);
        } else {
            name.push('-');
        }
    }
    if !name.starts_with('-') {
        name.insert(0, '-');
    }
    name
}

fn copy_test_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("copy dest");
    for entry in fs::read_dir(from).expect("fixture dir") {
        let entry = entry.expect("fixture entry");
        let source = entry.path();
        let dest = to.join(entry.file_name());
        if source.is_dir() {
            copy_test_dir(&source, &dest);
        } else {
            fs::copy(&source, &dest).expect("copy fixture file");
        }
    }
}

fn normalize_import_show(
    raw: &str,
    temp: &Path,
    home: &Path,
    root: &Path,
    cwd: &Path,
    run_id: &str,
    scope: &str,
) -> String {
    let mut text = raw.to_string();
    for (from, to) in [
        (root, "<IMPORT_ROOT>"),
        (home, "<HOME>"),
        (cwd, "<CWD>"),
        (temp, "<TEMP>"),
    ] {
        if let Ok(canonical) = from.canonicalize() {
            text = text.replace(&canonical.display().to_string(), to);
        }
        text = text.replace(&from.display().to_string(), to);
    }
    if let Some(source_root) = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2) {
        text = text.replace(&source_root.display().to_string(), "<SOURCE_ROOT>");
    }
    text = text.replace(run_id, "<RUN_ID>");
    text = text.replace(scope, "<SCOPE>");
    text = normalize_wrapped_import_show_paths(&text);
    let mut normalized = text
        .lines()
        .map(normalize_import_show_line)
        .collect::<Vec<_>>()
        .join("\n");
    normalized.push('\n');
    normalized
}

fn normalize_wrapped_import_show_paths(text: &str) -> String {
    text.replace("<HOME>/ru\n            nstate/", "<HOME>/runstate/")
        .replace("<TEMP>\n            /workspace", "<CWD>")
}

fn normalize_import_show_line(line: &str) -> String {
    let trimmed = line.trim_start();
    for key in ["started_at", "updated_at", "timestamp"] {
        let quoted = format!("\"{key}\":");
        if trimmed.starts_with(&quoted) {
            let indent = &line[..line.len() - trimmed.len()];
            let comma = if trimmed.ends_with(',') { "," } else { "" };
            return format!("{indent}\"{key}\": \"<TIMESTAMP>\"{comma}");
        }
    }
    line.to_string()
}

fn command_available(program: &str) -> bool {
    Command::new(program)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

fn imported_run_id(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            line.strip_prefix("imported ")
                .or_else(|| line.strip_prefix("completed import "))
        })
        .expect("imported id")
        .to_string()
}

fn import_run_with_env(
    source: &str,
    path_args: &[(&str, &Path)],
    home: &Path,
    envs: &[(&str, &Path)],
) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command
        .arg("import")
        .arg(source)
        .env("DEADRECKON_HOME", home);
    for (flag, path) in path_args {
        command.arg(flag).arg(path);
    }
    for (name, value) in envs {
        command.env(name, value);
    }
    let output = command.output().expect("import");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    imported_run_id(&output)
}

fn show_import_run(home: &Path, run_id: &str, cwd: &Path) -> String {
    show_import_run_from(home, run_id, cwd)
}

fn show_import_run_from(home: &Path, run_id: &str, cwd: &Path) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .current_dir(cwd)
        .arg("show")
        .arg(run_id)
        .env("DEADRECKON_HOME", home)
        .output()
        .expect("show");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn kill_storm_script(count: usize) -> Vec<FixtureResponse> {
    (0..count)
        .map(|_| FixtureResponse {
            content:
                "{\"action\":\"bash\",\"tool_call_id\":\"tool-slow\",\"command\":\"sleep 30\"}"
                    .to_string(),
            delay_ms: Some(30000),
            status: None,
            prompt_tokens: 100,
            completion_tokens: 20,
        })
        .collect()
}

fn wait_for_run_id(paths: &DeadreckonPaths) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(run) = list_runs(paths, None).expect("runs").into_iter().next()
            && !load_run(paths, &run.run_id)
                .expect("state")
                .child_pids
                .is_empty()
        {
            return run.run_id;
        }
        assert!(Instant::now() < deadline, "run state did not appear");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_run_events(run_root: &Path, needles: &[&str]) -> String {
    let events_path = run_root.join(RUN_EVENTS_JSONL);
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut last_events = String::new();
    loop {
        if let Ok(events) = fs::read_to_string(&events_path) {
            if needles.iter().all(|needle| events.contains(needle)) {
                return events;
            }
            last_events = events;
        }
        assert!(
            Instant::now() < deadline,
            "run events did not contain {needles:?}; events:\n{last_events}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_child_exit(child: &mut std::process::Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().expect("try wait").is_some() {
            return true;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn assert_jsonl_count(path: &std::path::Path, min: usize) {
    let count = fs::read_to_string(path)
        .expect("jsonl")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    assert!(
        count >= min,
        "{} has {count} lines, expected >= {min}",
        path.display()
    );
}

fn jsonl_values(path: &std::path::Path) -> Vec<Value> {
    fs::read_to_string(path)
        .expect("jsonl")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("json line"))
        .collect()
}

fn assert_provenance_ids_match_traces(run_root: &std::path::Path) {
    let traces = fs::read_to_string(run_root.join("traces.jsonl")).expect("traces");
    let provenance = fs::read_to_string(run_root.join("provenance.jsonl")).expect("provenance");
    for line in provenance.lines().filter(|line| !line.trim().is_empty()) {
        let value: Value = serde_json::from_str(line).expect("provenance json");
        let tool_call_id = value["tool_call_id"].as_str().expect("tool_call_id");
        assert!(
            traces.contains(tool_call_id),
            "trace missing tool_call_id {tool_call_id}"
        );
    }
}

#[derive(Clone)]
struct MockState {
    fixtures: Arc<Mutex<Vec<FixtureResponse>>>,
    journal: Arc<Mutex<Vec<Value>>>,
}

#[derive(Debug, Clone, Deserialize)]
struct FixtureResponse {
    content: String,
    delay_ms: Option<u64>,
    /// Non-2xx makes the fixture an error response with `content` as the
    /// error message — for exercising retry/failure paths.
    status: Option<u16>,
    prompt_tokens: u64,
    completion_tokens: u64,
}

struct MockServer {
    addr: SocketAddr,
    state: MockState,
}

impl MockServer {
    async fn start(fixtures: Vec<FixtureResponse>) -> Self {
        let state = MockState {
            fixtures: Arc::new(Mutex::new(fixtures)),
            journal: Arc::new(Mutex::new(Vec::new())),
        };
        let app = Router::new()
            .route("/chat/completions", post(chat_completions))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        Self { addr, state }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn journal(&self) -> Vec<Value> {
        self.state.journal.lock().expect("journal").clone()
    }
}

async fn chat_completions(
    State(state): State<MockState>,
    Json(request): Json<Value>,
) -> impl IntoResponse {
    state.journal.lock().expect("journal").push(request);
    let fixture = {
        let mut fixtures = state.fixtures.lock().expect("fixtures");
        if fixtures.is_empty() {
            None
        } else {
            Some(fixtures.remove(0))
        }
    };
    let Some(fixture) = fixture else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": {"message": "no fixture response left"}})),
        );
    };
    if let Some(delay_ms) = fixture.delay_ms {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
    }
    if let Some(status) = fixture.status
        && status >= 400
    {
        return (
            StatusCode::from_u16(status).expect("fixture status"),
            Json(json!({"error": {"message": fixture.content}})),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "id": "mock",
            "object": "chat.completion",
            "model": "mock-agent",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": fixture.content},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": fixture.prompt_tokens,
                "completion_tokens": fixture.completion_tokens,
                "total_tokens": fixture.prompt_tokens + fixture.completion_tokens
            }
        })),
    )
}

/// A 503 in front of the normal three-turn script: the loop must retry once
/// and complete as if nothing happened.
fn transient_then_success_script() -> Vec<FixtureResponse> {
    let mut script = three_turn_script();
    script.insert(
        0,
        serde_json::from_value(json!({
            "content": "mock overloaded",
            "status": 503,
            "prompt_tokens": 0,
            "completion_tokens": 0
        }))
        .expect("fixture"),
    );
    script
}

/// Nothing but 503s: the retry must also fail and the run must persist a
/// Failed status with a reason instead of lingering as Executing.
fn always_failing_script() -> Vec<FixtureResponse> {
    serde_json::from_value(json!([
        {"content": "mock overloaded", "status": 503, "prompt_tokens": 0, "completion_tokens": 0},
        {"content": "mock overloaded", "status": 503, "prompt_tokens": 0, "completion_tokens": 0}
    ]))
    .expect("script")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reshape_action_writes_inert_proposal_and_event() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let server = MockServer::start(reshape_script()).await;
    write_config(temp.path(), &server.base_url());

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("reshape proposing task")
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--untrusted")
        .arg("--max-spend")
        .arg("1")
        .arg("--no-docs")
        .env("DEADRECKON_HOME", temp.path().join("home"))
        .output()
        .expect("run deadreckon");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let run = list_runs(&paths, None)
        .expect("runs")
        .into_iter()
        .next()
        .expect("run");
    let state = load_run(&paths, &run.run_id).expect("state");
    // The run completed normally — recording a proposal is non-terminal.
    assert_eq!(state.status, RunStatus::Completed);
    let proposal_path = state.run_root.join("reshape-proposal.json");
    let raw = std::fs::read_to_string(&proposal_path).expect("proposal exists");
    let proposal: serde_json::Value = serde_json::from_str(&raw).expect("proposal parses");
    assert_eq!(proposal["schema"], 1, "{proposal}");
    assert_eq!(proposal["shape"], "plan", "{proposal}");
    assert_eq!(proposal["parent"], state.run_id.as_str(), "{proposal}");
    assert!(
        proposal.get("accepted_by").is_none_or(|v| v.is_null()),
        "inert: no acceptance recorded: {proposal}"
    );
    let traces = std::fs::read_to_string(state.run_root.join("traces.jsonl")).expect("traces");
    assert!(traces.contains("reshape.proposed"), "{traces}");
    // Inert means inert: no plan was dispatched by the run itself.
    let plans_dir = paths.plans_dir();
    let plan_count = std::fs::read_dir(&plans_dir)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(plan_count, 0, "no plan may exist without an accept");
}

fn reshape_script() -> Vec<FixtureResponse> {
    serde_json::from_value(json!([
        {
            "content": "{\"action\":\"reshape\",\"tool_call_id\":\"tool-reshape-1\",\"pieces\":[{\"goal\":\"piece a\",\"done_hint\":\"a tests\"},{\"goal\":\"piece b\"}]}",
            "prompt_tokens": 100,
            "completion_tokens": 40
        },
        {
            "content": "{\"action\":\"bash\",\"tool_call_id\":\"tool-bash-1\",\"command\":\"printf ok > done.txt\"}",
            "prompt_tokens": 100,
            "completion_tokens": 30
        },
        {
            "content": implementation_notes_write_action("notes-1", "Recorded the change and the reshape proposal."),
            "prompt_tokens": 100,
            "completion_tokens": 30
        },
        {
            "content": "{\"action\":\"done\",\"summary\":\"finished with a proposal on file\"}",
            "prompt_tokens": 100,
            "completion_tokens": 20
        }
    ]))
    .expect("script")
}

fn three_turn_script() -> Vec<FixtureResponse> {
    serde_json::from_value(json!([
        {
            "content": "{\"action\":\"bash\",\"tool_call_id\":\"tool-bash-1\",\"command\":\"printf 'turn 1' > turn1.txt\"}",
            "prompt_tokens": 120,
            "completion_tokens": 40
        },
        {
            "content": implementation_notes_write_action("notes-after-bash", "Recorded the first shell change."),
            "prompt_tokens": 120,
            "completion_tokens": 40
        },
        {
            "content": "{\"action\":\"write_file\",\"tool_call_id\":\"tool-write-2\",\"path\":\"notes.md\",\"content\":\"# Dead Reckoning\\n\\nTurn 2 wrote this file.\\n\"}",
            "prompt_tokens": 160,
            "completion_tokens": 60
        },
        {
            "content": implementation_notes_write_action("notes-after-file", "Recorded the markdown file change."),
            "prompt_tokens": 160,
            "completion_tokens": 40
        },
        {
            "content": "{\"action\":\"done\",\"summary\":\"mock task complete\"}",
            "prompt_tokens": 180,
            "completion_tokens": 30
        }
    ]))
    .expect("script")
}

fn implementation_notes_write_action(tool_call_id: &str, decision: &str) -> String {
    json!({
        "action": "write_file",
        "tool_call_id": tool_call_id,
        "path": "implementation-notes.html",
        "content": format!(r#"<!doctype html>
<html>
<head><meta charset="utf-8"><title>Implementation Notes</title></head>
<body>
<h1>Implementation Notes</h1>
<section id="design-decisions"><h2>Design decisions</h2>
<ul><li>{decision}</li></ul></section>
<section id="deviations"><h2>Deviations</h2>
<ul><li>None.</li></ul></section>
<section id="tradeoffs"><h2>Tradeoffs</h2>
<ul><li>A separate notes turn exercises the freshness gate for JSON-action providers.</li></ul></section>
<section id="open-questions"><h2>Open questions</h2>
<ul><li>None.</li></ul></section>
</body>
</html>
"#)
    })
    .to_string()
}

fn kill_script() -> Vec<FixtureResponse> {
    serde_json::from_value(json!([
        {
            "content": "{\"action\":\"bash\",\"tool_call_id\":\"tool-slow\",\"command\":\"sleep 30\"}",
            "delay_ms": 30000,
            "prompt_tokens": 100,
            "completion_tokens": 20
        }
    ]))
    .expect("script")
}
