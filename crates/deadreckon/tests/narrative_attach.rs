#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use chrono::Utc;
use deadreckon_core::flight::{FlightEvent, FlightEventKind, append_flight_event};
use deadreckon_core::{
    DeadreckonPaths, DocKind, Plan, PlanMode, PlanProviders, PlanRole, PlanTask, RunOptions,
    RunStatus, create_run, doc_path_for_kind, save_plan, save_state,
};
use tempfile::TempDir;

#[test]
fn narrative_attach_plain_smoke_reads_flight_events_and_architecture_map() {
    let temp = repo_tempdir();
    let (paths, state) = state(&temp, RunStatus::Executing);
    fs::create_dir_all(state.working_dir.join("src")).expect("src dir");
    fs::write(
        state.working_dir.join("src/lib.rs"),
        "pub fn value() -> u8 { 1 }\n",
    )
    .expect("source");
    append_flight_event(
        &state,
        &FlightEvent {
            version: 1,
            seq: 1,
            run_id: state.run_id.clone(),
            flight_session_id: "flight-turn-1-attempt-1".to_string(),
            deadreckon_turn: 1,
            attempt: 1,
            provider: "cli:test".to_string(),
            schema: "test".to_string(),
            timestamp: Some(Utc::now()),
            source_path: None,
            source_line: None,
            source_event: "{}".to_string(),
            raw_hash: "sha256:test".to_string(),
            kind: FlightEventKind::Tool,
            role: None,
            summary: "provider edited src/lib.rs".to_string(),
            tool_name: Some("write_file".to_string()),
            tool_category: None,
            files: vec![PathBuf::from("src/lib.rs")],
            usage: None,
            checkpoint_id: Some("cp-000001".to_string()),
        },
    )
    .expect("flight event");

    let output = deadreckon(&paths)
        .current_dir(&state.cwd)
        .args([
            "attach",
            &state.run_id,
            "--view",
            "narrative",
            "--visual",
            "architecture",
            "--plain",
        ])
        .output()
        .expect("attach");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("Latest provider-native event"), "{out}");
    assert!(out.contains("flight:"), "{out}");
    assert!(out.contains("Visual: architecture"), "{out}");
    assert!(out.contains("src/lib.rs"), "{out}");
}

#[test]
fn narrative_attach_plan_plain_smoke_lists_agents_and_dependencies() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut first = PlanTask::new(
        0,
        "Implement renderer",
        "Implement the narrative renderer",
        PlanRole::Child,
        Some("smoke:child".to_string()),
    );
    first.status = deadreckon_core::PlanTaskStatus::Running;
    let mut second = PlanTask::new(
        1,
        "Review renderer",
        "Review the narrative renderer",
        PlanRole::Child,
        Some("smoke:reviewer".to_string()),
    );
    second.depends_on = vec![first.task_id.clone()];
    let plan = Plan::new(
        "ship narrated attach",
        PlanMode::FullPlan,
        vec![first, second],
        PlanProviders {
            planner: Some("smoke:planner".to_string()),
            default_child: Some("smoke:child".to_string()),
            coder: None,
            reviewer: None,
            children: [(1, "smoke:reviewer".to_string())].into(),
        },
        None,
        "0.1.0",
    )
    .expect("plan");
    save_plan(&paths, &plan).expect("save plan");

    let output = deadreckon(&paths)
        .args([
            "attach",
            &plan.plan_id,
            "--view",
            "narrative",
            "--visual",
            "agents",
            "--plain",
        ])
        .output()
        .expect("attach");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("Agents"), "{out}");
    assert!(out.contains("task-0"), "{out}");
    assert!(out.contains("smoke:child"), "{out}");
    assert!(out.contains("Visual: agents"), "{out}");
    assert!(out.contains("deps=1"), "{out}");
}

#[test]
fn narrative_attach_completed_run_stays_separate_from_run_narrative_doc() {
    let temp = repo_tempdir();
    let (paths, state) = state(&temp, RunStatus::Completed);
    let docs_path = doc_path_for_kind(&state.working_dir, DocKind::Narrative).expect("doc path");
    fs::create_dir_all(docs_path.parent().expect("doc parent")).expect("doc dir");
    fs::write(
        &docs_path,
        "# Completed Narrative\n\nThis text belongs to RUN-NARRATIVE.md only.\n",
    )
    .expect("docs");

    let output = deadreckon(&paths)
        .current_dir(&state.cwd)
        .args(["attach", &state.run_id, "--view", "narrative", "--plain"])
        .output()
        .expect("attach");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("completed and is ready for inspection"),
        "{out}"
    );
    assert!(out.contains("freshness:"), "{out}");
    assert!(
        !out.contains("This text belongs to RUN-NARRATIVE.md only"),
        "{out}"
    );
}

fn state(temp: &TempDir, status: RunStatus) -> (DeadreckonPaths, deadreckon_core::PipelineState) {
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("repo");
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: "narrative attach smoke".to_string(),
            cwd,
            sandbox: "none".to_string(),
            provider: Some("cli:test".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(10.0),
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    state.status = status;
    save_state(&state).expect("save");
    (paths, state)
}

fn repo_tempdir() -> TempDir {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.test-tmp");
    fs::create_dir_all(&root).expect("test tmp root");
    TempDir::new_in(root).expect("tempdir")
}

fn deadreckon(paths: &DeadreckonPaths) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command.env("DEADRECKON_HOME", paths.home());
    command
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
