#![allow(clippy::expect_used)]

use std::path::PathBuf;

use deadreckon::cards::exit_summary::{
    BranchDiffSummary, ExitSummaryInput, OutcomeKind, build_exit_summary_card,
};
use deadreckon::proof_block::ProofBlock;
use deadreckon::ui_card::{CardOptions, Tone, render_card};
use deadreckon_core::VERDICT_VERIFIED;

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
        spend_label: "not metered (subscription) · wall 12.5s · 3 turns".to_string(),
        wall_seconds: 12.5,
        diff: Some(BranchDiffSummary {
            lines_added: 42,
            lines_deleted: 3,
            files_added: 1,
            files_updated: 2,
            files_deleted: 0,
        }),
        gate: "passed by dr-gate (2 checks)".to_string(),
        gate_tone: Tone::Neutral,
        tests_modified: Some(false),
        gate_caveats: Vec::new(),
        working_dir: PathBuf::from("/tmp/work"),
        proof_path: PathBuf::from("/tmp/run/proofs/turn-acceptance.json"),
        proof_block: Some(ProofBlock {
            proof_path: PathBuf::from("/tmp/run/proofs/turn-acceptance.json"),
            story_path: PathBuf::from("/tmp/library/docs/RUN-NARRATIVE.md"),
            lineage: "src/main.rs ← turn 2 · cli:codex · tool-write-2".to_string(),
            next_command: "deadreckon apply abc12345".to_string(),
        }),
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
fn exit_card_leads_with_one_verdict_and_one_primary_action() {
    let rendered = render_card(
        &build_exit_summary_card(&input(OutcomeKind::Completed)),
        &CardOptions {
            color: false,
            plain: true,
            terminal_columns: Some(140),
            no_color_env: false,
        },
    );

    assert!(
        rendered.contains(&format!("* {VERDICT_VERIFIED}")),
        "{rendered}"
    );
    assert_eq!(primary_action_count(&rendered), 1, "{rendered}");
    let primary = line_index(&rendered, "next", "deadreckon apply abc12345");
    let attach = line_index(&rendered, "attach", "deadreckon attach abc12345");
    let show = line_index(&rendered, "show", "deadreckon show abc12345");
    assert!(primary < attach, "{rendered}");
    assert!(primary < show, "{rendered}");
}

#[test]
fn paused_and_failed_cards_each_have_one_primary_action() {
    let mut paused = input(OutcomeKind::Paused);
    paused.hints = vec![
        (
            "attach".to_string(),
            "deadreckon attach abc12345".to_string(),
        ),
        (
            "resume".to_string(),
            "deadreckon resume abc12345".to_string(),
        ),
        ("show".to_string(), "deadreckon show abc12345".to_string()),
    ];
    let paused = render_card(
        &build_exit_summary_card(&paused),
        &CardOptions {
            color: false,
            plain: true,
            terminal_columns: Some(120),
            no_color_env: false,
        },
    );
    assert_eq!(primary_action_count(&paused), 1, "{paused}");
    assert!(
        line_index(&paused, "next", "deadreckon resume abc12345")
            < line_index(&paused, "attach", "deadreckon attach abc12345"),
        "{paused}"
    );

    let mut failed = input(OutcomeKind::Failed);
    failed.hints = vec![
        (
            "why".to_string(),
            "deadreckon show abc12345 --why-failed".to_string(),
        ),
        (
            "resume".to_string(),
            "deadreckon resume abc12345".to_string(),
        ),
        ("state".to_string(), "/tmp/run/state.json".to_string()),
    ];
    let failed = render_card(
        &build_exit_summary_card(&failed),
        &CardOptions {
            color: false,
            plain: true,
            terminal_columns: Some(120),
            no_color_env: false,
        },
    );
    assert_eq!(primary_action_count(&failed), 1, "{failed}");
    assert!(
        line_index(&failed, "next", "deadreckon show abc12345 --why-failed")
            < line_index(&failed, "resume", "deadreckon resume abc12345"),
        "{failed}"
    );
}

#[test]
fn accepted_exit_card_shows_proof_block() {
    let rendered = render_card(
        &build_exit_summary_card(&input(OutcomeKind::Completed)),
        &CardOptions {
            color: false,
            plain: true,
            terminal_columns: Some(140),
            no_color_env: false,
        },
    );

    assert!(rendered.contains("gate: SIGNED by dr-gate"), "{rendered}");
    assert!(
        rendered.contains("the agent could not have written this"),
        "{rendered}"
    );
    assert!(
        rendered.contains("proof:  /tmp/run/proofs/turn-acceptance.json"),
        "{rendered}"
    );
    assert!(
        rendered.contains("story:  /tmp/library/docs/RUN-NARRATIVE.md"),
        "{rendered}"
    );
    assert!(
        rendered.contains("lineage: src/main.rs ← turn 2 · cli:codex · tool-write-2"),
        "{rendered}"
    );
    assert!(
        rendered.contains("→ deadreckon apply abc12345"),
        "{rendered}"
    );
}

fn primary_action_count(rendered: &str) -> usize {
    rendered
        .lines()
        .filter(|line| line.contains(" next") && line.contains("deadreckon "))
        .count()
}

fn line_index(rendered: &str, label: &str, command: &str) -> usize {
    rendered
        .lines()
        .position(|line| line.contains(label) && line.contains(command))
        .unwrap_or_else(|| panic!("missing {label} / {command}\n{rendered}"))
}

#[test]
fn proof_block_shape_is_stable() {
    let block = ProofBlock {
        proof_path: PathBuf::from("/tmp/run/proofs/turn-acceptance.json"),
        story_path: PathBuf::from("/tmp/library/docs/RUN-NARRATIVE.md"),
        lineage: "src/main.rs ← turn 2 · cli:codex · tool-write-2".to_string(),
        next_command: "deadreckon apply abc12345".to_string(),
    };

    assert_eq!(
        block.render_text(),
        "gate: SIGNED by dr-gate — the agent could not have written this\nproof:  /tmp/run/proofs/turn-acceptance.json\nstory:  /tmp/library/docs/RUN-NARRATIVE.md\nlineage: src/main.rs ← turn 2 · cli:codex · tool-write-2\n→ deadreckon apply abc12345\n"
    );
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
    assert!(
        rendered.contains(&format!("* {VERDICT_VERIFIED}")),
        "{rendered}"
    );
    assert!(
        rendered.contains("not metered (subscription)"),
        "{rendered}"
    );
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
fn subscription_only_run_renders_not_metered() {
    let rendered = render_card(
        &build_exit_summary_card(&input(OutcomeKind::Completed)),
        &CardOptions {
            color: false,
            plain: true,
            terminal_columns: Some(88),
            no_color_env: false,
        },
    );
    assert!(
        rendered.contains("not metered (subscription) · wall 12.5s · 3 turns"),
        "{rendered}"
    );
    assert!(!rendered.contains("~$0.000000"), "{rendered}");
}

#[test]
fn mixed_route_run_renders_metered_total_plus_subscription_note() {
    let mut input = input(OutcomeKind::Completed);
    input.spend_usd = 0.25;
    input.approximate_spend = false;
    input.spend_label = "$0.250000 + subscription turns · wall 18.0s · 4 turns".to_string();

    let rendered = render_card(
        &build_exit_summary_card(&input),
        &CardOptions {
            color: false,
            plain: true,
            terminal_columns: Some(96),
            no_color_env: false,
        },
    );

    assert!(
        rendered.contains("$0.250000 + subscription turns"),
        "{rendered}"
    );
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
