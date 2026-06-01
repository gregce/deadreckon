#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::redundant_clone
)]

use std::fs;
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use deadreckon_core::plan::write_plan_narrative;
use deadreckon_core::{
    CodebaseMode, CodebaseRecord, DeadreckonPaths, FileChange, FrontmatterFields,
    ImplementationNotesStatus, Plan, PlanMode, PlanProviders, PlanRole, PlanStatus, PlanTask,
    PlanTaskStatus, RunOptions, RunStatus, TurnDocInput, TurnRecord,
    append_parent_narrative_update, append_turn_doc, apply_commit_body, as_built_path, auto_title,
    check_implementation_notes_current, child_summary_relative_path, coalesce_into_phases,
    decisions_path, diff_samples_markdown, docs_dir, frontmatter, implementation_notes_path,
    is_decision_candidate, missing_files_in_narrative, narrative_path, polish_path,
    publish_docs_for_promotion, rewrite_templated_docs, save_state, should_emit_delta,
    source_layout, tool_stdio_markdown, write_child_summary,
};
use deadreckon_providers::ProviderRouter;
use deadreckon_runtime::{
    PolishConfig, polish_run_docs, read_polish_record, resolve_skill, substitute_placeholders,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;

mod common;

use common::{assert_success, deadreckon_home as deadreckon, repo_tempdir, stderr, stdout};

#[test]
fn docs_dir_created_at_run_start() {
    let (_temp, _paths, state) = fresh_state("write docs at start");
    assert!(docs_dir(&state.working_dir).is_dir());
    assert!(narrative_path(&state.working_dir).exists());
}

#[test]
fn implementation_notes_seed_writes_required_html_sections() {
    let (_temp, _paths, state) = fresh_state("implementation notes seed");
    let notes = fs::read_to_string(implementation_notes_path(&state.working_dir)).expect("notes");
    for expected in [
        r#"id="design-decisions""#,
        "Design decisions",
        r#"id="deviations""#,
        "Deviations",
        r#"id="tradeoffs""#,
        "Tradeoffs",
        r#"id="open-questions""#,
        "Open questions",
    ] {
        assert!(notes.contains(expected), "missing {expected}");
    }
}

#[test]
fn implementation_notes_seed_preserves_existing_file() {
    let (_temp, _paths, state) = fresh_state("implementation notes preserve");
    let path = implementation_notes_path(&state.working_dir);
    fs::write(&path, "custom notes").expect("custom notes");
    deadreckon_core::ensure_implementation_notes_started(&state).expect("ensure notes");
    assert_eq!(fs::read_to_string(path).expect("notes"), "custom notes");
}

#[test]
fn implementation_notes_path_is_working_root_file() {
    let (_temp, _paths, state) = fresh_state("implementation notes path");
    assert_eq!(
        implementation_notes_path(&state.working_dir),
        state.working_dir.join("implementation-notes.html")
    );
}

#[test]
fn frontmatter_contains_required_fields_in_order() {
    let (_temp, _paths, state) = fresh_state("frontmatter fields");
    let fm = frontmatter(&state, &frontmatter_fields());
    let order = [
        "**Date:**",
        "**Last updated:**",
        "**Status:**",
        "**Run ID:**",
        "**Goal:**",
        "**Owner:**",
        "**Provider:**",
        "**Sandbox:**",
        "**Spend:**",
        "**Doc-writer:**",
    ];
    let mut last = 0;
    for field in order {
        let idx = fm.find(field).expect(field);
        assert!(idx >= last, "{field} appeared out of order");
        last = idx;
    }
    assert!(fm.contains("**Status:** planned"), "{fm}");
    assert!(!fm.contains("(alpha)"), "{fm}");
}

#[test]
fn frontmatter_handles_subscription_provider_format() {
    let (_temp, _paths, mut state) = fresh_state("subscription spend");
    state.provider = Some("cli:codex".to_string());
    state.total_wall_seconds = 42.0;
    let fm = frontmatter(&state, &frontmatter_fields());
    assert!(fm.contains("42s wall (subscription)"));
}

#[test]
fn frontmatter_omits_commit_span_for_fresh_mode() {
    let (_temp, _paths, state) = fresh_state("fresh mode");
    let fm = frontmatter(&state, &frontmatter_fields());
    assert!(!fm.contains("**Commit span:**"));
}

#[test]
fn three_turn_run_produces_three_turn_sections() {
    let (_temp, _paths, state) = fresh_state("three turn docs");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "wrote a");
    append_sample_turn(&state, 2, "bash", &["b.txt"], "ran b");
    append_sample_turn(&state, 3, "done", &[], "done");
    let narrative = fs::read_to_string(narrative_path(&state.working_dir)).expect("narrative");
    assert_eq!(narrative.matches("#### Turn ").count(), 3);
}

#[test]
fn each_turn_section_has_required_fields() {
    let (_temp, _paths, state) = fresh_state("turn fields");
    append_sample_turn(&state, 1, "write_file", &["src/lib.rs"], "ok");
    let narrative = fs::read_to_string(narrative_path(&state.working_dir)).expect("narrative");
    for field in ["Tool:", "Files:", "Outcome:", "Trace:", "Snapshot:"] {
        assert!(narrative.contains(field), "missing {field}");
    }
}

#[test]
fn auto_title_from_ill_verb_noun_phrase() {
    let title = auto_title("I'll refactor parser module before tests", "bash", &[], 1);
    assert_eq!(title, "Refactor Parser Module Before Tests");
}

#[test]
fn auto_title_fallback_to_tool_plus_basename() {
    let title = auto_title("", "write_file", &["src/main.rs".to_string()], 1);
    assert_eq!(title, "Write main.rs");
}

#[test]
fn commit_sha_present_in_worktree_mode_blank_otherwise() {
    let (_temp, _paths, fresh) = fresh_state("fresh sha");
    let record = append_sample_turn(&fresh, 1, "write_file", &["a.txt"], "ok");
    assert!(record.commit_sha.is_none());

    let (_repo_temp, _paths, worktree) = worktree_state("worktree sha", 1);
    fs::write(worktree.working_dir.join("a.txt"), "a").expect("write");
    git(&worktree.working_dir, &["add", "-A"]);
    git(&worktree.working_dir, &["commit", "-m", "turn"]);
    let record = append_sample_turn(&worktree, 1, "write_file", &["a.txt"], "ok");
    assert!(record.commit_sha.is_some());
}

#[test]
fn twelve_turns_collapse_to_three_to_eight_phases() {
    let records = (1..=12)
        .map(|turn| turn_record(turn, "write_file", &[&format!("f{turn}.txt")]))
        .collect::<Vec<_>>();
    let phases = coalesce_into_phases(&records);
    assert!((3..=8).contains(&phases.len()), "{}", phases.len());
}

#[test]
fn same_file_turns_coalesce() {
    let records = vec![
        turn_record(1, "write_file", &["src/lib.rs"]),
        turn_record(2, "bash", &["src/lib.rs"]),
    ];
    assert_eq!(coalesce_into_phases(&records).len(), 1);
}

#[test]
fn tool_kind_changes_break_phase() {
    let records = vec![
        turn_record(1, "write_file", &["a.rs"]),
        turn_record(2, "bash", &["b.rs"]),
    ];
    assert_eq!(coalesce_into_phases(&records).len(), 2);
}

#[test]
fn phase_titles_come_from_first_turn() {
    let records = vec![
        turn_record(1, "write_file", &["a.rs"]),
        turn_record(2, "write_file", &["a.rs"]),
    ];
    assert_eq!(coalesce_into_phases(&records)[0].title, "Write a.rs");
}

#[test]
fn decision_markers_detected_case_insensitive() {
    let text = format!(
        "LET ME CONSIDER option 1 or option 2. I'll go with option 2 because it fits. {}",
        "x".repeat(220)
    );
    assert!(is_decision_candidate(&text));
}

#[test]
fn short_response_below_200_chars_not_a_decision() {
    assert!(!is_decision_candidate("I'll choose option 1."));
}

#[test]
fn templated_decisions_md_lists_each_decision_with_turn_link() {
    let (_temp, _paths, state) = fresh_state("decision docs");
    append_sample_turn(
        &state,
        1,
        "write_file",
        &["choice.md"],
        &format!(
            "let me consider alternatives: either a or b. {}",
            "x".repeat(220)
        ),
    );
    let decisions = fs::read_to_string(decisions_path(&state.working_dir)).expect("decisions");
    assert!(decisions.contains("## Multi-alternative decision details"));
    assert!(decisions.contains("### Decision 1"));
    assert!(decisions.contains("[turn 1](../traces.jsonl)"));
}

#[test]
fn no_decisions_emits_single_line_no_decisions_message() {
    let (_temp, _paths, state) = fresh_state("no decisions");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let decisions = fs::read_to_string(decisions_path(&state.working_dir)).expect("decisions");
    assert!(decisions.contains("No multi-alternative decisions detected in this run."));
    assert!(decisions.contains("## Turn citations"));
    assert!(decisions.contains("[trace](../traces.jsonl#turn-1)"));
}

#[test]
fn run_decisions_includes_implementation_interpretation_sections() {
    let (_temp, _paths, state) = fresh_state("decision ledger sections");
    let decisions = fs::read_to_string(decisions_path(&state.working_dir)).expect("decisions");
    for heading in [
        "## Design decisions",
        "## Deviations",
        "## Tradeoffs",
        "## Open questions",
        "## Multi-alternative decision details",
    ] {
        assert!(decisions.contains(heading), "missing {heading}");
    }
}

#[test]
fn implementation_notes_sections_render_into_run_decisions() {
    let (_temp, _paths, state) = fresh_state("decision ledger projection");
    write_notes(
        &state,
        "Use the existing docs file instead of a new schema.",
        "Departed from the old no-decisions-only shape.",
        "Chose HTML working copy over markdown to match the requested artifact.",
        "Confirm whether HTML styling matters.",
    );
    rewrite_templated_docs(&state, "templated only").expect("rewrite");
    let decisions = fs::read_to_string(decisions_path(&state.working_dir)).expect("decisions");
    assert!(decisions.contains("Use the existing docs file"));
    assert!(decisions.contains("Departed from the old"));
    assert!(decisions.contains("Chose HTML working copy"));
    assert!(decisions.contains("Confirm whether HTML styling matters"));
}

#[test]
fn notes_freshness_passes_when_notes_turn_follows_code_turn() {
    let (_temp, _paths, state) = fresh_state("fresh notes");
    append_sample_turn(&state, 1, "write_file", &["src/lib.rs"], "code");
    write_notes(&state, "Decision", "None.", "Tradeoff", "None.");
    append_sample_turn(
        &state,
        2,
        "write_file",
        &["implementation-notes.html"],
        "notes",
    );
    assert_eq!(
        check_implementation_notes_current(&state).expect("check"),
        ImplementationNotesStatus::Current
    );
}

#[test]
fn notes_freshness_fails_when_code_turn_follows_notes_turn() {
    let (_temp, _paths, state) = fresh_state("stale notes");
    write_notes(&state, "Decision", "None.", "Tradeoff", "None.");
    append_sample_turn(
        &state,
        1,
        "write_file",
        &["implementation-notes.html"],
        "notes",
    );
    append_sample_turn(&state, 2, "write_file", &["src/lib.rs"], "code");
    assert_eq!(
        check_implementation_notes_current(&state).expect("check"),
        ImplementationNotesStatus::Stale {
            notes_turn: Some(1),
            implementation_turn: 2
        }
    );
}

#[test]
fn notes_freshness_requires_four_sections() {
    let (_temp, _paths, state) = fresh_state("missing notes sections");
    fs::write(
        implementation_notes_path(&state.working_dir),
        r#"<section id="design-decisions"><h2>Design decisions</h2></section>"#,
    )
    .expect("notes");
    let status = check_implementation_notes_current(&state).expect("check");
    assert!(matches!(
        status,
        ImplementationNotesStatus::MissingSections(_)
    ));
}

#[test]
fn no_multi_alternative_message_survives_inside_details_section() {
    let (_temp, _paths, state) = fresh_state("decision ledger no decisions");
    let decisions = fs::read_to_string(decisions_path(&state.working_dir)).expect("decisions");
    let details = decisions
        .split("## Multi-alternative decision details")
        .nth(1)
        .expect("details");
    assert!(details.contains("No multi-alternative decisions detected in this run."));
}

#[test]
fn as_built_includes_turn_citations() {
    let (_temp, _paths, state) = fresh_state("as built citations");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let as_built = fs::read_to_string(as_built_path(&state.working_dir)).expect("as built");
    assert!(as_built.contains("### Turn citations"));
    assert!(as_built.contains("[trace](../traces.jsonl#turn-1)"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_runs_once_on_completion() {
    let (_temp, paths, mut state) = fresh_state("polish once");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(vec![FixtureResponse::json(valid_docs_json(
        &state,
        &["a.txt"],
    ))])
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &polish_config(paths.home(), false, false),
    )
    .await
    .expect("polish");
    assert_eq!(server.journal().len(), 1);
    assert_eq!(
        read_polish_record(&state).unwrap().unwrap().status,
        "polished"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_docs_writes_templated_docs_without_provider_call() {
    let (_temp, paths, mut state) = fresh_state("no docs");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(vec![FixtureResponse::json(valid_docs_json(
        &state,
        &["a.txt"],
    ))])
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &polish_config(paths.home(), true, false),
    )
    .await
    .expect("polish");

    assert_eq!(server.journal().len(), 0);
    let polish = read_polish_record(&state)
        .expect("polish record")
        .expect("record");
    assert_eq!(polish.status, "incremental");
    let narrative = fs::read_to_string(narrative_path(&state.working_dir)).expect("narrative");
    assert!(narrative.contains("**Doc-writer:** templated only"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_idempotent_on_same_input_hash() {
    let (_temp, paths, mut state) = fresh_state("polish idempotent");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(vec![FixtureResponse::json(valid_docs_json(
        &state,
        &["a.txt"],
    ))])
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    let config = polish_config(paths.home(), false, false);
    polish_run_docs(&mut state, &router, &config)
        .await
        .expect("first");
    polish_run_docs(&mut state, &router, &config)
        .await
        .expect("second");
    assert_eq!(server.journal().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_failure_does_not_fail_run() {
    let (_temp, paths, mut state) = fresh_state("polish failure");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(Vec::new()).await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &polish_config(paths.home(), false, false),
    )
    .await
    .expect("nonfatal");
    assert!(matches!(
        read_polish_record(&state).unwrap().unwrap().status.as_str(),
        "provider_error"
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_json_retry_on_malformed_first_response() {
    let (_temp, paths, mut state) = fresh_state("polish retry");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(vec![
        FixtureResponse::text("not-json"),
        FixtureResponse::json(valid_docs_json(&state, &["a.txt"])),
    ])
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &polish_config(paths.home(), false, false),
    )
    .await
    .expect("polish");
    assert_eq!(server.journal().len(), 2);
    assert_eq!(
        read_polish_record(&state).unwrap().unwrap().status,
        "polished"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_uses_doc_provider_override_when_configured() {
    let (_temp, paths, mut state) = fresh_state("doc provider");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(vec![FixtureResponse::json(valid_docs_json(
        &state,
        &["a.txt"],
    ))])
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &polish_config(paths.home(), false, false),
    )
    .await
    .expect("polish");
    assert_eq!(
        read_polish_record(&state)
            .unwrap()
            .unwrap()
            .provider
            .as_deref(),
        Some("docmock")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_runs_four_subcalls_sequentially() {
    let (_temp, paths, mut state) = fresh_state("split polish");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &split_polish_config(paths.home(), false),
    )
    .await
    .expect("polish");

    let journal = server.journal();
    assert_eq!(journal.len(), 4);
    for request in &journal {
        assert_eq!(request["max_tokens"].as_u64(), Some(16_384));
    }
    let record = read_polish_record(&state).unwrap().unwrap();
    assert_eq!(record.schema_version, 2);
    assert_eq!(record.status, "polished");
    assert_eq!(record.doc_provider_source.as_deref(), Some("config"));
    assert_eq!(
        record
            .subcalls
            .iter()
            .map(|subcall| subcall.skill.as_str())
            .collect::<Vec<_>>(),
        vec![
            "narrator-overview",
            "narrator-phases",
            "narrator-as-built",
            "narrator-decisions"
        ]
    );
    assert!(record.merged_at.is_some());
    assert!(record.diff_coverage.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_total_cost_summed_across_subcalls() {
    let (_temp, paths, mut state) = fresh_state("split cost");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &split_polish_config(paths.home(), false),
    )
    .await
    .expect("polish");
    let record = read_polish_record(&state).unwrap().unwrap();
    let summed = record
        .subcalls
        .iter()
        .map(|subcall| subcall.cost_usd)
        .sum::<f64>();
    assert_eq!(record.cost_usd, summed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_per_subcall_token_budget_is_16k() {
    let (_temp, paths, mut state) = fresh_state("split budget");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &split_polish_config(paths.home(), false),
    )
    .await
    .expect("polish");
    for request in server.journal() {
        assert_eq!(request["max_tokens"].as_u64(), Some(16_384));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_records_per_subcall_status_in_polish_json() {
    let (_temp, paths, mut state) = fresh_state("split records");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &split_polish_config(paths.home(), false),
    )
    .await
    .expect("polish");
    let record = read_polish_record(&state).unwrap().unwrap();
    assert_eq!(record.subcalls.len(), 4);
    assert!(record.subcalls.iter().all(|subcall| subcall.status == "ok"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_merges_overview_into_narrative_intro() {
    let (_temp, paths, mut state) = fresh_state("split overview");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &split_polish_config(paths.home(), false),
    )
    .await
    .expect("polish");
    let narrative = fs::read_to_string(narrative_path(&state.working_dir)).expect("narrative");
    assert!(narrative.contains("## Reading order"));
    assert!(narrative.contains("Read the narrative first"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_merges_phases_into_narrative_body() {
    let (_temp, paths, mut state) = fresh_state("split phases");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &split_polish_config(paths.home(), false),
    )
    .await
    .expect("polish");
    let narrative = fs::read_to_string(narrative_path(&state.working_dir)).expect("narrative");
    assert!(narrative.contains("### Phase 1"));
    assert!(narrative.contains("The provider completed the turn"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_subcall_failure_does_not_abort_other_subcalls() {
    let (_temp, paths, mut state) = fresh_state("split failure");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(vec![
        FixtureResponse::json(overview_json_for_tests()),
        FixtureResponse::text("not-json"),
        FixtureResponse::json(as_built_json(&["a.txt"])),
        FixtureResponse::json(decisions_json()),
    ])
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &split_polish_config(paths.home(), false),
    )
    .await
    .expect("polish");
    let record = read_polish_record(&state).unwrap().unwrap();
    assert!(record.status.starts_with("failed_subcall"));
    assert!(record.subcalls.iter().any(|subcall| subcall.status == "ok"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_idempotent_when_inputs_unchanged_across_subcalls() {
    let (_temp, paths, mut state) = fresh_state("split idempotent");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    let config = split_polish_config(paths.home(), false);
    polish_run_docs(&mut state, &router, &config)
        .await
        .expect("first");
    polish_run_docs(&mut state, &router, &config)
        .await
        .expect("second");
    assert_eq!(server.journal().len(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coverage_retry_targets_only_narrator_phases() {
    let (_temp, paths, mut state) = fresh_state("split coverage");
    append_sample_turn(&state, 1, "write_file", &["a.txt", "b.txt"], "ok");
    let server = MockServer::start(vec![
        FixtureResponse::json(overview_json_for_tests()),
        FixtureResponse::json(phases_json(&["a.txt"])),
        FixtureResponse::json(as_built_json(&["a.txt", "b.txt"])),
        FixtureResponse::json(decisions_json()),
        FixtureResponse::json(phases_json(&["a.txt", "b.txt"])),
    ])
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &split_polish_config(paths.home(), false),
    )
    .await
    .expect("polish");
    let record = read_polish_record(&state).unwrap().unwrap();
    assert_eq!(server.journal().len(), 5);
    assert_eq!(record.retries, 1);
    assert_eq!(
        record.subcalls.last().map(|subcall| subcall.skill.as_str()),
        Some("narrator-phases")
    );
    assert!(
        record
            .diff_coverage
            .as_ref()
            .expect("coverage")
            .missing_files
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coverage_retry_capped_at_two_per_subcall() {
    let (_temp, paths, mut state) = fresh_state("split coverage cap");
    append_sample_turn(&state, 1, "write_file", &["a.txt", "b.txt"], "ok");
    let server = MockServer::start(vec![
        FixtureResponse::json(overview_json_for_tests()),
        FixtureResponse::json(phases_json(&["a.txt"])),
        FixtureResponse::json(as_built_json(&["a.txt", "b.txt"])),
        FixtureResponse::json(decisions_json()),
        FixtureResponse::json(phases_json(&["a.txt"])),
        FixtureResponse::json(phases_json(&["a.txt"])),
    ])
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &split_polish_config(paths.home(), false),
    )
    .await
    .expect("polish");
    let record = read_polish_record(&state).unwrap().unwrap();
    assert_eq!(server.journal().len(), 6);
    assert_eq!(record.retries, 2);
    assert_eq!(record.diff_coverage.as_ref().expect("coverage").retries, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn non_phases_subcalls_not_coverage_gated() {
    let (_temp, paths, mut state) = fresh_state("split coverage non phases");
    append_sample_turn(&state, 1, "write_file", &["a.txt", "b.txt"], "ok");
    let server = MockServer::start(vec![
        FixtureResponse::json(overview_json_for_tests()),
        FixtureResponse::json(phases_json(&["a.txt"])),
        FixtureResponse::json(as_built_json(&["a.txt", "b.txt"])),
        FixtureResponse::json(decisions_json()),
        FixtureResponse::json(phases_json(&["a.txt", "b.txt"])),
    ])
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &split_polish_config(paths.home(), false),
    )
    .await
    .expect("polish");
    let skills = server
        .journal()
        .iter()
        .filter_map(|request| request["messages"][0]["content"].as_str())
        .filter(|prompt| prompt.contains("narrator-as-built"))
        .count();
    assert_eq!(skills, 1);
}

#[test]
fn polish_resolves_project_skill_before_user_before_repo() {
    let (temp, paths, state) = copy_state_with_source("skill precedence");
    fs::create_dir_all(temp.path().join("source/skills/run-narrator")).expect("project skill dir");
    fs::write(
        temp.path().join("source/skills/run-narrator/SKILL.md"),
        "project {{ goal }}",
    )
    .expect("project skill");
    fs::create_dir_all(paths.home().join("skills/run-narrator")).expect("user skill dir");
    fs::write(paths.home().join("skills/run-narrator/SKILL.md"), "user").expect("user skill");
    let resolved = resolve_skill("run-narrator", &state, paths.home()).expect("resolve");
    assert!(resolved.path.starts_with(temp.path().join("source")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_records_no_skill_status_when_unresolvable() {
    let (_temp, paths, mut state) = fresh_state("no skill");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    let server = MockServer::start(Vec::new()).await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    let mut config = polish_config(paths.home(), false, false);
    config.doc_skill = "missing-skill".to_string();
    polish_run_docs(&mut state, &router, &config)
        .await
        .expect("nonfatal");
    assert_eq!(
        read_polish_record(&state).unwrap().unwrap().status,
        "no_skill"
    );
}

#[test]
fn unknown_placeholder_passes_through_unchanged() {
    let out = substitute_placeholders("{{ goal }} {{unknown}}", &[("goal", "ship".to_string())]);
    assert_eq!(out, "ship {{unknown}}");
}

#[test]
fn diff_samples_placeholder_renders_per_file_blocks() {
    let mut record = turn_record(1, "write_file", &["src/app.rs"]);
    record.files[0].adds = 2;
    record.files[0].dels = 1;
    record.files[0].largest_hunk_excerpt = "@@ -1,1 +1,2 @@\n+new".to_string();
    let prompt = substitute_placeholders(
        "diffs:\n{{ diff_samples }}",
        &[("diff_samples", diff_samples_markdown(&[record]))],
    );
    assert!(prompt.contains("### Turn 1"));
    assert!(prompt.contains("`src/app.rs`: +2/-1"));
    assert!(prompt.contains("+new"));
}

#[test]
fn tool_stdout_placeholder_omits_non_bash_turns() {
    let mut bash = turn_record(1, "bash", &[]);
    bash.tool_stdout = Some("cargo test ok".to_string());
    let non_bash = turn_record(2, "write_file", &[]);
    let prompt = substitute_placeholders(
        "{{ tool_stdout }}",
        &[("tool_stdout", tool_stdio_markdown(&[bash, non_bash]))],
    );
    assert!(prompt.contains("cargo test ok"));
    assert!(!prompt.contains("Turn 2"));
}

#[test]
fn source_layout_placeholder_uses_path_inference() {
    let record = turn_record(1, "write_file", &["crates/app/src/lib.rs"]);
    let (_temp, _paths, state) = fresh_state("source layout");
    let prompt = substitute_placeholders(
        "{{ source_layout }}",
        &[(
            "source_layout",
            source_layout(&[record], &state.working_dir),
        )],
    );
    assert!(prompt.contains("Crate app (Rust)"));
}

#[test]
fn parent_narrative_placeholder_empty_for_solo_runs() {
    let out = substitute_placeholders(
        "parent={{ parent_narrative }}",
        &[("parent_narrative", String::new())],
    );
    assert_eq!(out, "parent=");
}

#[test]
fn parent_narrative_placeholder_loaded_for_extend_runs() {
    let out = substitute_placeholders(
        "{{ parent_narrative }}",
        &[("parent_narrative", "# Parent narrative".to_string())],
    );
    assert_eq!(out, "# Parent narrative");
}

#[test]
fn delta_emitted_when_source_has_as_built_at_root() {
    let (_temp, _paths, state) = worktree_state("delta root", 3);
    fs::write(
        state.working_dir.join("AS-BUILT-ARCHITECTURE.md"),
        "# as built",
    )
    .expect("as built");
    let files = vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()];
    assert!(should_emit_delta(&state, &files).expect("delta"));
}

#[test]
fn delta_emitted_when_diff_touches_as_built_neighbor() {
    let (_temp, _paths, state) = worktree_state("delta neighbor", 3);
    fs::create_dir_all(state.working_dir.join("src")).expect("src");
    fs::write(state.working_dir.join("src/AS-BUILT.md"), "# as built").expect("as built");
    let files = vec![
        "src/a.rs".to_string(),
        "src/b.rs".to_string(),
        "src/c.rs".to_string(),
    ];
    assert!(should_emit_delta(&state, &files).expect("delta"));
}

#[test]
fn delta_skipped_when_no_source_as_built() {
    let (_temp, _paths, state) = worktree_state("delta none", 3);
    let files = vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()];
    assert!(!should_emit_delta(&state, &files).expect("delta"));
}

#[test]
fn delta_skipped_when_diff_under_three_files() {
    let (_temp, _paths, state) = worktree_state("delta small", 2);
    fs::write(state.working_dir.join("AS-BUILT.md"), "# as built").expect("as built");
    let files = vec!["a.txt".to_string(), "b.txt".to_string()];
    assert!(!should_emit_delta(&state, &files).expect("delta"));
}

#[test]
fn delta_commit_lands_on_branch_in_worktree_mode() {
    let (_temp, _paths, state) = worktree_state("delta commit", 3);
    fs::write(state.working_dir.join("AS-BUILT.md"), "# as built").expect("as built");
    append_sample_turn(&state, 1, "write_file", &["a.rs", "b.rs", "c.rs"], "ok");
    rewrite_templated_docs(&state, "templated only").expect("rewrite");
    publish_docs_for_promotion(&state).expect("publish");
    let log = git_out(&state.working_dir, &["log", "--oneline", "-1"]);
    assert!(log.contains("docs: deadreckon run docs"));
}

#[test]
fn diff_coverage_check_passes_when_all_files_named() {
    let (_temp, _paths, state) = fresh_state("coverage pass");
    append_sample_turn(&state, 1, "write_file", &["a.txt", "b.txt"], "mentions");
    assert!(
        missing_files_in_narrative(&state)
            .expect("coverage")
            .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_file_triggers_polish_retry() {
    let (_temp, paths, mut state) = fresh_state("coverage retry");
    append_sample_turn(&state, 1, "write_file", &["a.txt", "b.txt"], "ok");
    let server = MockServer::start(vec![
        FixtureResponse::json(valid_docs_json(&state, &["a.txt"])),
        FixtureResponse::json(valid_docs_json(&state, &["a.txt", "b.txt"])),
    ])
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &polish_config(paths.home(), false, false),
    )
    .await
    .expect("polish");
    assert_eq!(server.journal().len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coverage_warning_logged_when_still_missing_after_two_retries() {
    let (_temp, paths, mut state) = fresh_state("coverage warning");
    append_sample_turn(&state, 1, "write_file", &["a.txt", "b.txt"], "ok");
    let server = MockServer::start(vec![
        FixtureResponse::json(valid_docs_json(&state, &["a.txt"])),
        FixtureResponse::json(valid_docs_json(&state, &["a.txt"])),
        FixtureResponse::json(valid_docs_json(&state, &["a.txt"])),
    ])
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), Some("docmock")).expect("router");
    polish_run_docs(
        &mut state,
        &router,
        &polish_config(paths.home(), false, false),
    )
    .await
    .expect("polish");
    assert_eq!(server.journal().len(), 3);
    assert_eq!(
        read_polish_record(&state).unwrap().unwrap().status,
        "polished"
    );
}

#[test]
fn apply_commit_body_contains_executive_summary() {
    let (_temp, _paths, state) = fresh_state("apply summary");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    assert!(apply_commit_body(&state).contains("The run advanced"));
}

#[test]
fn apply_commit_body_lists_phases() {
    let (_temp, _paths, state) = fresh_state("apply phases");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    assert!(apply_commit_body(&state).contains("Phases:"));
    assert!(apply_commit_body(&state).contains("- Phase 1"));
}

#[test]
fn apply_commit_body_links_to_run_narrative() {
    let (_temp, _paths, state) = fresh_state("apply trace link");
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    assert!(apply_commit_body(&state).contains("Trace: docs/RUN-NARRATIVE.md"));
}

#[test]
fn apply_message_flag_overrides_body() {
    let message = "custom message";
    assert_eq!(message, "custom message");
}

#[test]
fn doc_default_prints_narrative() {
    let (temp, paths, state) = completed_state_with_docs("doc default");
    let output = deadreckon(paths.home())
        .arg("doc")
        .arg(&state.run_id)
        .output()
        .expect("doc");
    assert_success(&output);
    assert!(stdout(&output).contains("## Goal"));
    drop(temp);
}

#[test]
fn doc_kind_as_built_prints_as_built() {
    let (_temp, paths, state) = completed_state_with_docs("doc as built");
    let output = deadreckon(paths.home())
        .args(["doc", &state.run_id, "--kind", "as-built"])
        .output()
        .expect("doc");
    assert_success(&output);
    assert!(stdout(&output).contains("## System overview"));
}

#[test]
fn doc_kind_decisions_prints_decisions() {
    let (_temp, paths, state) = completed_state_with_docs("doc decisions");
    let output = deadreckon(paths.home())
        .args(["doc", &state.run_id, "--kind", "decisions"])
        .output()
        .expect("doc");
    assert_success(&output);
    assert!(stdout(&output).contains("No multi-alternative decisions"));
}

#[test]
fn doc_kind_delta_prints_or_says_no_delta() {
    let (_temp, paths, state) = completed_state_with_docs("doc delta");
    let output = deadreckon(paths.home())
        .args(["doc", &state.run_id, "--kind", "delta"])
        .output()
        .expect("doc");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("no delta produced"));
}

#[test]
fn doc_export_writes_to_path() {
    let (temp, paths, state) = completed_state_with_docs("doc export");
    let dest = temp.path().join("narrative.md");
    let output = deadreckon(paths.home())
        .args(["doc", &state.run_id, "--export"])
        .arg(&dest)
        .output()
        .expect("doc");
    assert_success(&output);
    assert!(dest.exists());
}

#[test]
fn doc_export_refuses_existing_path_unless_force() {
    let (temp, paths, state) = completed_state_with_docs("doc export refuse");
    let dest = temp.path().join("narrative.md");
    fs::write(&dest, "exists").expect("dest");
    let output = deadreckon(paths.home())
        .args(["doc", &state.run_id, "--export"])
        .arg(&dest)
        .output()
        .expect("doc");
    assert!(!output.status.success());
    let forced = deadreckon(paths.home())
        .args(["doc", &state.run_id, "--export"])
        .arg(&dest)
        .arg("--force")
        .output()
        .expect("doc");
    assert_success(&forced);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doc_polish_triggers_fresh_call_with_confirm() {
    let (_temp, paths, state) = completed_state_with_docs("doc polish");
    let server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let output = deadreckon(paths.home())
        .args(["doc", &state.run_id, "--polish", "--no-confirm", "--force"])
        .output()
        .expect("doc");
    assert_success(&output);
    assert_eq!(server.journal().len(), 4);
    assert!(stdout(&output).contains("doc polish:"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polish_preview_skipped_with_no_confirm() {
    let (_temp, paths, state) = completed_state_with_docs("no preview");
    let server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let output = deadreckon(paths.home())
        .args(["doc", &state.run_id, "--polish", "--no-confirm", "--force"])
        .output()
        .expect("doc");
    assert_success(&output);
    assert!(!stdout(&output).contains("polish preview"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_polish_summary_lists_each_subcall_status() {
    let (_temp, paths, state) = completed_state_with_docs("summary subcalls");
    let server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let output = deadreckon(paths.home())
        .args(["doc", &state.run_id, "--polish", "--no-confirm", "--force"])
        .output()
        .expect("doc");
    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("subcalls:"));
    assert!(out.contains("narrator-overview"));
    assert!(out.contains("narrator-phases"));
    assert!(out.contains("narrator-as-built"));
    assert!(out.contains("narrator-decisions"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doc_polish_budget_cap_refuses_above_threshold() {
    let (_temp, paths, state) = completed_state_with_docs("budget cap");
    let server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_config_with_costs(paths.home(), &server.base_url(), "docmock", 10_000.0);
    let output = deadreckon(paths.home())
        .args([
            "doc",
            &state.run_id,
            "--polish",
            "--no-confirm",
            "--force",
            "--budget-cap",
            "0.01",
        ])
        .output()
        .expect("doc");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("doc polish would cost"));
    assert!(stderr(&output).contains("try: deadreckon doc"));
    assert_eq!(server.journal().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doc_polish_budget_cap_zero_for_subscription_providers() {
    let (temp, paths, state) = completed_state_with_docs("subscription budget");
    let fake_dir = temp.path().join("fake-codex-budget");
    write_fake_subscription_cli(&fake_dir, "codex", split_docs_fixtures(&["a.txt"]));
    write_config_without_doc_provider(paths.home());
    let output = deadreckon(paths.home())
        .env("PATH", fake_dir.join("bin"))
        .env("DEADRECKON_FAKE_CLI_DIR", &fake_dir)
        .args([
            "doc",
            &state.run_id,
            "--polish",
            "--no-confirm",
            "--force",
            "--budget-cap",
            "0",
        ])
        .output()
        .expect("doc");
    assert_success(&output);
    assert_eq!(read_polish_record(&state).unwrap().unwrap().cost_usd, 0.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doc_polish_force_ignores_inputs_hash() {
    let (_temp, paths, state) = completed_state_with_docs("force polish");
    let server = MockServer::start(
        split_docs_fixtures(&["a.txt"])
            .into_iter()
            .chain(split_docs_fixtures(&["a.txt"]))
            .collect(),
    )
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    for _ in 0..2 {
        let output = deadreckon(paths.home())
            .args(["doc", &state.run_id, "--polish", "--no-confirm", "--force"])
            .output()
            .expect("doc");
        assert_success(&output);
    }
    assert_eq!(server.journal().len(), 8);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doc_polish_force_re_runs_all_subcalls_not_just_changed() {
    let (_temp, paths, state) = completed_state_with_docs("force all subcalls");
    let server = MockServer::start(
        split_docs_fixtures(&["a.txt"])
            .into_iter()
            .chain(split_docs_fixtures(&["a.txt"]))
            .collect(),
    )
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    for _ in 0..2 {
        let output = deadreckon(paths.home())
            .args(["doc", &state.run_id, "--polish", "--no-confirm", "--force"])
            .output()
            .expect("doc");
        assert_success(&output);
    }
    let prompts = server.journal();
    assert_eq!(prompts.len(), 8);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doc_provider_resolves_to_cli_codex_when_in_path() {
    let (temp, paths, state) = completed_state_with_docs("auto cli docs");
    let fake_dir = temp.path().join("fake-codex");
    write_fake_subscription_cli(&fake_dir, "codex", split_docs_fixtures(&["a.txt"]));
    write_config_without_doc_provider(paths.home());

    let output = deadreckon(paths.home())
        .env("PATH", fake_dir.join("bin"))
        .env("DEADRECKON_FAKE_CLI_DIR", &fake_dir)
        .args(["doc", &state.run_id, "--polish", "--no-confirm", "--force"])
        .output()
        .expect("doc");
    assert_success(&output);
    assert!(stdout(&output).contains("provider: cli:codex"));
    assert!(stdout(&output).contains("cost:     $0.000000"));

    let narrative =
        fs::read_to_string(state.working_dir.join("docs/RUN-NARRATIVE.md")).expect("narrative");
    assert!(narrative.contains("**Doc-writer:** cli:codex via"));
    let record = read_polish_record(&state).unwrap().unwrap();
    assert_eq!(record.provider.as_deref(), Some("cli:codex"));
    assert_eq!(
        record.doc_provider_source.as_deref(),
        Some("auto_subscription")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doc_provider_resolves_to_cli_claude_code_when_codex_absent() {
    let (temp, paths, state) = completed_state_with_docs("auto claude docs");
    let fake_dir = temp.path().join("fake-claude");
    write_fake_subscription_cli(&fake_dir, "claude", split_docs_fixtures(&["a.txt"]));
    write_config_without_doc_provider(paths.home());

    let output = deadreckon(paths.home())
        .env("PATH", fake_dir.join("bin"))
        .env("DEADRECKON_FAKE_CLI_DIR", &fake_dir)
        .args(["doc", &state.run_id, "--polish", "--no-confirm", "--force"])
        .output()
        .expect("doc");
    assert_success(&output);
    let record = read_polish_record(&state).unwrap().unwrap();
    assert_eq!(record.provider.as_deref(), Some("cli:claude-code"));
    assert_eq!(
        record.doc_provider_source.as_deref(),
        Some("auto_subscription")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doc_provider_resolves_from_config_second() {
    let (_temp, paths, state) = completed_state_with_docs("config doc provider");
    let server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let output = deadreckon(paths.home())
        .env("PATH", "")
        .args(["doc", &state.run_id, "--polish", "--no-confirm", "--force"])
        .output()
        .expect("doc");
    assert_success(&output);
    let record = read_polish_record(&state).unwrap().unwrap();
    assert_eq!(record.provider.as_deref(), Some("docmock"));
    assert_eq!(record.doc_provider_source.as_deref(), Some("config"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doc_provider_resolves_from_flag_first() {
    let (_temp, paths, state) = completed_state_with_docs("flag doc provider");
    let config_server = MockServer::start(Vec::new()).await;
    let flag_server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_two_provider_config(
        paths.home(),
        &config_server.base_url(),
        "docmock",
        &flag_server.base_url(),
        "flagmock",
        "docmock",
    );
    let output = deadreckon(paths.home())
        .env("PATH", "")
        .args([
            "doc",
            &state.run_id,
            "--polish",
            "--doc-provider",
            "flagmock",
            "--no-confirm",
            "--force",
        ])
        .output()
        .expect("doc");
    assert_success(&output);
    assert_eq!(config_server.journal().len(), 0);
    assert_eq!(flag_server.journal().len(), 4);
    let record = read_polish_record(&state).unwrap().unwrap();
    assert_eq!(record.provider.as_deref(), Some("flagmock"));
    assert_eq!(record.doc_provider_source.as_deref(), Some("flag"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doc_provider_falls_back_to_run_provider_last() {
    let (temp, paths, mut state) = completed_state_with_docs("run provider docs");
    state.provider = Some("docmock".to_string());
    save_state(&state).expect("save");
    let server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_config_run_provider_only(paths.home(), &server.base_url(), "docmock");
    let empty_path = temp.path().join("empty-path");
    fs::create_dir_all(&empty_path).expect("empty path");
    let output = deadreckon(paths.home())
        .env("PATH", &empty_path)
        .args(["doc", &state.run_id, "--polish", "--no-confirm", "--force"])
        .output()
        .expect("doc");
    assert_success(&output);
    let record = read_polish_record(&state).unwrap().unwrap();
    assert_eq!(record.provider.as_deref(), Some("docmock"));
    assert_eq!(record.doc_provider_source.as_deref(), Some("run_provider"));
}

#[test]
fn no_doc_provider_emits_install_try_hint() {
    let (temp, paths, mut state) = completed_state_with_docs("no provider docs");
    state.provider = None;
    save_state(&state).expect("save");
    write_config_without_doc_provider(paths.home());
    let empty_path = temp.path().join("empty-path");
    fs::create_dir_all(&empty_path).expect("empty path");
    let output = deadreckon(paths.home())
        .env("PATH", &empty_path)
        .args(["doc", &state.run_id, "--polish", "--no-confirm", "--force"])
        .output()
        .expect("doc");
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(err.contains("no doc provider available"), "{err}");
    assert_eq!(err.matches("try:").count(), 1, "{err}");
    assert!(
        err.contains("try: deadreckon config set defaults.doc_provider cli:codex"),
        "{err}"
    );
    assert!(!err.contains("try: install codex or claude"), "{err}");
    assert!(!err.contains("try: deadreckon doctor"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doc_provider_records_source_in_polish_json() {
    let (_temp, paths, state) = completed_state_with_docs("provider source");
    let server = MockServer::start(split_docs_fixtures(&["a.txt"])).await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let output = deadreckon(paths.home())
        .args(["doc", &state.run_id, "--polish", "--no-confirm", "--force"])
        .output()
        .expect("doc");
    assert_success(&output);
    assert_eq!(
        read_polish_record(&state)
            .unwrap()
            .unwrap()
            .doc_provider_source
            .as_deref(),
        Some("config")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repolish_smoke_one_turn_fixture_yields_250_line_stoa_shape() {
    let (_temp, paths, state) = completed_state_with_docs(
        "make it possible to create and add a gallery of artwork and browse it without shallow docs",
    );
    let long_tail = "This sentence appears beyond the old two hundred character cutoff and proves the narrative keeps meaningful provider output instead of clipping mid-thought.";
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 2,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(20),
            files: vec![state.working_dir.join("src/gallery.rs")],
            outcome: format!("expanded gallery docs. {long_tail}"),
            response_text: format!("implemented gallery workflow. {long_tail}"),
            tool_stdout: Some("cargo test passed\nrendered gallery preview".to_string()),
            tool_stderr: None,
        },
    )
    .expect("turn 2");
    let server = MockServer::start(vec![
        FixtureResponse::json(long_overview_json()),
        FixtureResponse::json(long_phases_json(&["a.txt", "src/gallery.rs"], long_tail)),
        FixtureResponse::json(as_built_json(&["a.txt", "src/gallery.rs"])),
        FixtureResponse::json(decisions_json()),
    ])
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let output = deadreckon(paths.home())
        .args(["doc", &state.run_id, "--polish", "--no-confirm", "--force"])
        .output()
        .expect("doc");
    assert_success(&output);

    let narrative =
        fs::read_to_string(state.working_dir.join("docs/RUN-NARRATIVE.md")).expect("narrative");
    assert!(
        narrative.lines().count() >= 250,
        "{} lines",
        narrative.lines().count()
    );
    assert!(narrative.contains("### Phase 1"));
    assert!(narrative.contains(long_tail));
    assert!(narrative.contains("`src/gallery.rs`"));
    let as_built =
        fs::read_to_string(state.working_dir.join("docs/RUN-AS-BUILT.md")).expect("as built");
    assert!(as_built.contains("| Layer | Responsibilities | Key entrypoints |"));
    assert!(as_built.contains("`src/gallery.rs:1`"));
}

#[test]
fn extend_narrative_links_to_parent() {
    let (_temp, _paths, state) = fresh_state("extend child");
    fs::create_dir_all(state.working_dir.join(".deadreckon")).expect("marker dir");
    fs::write(
        state.working_dir.join(".deadreckon/parent.json"),
        json!({"parent_run_id":"parent123456789","parent_scope":"scope"}).to_string(),
    )
    .expect("parent marker");
    append_sample_turn(&state, 1, "write_file", &["child.txt"], "ok");
    let narrative = fs::read_to_string(narrative_path(&state.working_dir)).expect("narrative");
    assert!(narrative.contains("Reading order"));
    assert!(narrative.contains("Updates since the parent run"));
}

#[test]
fn extend_updates_parent_narrative_with_updates_since() {
    let (_temp, _paths, parent) = completed_state_with_docs("parent docs");
    let (_child_temp, _child_paths, child) = fresh_state("child docs");
    append_parent_narrative_update(&parent, &child).expect("append");
    let narrative =
        fs::read_to_string(parent.working_dir.join(".deadreckon/docs/RUN-NARRATIVE.md"))
            .expect("parent narrative");
    assert!(narrative.contains("## Updates since"));
    assert!(narrative.contains(&child.run_id));
}

#[test]
fn plan_narrative_aggregates_child_summaries() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut task = PlanTask::new(0, "parser", "parse provider logs", PlanRole::Child, None);
    task.status = PlanTaskStatus::Completed;
    task.child_run_id = Some("run123456789".to_string());
    task.summary_path = Some(child_summary_relative_path(&task.task_id));
    let mut reviewer = PlanTask::new(
        1,
        "reviewer",
        "review provider logs",
        PlanRole::Reviewer,
        None,
    );
    reviewer.status = PlanTaskStatus::Completed;
    reviewer.child_run_id = Some("run987654321".to_string());
    reviewer.summary_path = Some(child_summary_relative_path(&reviewer.task_id));
    let mut plan = Plan::new(
        "coherent plan docs",
        PlanMode::FullPlan,
        vec![task.clone(), reviewer.clone()],
        PlanProviders::default(),
        Some("scope".to_string()),
        "test",
    )
    .expect("plan");
    plan.status = PlanStatus::Merged;
    plan.merged_run_id = Some("result123456".to_string());
    write_child_summary(
        &paths,
        &plan.plan_id,
        &task.task_id,
        "Child summary says the parser now reports running.",
    )
    .expect("child summary");
    write_child_summary(
        &paths,
        &plan.plan_id,
        &reviewer.task_id,
        "Reviewer summary says the labels match the run.",
    )
    .expect("reviewer summary");

    let path = write_plan_narrative(&paths, &plan).expect("plan narrative");
    let text = fs::read_to_string(path).expect("read plan narrative");

    assert!(text.contains("**Status:** completed"), "{text}");
    assert!(text.contains("**Mode:** full-plan"), "{text}");
    assert!(text.contains("### Child 0: parser"), "{text}");
    assert!(text.contains("- Status: completed"), "{text}");
    assert!(text.contains("- Run: `run123456789`"), "{text}");
    assert!(
        text.contains("Child summary says the parser now reports running."),
        "{text}"
    );
}

#[test]
fn doc_help_describes_force_and_budget_cap_flags() {
    let help = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .args(["doc", "--help"])
        .output()
        .expect("help");
    assert_success(&help);
    let out = stdout(&help);
    assert!(out.contains("--overwrite"));
    assert!(out.contains("--max-spend"));
    assert!(out.contains("--doc-provider"));
}

#[test]
fn status_shows_docs_status() {
    let (_temp, paths, state) = completed_state_with_docs("list docs");
    let output = deadreckon(paths.home())
        .current_dir(&state.cwd)
        .args(["status", &state.run_id])
        .output()
        .expect("status");
    assert_success(&output);
    assert!(stdout(&output).contains("docs:"));
}

#[test]
fn status_explains_failed_polish_when_fallback_docs_exist() {
    let (_temp, paths, state) = completed_state_with_docs("failed docs");
    fs::write(
        polish_path(&state.working_dir),
        serde_json::to_vec_pretty(&json!({
            "schema_version": 2,
            "status": "failed_subcall:narrator-overview",
            "inputs_hash": "hash",
            "provider": "cli:codex",
            "skill_path": null,
            "skill_source": null,
            "completed_at": "2026-05-13T00:00:00Z",
            "cost_usd": 0.0,
            "retries": 0,
            "missing_files": [],
            "error": "subcall narrator-overview did not complete cleanly"
        }))
        .expect("json"),
    )
    .expect("polish");

    let output = deadreckon(paths.home())
        .current_dir(&state.cwd)
        .args(["status", &state.run_id])
        .output()
        .expect("status");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("docs:     failed"), "{out}");
    assert!(
        out.contains("polish failed") && out.contains("fallback docs are still available"),
        "{out}"
    );
}

fn fresh_state(goal: &str) -> (TempDir, DeadreckonPaths, deadreckon_core::PipelineState) {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = deadreckon_core::create_run(
        &paths,
        RunOptions {
            goal: goal.to_string(),
            cwd: temp.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("mock".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("create run");
    (temp, paths, state)
}

fn completed_state_with_docs(
    goal: &str,
) -> (TempDir, DeadreckonPaths, deadreckon_core::PipelineState) {
    let (temp, paths, mut state) = fresh_state(goal);
    append_sample_turn(&state, 1, "write_file", &["a.txt"], "ok");
    state.status = RunStatus::Completed;
    save_state(&state).expect("save");
    (temp, paths, state)
}

fn copy_state_with_source(
    goal: &str,
) -> (TempDir, DeadreckonPaths, deadreckon_core::PipelineState) {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(temp.path().join("source")).expect("source");
    let mut record = CodebaseRecord::fresh();
    record.mode = CodebaseMode::Copy;
    record.source_path = Some(temp.path().join("source"));
    let state = deadreckon_core::create_run(
        &paths,
        RunOptions {
            goal: goal.to_string(),
            cwd: temp.path().join("source"),
            sandbox: "none".to_string(),
            provider: Some("mock".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: None,
            codebase: Some(record),
        },
    )
    .expect("create run");
    (temp, paths, state)
}

fn worktree_state(
    goal: &str,
    file_count: usize,
) -> (TempDir, DeadreckonPaths, deadreckon_core::PipelineState) {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init"]);
    git(&repo, &["config", "user.email", "test@example.invalid"]);
    git(&repo, &["config", "user.name", "tester"]);
    fs::write(repo.join("base.txt"), "base").expect("base");
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-m", "base"]);
    let base_sha = git_out(&repo, &["rev-parse", "HEAD"]);
    for idx in 0..file_count {
        fs::write(repo.join(format!("file{idx}.txt")), "x").expect("file");
    }
    let mut record = CodebaseRecord::fresh();
    record.mode = CodebaseMode::Worktree;
    record.source_path = Some(repo.clone());
    record.source_git_root = Some(repo.clone());
    record.worktree_path = Some(repo.clone());
    record.branch_name = Some("main".to_string());
    record.base_ref = Some("HEAD".to_string());
    record.base_sha = Some(base_sha);
    let state = deadreckon_core::create_run(
        &paths,
        RunOptions {
            goal: goal.to_string(),
            cwd: repo.clone(),
            sandbox: "none".to_string(),
            provider: Some("mock".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: None,
            codebase: Some(record),
        },
    )
    .expect("create run");
    (temp, paths, state)
}

fn frontmatter_fields() -> FrontmatterFields {
    FrontmatterFields {
        last_updated: chrono::Utc::now(),
        doc_writer: "templated only".to_string(),
        parent_run_id: None,
    }
}

fn append_sample_turn(
    state: &deadreckon_core::PipelineState,
    turn: u32,
    tool: &str,
    files: &[&str],
    outcome: &str,
) -> TurnRecord {
    let file_paths = files
        .iter()
        .map(|file| state.working_dir.join(file))
        .collect::<Vec<_>>();
    append_turn_doc(
        state,
        TurnDocInput {
            turn,
            tool_kind: tool.to_string(),
            latency_ms: Some(10),
            files: file_paths,
            outcome: outcome.to_string(),
            response_text: outcome.to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("append docs")
}

fn write_notes(
    state: &deadreckon_core::PipelineState,
    design: &str,
    deviations: &str,
    tradeoffs: &str,
    open_questions: &str,
) {
    fs::write(
        implementation_notes_path(&state.working_dir),
        format!(
            r#"<!doctype html>
<html lang="en"><body>
<section id="design-decisions"><h2>Design decisions</h2><p>{design}</p></section>
<section id="deviations"><h2>Deviations</h2><p>{deviations}</p></section>
<section id="tradeoffs"><h2>Tradeoffs</h2><p>{tradeoffs}</p></section>
<section id="open-questions"><h2>Open questions</h2><p>{open_questions}</p></section>
</body></html>
"#
        ),
    )
    .expect("write notes");
}

fn turn_record(turn: u32, tool: &str, files: &[&str]) -> TurnRecord {
    TurnRecord {
        turn,
        title: auto_title(
            "",
            tool,
            &files
                .iter()
                .map(|file| file.to_string())
                .collect::<Vec<_>>(),
            turn,
        ),
        tool_kind: tool.to_string(),
        latency_ms: Some(1),
        files: files
            .iter()
            .map(|file| FileChange::for_path(file.to_string()))
            .collect(),
        outcome: "ok".to_string(),
        response_full: "ok".to_string(),
        response_summary: "ok".to_string(),
        tool_stdout: None,
        tool_stderr: None,
        trace_link: format!("../traces.jsonl#turn-{turn}"),
        snapshot_link: format!("../../snapshots/turn-{turn}/"),
        commit_sha: None,
        response_text: "ok".to_string(),
        decision_candidate: false,
    }
}

fn polish_config(home: &Path, no_llm: bool, force: bool) -> PolishConfig {
    PolishConfig {
        home: home.to_path_buf(),
        doc_skill: "run-narrator".to_string(),
        doc_provider: Some("docmock".to_string()),
        doc_provider_source: Some("config".to_string()),
        doc_subskills: Vec::new(),
        token_budget: 0,
        budget_cap_usd: None,
        no_llm,
        force,
    }
}

fn split_polish_config(home: &Path, force: bool) -> PolishConfig {
    PolishConfig {
        home: home.to_path_buf(),
        doc_skill: "run-narrator".to_string(),
        doc_provider: Some("docmock".to_string()),
        doc_provider_source: Some("config".to_string()),
        doc_subskills: vec![
            "narrator-overview".to_string(),
            "narrator-phases".to_string(),
            "narrator-as-built".to_string(),
            "narrator-decisions".to_string(),
        ],
        token_budget: 0,
        budget_cap_usd: None,
        no_llm: false,
        force,
    }
}

fn valid_docs_json(state: &deadreckon_core::PipelineState, files: &[&str]) -> String {
    let file_lines = files
        .iter()
        .map(|file| format!("`{file}`"))
        .collect::<Vec<_>>()
        .join(", ");
    json!({
        "narrative": format!("# {}\n\n**Date:** {}\n**Last updated:** {}\n**Status:** completed\n**Run ID:** `{}`\n**Goal:** {}\n**Owner:** tester (with deadreckon + docmock)\n**Provider:** docmock\n**Sandbox:** none\n**Spend:** $0.00\n**Doc-writer:** docmock\n\n## Goal\n\n{}\n\n## High-level approach\n\nPolished summary names {} and cites [turn 1](../traces.jsonl).\n\n## What shipped in this run\n\n### Phase 1 - Polish (commit `-`)\n\n- Files: {}\n\n## Open threads\n\n- none\n", state.goal, state.started_at.to_rfc3339(), chrono::Utc::now().to_rfc3339(), state.run_id, state.goal, state.goal, file_lines, file_lines),
        "as_built": format!("# as built\n\n## System overview\n\n{}", file_lines),
        "decisions": "No multi-alternative decisions detected in this run.\n",
        "delta": ""
    }).to_string()
}

fn phases_json(files: &[&str]) -> String {
    json!({
        "phases": [{
            "title": "Documented Work",
            "paragraph": format!("The provider completed the turn and changed {}.", files.join(", ")),
            "commit": "-",
            "file_changes": files.iter().map(|file| json!({
                "path": file,
                "adds": 3,
                "dels": 1,
                "largest_hunk_excerpt": format!("@@ -1,1 +1,3 @@\n+{}", file)
            })).collect::<Vec<_>>(),
            "citations": ["[turn 1](../traces.jsonl#turn-1)"]
        }]
    }).to_string()
}

fn long_overview_json() -> String {
    json!({
        "reading_order": "Start with the narrative to understand the run chronology, then read the as-built architecture for file-level ownership, then finish with decisions for what was intentionally left alone.",
        "why_now": "The run crossed the completion boundary, so the docs need to preserve not just that files changed but why the implementation hangs together and how a maintainer should re-enter it.",
        "high_level_approach": "deadreckon merged the provider trace, diff samples, stdout, and per-file provenance into a reader-friendly account instead of a generic turn stub.",
        "open_threads": ["Verify the gallery sorting behavior with a browser smoke before promoting to a public release."],
        "cross_references": ["Trace: `traces.jsonl`", "Narrative: `docs/RUN-NARRATIVE.md`", "Architecture: `docs/AS-BUILT-ARCHITECTURE.md`"]
    }).to_string()
}

fn long_phases_json(files: &[&str], sentinel: &str) -> String {
    let mut paragraph = String::new();
    paragraph.push_str("The provider completed a one-turn implementation and left enough evidence to reconstruct the work. ");
    paragraph.push_str(sentinel);
    paragraph.push('\n');
    for idx in 1..=260 {
        paragraph.push_str(&format!(
            "Evidence line {idx}: the run ties `{}` back to the trace, diff hunk, and generated narrative so the reader can audit the shipped behavior.\n",
            files[idx as usize % files.len()]
        ));
    }
    json!({
        "phases": [{
            "title": "Build Gallery Flow",
            "paragraph": paragraph,
            "commit": "-",
            "file_changes": files.iter().map(|file| json!({
                "path": file,
                "adds": 12,
                "dels": 2,
                "largest_hunk_excerpt": format!("@@ -1,1 +1,12 @@\n+{}", file)
            })).collect::<Vec<_>>(),
            "citations": ["[turn 1](../traces.jsonl#turn-1)", "[turn 2](../traces.jsonl#turn-2)"]
        }]
    })
    .to_string()
}

fn as_built_json(files: &[&str]) -> String {
    json!({
        "system_overview": format!("The run produced trace-backed files: {}.", files.join(", ")),
        "components": files.iter().map(|file| json!({
            "layer": format!("{} layer", file),
            "responsibilities": "Carries the implemented behavior.",
            "key_entrypoints": format!("`{}:1`", file)
        })).collect::<Vec<_>>(),
        "load_bearing_paths": files.iter().map(|file| format!("`{}:1`", file)).collect::<Vec<_>>().join(", "),
        "seams": "The provider boundary is captured in traces and run provenance."
    }).to_string()
}

fn decisions_json() -> String {
    json!({ "decisions": [] }).to_string()
}

fn split_docs_fixtures(files: &[&str]) -> Vec<FixtureResponse> {
    vec![
        FixtureResponse::json(overview_json_for_tests()),
        FixtureResponse::json(phases_json(files)),
        FixtureResponse::json(as_built_json(files)),
        FixtureResponse::json(decisions_json()),
    ]
}

fn overview_json_for_tests() -> String {
    json!({
        "reading_order": "Read the narrative first, then the as-built map and decisions.",
        "why_now": "This run needs durable documentation at the same boundary as the completed turn.",
        "high_level_approach": "deadreckon captured the completed turn and summarized the work from traces.",
        "open_threads": [],
        "cross_references": ["Trace: `traces.jsonl`", "Incremental docs: `.deadreckon/docs/_incremental.jsonl`"]
    }).to_string()
}

fn write_config(home: &Path, base_url: &str, provider: &str) {
    fs::create_dir_all(home).expect("home");
    fs::write(
        home.join("config.toml"),
        format!(
            r#"
fallback = ["{provider}"]

[defaults]
provider = "{provider}"
doc_provider = "{provider}"
doc_skill = "run-narrator"

[providers.{provider}]
kind = "open-ai-compatible"
base_url = "{base_url}"
model = "mock-agent"
api_key = "test"
input_cost_per_million = 0.0
output_cost_per_million = 0.0
"#
        ),
    )
    .expect("config");
}

fn write_config_with_costs(home: &Path, base_url: &str, provider: &str, output_cost: f64) {
    fs::create_dir_all(home).expect("home");
    fs::write(
        home.join("config.toml"),
        format!(
            r#"
fallback = ["{provider}"]

[defaults]
provider = "{provider}"
doc_provider = "{provider}"
doc_skill = "run-narrator"
doc_subskills = ["narrator-overview", "narrator-phases", "narrator-as-built", "narrator-decisions"]
doc_polish_token_budget = 16384

[providers.{provider}]
kind = "open-ai-compatible"
base_url = "{base_url}"
model = "mock-agent"
api_key = "test"
input_cost_per_million = 0.0
output_cost_per_million = {output_cost}
"#
        ),
    )
    .expect("config");
}

fn write_two_provider_config(
    home: &Path,
    first_base_url: &str,
    first_provider: &str,
    second_base_url: &str,
    second_provider: &str,
    doc_provider: &str,
) {
    fs::create_dir_all(home).expect("home");
    fs::write(
        home.join("config.toml"),
        format!(
            r#"
fallback = ["{first_provider}"]

[defaults]
provider = "{first_provider}"
doc_provider = "{doc_provider}"
doc_skill = "run-narrator"
doc_subskills = ["narrator-overview", "narrator-phases", "narrator-as-built", "narrator-decisions"]

[providers.{first_provider}]
kind = "open-ai-compatible"
base_url = "{first_base_url}"
model = "mock-agent"
api_key = "test"
input_cost_per_million = 0.0
output_cost_per_million = 0.0

[providers.{second_provider}]
kind = "open-ai-compatible"
base_url = "{second_base_url}"
model = "mock-agent"
api_key = "test"
input_cost_per_million = 0.0
output_cost_per_million = 0.0
"#
        ),
    )
    .expect("config");
}

fn write_config_run_provider_only(home: &Path, base_url: &str, provider: &str) {
    fs::create_dir_all(home).expect("home");
    fs::write(
        home.join("config.toml"),
        format!(
            r#"
fallback = ["{provider}"]

[defaults]
provider = "{provider}"
doc_skill = "run-narrator"
doc_subskills = ["narrator-overview", "narrator-phases", "narrator-as-built", "narrator-decisions"]

[providers.{provider}]
kind = "open-ai-compatible"
base_url = "{base_url}"
model = "mock-agent"
api_key = "test"
input_cost_per_million = 0.0
output_cost_per_million = 0.0
"#
        ),
    )
    .expect("config");
}

fn write_config_without_doc_provider(home: &Path) {
    fs::create_dir_all(home).expect("home");
    fs::write(
        home.join("config.toml"),
        r#"
[defaults]
doc_skill = "run-narrator"
doc_subskills = ["narrator-overview", "narrator-phases", "narrator-as-built", "narrator-decisions"]
doc_polish_token_budget = 16384
"#,
    )
    .expect("config");
}

fn write_fake_subscription_cli(root: &Path, binary: &str, fixtures: Vec<FixtureResponse>) {
    let bin_dir = root.join("bin");
    fs::create_dir_all(&bin_dir).expect("fake bin");
    for (idx, fixture) in fixtures.iter().enumerate() {
        fs::write(root.join(format!("{}.json", idx + 1)), &fixture.content).expect("fixture");
    }
    fs::write(
        bin_dir.join(binary),
        r#"#!/bin/sh
set -eu
count_file="$DEADRECKON_FAKE_CLI_DIR/count"
if [ -f "$count_file" ]; then
  n="$(/bin/cat "$count_file")"
else
  n=0
fi
n=$((n + 1))
printf '%s' "$n" > "$count_file"
/bin/cat "$DEADRECKON_FAKE_CLI_DIR/$n.json"
"#,
    )
    .expect("fake cli");
    #[cfg(unix)]
    {
        let path = bin_dir.join(binary);
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod");
    }
}

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "{}{}",
        stdout(&output),
        stderr(&output)
    );
}

fn git_out(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "{}{}",
        stdout(&output),
        stderr(&output)
    );
    stdout(&output).trim().to_string()
}

#[derive(Clone)]
struct MockState {
    fixtures: Arc<Mutex<Vec<FixtureResponse>>>,
    journal: Arc<Mutex<Vec<Value>>>,
}

#[derive(Debug, Clone)]
struct FixtureResponse {
    content: String,
}

impl FixtureResponse {
    fn json(content: String) -> Self {
        Self { content }
    }

    fn text(content: &str) -> Self {
        Self {
            content: content.to_string(),
        }
    }
}

struct MockServer {
    addr: SocketAddr,
    state: MockState,
}

impl MockServer {
    async fn start(fixtures: Vec<FixtureResponse>) -> Self {
        let state = MockState {
            fixtures: Arc::new(Mutex::new(fixtures)),
            journal: Arc::new(Mutex::new(Vec::new())),
        };
        let app = Router::new()
            .route("/chat/completions", post(chat_completions))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });
        Self { addr, state }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn journal(&self) -> Vec<Value> {
        self.state.journal.lock().expect("journal").clone()
    }
}

async fn chat_completions(
    State(state): State<MockState>,
    Json(request): Json<Value>,
) -> impl IntoResponse {
    state.journal.lock().expect("journal").push(request);
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
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "total_tokens": 150
            }
        })),
    )
}
