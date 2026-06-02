use tempfile::TempDir;

use super::*;

fn fixture_state(goal: &str) -> (TempDir, deadreckon_core::PipelineState) {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let state = create_run(
        &paths,
        RunOptions {
            goal: goal.to_string(),
            cwd: temp.path().to_path_buf(),
            sandbox: "none".to_string(),
            provider: Some("cli:test".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    (temp, state)
}

fn failed_cargo_progress(state: &deadreckon_core::PipelineState) {
    let path = acceptance_progress_path_for_run_root(&state.run_root);
    std::fs::create_dir_all(path.parent().expect("proofs")).expect("proofs");
    let entry = AcceptanceProgressEntry {
        checked_at: Utc::now(),
        status: "failed".to_string(),
        index: 1,
        total: 1,
        result: Some(deadreckon_core::AcceptanceCheckResult {
            kind: "cargo_test".to_string(),
            passed: false,
            must_pass: true,
            detail: "auth::tests::expired_token".to_string(),
            command: Some("cargo test".to_string()),
            cwd: Some(state.working_dir.clone()),
            duration_ms: Some(10),
            stdout: None,
            stderr: None,
        }),
    };
    std::fs::write(
        path,
        format!("{}\n", serde_json::to_string(&entry).expect("json")),
    )
    .expect("progress");
}

fn write_caveat_proof(state: &deadreckon_core::PipelineState) {
    let tamper = deadreckon_core::tamper::AcceptanceTamper {
        schema_version: 1,
        run_id: state.run_id.clone(),
        evaluated_at: Utc::now(),
        verdict: deadreckon_core::tamper::AcceptanceTamperVerdict::Caveat,
        spec_modified: false,
        lint_findings: Vec::new(),
        covered_files_touched: vec![deadreckon_core::tamper::CoveredFileTouch {
            path: "tests/auth_test.rs".to_string(),
            change: deadreckon_core::tamper::TouchedChange::Modified,
            by_check: "cargo_test".to_string(),
            classification: deadreckon_core::tamper::CoverageClassification::Test,
        }],
        caveats: vec!["agent modified test file tests/auth_test.rs this run".to_string()],
        refusal_reasons: Vec::new(),
    };
    deadreckon_core::tamper::write_acceptance_tamper(&state.run_root, &tamper).expect("tamper");
    deadreckon_core::write_acceptance_marker_with_results(
        &state.run_root,
        state.run_id.clone(),
        state.working_dir.clone(),
        vec![deadreckon_core::AcceptanceCheckResult {
            kind: "cargo_test".to_string(),
            passed: true,
            must_pass: true,
            detail: "cargo test exited with exit status: 0".to_string(),
            command: Some("cargo test".to_string()),
            cwd: Some(state.working_dir.clone()),
            duration_ms: Some(10),
            stdout: None,
            stderr: None,
        }],
    )
    .expect("marker");
}

fn write_refusal_proof(state: &deadreckon_core::PipelineState) {
    let tamper = deadreckon_core::tamper::AcceptanceTamper {
        schema_version: 1,
        run_id: state.run_id.clone(),
        evaluated_at: Utc::now(),
        verdict: deadreckon_core::tamper::AcceptanceTamperVerdict::Refuse,
        spec_modified: false,
        lint_findings: vec![deadreckon_core::tamper::SuppressionFinding {
            check_kind: "shell".to_string(),
            command: "cargo test || true".to_string(),
            pattern: "|| true".to_string(),
        }],
        covered_files_touched: Vec::new(),
        caveats: Vec::new(),
        refusal_reasons: vec!["suppression pattern '|| true' in shell check".to_string()],
    };
    deadreckon_core::tamper::write_acceptance_tamper(&state.run_root, &tamper).expect("tamper");
}

#[test]
fn exit_card_shows_per_check_verdict_and_failing_detail() {
    let (_temp, state) = fixture_state("failed gate render");
    failed_cargo_progress(&state);

    let rendered = render_exit_summary_card(&state, &RunLoopOutcome::Failed, true, true);

    assert!(rendered.contains("gate: FAILED 0/1"), "{rendered}");
    assert!(
        rendered.contains("cargo_test x auth::tests::expired_token"),
        "{rendered}"
    );
}

#[test]
fn caveat_run_renders_warn_tone_caveat_line() {
    let (_temp, state) = fixture_state("caveat gate render");
    write_caveat_proof(&state);

    let input = exit_summary_input(&state, &RunLoopOutcome::Done);
    let rendered = render_card(
        &build_exit_summary_card(&input),
        &card_options(ui::Stream::Stdout, true),
    );

    assert_eq!(input.gate_tone, Tone::Warn);
    assert!(rendered.contains("caveat"), "{rendered}");
    assert!(rendered.contains("tests/auth_test.rs"), "{rendered}");
}

#[test]
fn status_shows_tests_modified_flag() {
    let (_temp, state) = fixture_state("status test modified");
    write_caveat_proof(&state);

    let status = acceptance_status_line(&state);

    assert!(status.contains("tests modified this run: yes"), "{status}");
}

#[test]
fn status_prefers_refusal_reason_over_passing_progress() {
    let (_temp, state) = fixture_state("status refusal");
    let path = acceptance_progress_path_for_run_root(&state.run_root);
    std::fs::create_dir_all(path.parent().expect("proofs")).expect("proofs");
    let entry = AcceptanceProgressEntry {
        checked_at: Utc::now(),
        status: "passed".to_string(),
        index: 1,
        total: 1,
        result: Some(deadreckon_core::AcceptanceCheckResult {
            kind: "shell".to_string(),
            passed: true,
            must_pass: true,
            detail: "shell exited with exit status: 0".to_string(),
            command: Some("cargo test || true".to_string()),
            cwd: Some(state.working_dir.clone()),
            duration_ms: Some(10),
            stdout: None,
            stderr: None,
        }),
    };
    std::fs::write(
        path,
        format!("{}\n", serde_json::to_string(&entry).expect("json")),
    )
    .expect("progress");
    write_refusal_proof(&state);

    let status = acceptance_status_line(&state);
    let evidence = acceptance_failure_evidence_lines(&state).join("\n");

    assert!(status.contains("gate: REFUSED"), "{status}");
    assert!(status.contains("suppression pattern '|| true'"), "{status}");
    assert!(evidence.contains("acceptance refused"), "{evidence}");
}
