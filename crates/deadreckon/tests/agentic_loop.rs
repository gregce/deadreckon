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
use deadreckon_core::{DeadreckonPaths, RunStatus, list_runs, load_run};
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
async fn kill_mid_turn_sets_killed_and_stops_process() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let server = MockServer::start(kill_script()).await;
    write_config(temp.path(), &server.base_url());
    let home = temp.path().join("home");
    let mut child = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_preserves_history_file() {
    let _gate = env!("CARGO_BIN_EXE_dr-gate");
    let temp = repo_tempdir();
    let server = MockServer::start(three_turn_script()).await;
    write_config(temp.path(), &server.base_url());
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("run")
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
