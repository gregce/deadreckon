#![allow(clippy::expect_used)]

use std::fs;
use std::path::Path;
use std::process::Command;

use chrono::Utc;
use deadreckon_core::learning::{
    LearningAutoPrStatus, LearningCandidate, LearningCandidateDiff, LearningEval,
    LearningEvalCommand, LearningProposal, LearningProposalTarget, LearningRisk, LearningSignal,
    LearningStimulus, read_signals,
};
use deadreckon_core::{DeadreckonPaths, RunOptions, RunStatus, create_run, save_state};
use serde_json::Value;

mod common;

use common::{
    assert_success_with_labels as assert_success, deadreckon_home_no_color as deadreckon,
    repo_tempdir,
};

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
    assert_eq!(json["verdict"]["kind"], "completed");
    assert_eq!(json["primary_action"], "deadreckon learn propose");
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
fn improve_self_preview_has_no_worktree_or_run_side_effect() {
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
    assert_eq!(json["verdict"]["kind"], "preview");
    assert_eq!(
        json["primary_action"],
        format!(
            "deadreckon improve self {} --yes",
            json["proposal_id"].as_str().expect("proposal id")
        )
    );
    assert!(!temp.path().join("learning").join("candidates").exists());

    let human = deadreckon(temp.path())
        .args(["improve", "self", goal.to_str().expect("utf8"), "--preview"])
        .output()
        .expect("improve preview human");

    assert_success(&human);
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.starts_with("preview improve self"), "{stdout}");
    assert!(stdout.contains("Explanation\n"), "{stdout}");
    assert!(stdout.contains("Evidence\n"), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(stdout.contains(" --yes"), "{stdout}");
    assert!(!stdout.contains("next:"), "{stdout}");
}

#[test]
fn improve_self_launch_uses_isolated_worktree_and_existing_provider_resolver() {
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
    assert_eq!(json["provider"], "existing resolver");
}

#[test]
fn improve_self_refuses_dirty_base_and_sandbox_none() {
    let temp = repo_tempdir();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    run_git(&repo, &["init"]);
    fs::write(repo.join("README.md"), "# repo\n").expect("readme");
    run_git(&repo, &["add", "README.md"]);
    run_git(
        &repo,
        &[
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "init",
        ],
    );
    fs::write(
        temp.path().join("config.toml"),
        "default_provider = \"smoke\"\n\n[defaults]\nsandbox = \"none\"\n",
    )
    .expect("config");
    let goal = temp.path().join("goal.md");
    fs::write(&goal, "Improve from isolated worktree.").expect("goal");

    let output = deadreckon(temp.path())
        .current_dir(&repo)
        .args(["improve", "self", goal.to_str().expect("utf8"), "--yes"])
        .output()
        .expect("improve self");

    assert_failure(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("self-improve refuses sandbox none"));
    assert!(stderr.contains("deadreckon config sandbox auto"));
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
fn learn_propose_success_has_one_verdict_and_primary_action() {
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
    let signals = read_signals(&paths).expect("signals");
    let signal = signals.first().expect("signal");
    write_fake_learning_reflection_provider(&paths, temp.path(), signal);

    let json_output = deadreckon(temp.path())
        .args(["learn", "propose", "--limit", "1", "--json"])
        .output()
        .expect("learn propose json");
    assert_success(&json_output);
    let json: Value = serde_json::from_slice(&json_output.stdout).expect("json");
    assert_eq!(json["insights_written"], 1);
    assert_eq!(json["proposals_written"], 1);
    let proposal_id = json["proposals"][0]["proposal_id"]
        .as_str()
        .expect("proposal id");
    let expected_primary = format!("deadreckon improve self {proposal_id} --preview");
    assert_eq!(json["verdict"]["kind"], "completed");
    assert_eq!(json["primary_action"], expected_primary);

    let human = deadreckon(temp.path())
        .args(["learn", "propose", "--limit", "1"])
        .output()
        .expect("learn propose human");
    assert_success(&human);
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.starts_with("completed learn propose"), "{stdout}");
    assert!(stdout.contains("Explanation\n"), "{stdout}");
    assert!(stdout.contains("Evidence\n"), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(stdout.contains(&expected_primary), "{stdout}");
    assert!(!stdout.contains("next:"), "{stdout}");
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
    let export_json: Value = serde_json::from_slice(&export.stdout).expect("export json");
    assert_eq!(export_json["verdict"]["kind"], "completed");
    assert!(
        export_json["primary_action"]
            .as_str()
            .expect("primary action")
            .contains("deadreckon learn import-bundle")
    );
    let raw = fs::read_to_string(&bundle).expect("bundle");
    assert!(!raw.contains(temp.path().to_string_lossy().as_ref()));

    let human_bundle = temp.path().join("bundle-human.json");
    let human_export = deadreckon(temp.path())
        .args([
            "learn",
            "export",
            &state.run_id,
            "--output",
            human_bundle.to_str().expect("utf8"),
            "--redacted",
        ])
        .output()
        .expect("learn export human");
    assert_success(&human_export);
    let stdout = String::from_utf8_lossy(&human_export.stdout);
    assert!(stdout.starts_with("completed learn export"), "{stdout}");
    assert!(stdout.contains("Explanation\n"), "{stdout}");
    assert!(stdout.contains("Evidence\n"), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(
        stdout.contains("deadreckon learn import-bundle"),
        "{stdout}"
    );
    assert!(!stdout.contains("next:"), "{stdout}");

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
    assert_eq!(json["verdict"]["kind"], "preview");
    assert!(
        json["primary_action"]
            .as_str()
            .expect("primary action")
            .ends_with("--yes")
    );

    let human_preview = deadreckon(temp.path())
        .args([
            "learn",
            "import-bundle",
            bundle.to_str().expect("utf8"),
            "--preview",
        ])
        .output()
        .expect("learn import-bundle human");
    assert_success(&human_preview);
    let stdout = String::from_utf8_lossy(&human_preview.stdout);
    assert!(
        stdout.starts_with("preview learn import-bundle"),
        "{stdout}"
    );
    assert!(stdout.contains("Explanation\n"), "{stdout}");
    assert!(stdout.contains("Evidence\n"), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(stdout.contains(" --yes"), "{stdout}");
    assert!(!stdout.contains("next:"), "{stdout}");
}

#[test]
fn pr_dry_run_writes_body_without_network_or_push() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path());
    write_synthetic_pr_fixture(&paths);

    let output = deadreckon(temp.path())
        .args(["improve", "self", "prop-cli", "--pr-dry-run", "--json"])
        .output()
        .expect("pr dry-run");

    assert_success(&output);
    let json: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(json["verdict"]["kind"], "completed");
    assert_eq!(
        json["primary_action"],
        "deadreckon improve self prop-cli --open-pr"
    );
    assert_eq!(json["decision"]["eligible"], true);
    let body_path = json["body_path"].as_str().expect("body_path");
    assert!(Path::new(body_path).exists());
    assert!(paths.learning_pr_events_path().exists());

    let human = deadreckon(temp.path())
        .args(["improve", "self", "prop-cli", "--pr-dry-run"])
        .output()
        .expect("pr dry-run human");
    assert_success(&human);
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.starts_with("completed improve self"), "{stdout}");
    assert!(stdout.contains("Explanation\n"), "{stdout}");
    assert!(stdout.contains("Evidence\n"), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(
        stdout.contains("deadreckon improve self prop-cli --open-pr"),
        "{stdout}"
    );
    assert!(!stdout.contains("next:"), "{stdout}");
}

#[test]
fn learn_report_json_matches_text_counts() {
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
    let index_output = deadreckon(temp.path())
        .args(["learn", "index", "--all"])
        .output()
        .expect("index");
    assert_success(&index_output);
    let index_text = String::from_utf8_lossy(&index_output.stdout);
    assert!(
        index_text.starts_with("completed learn index"),
        "{index_text}"
    );
    assert!(index_text.contains("Explanation\n"), "{index_text}");
    assert!(index_text.contains("Evidence\n"), "{index_text}");
    assert_eq!(
        index_text.matches("\nRecommended\n").count(),
        1,
        "{index_text}"
    );
    assert!(
        index_text.contains("Recommended\ndeadreckon learn propose"),
        "{index_text}"
    );
    assert!(!index_text.contains("next:"), "{index_text}");

    let json_output = deadreckon(temp.path())
        .args(["learn", "report", "--json"])
        .output()
        .expect("json report");
    let text_output = deadreckon(temp.path())
        .args(["learn", "report"])
        .output()
        .expect("text report");

    assert_success(&json_output);
    assert_success(&text_output);
    let json: Value = serde_json::from_slice(&json_output.stdout).expect("json");
    assert_eq!(json["verdict"]["kind"], "completed");
    assert_eq!(json["primary_action"], "deadreckon learn propose");
    let text = String::from_utf8_lossy(&text_output.stdout);
    assert!(text.starts_with("completed learn report"), "{text}");
    assert!(text.contains("Explanation\n"), "{text}");
    assert!(text.contains("Evidence\n"), "{text}");
    assert_eq!(text.matches("\nRecommended\n").count(), 1, "{text}");
    assert!(
        text.contains("Recommended\ndeadreckon learn propose"),
        "{text}"
    );
    assert!(!text.contains("next:"), "{text}");
    assert_text_count(
        &text,
        "episodes",
        json["episodes"].as_u64().expect("episodes"),
    );
    assert_text_count(&text, "signals", json["signals"].as_u64().expect("signals"));
}

#[test]
fn learn_and_improve_help_use_provider_route_and_done_criteria_vocabulary() {
    let temp = repo_tempdir();
    let learn = deadreckon(temp.path())
        .args(["learn", "--help"])
        .output()
        .expect("learn help");
    let improve = deadreckon(temp.path())
        .args(["improve", "self", "--help"])
        .output()
        .expect("improve help");

    assert_success(&learn);
    assert_success(&improve);
    let learn_text = String::from_utf8_lossy(&learn.stdout);
    let improve_text = String::from_utf8_lossy(&improve.stdout);
    assert!(learn_text.contains("provider-backed reflection"));
    assert!(improve_text.contains("Proposal id or goal file"));
    assert!(improve_text.contains("evidence gate"));
}

#[test]
fn improve_self_missing_candidate_refusal_uses_verdict_surface() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path());
    let proposal = LearningProposal {
        version: 1,
        proposal_id: "prop-refusal".to_string(),
        created_at: Utc::now(),
        title: "Refusal".to_string(),
        insights: Vec::new(),
        stimulus: Vec::new(),
        hypothesis: "test".to_string(),
        target: LearningProposalTarget {
            repo: "/Users/gdc/deadreckon".to_string(),
            scope: "cli".to_string(),
        },
        goal_text: "goal".to_string(),
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

    let output = deadreckon(temp.path())
        .args(["improve", "self", "prop-refusal", "--pr-dry-run"])
        .output()
        .expect("refusal");

    assert_failure(&output);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("blocked improve self"), "{stderr}");
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(
        stderr.contains("Recommended\ndeadreckon improve self prop-refusal --yes"),
        "{stderr}"
    );
    assert!(stderr.contains("proposal: prop-refusal"), "{stderr}");
    assert!(stderr.contains("candidate evidence:"), "{stderr}");
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");
}

#[test]
fn docs_as_built_mentions_learning_files_evidence_gate_and_pr_limits() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let as_built =
        fs::read_to_string(root.join("docs/AS-BUILT-ARCHITECTURE.md")).expect("as-built");

    assert!(as_built.contains("DEADRECKON_HOME/learning/"));
    assert!(as_built.contains("Evidence-Gated PR Opening"));
    assert!(as_built.contains("without network or push"));
}

#[test]
fn v1_candidates_record_out_of_scope_learning_items() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let v1 = fs::read_to_string(root.join("docs/V1-CANDIDATES.md")).expect("v1");

    assert!(v1.contains("Self-improvement beyond local PR gating"));
    assert!(v1.contains("model-training/fine-tuning"));
}

#[test]
fn changelog_has_self_improvement_loop_alpha_entry() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let changelog = fs::read_to_string(root.join("CHANGELOG.md")).expect("changelog");

    assert!(changelog.contains("Self-Improvement Loop (alpha)"));
    assert!(changelog.contains("redacted bundles"));
}

fn run_git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("git");
    assert_success(&output);
}

fn assert_failure(output: &std::process::Output) {
    assert!(
        !output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_text_count(text: &str, label: &str, expected: u64) {
    assert!(
        text.lines().any(|line| {
            let normalized = line.replace(':', " ");
            let parts = normalized.split_whitespace().collect::<Vec<_>>();
            parts.len() == 2 && parts[0] == label && parts[1] == expected.to_string()
        }),
        "missing {label}={expected} in:\n{text}"
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

fn write_fake_learning_reflection_provider(
    paths: &DeadreckonPaths,
    root: &Path,
    signal: &LearningSignal,
) {
    fs::create_dir_all(paths.home()).expect("home");
    let response = root.join("learning-reflection-response.json");
    fs::write(
        &response,
        serde_json::to_vec_pretty(&serde_json::json!({
            "insights": [{
                "stimulus": [{
                    "signal_id": signal.signal_id,
                    "run_id": signal.run_id
                }],
                "summary": "Provider setup failures need one recovery surface.",
                "user_need": "A clear recovery command after learning reflection.",
                "hypothesis": "A verdict surface makes learning proposals easier to act on.",
                "confidence": "high",
                "rejected_claims": []
            }],
            "proposals": [{
                "title": "Normalize learning propose output",
                "insights": [],
                "stimulus": [{
                    "signal_id": signal.signal_id,
                    "run_id": signal.run_id
                }],
                "hypothesis": "Learning propose output should lead with one action.",
                "target": {
                    "repo": "/Users/gdc/deadreckon",
                    "scope": "cli-friendliness"
                },
                "goal_text": "Normalize learning propose output to the verdict surface.",
                "done_criteria": ["focused learning_cli tests pass"],
                "expected_risk": "low",
                "blocked_auto_pr_reasons": []
            }]
        }))
        .expect("response"),
    )
    .expect("write response");

    let binary = root.join("learning-reflection-provider.sh");
    fs::write(
        &binary,
        format!("#!/bin/sh\ncat '{}'\n", response.display()),
    )
    .expect("provider script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut perms = fs::metadata(&binary)
            .expect("script metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&binary, perms).expect("chmod");
    }

    let providers_dir = paths.home().join("providers.d");
    fs::create_dir_all(&providers_dir).expect("providers dir");
    fs::write(
        providers_dir.join("learning-reflection.toml"),
        format!(
            r#"
id = "learning-reflection"
display_name = "Learning Reflection Fixture"
kind = "cli"
default_binary = "{binary}"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["{{prompt}}"]
"#,
            binary = binary.display()
        ),
    )
    .expect("descriptor");
    fs::write(
        paths.config_path(),
        format!(
            r#"
default_provider = "learning-reflection"
fallback = ["learning-reflection"]

[providers.learning-reflection]
binary = "{binary}"
"#,
            binary = binary.display()
        ),
    )
    .expect("config");
}
