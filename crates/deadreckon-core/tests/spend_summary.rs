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
            kind: "loop".to_string(),
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
            kind: "loop".to_string(),
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
            kind: "loop".to_string(),
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
            kind: "loop".to_string(),
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
            kind: "loop".to_string(),
        },
    )
    .expect("spend");
    let summary = spend_summary(&state).expect("summary");
    assert!(summary.any_estimated_turn);
    assert_eq!(summary.total_usd, 1.23);
    assert_eq!(summary.input_tokens, 11);
    assert_eq!(summary.output_tokens, 7);
}

fn record(kind: &str, turn: u32, cost: f64, total: f64, input: u64) -> SpendRecord {
    SpendRecord {
        timestamp: Utc::now(),
        turn,
        provider: "p".to_string(),
        model: "m".to_string(),
        input_tokens: input,
        output_tokens: 0,
        cost_usd: cost,
        total_cost_usd: total,
        cap_usd: None,
        subscription: false,
        estimated: false,
        wall_time_seconds: Some(1.0),
        wall_time_cap_seconds: None,
        kind: kind.to_string(),
    }
}

#[test]
fn spend_summary_excludes_kind_narrator_rows_from_total() {
    let (_temp, state) = state();
    append_spend(&state, &record("loop", 1, 0.02, 0.02, 100)).expect("loop row");
    append_spend(&state, &record("narrator", 1, 0.50, 0.50, 9999)).expect("narrator row");
    let summary = spend_summary(&state).expect("summary");
    assert_eq!(summary.input_tokens, 100, "narrator tokens excluded");
    assert_eq!(
        summary.turns, 1,
        "narrator row is not counted as a run turn"
    );
}

#[test]
fn spend_summary_total_usd_taken_from_last_loop_row() {
    let (_temp, state) = state();
    append_spend(&state, &record("loop", 1, 0.02, 0.02, 10)).expect("loop 1");
    append_spend(&state, &record("loop", 2, 0.03, 0.05, 10)).expect("loop 2");
    append_spend(&state, &record("narrator", 2, 0.50, 0.50, 0)).expect("trailing narrator");
    let summary = spend_summary(&state).expect("summary");
    assert_eq!(
        summary.total_usd, 0.05,
        "total comes from the last loop row, not the trailing narrator row"
    );
}
