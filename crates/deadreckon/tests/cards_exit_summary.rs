#![allow(clippy::expect_used)]

use std::path::PathBuf;

use deadreckon::cards::exit_summary::{
    BranchDiffSummary, ExitSummaryInput, OutcomeKind, build_exit_summary_card,
};
use deadreckon::ui_card::{CardOptions, render_card};

fn input(outcome: OutcomeKind) -> ExitSummaryInput {
    ExitSummaryInput {
        run_id: "abc123456789".to_string(),
        goal: "ship it".to_string(),
        provider: "cli:codex".to_string(),
        branch: Some("dr/ship-it-abc12345".to_string()),
        outcome,
        turns: 3,
        input_tokens: 100,
        output_tokens: 50,
        spend_usd: 0.0,
        approximate_spend: true,
        wall_seconds: 12.5,
        diff: Some(BranchDiffSummary {
            lines_added: 42,
            lines_deleted: 3,
            files_added: 1,
            files_updated: 2,
            files_deleted: 0,
        }),
        gate: "passed by dr-gate (2 checks)".to_string(),
        working_dir: PathBuf::from("/tmp/work"),
        proof_path: PathBuf::from("/tmp/run/proofs/turn-acceptance.json"),
        hints: vec![
            (
                "attach".to_string(),
                "deadreckon attach abc12345".to_string(),
            ),
            ("show".to_string(), "deadreckon show abc12345".to_string()),
            ("apply".to_string(), "deadreckon apply abc12345".to_string()),
        ],
    }
}

#[test]
fn exit_summary_completed_run_includes_attach_show_apply_hints() {
    let card = build_exit_summary_card(&input(OutcomeKind::Completed));
    let rendered = render_card(
        &card,
        &CardOptions {
            color: false,
            plain: true,
            terminal_columns: Some(88),
            no_color_env: false,
        },
    );
    assert!(rendered.contains("* completed run"), "{rendered}");
    assert!(rendered.contains("~$0.000000"), "{rendered}");
    assert!(
        rendered.contains("deadreckon attach abc12345"),
        "{rendered}"
    );
    assert!(rendered.contains("deadreckon show abc12345"), "{rendered}");
    assert!(rendered.contains("deadreckon apply abc12345"), "{rendered}");
}

#[test]
fn exit_summary_failed_run_uses_failed_glyph_and_logs_hint() {
    let mut input = input(OutcomeKind::Failed);
    input.hints = vec![(
        "logs".to_string(),
        "deadreckon show abc12345 --why-failed".to_string(),
    )];
    let rendered = render_card(
        &build_exit_summary_card(&input),
        &CardOptions {
            color: false,
            plain: true,
            terminal_columns: Some(88),
            no_color_env: false,
        },
    );
    assert!(rendered.contains("! failed run"), "{rendered}");
    assert!(
        rendered.contains("deadreckon show abc12345 --why-failed"),
        "{rendered}"
    );
}

#[test]
fn exit_summary_paused_run_uses_paused_glyph_and_resume_hint() {
    let mut input = input(OutcomeKind::Paused);
    input.hints = vec![(
        "resume".to_string(),
        "deadreckon resume abc12345".to_string(),
    )];
    let rendered = render_card(
        &build_exit_summary_card(&input),
        &CardOptions {
            color: false,
            plain: true,
            terminal_columns: Some(88),
            no_color_env: false,
        },
    );
    assert!(rendered.contains("~ paused run"), "{rendered}");
    assert!(
        rendered.contains("deadreckon resume abc12345"),
        "{rendered}"
    );
}

#[test]
fn exit_summary_killed_run_uses_stopped_glyph_and_reason() {
    let mut input = input(OutcomeKind::Killed);
    input.gate = "killed by user request".to_string();
    let rendered = render_card(
        &build_exit_summary_card(&input),
        &CardOptions {
            color: false,
            plain: true,
            terminal_columns: Some(88),
            no_color_env: false,
        },
    );
    assert!(rendered.contains("x killed run"), "{rendered}");
    assert!(rendered.contains("killed by user request"), "{rendered}");
}

#[test]
fn exit_summary_subscription_turn_marks_spend_with_tilde() {
    let rendered = render_card(
        &build_exit_summary_card(&input(OutcomeKind::Completed)),
        &CardOptions {
            color: false,
            plain: true,
            terminal_columns: Some(88),
            no_color_env: false,
        },
    );
    assert!(rendered.contains("~$0.000000"), "{rendered}");
}

#[test]
fn exit_summary_no_branch_diff_when_codebase_is_fresh() {
    let mut input = input(OutcomeKind::Completed);
    input.diff = None;
    let rendered = render_card(
        &build_exit_summary_card(&input),
        &CardOptions {
            color: false,
            plain: true,
            terminal_columns: Some(88),
            no_color_env: false,
        },
    );
    assert!(!rendered.contains("branch diff"), "{rendered}");
    assert!(!rendered.contains("files         "), "{rendered}");
}

#[test]
fn exit_summary_matches_golden_fixture_for_three_turn_smoke_run() {
    let rendered = render_card(
        &build_exit_summary_card(&input(OutcomeKind::Completed)),
        &CardOptions {
            color: false,
            plain: true,
            terminal_columns: Some(88),
            no_color_env: false,
        },
    );
    assert_eq!(
        rendered,
        include_str!("fixtures/cards/exit-summary-three-turn.golden")
    );
}
