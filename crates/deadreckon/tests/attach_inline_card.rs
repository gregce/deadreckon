#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use deadreckon_core::{DeadreckonPaths, RunOptions, RunStatus, create_run, save_state};
use tempfile::TempDir;

#[test]
fn attach_exit_renders_completion_card_on_run_completed_event() {
    let temp = repo_tempdir();
    let (paths, state) = state(&temp, RunStatus::Completed);

    let output = deadreckon(&paths)
        .current_dir(&state.cwd)
        .args(["attach", &state.run_id, "--plain"])
        .output()
        .expect("attach");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("completed run"), "{out}");
    assert!(out.contains("deadreckon show"), "{out}");
}

#[test]
fn attach_exit_preserves_ctrl_d_detach_during_running_state() {
    let temp = repo_tempdir();
    let (paths, state) = state(&temp, RunStatus::Executing);

    let output = deadreckon(&paths)
        .current_dir(&state.cwd)
        .args(["attach", &state.run_id, "--plain"])
        .output()
        .expect("attach");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("run "), "{out}");
    assert!(!out.contains("completed run"), "{out}");
}

fn state(temp: &TempDir, status: RunStatus) -> (DeadreckonPaths, deadreckon_core::PipelineState) {
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("repo");
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: "attach card".to_string(),
            cwd,
            sandbox: "none".to_string(),
            provider: Some("smoke".to_string()),
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
