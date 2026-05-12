use std::fs;
use std::net::SocketAddr;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use deadreckon_core::{
    DeadreckonPaths, RUN_EVENTS_JSONL, RunStatus, cancel_marker_path, list_runs, load_run,
    write_cancel_marker,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;

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
        .arg("--max-spend")
        .arg("1")
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
    assert_eq!(state.turn, 3);
    assert_jsonl_count(&state.run_root.join("traces.jsonl"), 5);
    assert_jsonl_count(&state.run_root.join("spend.jsonl"), 3);
    assert_jsonl_count(&state.run_root.join("provenance.jsonl"), 2);
    assert!(state.run_root.join("proofs/turn-acceptance.json").exists());
    assert!(state.working_dir.join("turn1.txt").exists());
    assert!(state.working_dir.join("notes.md").exists());
    assert_provenance_ids_match_traces(&state.run_root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cross_process_kill_writes_cancel_marker_before_signal() {
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
    assert!(kill.status.success());
    assert!(started.elapsed() < Duration::from_secs(2));
    let _ = child.wait();
    let state = load_run(&paths, &run_id).expect("state");
    assert_eq!(state.status, RunStatus::Killed);
    assert!(cancel_marker_path(&state).exists());
    let events = fs::read_to_string(state.run_root.join(RUN_EVENTS_JSONL)).expect("events");
    assert!(events.contains(r#""kind":"run_completed""#));
    assert!(events.contains(r#""status":"killed""#));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_http_call_aborts_when_cancel_marker_appears() {
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
    let events = fs::read_to_string(state.run_root.join(RUN_EVENTS_JSONL)).expect("events");
    assert!(events.contains(r#""kind":"run_completed""#));
    assert!(events.contains(r#""status":"killed""#));
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
        .arg("--max-spend")
        .arg("1")
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
        "#!/bin/sh\nprintf \"wall run %s\\n\" \"$(date +%s%N)\" > notes.md\nsleep 1\nprintf 'changed notes\\n'\n",
    )
    .expect("fake codex");
    chmod_exec(&fake_codex);
    write_cli_config(temp.path(), &fake_codex);

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
        .arg("--fresh")
        .arg("--yes")
        .arg("cli wall budget")
        .arg("--provider")
        .arg("cli:codex")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--max-wall-seconds")
        .arg("0.1")
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
    assert_eq!(
        state.pause_reason.as_deref(),
        Some("wall-clock cap reached")
    );
    let spend = fs::read_to_string(state.run_root.join("spend.jsonl")).expect("spend");
    assert!(spend.contains("wall_time_seconds"));

    let resume = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("resume")
        .arg(&run.run_id)
        .arg("--max-wall-seconds")
        .arg("10")
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("resume");
    assert!(
        resume.status.success(),
        "{}{}",
        String::from_utf8_lossy(&resume.stdout),
        String::from_utf8_lossy(&resume.stderr)
    );
    let state = load_run(&paths, &run.run_id).expect("state");
    assert_eq!(state.status, RunStatus::Completed);
    assert!(state.working_dir.join("notes.md").exists());
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
async fn doctor_fails_actionably() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("doctor")
        .env("DEADRECKON_HOME", &home)
        .output()
        .expect("doctor");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("config.toml missing"));
    assert!(stdout.contains("fix: deadreckon init"));
    assert!(stdout.contains("disk space"));
    assert!(stdout.contains("runstate dir"));
    for line in stdout.lines().filter(|line| line.contains("✓")) {
        assert!(
            line.contains("try:"),
            "doctor success line is not actionable: {line}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_claude_code_roundtrip() {
    import_jsonl_roundtrip("claude-code", "DEADRECKON_IMPORT_CLAUDE_ROOT");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_codex_roundtrip() {
    import_jsonl_roundtrip("codex", "DEADRECKON_IMPORT_CODEX_ROOT");
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
        .arg("create table messages (role text, content text, tool_call_id text, path text); insert into messages values ('assistant','edited','cursor-tool','cursor.md');")
        .status();
    if status.is_err() {
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
    assert!(stdout.contains("cursor.md"));
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
            "#!/bin/sh\nprintf 'scope:%s\\n' \"$PWD\" > notes.md\nsleep {seconds}\nprintf 'done\\n'\n"
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

fn repo_tempdir() -> TempDir {
    let root = std::path::Path::new("/Users/gdc/deadreckon/.test-tmp");
    fs::create_dir_all(root).expect("test tmp root");
    TempDir::new_in(root).expect("tempdir")
}

fn write_config(temp: &std::path::Path, base_url: &str) {
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
"#
        ),
    )
    .expect("config");
}

fn write_cli_config(temp: &std::path::Path, binary: &std::path::Path) {
    let home = temp.join("home");
    fs::create_dir_all(&home).expect("home");
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
    fs::create_dir_all(&root).expect("import root");
    fs::write(
        root.join("session.jsonl"),
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

fn imported_run_id(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("imported "))
        .expect("imported id")
        .to_string()
}

fn kill_storm_script(count: usize) -> Vec<FixtureResponse> {
    (0..count)
        .map(|_| FixtureResponse {
            content:
                "{\"action\":\"bash\",\"tool_call_id\":\"tool-slow\",\"command\":\"sleep 30\"}"
                    .to_string(),
            delay_ms: Some(30000),
            prompt_tokens: 100,
            completion_tokens: 20,
        })
        .collect()
}

fn wait_for_run_id(paths: &DeadreckonPaths) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(run) = list_runs(paths, None).expect("runs").into_iter().next() {
            if load_run(paths, &run.run_id)
                .expect("state")
                .child_pids
                .first()
                .is_some()
            {
                return run.run_id;
            }
        }
        assert!(Instant::now() < deadline, "run state did not appear");
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

fn three_turn_script() -> Vec<FixtureResponse> {
    serde_json::from_value(json!([
        {
            "content": "{\"action\":\"bash\",\"tool_call_id\":\"tool-bash-1\",\"command\":\"printf 'turn 1' > turn1.txt\"}",
            "prompt_tokens": 120,
            "completion_tokens": 40
        },
        {
            "content": "{\"action\":\"write_file\",\"tool_call_id\":\"tool-write-2\",\"path\":\"notes.md\",\"content\":\"# Dead Reckoning\\n\\nTurn 2 wrote this file.\\n\"}",
            "prompt_tokens": 160,
            "completion_tokens": 60
        },
        {
            "content": "{\"action\":\"done\",\"summary\":\"mock task complete\"}",
            "prompt_tokens": 180,
            "completion_tokens": 30
        }
    ]))
    .expect("script")
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
