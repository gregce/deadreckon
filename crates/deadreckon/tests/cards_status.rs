#![allow(clippy::expect_used)]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use chrono::Utc;
use deadreckon::sleep::{SleepMetadata, SleepMode, metadata_path};
use deadreckon_core::{
    DeadreckonPaths, RunOptions, RunStatus, SpendRecord, append_spend, create_run, save_state,
};
use serde_json::json;
use tempfile::TempDir;

#[test]
fn status_report_shows_sleep_mode_when_active() {
    let temp = repo_tempdir();
    let (paths, state) = state(&temp, "sleep status");
    let metadata = SleepMetadata {
        mode: SleepMode::Caffeinate,
        pid: Some(std::process::id()),
        armed_at: Utc::now(),
        inhibitor_binary: Some(PathBuf::from("/usr/bin/caffeinate")),
        reason: "test".to_string(),
        skip_reason: None,
    };
    let path = metadata_path(&state.working_dir);
    fs::create_dir_all(path.parent().expect("metadata parent")).expect("metadata dir");
    fs::write(
        &path,
        serde_json::to_string_pretty(&metadata).expect("metadata json"),
    )
    .expect("metadata");

    let output = deadreckon(&paths)
        .current_dir(&state.cwd)
        .args(["status", &state.run_id, "--global", "--plain"])
        .output()
        .expect("status");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("deadreckon status"), "{out}");
    assert!(out.contains("sleep   : caffeinate pid="), "{out}");
}

#[test]
fn show_output_includes_lineage_when_extend_parent() {
    let temp = repo_tempdir();
    let (paths, state) = state(&temp, "child goal");
    let parent_dir = state.working_dir.join(".deadreckon");
    fs::create_dir_all(&parent_dir).expect("parent dir");
    fs::write(
        parent_dir.join("parent.json"),
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "kind": "extended",
            "parent_run_id": "parent1234567890",
            "parent_scope": state.scope,
            "parent_goal": "parent goal",
            "parent_completed_at": Utc::now(),
            "extended_at": Utc::now(),
            "new_goal": "child goal",
            "context_turns_included": 1,
            "deadreckon_version": env!("CARGO_PKG_VERSION")
        }))
        .expect("parent json"),
    )
    .expect("parent marker");

    let output = deadreckon(&paths)
        .current_dir(&state.cwd)
        .args(["show", &state.run_id, "--plain"])
        .output()
        .expect("show");

    assert_success(&output);
    let out = stdout(&output);
    assert!(!out.starts_with('+'), "{out}");
    assert!(out.contains("Extended from parent1234567890"), "{out}");
}

#[test]
fn list_table_is_plain_and_truncates_very_long_goal() {
    let temp = repo_tempdir();
    let long_goal = std::iter::repeat_n(
        "this is a deliberately long goal fragment that cannot fit in one row forever",
        18,
    )
    .collect::<Vec<_>>()
    .join(" ");
    let (paths, state) = state(&temp, &long_goal);

    let output = deadreckon(&paths)
        .current_dir(&state.cwd)
        .args(["list", "--plain"])
        .output()
        .expect("list");

    assert_success(&output);
    let out = stdout(&output);
    assert!(!out.starts_with('+'), "{out}");
    assert!(
        out.lines().next().is_some_and(|line| line.contains("ID")),
        "{out}"
    );
    assert!(out.contains("..."), "{out}");
}

#[test]
fn list_full_keeps_old_layout_for_scripts() {
    let temp = repo_tempdir();
    let (paths, state) = state(&temp, "script list");

    let output = deadreckon(&paths)
        .current_dir(&state.cwd)
        .args(["list", "--full", "--plain"])
        .output()
        .expect("list");

    assert_success(&output);
    let out = stdout(&output);
    assert!(!out.starts_with('+'), "{out}");
    assert!(out.contains("ID        STATUS"), "{out}");
}

#[test]
fn status_report_marks_subscription_spend_not_metered() {
    let temp = repo_tempdir();
    let (paths, state) = state(&temp, "subscription spend");
    append_spend(
        &state,
        &SpendRecord {
            timestamp: Utc::now(),
            turn: 1,
            provider: "cli".to_string(),
            model: "subscription".to_string(),
            input_tokens: 1,
            output_tokens: 1,
            cost_usd: 0.0,
            total_cost_usd: 0.0,
            cap_usd: Some(10.0),
            subscription: true,
            estimated: false,
            wall_time_seconds: Some(1.0),
            wall_time_cap_seconds: None,
        },
    )
    .expect("spend");

    let output = deadreckon(&paths)
        .current_dir(&state.cwd)
        .args(["status", &state.run_id, "--global", "--plain"])
        .output()
        .expect("status");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("not metered (subscription)"), "{out}");
    assert!(!out.contains("~$0.000000"), "{out}");
}

#[test]
fn status_report_marks_mixed_route_spend_with_subscription_note() {
    let temp = repo_tempdir();
    let (paths, state) = state(&temp, "mixed spend");
    append_spend(
        &state,
        &SpendRecord {
            timestamp: Utc::now(),
            turn: 1,
            provider: "cli".to_string(),
            model: "subscription".to_string(),
            input_tokens: 1,
            output_tokens: 1,
            cost_usd: 0.0,
            total_cost_usd: 0.0,
            cap_usd: Some(10.0),
            subscription: true,
            estimated: false,
            wall_time_seconds: Some(1.0),
            wall_time_cap_seconds: None,
        },
    )
    .expect("spend");
    append_spend(
        &state,
        &SpendRecord {
            timestamp: Utc::now(),
            turn: 2,
            provider: "http".to_string(),
            model: "metered".to_string(),
            input_tokens: 1,
            output_tokens: 1,
            cost_usd: 0.25,
            total_cost_usd: 0.25,
            cap_usd: Some(10.0),
            subscription: false,
            estimated: false,
            wall_time_seconds: Some(2.0),
            wall_time_cap_seconds: None,
        },
    )
    .expect("spend");

    let output = deadreckon(&paths)
        .current_dir(&state.cwd)
        .args(["status", &state.run_id, "--global", "--plain"])
        .output()
        .expect("status");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("$0.250000 + subscription turns"), "{out}");
}

#[test]
fn status_latest_uses_same_kv_layout_for_completed_failed_running() {
    let cases = [
        (RunStatus::Completed, "completed ->"),
        (RunStatus::Failed, "failed ->"),
        (RunStatus::Executing, "running ->"),
    ];
    let mut layouts = Vec::new();

    for (status, expected_state) in cases {
        let temp = repo_tempdir();
        let (paths, mut state) = state(&temp, expected_state);
        state.status = status;
        state.failure_reason = (status == RunStatus::Failed).then(|| "acceptance failed".into());
        save_state(&state).expect("save state");

        let output = deadreckon(&paths)
            .current_dir(&state.cwd)
            .args(["status", "latest", "--plain"])
            .output()
            .expect("status latest");

        assert_success(&output);
        let out = stdout(&output);
        assert!(out.contains("deadreckon status"), "{out}");
        assert!(out.contains(expected_state), "{out}");
        assert!(!out.contains("executing"), "{out}");
        layouts.push(status_layout_keys(&out));
    }

    assert_eq!(layouts[0], layouts[1]);
    assert_eq!(layouts[0], layouts[2]);
    assert_eq!(
        layouts[0],
        [
            "run",
            "state",
            "phase",
            "scope",
            "updated",
            "provider",
            "sandbox",
            "spend",
            "wall",
            "goal",
            "state",
            "launch-dir",
            "working",
            "mode",
        ]
    );
}

#[test]
fn status_report_has_one_primary_action_and_demoted_secondary_actions() {
    let temp = repo_tempdir();
    let (paths, state) = state(&temp, "status primary action");

    let output = deadreckon(&paths)
        .current_dir(&state.cwd)
        .args(["status", &state.run_id, "--global", "--plain"])
        .output()
        .expect("status");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("primary action:"), "{out}");
    assert_eq!(count_action_label(&out, "next"), 1, "{out}");
    assert!(
        out.contains(&format!("deadreckon finish {}", &state.run_id[..8])),
        "{out}"
    );
    assert!(out.contains("secondary actions:"), "{out}");
    assert_eq!(out.matches("secondary actions:").count(), 1, "{out}");
}

fn state(temp: &TempDir, goal: &str) -> (DeadreckonPaths, deadreckon_core::PipelineState) {
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("repo");
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: goal.to_string(),
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
    state.status = RunStatus::Completed;
    save_state(&state).expect("save");
    (paths, state)
}

fn count_action_label(out: &str, label: &str) -> usize {
    out.lines()
        .filter(|line| line.trim_start().starts_with(&format!("{label}:")))
        .count()
}

fn status_layout_keys(out: &str) -> Vec<String> {
    out.lines()
        .skip_while(|line| *line != "deadreckon status")
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_once(':').map(|(key, _)| key.trim().to_string()))
        .collect()
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
