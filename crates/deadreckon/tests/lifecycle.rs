use std::fs;
use std::net::SocketAddr;
use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use deadreckon_core::{
    DeadreckonPaths, PhaseId, PhaseStatus, PipelineState, RunOptions, RunStatus, TraceRecord,
    acquire_lock, append_trace, create_run, list_runs, load_run, promote_completed_run, save_state,
    write_acceptance_marker,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;

#[test]
fn materialize_copies_library_to_dest() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "materialize copy");
    let dest = temp.path().join("materialized-copy");

    let output = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("materialize");

    assert_success(&output);
    assert_eq!(
        fs::read_to_string(dest.join("app.txt")).expect("app"),
        "parent app"
    );
    assert!(!dest.join("manifest.json").exists());
    assert_eq!(
        parent_json(&dest)["kind"].as_str().expect("kind"),
        "materialized"
    );
}

#[test]
fn materialize_refuses_existing_nonempty_dest() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "materialize refuse");
    let dest = temp.path().join("nonempty");
    fs::create_dir_all(&dest).expect("dest");
    fs::write(dest.join("keep.txt"), "keep").expect("keep");

    let output = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("materialize");

    assert!(!output.status.success());
    assert!(stderr(&output).contains("is not empty"));
    assert_eq!(
        fs::read_to_string(dest.join("keep.txt")).expect("keep"),
        "keep"
    );
}

#[test]
fn materialize_force_overwrites() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "materialize force");
    let dest = temp.path().join("force");
    fs::create_dir_all(&dest).expect("dest");
    fs::write(dest.join("stale.txt"), "stale").expect("stale");

    let output = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .arg("--force")
        .output()
        .expect("materialize");

    assert_success(&output);
    assert!(!dest.join("stale.txt").exists());
    assert!(dest.join("app.txt").exists());
}

#[test]
fn materialize_writes_parent_manifest() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "materialize parent manifest");
    let dest = temp.path().join("manifest");

    let output = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .arg("--include-manifest")
        .output()
        .expect("materialize");

    assert_success(&output);
    assert!(dest.join("manifest.json").exists());
    let parent_marker = parent_json(&dest);
    assert_eq!(parent_marker["parent_run_id"], parent.run_id);
    assert_eq!(parent_marker["parent_scope"], parent.scope);
}

#[test]
fn materialize_records_reverse_marker_in_library() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "materialize reverse");
    let dest = temp.path().join("reverse");

    let output = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("materialize");

    assert_success(&output);
    let marker = fs::read_to_string(
        paths
            .library_dir(&parent.scope, &parent.run_id)
            .join(".materialized-to"),
    )
    .expect("reverse marker");
    assert!(marker.contains(&dest.display().to_string()));
}

#[test]
fn materialize_refuses_dest_inside_runstate() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "materialize inside runstate");
    let dest = paths.home().join("runstate").join("bad-dest");

    let output = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("materialize");

    assert!(!output.status.success());
    assert!(stderr(&output).contains("refusing to materialize back into runstate"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extend_creates_new_run_with_parent_artifacts() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "extend parent artifacts");
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());

    let output = extend_command(&paths, &parent, "add child file")
        .output()
        .expect("extend");

    assert_success(&output);
    let child = load_run(&paths, &extended_run_id(&output)).expect("child");
    assert_eq!(child.status, RunStatus::Completed);
    assert_eq!(child.scope, parent.scope);
    assert_eq!(child.task_key, parent.task_key);
    assert_eq!(
        fs::read_to_string(child.working_dir.join("app.txt")).expect("parent app"),
        "parent app"
    );
    assert_eq!(
        fs::read_to_string(child.working_dir.join("child.txt")).expect("child"),
        "extended child"
    );
    assert_eq!(
        parent_json(&child.working_dir)["kind"]
            .as_str()
            .expect("kind"),
        "extended"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extend_pre_populates_history_with_parent_summary() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "extend history parent");
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());

    let output = extend_command(&paths, &parent, "add child history")
        .arg("--max-context-turns")
        .arg("2")
        .output()
        .expect("extend");

    assert_success(&output);
    let child = load_run(&paths, &extended_run_id(&output)).expect("child");
    let history = fs::read_to_string(child.run_root.join("history.json")).expect("history");
    assert!(history.contains("Previous run summary"));
    assert!(history.contains("extend history parent"));
    assert!(history.contains("Recent activity"));
    assert!(history.contains("parent-tool-1"));
    let traces = fs::read_to_string(child.run_root.join("traces.jsonl")).expect("traces");
    assert!(traces.contains("extended_from_parent"));
}

#[test]
fn extend_refuses_incomplete_parent() {
    let temp = repo_tempdir();
    let home = temp.path().join("home");
    let paths = DeadreckonPaths::from_home(&home);
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&cwd).expect("workspace");
    let parent = create_run(
        &paths,
        RunOptions {
            goal: "incomplete parent".to_string(),
            cwd,
            sandbox: "none".to_string(),
            provider: Some("mock".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(30.0),
        },
    )
    .expect("parent");

    let output = deadreckon(&paths)
        .arg("extend")
        .arg(&parent.run_id)
        .arg("should refuse")
        .output()
        .expect("extend");

    assert!(!output.status.success());
    assert!(stderr(&output).contains("use 'deadreckon resume' for incomplete runs"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extend_locks_correctly_against_concurrent_extension() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "extend locked parent");
    let _lock = acquire_lock(
        &paths,
        &parent.task_key,
        "held-run",
        &parent.scope,
        "test-held",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    )
    .expect("lock");

    let output = deadreckon(&paths)
        .arg("extend")
        .arg(&parent.run_id)
        .arg("blocked by lock")
        .output()
        .expect("extend");

    assert!(!output.status.success());
    assert!(stderr(&output).contains("lock held"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extend_no_context_flag_omits_recent_turns() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "extend no context parent");
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());

    let output = extend_command(&paths, &parent, "add without context")
        .arg("--no-context")
        .output()
        .expect("extend");

    assert_success(&output);
    let child = load_run(&paths, &extended_run_id(&output)).expect("child");
    let history = fs::read_to_string(child.run_root.join("history.json")).expect("history");
    assert!(history.contains("Previous run summary"));
    assert!(!history.contains("Recent activity"));
    assert!(!history.contains("parent-tool-1"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn materialize_then_extend_roundtrip() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "roundtrip parent");
    let dest = temp.path().join("roundtrip-materialized");
    let materialize = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("materialize");
    assert_success(&materialize);
    assert!(dest.join("app.txt").exists());

    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());
    let output = extend_command(&paths, &parent, "extend after materialize")
        .output()
        .expect("extend");
    assert_success(&output);
    let child = load_run(&paths, &extended_run_id(&output)).expect("child");
    assert!(child.working_dir.join("app.txt").exists());
    assert!(child.working_dir.join("child.txt").exists());
}

#[test]
fn list_shows_materialized_status() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "list materialized parent");
    let dest = temp.path().join("listed-materialized");
    let materialize = deadreckon(&paths)
        .arg("materialize")
        .arg(&parent.run_id)
        .arg("--dest")
        .arg(&dest)
        .output()
        .expect("materialize");
    assert_success(&materialize);
    assert_eq!(list_runs(&paths, None).expect("runs").len(), 1);

    let output = deadreckon(&paths).arg("list").output().expect("list");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("MATERIALIZED"));
    assert!(stdout.contains("yes (1 time)"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn show_reveals_parent_lineage() {
    let temp = repo_tempdir();
    let (paths, parent) = completed_parent(&temp, "show lineage parent");
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());
    let output = extend_command(&paths, &parent, "lineage child")
        .output()
        .expect("extend");
    assert_success(&output);
    let child_id = extended_run_id(&output);

    let show = deadreckon(&paths)
        .arg("show")
        .arg(&child_id)
        .output()
        .expect("show");

    assert_success(&show);
    assert!(stdout(&show).contains(&format!("Extended from {}", parent.run_id)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_completion_prints_lifecycle_hints_and_no_hints_suppresses() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());
    let output = deadreckon(&paths)
        .arg("run")
        .arg("hinted run")
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .output()
        .expect("run");
    assert_success(&output);
    assert!(stdout(&output).contains("materialize:"));
    assert!(stdout(&output).contains("extend:"));
    let run_id = run_id_from_stdout(&output);
    let attach = deadreckon(&paths)
        .arg("attach")
        .arg(&run_id)
        .output()
        .expect("attach");
    assert_success(&attach);
    assert!(stdout(&attach).contains("materialize:"));
    assert!(stdout(&attach).contains("extend:"));

    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let server = MockServer::start(extend_script()).await;
    write_config(paths.home(), &server.base_url());
    let output = deadreckon(&paths)
        .arg("run")
        .arg("quiet hinted run")
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1")
        .arg("--no-hints")
        .output()
        .expect("run no hints");
    assert_success(&output);
    assert!(!stdout(&output).contains("materialize:"));
    assert!(!stdout(&output).contains("extend:"));
    let run_id = run_id_from_stdout(&output);
    let attach = deadreckon(&paths)
        .arg("attach")
        .arg(&run_id)
        .arg("--no-hints")
        .output()
        .expect("attach no hints");
    assert_success(&attach);
    assert!(!stdout(&attach).contains("materialize:"));
    assert!(!stdout(&attach).contains("extend:"));
}

#[test]
fn help_lists_lifecycle_verbs() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let output = deadreckon(&paths).arg("--help").output().expect("help");
    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("Lifecycle:"));
    assert!(stdout.contains("materialize <run-id>"));
    assert!(stdout.contains("extend <run-id>"));
}

fn completed_parent(temp: &TempDir, goal: &str) -> (DeadreckonPaths, PipelineState) {
    let home = temp.path().join("home");
    let paths = DeadreckonPaths::from_home(&home);
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&cwd).expect("workspace");
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: goal.to_string(),
            cwd,
            sandbox: "none".to_string(),
            provider: Some("mock".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(30.0),
        },
    )
    .expect("run");
    fs::write(state.working_dir.join("app.txt"), "parent app").expect("app");
    fs::write(state.working_dir.join("notes.md"), "parent notes").expect("notes");
    append_trace(
        &state,
        &TraceRecord {
            timestamp: chrono::Utc::now(),
            run_id: state.run_id.clone(),
            turn: 1,
            event: "tool.write_file".to_string(),
            latency_ms: None,
            detail: json!({"tool_call_id": "parent-tool-1", "path": "app.txt"}),
        },
    )
    .expect("trace");
    state.turn = 2;
    state
        .set_phase_status(PhaseId(60), PhaseStatus::Completed)
        .expect("complete");
    save_state(&state).expect("save");
    write_acceptance_marker(
        &state.run_root,
        state.run_id.clone(),
        state.working_dir.clone(),
        state.turn as usize,
    )
    .expect("acceptance marker");
    promote_completed_run(&paths, &mut state).expect("promote");
    let state = load_run(&paths, &state.run_id).expect("reload");
    assert_eq!(state.status, RunStatus::Completed);
    (paths, state)
}

fn parent_json(dest: &std::path::Path) -> Value {
    serde_json::from_slice(&fs::read(dest.join(".deadreckon/parent.json")).expect("parent marker"))
        .expect("parent json")
}

fn repo_tempdir() -> TempDir {
    let root = std::path::Path::new("/Users/gdc/deadreckon/.test-tmp");
    fs::create_dir_all(root).expect("test tmp root");
    TempDir::new_in(root).expect("tempdir")
}

fn deadreckon(paths: &DeadreckonPaths) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command.env("DEADRECKON_HOME", paths.home());
    command
}

fn extend_command(paths: &DeadreckonPaths, parent: &PipelineState, goal: &str) -> Command {
    let mut command = deadreckon(paths);
    command
        .arg("extend")
        .arg(&parent.run_id)
        .arg(goal)
        .arg("--provider")
        .arg("mock")
        .arg("--sandbox")
        .arg("none")
        .arg("--max-spend")
        .arg("1");
    command
}

fn write_config(home: &std::path::Path, base_url: &str) {
    fs::create_dir_all(home).expect("home");
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

fn extend_script() -> Vec<FixtureResponse> {
    serde_json::from_value(json!([
        {
            "content": "{\"action\":\"write_file\",\"tool_call_id\":\"extend-write\",\"path\":\"child.txt\",\"content\":\"extended child\"}",
            "prompt_tokens": 120,
            "completion_tokens": 40
        },
        {
            "content": "{\"action\":\"done\",\"summary\":\"extended complete\"}",
            "prompt_tokens": 160,
            "completion_tokens": 40
        }
    ]))
    .expect("script")
}

fn extended_run_id(output: &std::process::Output) -> String {
    stdout(output)
        .lines()
        .find_map(|line| line.strip_prefix("completed extended run "))
        .expect("extended run id")
        .to_string()
}

fn run_id_from_stdout(output: &std::process::Output) -> String {
    stdout(output)
        .lines()
        .find_map(|line| line.strip_prefix("completed run "))
        .expect("run id")
        .to_string()
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}{}",
        stdout(output),
        stderr(output)
    );
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

#[derive(Clone)]
struct MockState {
    fixtures: Arc<Mutex<Vec<FixtureResponse>>>,
}

#[derive(Debug, Clone, Deserialize)]
struct FixtureResponse {
    content: String,
    prompt_tokens: u64,
    completion_tokens: u64,
}

struct MockServer {
    addr: SocketAddr,
}

impl MockServer {
    async fn start(fixtures: Vec<FixtureResponse>) -> Self {
        let state = MockState {
            fixtures: Arc::new(Mutex::new(fixtures)),
        };
        let app = Router::new()
            .route("/chat/completions", post(chat_completions))
            .with_state(state);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        Self { addr }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

async fn chat_completions(
    State(state): State<MockState>,
    Json(_request): Json<Value>,
) -> impl IntoResponse {
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
