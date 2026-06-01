#![allow(clippy::expect_used)]

use std::collections::BTreeMap;
use std::fs;

use deadreckon::ui_card::CardOptions;
use deadreckon::verdict_surface::{
    ExplanationPanel, TERMINAL_OUTCOME_SURFACE_VERBS, VerdictKind, VerdictSurface,
    VerdictSurfaceError,
};
use serde_json::json;

#[test]
fn verdict_surface_friendliness_audit_tracks_terminal_outcome_surfaces() {
    let audit = fs::read_to_string(workspace_root().join("docs/FRIENDLINESS-AUDIT.md"))
        .expect("friendliness audit");
    let rows = audit_rows(&audit);

    for verb in TERMINAL_OUTCOME_SURFACE_VERBS {
        let status = rows
            .get(&(
                (*verb).to_string(),
                "One verdict + ONE primary action".to_string(),
            ))
            .unwrap_or_else(|| panic!("missing one-primary-action row for {verb}"));
        assert!(
            matches!(status.as_str(), "pass" | "fail" | "n-a"),
            "unexpected status {status:?} for {verb}"
        );
    }
}

#[test]
fn verdict_surface_contract_rejects_multiple_primary_actions() {
    let err = VerdictSurface::try_new(
        VerdictKind::Failed,
        "plan",
        Some("4b4fdc93"),
        explanation(),
        vec![
            ("Recommended", "deadreckon merge 4b4fdc93"),
            ("Recommended", "deadreckon show 4b4fdc93 --why-failed"),
        ],
        Vec::<(&str, &str)>::new(),
    )
    .expect_err("multiple primary actions must be rejected");

    assert_eq!(
        err,
        VerdictSurfaceError::ExpectedOnePrimaryAction { actual: 2 }
    );
}

#[test]
fn verdict_surface_contract_requires_explanation_panel() {
    let err = VerdictSurface::try_new(
        VerdictKind::Failed,
        "plan",
        Some("4b4fdc93"),
        ExplanationPanel::new("", "", Vec::<(&str, &str)>::new()),
        vec![("Recommended", "deadreckon merge 4b4fdc93")],
        Vec::<(&str, &str)>::new(),
    )
    .expect_err("empty explanation must be rejected");

    assert_eq!(err, VerdictSurfaceError::MissingExplanation);
}

#[test]
fn verdict_surface_renders_one_recommended_command() {
    let rendered = surface().render_card(&CardOptions {
        color: false,
        plain: true,
        terminal_columns: Some(120),
        no_color_env: false,
    });

    assert!(rendered.contains("failed plan 4b4fdc93"), "{rendered}");
    assert_eq!(rendered.matches("deadreckon merge 4b4fdc93").count(), 1);
    assert_eq!(primary_command_count(&rendered), 1, "{rendered}");
    assert!(rendered.contains("Explanation"), "{rendered}");
    assert!(rendered.contains("Evidence"), "{rendered}");
    assert!(rendered.contains("Secondary"), "{rendered}");
}

#[test]
fn verdict_surface_plain_output_contains_same_verdict_and_command() {
    let rendered = surface().render_plain(false);

    assert!(rendered.starts_with("failed plan 4b4fdc93"), "{rendered}");
    assert!(rendered.contains("Explanation\n"), "{rendered}");
    assert!(
        rendered.contains("Recommended\ndeadreckon merge 4b4fdc93"),
        "{rendered}"
    );
    assert!(rendered.contains("Secondary\n"), "{rendered}");
}

#[test]
fn verdict_surface_json_is_additive_and_matches_primary_action() {
    let surface = surface();
    let value = surface.add_to_json(json!({
        "plan_id": "4b4fdc93",
        "status": "failed",
        "next_actions": [
            "deadreckon merge 4b4fdc93",
            "deadreckon show 4b4fdc93 --why-failed"
        ]
    }));

    assert_eq!(value["plan_id"], "4b4fdc93");
    assert_eq!(value["status"], "failed");
    assert_eq!(value["primary_action"], "deadreckon merge 4b4fdc93");
    assert_eq!(
        value["verdict"]["recommended_command"],
        value["primary_action"]
    );
    assert_eq!(value["next_actions"][0], value["primary_action"]);
    assert_eq!(value["verdict"]["kind"], "failed");
}

#[test]
fn verdict_surface_no_hints_suppresses_secondary_not_primary() {
    let rendered = surface().render_plain(true);

    assert!(
        rendered.contains("Recommended\ndeadreckon merge 4b4fdc93"),
        "{rendered}"
    );
    assert!(!rendered.contains("Secondary"), "{rendered}");
    assert!(
        !rendered.contains("deadreckon show 4b4fdc93 --why-failed"),
        "{rendered}"
    );
}

fn surface() -> VerdictSurface {
    VerdictSurface::try_new(
        VerdictKind::Failed,
        "plan",
        Some("4b4fdc93"),
        explanation(),
        vec![("Recommended", "deadreckon merge 4b4fdc93")],
        vec![("Secondary", "deadreckon show 4b4fdc93 --why-failed")],
    )
    .expect("surface")
}

fn explanation() -> ExplanationPanel {
    ExplanationPanel::new(
        "task-3 could not start because its dependency inputs conflict.",
        "DeadReckon refused to choose a file winner without dependency ordering.",
        vec![
            ("plan", "4b4fdc93"),
            ("task", "task-3"),
            ("conflict", "index.html"),
        ],
    )
}

fn primary_command_count(rendered: &str) -> usize {
    rendered
        .lines()
        .filter(|line| line.contains("Recommended") && line.contains("deadreckon "))
        .count()
}

fn audit_rows(audit: &str) -> BTreeMap<(String, String), String> {
    let mut rows = BTreeMap::new();
    for line in audit.lines() {
        if !line.starts_with("| `") {
            continue;
        }
        let cells = line.split('|').skip(1).map(str::trim).collect::<Vec<_>>();
        if cells.len() < 4 {
            continue;
        }
        rows.insert(
            (cells[0].trim_matches('`').to_string(), cells[1].to_string()),
            cells[2].to_string(),
        );
    }
    rows
}

fn workspace_root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates")
        .parent()
        .expect("workspace")
        .to_path_buf()
}
