use std::fs;

use deadreckon_core::{
    DeadreckonPaths, RunOptions, TurnDocInput, append_turn_doc, as_built_path,
    capture_response_full, capture_response_summary, diff_samples_markdown, narrative_path,
    read_turn_records, rewrite_templated_docs, snapshot_working, source_layout,
};
use tempfile::TempDir;

#[test]
fn incremental_jsonl_carries_full_response_up_to_50kb() {
    let long = "a".repeat(70 * 1024);
    let captured = capture_response_full(&long);
    assert_eq!(captured.len(), 50 * 1024);
}

#[test]
fn response_summary_ends_on_word_boundary() {
    let text = format!("{} {}", "alpha ".repeat(80), "tailword");
    let summary = capture_response_summary(&text);
    assert!(summary.len() <= 280);
    assert!(!summary.ends_with("tailwo"));
}

#[test]
fn incremental_jsonl_carries_diff_samples_per_file() {
    let (_temp, state) = fresh_state("diff sample");
    fs::write(state.working_dir.join("README.md"), "one\ntwo\nthree\n").expect("base");
    snapshot_working(&state, 0).expect("snapshot");
    fs::write(
        state.working_dir.join("README.md"),
        "one\nchanged\nthree\nnew\n",
    )
    .expect("changed");

    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(1),
            files: vec![state.working_dir.join("README.md")],
            outcome: "updated README".to_string(),
            response_text: "updated README".to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let records = read_turn_records(&state.working_dir).expect("records");
    let file = &records[0].files[0];
    assert_eq!(file.path, "README.md");
    assert!(file.adds >= 1);
    assert!(file.dels >= 1);
    assert!(file.largest_hunk_excerpt.contains("@@"));
    assert!(diff_samples_markdown(&records).contains("+changed"));
}

#[test]
fn binary_files_marked_is_binary_no_excerpt() {
    let (_temp, state) = fresh_state("binary sample");
    snapshot_working(&state, 0).expect("snapshot");
    fs::write(state.working_dir.join("asset.bin"), [0, 159, 146, 150]).expect("binary");
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(1),
            files: vec![state.working_dir.join("asset.bin")],
            outcome: "wrote binary".to_string(),
            response_text: "wrote binary".to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let records = read_turn_records(&state.working_dir).expect("records");
    assert!(records[0].files[0].is_binary);
    assert!(records[0].files[0].largest_hunk_excerpt.is_empty());
}

#[test]
fn narrative_heading_uses_full_goal_no_truncation() {
    let goal = "make it possible to create and add a gallery of artwork and browse it with a deliberately long title";
    let (_temp, state) = fresh_state(goal);
    rewrite_templated_docs(&state, "templated only").expect("docs");
    let narrative = fs::read_to_string(narrative_path(&state.working_dir)).expect("narrative");
    assert!(narrative.starts_with(&format!("# {goal}")));
}

#[test]
fn component_table_omits_unmapped_project_files() {
    let (_temp, state) = fresh_state("component inference");
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(1),
            files: vec![
                state.working_dir.join("crates/app/src/lib.rs"),
                state.working_dir.join("misc.dat"),
            ],
            outcome: "changed crate".to_string(),
            response_text: "changed crate".to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let as_built = fs::read_to_string(as_built_path(&state.working_dir)).expect("as built");
    assert!(as_built.contains("Crate app (Rust)"));
    assert!(!as_built.contains("Project files"));
}

#[test]
fn topology_emitted_only_when_three_or_more_top_dirs() {
    let (_temp, state) = fresh_state("topology");
    append_turn_doc(
        &state,
        TurnDocInput {
            turn: 1,
            tool_kind: "write_file".to_string(),
            latency_ms: Some(1),
            files: vec![
                state.working_dir.join("crates/app/src/lib.rs"),
                state.working_dir.join("skills/x/SKILL.md"),
                state.working_dir.join("docs/guide.md"),
            ],
            outcome: "changed dirs".to_string(),
            response_text: "changed dirs".to_string(),
            tool_stdout: None,
            tool_stderr: None,
        },
    )
    .expect("turn");
    let records = read_turn_records(&state.working_dir).expect("records");
    let layout = source_layout(&records, &state.working_dir);
    assert!(layout.contains("+-----------+"));
    assert!(layout.contains("| crates/"));
}

fn fresh_state(goal: &str) -> (TempDir, deadreckon_core::PipelineState) {
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
    (temp, state)
}
