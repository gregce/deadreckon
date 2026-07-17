#![allow(clippy::expect_used)]

use std::fs;

use deadreckon_core::steer_inbox::{SteerSource, append_steer, mark_steer_delivered};
use deadreckon_core::{DeadreckonPaths, RunOptions, RunStatus, create_run, save_state};
use tempfile::TempDir;

mod common;

use common::{assert_success, deadreckon, repo_tempdir, stdout};

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

#[test]
fn plain_attach_lists_steer_inbox_state() {
    let temp = repo_tempdir();
    let (paths, mut state) = state(&temp, RunStatus::Executing);
    state.provider = Some("cli:codex-server".to_string());
    save_state(&state).expect("save server route");
    let delivered = append_steer(
        &state.run_root,
        SteerSource::Cli,
        "keep the public API stable",
    )
    .expect("append delivered steer");
    mark_steer_delivered(&state.run_root, &delivered.identity(), "turn_2").expect("mark delivered");
    append_steer(
        &state.run_root,
        SteerSource::Tui,
        "focus on the failing integration test",
    )
    .expect("append pending steer");

    let output = deadreckon(&paths)
        .current_dir(&state.cwd)
        .args(["attach", &state.run_id, "--plain"])
        .output()
        .expect("attach");

    assert_success(&output);
    let out = stdout(&output);
    assert!(
        out.contains("steer delivered (turn turn_2): keep the public API stable"),
        "{out}"
    );
    assert!(
        out.contains("steer pending: focus on the failing integration test"),
        "{out}"
    );
    assert!(
        out.contains("1 pending steer waiting for delivery"),
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
