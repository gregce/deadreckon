use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use deadreckon_core::{
    CodebaseMode, CodebaseRecord, DeadreckonPaths, FrontmatterFields, RunOptions, RunStatus,
    TurnDocInput, TurnRecord, append_parent_narrative_update, append_turn_doc, apply_commit_body,
    as_built_path, auto_title, coalesce_into_phases, decisions_path, docs_dir, frontmatter,
    is_decision_candidate, missing_files_in_narrative, narrative_path, publish_docs_for_promotion,
    rewrite_templated_docs, save_state, should_emit_delta,
};
use deadreckon_providers::ProviderRouter;
use deadreckon_runtime::{
    PolishConfig, polish_run_docs, read_polish_record, resolve_skill, substitute_placeholders,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;

#[test]
fn docs_dir_created_at_run_start() {
    let (_temp, _paths, state) = fresh_state("write docs at start");
    assert!(docs_dir(&state.working_dir).is_dir());
    assert!(narrative_path(&state.working_dir).exists());
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
    assert!(decisions.contains("## Decision 1"));
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
fn placeholder_substitution_replaces_known_handles_unknown_passthrough() {
    let out = substitute_placeholders("{{ goal }} {{unknown}}", &[("goal", "ship".to_string())]);
    assert_eq!(out, "ship {{unknown}}");
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
async fn still_missing_after_two_retries_logs_warning_but_promotes() {
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
    assert!(apply_commit_body(&state).contains("deadreckon progressed"));
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
    let server = MockServer::start(vec![FixtureResponse::json(valid_docs_json(
        &state,
        &["a.txt"],
    ))])
    .await;
    write_config(paths.home(), &server.base_url(), "docmock");
    let output = deadreckon(paths.home())
        .args(["doc", &state.run_id, "--polish", "--no-confirm", "--force"])
        .output()
        .expect("doc");
    assert_success(&output);
    assert_eq!(server.journal().len(), 1);
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
    let help = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .arg("--help")
        .output()
        .expect("help");
    assert!(
        !stdout(&help).contains(" plan "),
        "orchestrate not landed; plan narrative gated"
    );
}

#[test]
fn list_shows_docs_status_column() {
    let (_temp, paths, state) = completed_state_with_docs("list docs");
    let output = deadreckon(paths.home())
        .current_dir(&state.cwd)
        .args(["list", "--full"])
        .output()
        .expect("list");
    assert_success(&output);
    assert!(stdout(&output).contains("DOCS"));
}

fn fresh_state(goal: &str) -> (TempDir, DeadreckonPaths, deadreckon_core::PipelineState) {
    let temp = TempDir::new_in("/Users/gdc/deadreckon/.test-tmp").expect("tempdir");
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
    let temp = TempDir::new_in("/Users/gdc/deadreckon/.test-tmp").expect("tempdir");
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
    let temp = TempDir::new_in("/Users/gdc/deadreckon/.test-tmp").expect("tempdir");
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
        },
    )
    .expect("append docs")
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
        files: files.iter().map(|file| file.to_string()).collect(),
        outcome: "ok".to_string(),
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
        no_llm,
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
        "narrative": format!("# {}\n\n**Date:** {}\n**Last updated:** {}\n**Status:** completed (alpha)\n**Run ID:** `{}`\n**Goal:** {}\n**Owner:** tester (with deadreckon + docmock)\n**Provider:** docmock\n**Sandbox:** none\n**Spend:** $0.00\n**Doc-writer:** docmock\n\n## Goal\n\n{}\n\n## High-level approach\n\nPolished summary names {} and cites [turn 1](../traces.jsonl).\n\n## What shipped in this run\n\n### Phase 1 - Polish (commit `-`)\n\n- Files: {}\n\n## Open threads\n\n- none\n", state.goal, state.started_at.to_rfc3339(), chrono::Utc::now().to_rfc3339(), state.run_id, state.goal, state.goal, file_lines, file_lines),
        "as_built": format!("# as built\n\n## System overview\n\n{}", file_lines),
        "decisions": "No multi-alternative decisions detected in this run.\n",
        "delta": ""
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

fn deadreckon(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    cmd.env("DEADRECKON_HOME", home);
    cmd
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

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}{}",
        stdout(output),
        stderr(output)
    );
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
