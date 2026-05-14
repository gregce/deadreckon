#![allow(clippy::expect_used)]

use chrono::Utc;
use deadreckon_core::state::spend_summary;
use deadreckon_core::{DeadreckonPaths, RunOptions, SpendRecord, append_spend, create_run};
use tempfile::TempDir;

fn state() -> (TempDir, deadreckon_core::PipelineState) {
    let temp = TempDir::new().expect("temp");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cwd = temp.path().join("repo");
    std::fs::create_dir_all(&cwd).expect("repo");
    let state = create_run(
        &paths,
        RunOptions {
            goal: "spend summary".to_string(),
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
    (temp, state)
}

#[test]
fn spend_summary_marks_tilde_when_any_turn_subscription() {
    let (_temp, state) = state();
    append_spend(
        &state,
        &SpendRecord {
            timestamp: Utc::now(),
            turn: 1,
            provider: "cli".to_string(),
            model: "sub".to_string(),
            input_tokens: 10,
            output_tokens: 20,
            cost_usd: 0.0,
            total_cost_usd: 0.0,
            cap_usd: Some(10.0),
            subscription: true,
            estimated: false,
            wall_time_seconds: Some(2.0),
            wall_time_cap_seconds: None,
        },
    )
    .expect("spend");
    let summary = spend_summary(&state).expect("summary");
    assert!(summary.any_subscription_turn);
    assert!(!summary.any_estimated_turn);
}

#[test]
fn spend_summary_no_tilde_when_all_http_priced() {
    let (_temp, state) = state();
    append_spend(
        &state,
        &SpendRecord {
            timestamp: Utc::now(),
            turn: 1,
            provider: "http".to_string(),
            model: "priced".to_string(),
            input_tokens: 10,
            output_tokens: 20,
            cost_usd: 0.42,
            total_cost_usd: 0.42,
            cap_usd: Some(10.0),
            subscription: false,
            estimated: false,
            wall_time_seconds: Some(2.0),
            wall_time_cap_seconds: None,
        },
    )
    .expect("spend");

    let summary = spend_summary(&state).expect("summary");
    assert!(!summary.any_subscription_turn);
    assert!(!summary.any_estimated_turn);
    assert_eq!(summary.total_usd, 0.42);
}

#[test]
fn spend_summary_tilde_persists_after_resume_via_jsonl_replay() {
    let (_temp, state) = state();
    append_spend(
        &state,
        &SpendRecord {
            timestamp: Utc::now(),
            turn: 1,
            provider: "cli".to_string(),
            model: "subscription".to_string(),
            input_tokens: 1,
            output_tokens: 2,
            cost_usd: 0.0,
            total_cost_usd: 0.0,
            cap_usd: Some(10.0),
            subscription: true,
            estimated: false,
            wall_time_seconds: Some(1.0),
            wall_time_cap_seconds: None,
        },
    )
    .expect("first spend");
    append_spend(
        &state,
        &SpendRecord {
            timestamp: Utc::now(),
            turn: 2,
            provider: "http".to_string(),
            model: "priced".to_string(),
            input_tokens: 3,
            output_tokens: 4,
            cost_usd: 0.25,
            total_cost_usd: 0.25,
            cap_usd: Some(10.0),
            subscription: false,
            estimated: false,
            wall_time_seconds: Some(2.0),
            wall_time_cap_seconds: None,
        },
    )
    .expect("second spend");

    let summary = spend_summary(&state).expect("summary");
    assert!(summary.any_subscription_turn);
    assert_eq!(summary.total_usd, 0.25);
    assert_eq!(summary.input_tokens, 4);
    assert_eq!(summary.output_tokens, 6);
}

#[test]
fn spend_summary_total_unchanged_by_tilde_flag() {
    let (_temp, state) = state();
    append_spend(
        &state,
        &SpendRecord {
            timestamp: Utc::now(),
            turn: 1,
            provider: "http".to_string(),
            model: "priced".to_string(),
            input_tokens: 11,
            output_tokens: 7,
            cost_usd: 1.23,
            total_cost_usd: 1.23,
            cap_usd: Some(10.0),
            subscription: false,
            estimated: true,
            wall_time_seconds: Some(3.0),
            wall_time_cap_seconds: None,
        },
    )
    .expect("spend");
    let summary = spend_summary(&state).expect("summary");
    assert!(summary.any_estimated_turn);
    assert_eq!(summary.total_usd, 1.23);
    assert_eq!(summary.input_tokens, 11);
    assert_eq!(summary.output_tokens, 7);
}
