#![allow(clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::process::Command;

use chrono::Utc;
use deadreckon_core::learning::{
    LearningAutoPrStatus, LearningCandidate, LearningCandidateDiff, LearningEval,
    LearningEvalCommand, LearningProposal, LearningProposalTarget, LearningRisk, LearningStimulus,
};
use deadreckon_core::{DeadreckonPaths, RunOptions, RunStatus, create_run, save_state};
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn learn_index_writes_episode_and_signals_for_completed_run() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path());
    let cwd = std::env::current_dir().expect("cwd");
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: "provider setup failure".to_string(),
            cwd,
            sandbox: "seatbelt".to_string(),
            provider: Some("cli:codex".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    state.status = RunStatus::Failed;
    state.updated_at = Utc::now();
    state.failure_reason = Some("provider route cli:missing has no credential".to_string());
    save_state(&state).expect("save");

    let output = deadreckon(temp.path())
        .args(["learn", "index", "--all", "--json"])
        .output()
        .expect("learn index");

    assert_success(&output);
    let json: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(json["indexed"], 1);
    assert!(json["signals_written"].as_u64().unwrap_or(0) > 0);
    assert!(
        paths
            .learning_episode_path(&state.scope, &state.run_id)
            .exists()
    );

    let report = deadreckon(temp.path())
        .args(["learn", "report", "--scope", &state.scope, "--json"])
        .output()
        .expect("learn report");

    assert_success(&report);
    let json: Value = serde_json::from_slice(&report.stdout).expect("json");
    assert_eq!(json["episodes"], 1);
    assert!(json["signals"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn improve_self_preview_accepts_goal_file_without_side_effect() {
    let temp = repo_tempdir();
    let goal = temp.path().join("self-goal.md");
    fs::write(&goal, "Make the learning report friendlier.").expect("goal");

    let output = deadreckon(temp.path())
        .args([
            "improve",
            "self",
            goal.to_str().expect("utf8"),
            "--preview",
            "--json",
        ])
        .output()
        .expect("improve preview");

    assert_success(&output);
    let json: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(json["mode"], "isolated-worktree");
    assert!(!temp.path().join("learning").join("candidates").exists());
}

#[test]
fn learn_propose_defaults_to_local_evidence_source() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path());
    let cwd = std::env::current_dir().expect("cwd");
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: "provider setup failure".to_string(),
            cwd,
            sandbox: "seatbelt".to_string(),
            provider: Some("cli:codex".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    state.status = RunStatus::Failed;
    state.updated_at = Utc::now();
    state.failure_reason = Some("provider route cli:missing has no credential".to_string());
    save_state(&state).expect("save");

    let index = deadreckon(temp.path())
        .args(["learn", "index", "--all", "--json"])
        .output()
        .expect("learn index");
    assert_success(&index);

    fs::write(
        paths.config_path(),
        "default_provider = \"missing-learning-provider\"\n",
    )
    .expect("config");
    let output = deadreckon(temp.path())
        .args(["learn", "propose", "--json"])
        .output()
        .expect("learn propose");

    assert_failure(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown provider route missing-learning-provider"));
    assert!(!stderr.contains("needs an evidence source"));
}

#[test]
fn learn_export_import_bundle_preview_roundtrips_redacted_counts() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path());
    let cwd = std::env::current_dir().expect("cwd");
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: format!("provider setup failure {}", paths.home().display()),
            cwd,
            sandbox: "seatbelt".to_string(),
            provider: Some("cli:codex".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    state.status = RunStatus::Failed;
    state.updated_at = Utc::now();
    state.failure_reason = Some("provider route cli:missing has no credential".to_string());
    save_state(&state).expect("save");

    let index = deadreckon(temp.path())
        .args(["learn", "index", "--all", "--json"])
        .output()
        .expect("learn index");
    assert_success(&index);
    let bundle = temp.path().join("bundle.json");
    let export = deadreckon(temp.path())
        .args([
            "learn",
            "export",
            &state.run_id,
            "--output",
            bundle.to_str().expect("utf8"),
            "--redacted",
            "--json",
        ])
        .output()
        .expect("learn export");
    assert_success(&export);
    let raw = fs::read_to_string(&bundle).expect("bundle");
    assert!(!raw.contains(temp.path().to_string_lossy().as_ref()));

    let preview = deadreckon(temp.path())
        .args([
            "learn",
            "import-bundle",
            bundle.to_str().expect("utf8"),
            "--preview",
            "--json",
        ])
        .output()
        .expect("learn import-bundle");
    assert_success(&preview);
    let json: Value = serde_json::from_slice(&preview.stdout).expect("json");
    assert_eq!(json["preview"], true);
    assert_eq!(json["applied"], false);
    assert_eq!(json["episodes"], 1);
    assert!(json["signals"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn improve_self_pr_dry_run_writes_body_without_network() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path());
    write_synthetic_pr_fixture(&paths);

    let output = deadreckon(temp.path())
        .args(["improve", "self", "prop-cli", "--pr-dry-run", "--json"])
        .output()
        .expect("pr dry-run");

    assert_success(&output);
    let json: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(json["decision"]["eligible"], true);
    let body_path = json["body_path"].as_str().expect("body_path");
    assert!(Path::new(body_path).exists());
    assert!(paths.learning_pr_events_path().exists());
}

fn repo_tempdir() -> TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.test-tmp");
    fs::create_dir_all(&root).expect("test tmp root");
    TempDir::new_in(&root).expect("tempdir")
}

fn deadreckon(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command.env("DEADRECKON_HOME", home).env("NO_COLOR", "1");
    command
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(output: &std::process::Output) {
    assert!(
        !output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_synthetic_pr_fixture(paths: &DeadreckonPaths) {
    let proposal = LearningProposal {
        version: 1,
        proposal_id: "prop-cli".to_string(),
        created_at: Utc::now(),
        title: "CLI dry run".to_string(),
        insights: vec!["ins-cli".to_string()],
        stimulus: vec![LearningStimulus {
            signal_id: "sig-cli".to_string(),
            run_id: "run-cli".to_string(),
        }],
        hypothesis: "dry-run path works".to_string(),
        target: LearningProposalTarget {
            repo: "/Users/gdc/deadreckon".to_string(),
            scope: "cli".to_string(),
        },
        goal_text: "Make PR dry-run testable.".to_string(),
        done_criteria: vec!["focused test passes".to_string()],
        expected_risk: "low".to_string(),
        blocked_auto_pr_reasons: Vec::new(),
    };
    fs::create_dir_all(paths.learning_proposals_dir()).expect("proposals");
    fs::write(
        paths.learning_proposal_path(&proposal.proposal_id),
        serde_json::to_vec_pretty(&proposal).expect("proposal"),
    )
    .expect("proposal write");

    let candidate = LearningCandidate {
        version: 1,
        candidate_id: "cand-cli".to_string(),
        proposal_id: proposal.proposal_id,
        branch: "deadreckon/self/cand-cli".to_string(),
        base_commit: "base".to_string(),
        head_commit: "head".to_string(),
        run_id: "run-candidate".to_string(),
        worktree: paths.learning_candidate_dir("cand-cli").join("worktree"),
        diff: LearningCandidateDiff {
            files: 1,
            insertions: 2,
            deletions: 0,
            changed_files: vec!["crates/deadreckon/src/main.rs".to_string()],
        },
        risk: LearningRisk {
            class: "low".to_string(),
            reasons: Vec::new(),
        },
        status: "verified".to_string(),
        evidence_packet: "evidence.json".to_string(),
    };
    fs::create_dir_all(paths.learning_candidate_dir(&candidate.candidate_id)).expect("candidate");
    fs::write(
        paths.learning_candidate_path(&candidate.candidate_id),
        serde_json::to_vec_pretty(&candidate).expect("candidate"),
    )
    .expect("candidate write");

    let eval = LearningEval {
        version: 1,
        candidate_id: candidate.candidate_id,
        evaluated_at: Utc::now(),
        accepted_run: true,
        commands: vec![LearningEvalCommand {
            cmd: "cargo test -p deadreckon-core learning --lib".to_string(),
            status: 0,
        }],
        docs_updated: true,
        redaction_passed: true,
        evidence_score: 1.0,
        auto_pr: LearningAutoPrStatus {
            eligible: true,
            reasons: Vec::new(),
        },
    };
    let eval_path = paths.learning_eval_path(&eval.candidate_id);
    fs::create_dir_all(eval_path.parent().expect("eval parent")).expect("eval dir");
    fs::write(eval_path, serde_json::to_vec_pretty(&eval).expect("eval")).expect("eval write");
}
