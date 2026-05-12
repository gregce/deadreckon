use std::fs;
use std::io::Write;
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use deadreckon_core::{
    ApplyMode, ApplyStrategy, BranchPolicy, Chain, ChainNewOptions, ChainStatus, ChainStepStatus,
    DeadreckonPaths, OnFail, RunEvent, RunEventKind, RunOptions, chain_task_key, create_run,
    load_chain, promote_completed_run,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;

fn sample_chain(temp: &TempDir) -> Chain {
    Chain::new(ChainNewOptions {
        root_goal: "manual: 2 steps".to_string(),
        goals: vec!["first".to_string(), "second".to_string()],
        scope: "test-scope".to_string(),
        base_branch: "main".to_string(),
        base_sha: "abcdef0".to_string(),
        cwd: temp.path().join("repo"),
        provider: Some("mock".to_string()),
        model: Some("model-a".to_string()),
        sandbox: "none".to_string(),
        branch_policy: BranchPolicy::Stack,
        apply_mode: ApplyMode::Auto,
        apply_strategy: ApplyStrategy::Squash,
        apply_allowlist: Vec::new(),
        on_fail: OnFail::Stop,
        circuit_breaker_threshold: 2,
        max_spend_usd: Some(5.0),
        max_wall_seconds: Some(600.0),
        deadreckon_version: "0.1.0".to_string(),
    })
    .expect("chain")
}

#[test]
fn chain_json_serializes_roundtrip() {
    let temp = TempDir::new().expect("tempdir");
    let chain = sample_chain(&temp);
    let json = serde_json::to_string_pretty(&chain).expect("json");
    let decoded = serde_json::from_str::<Chain>(&json).expect("decoded");

    assert_eq!(decoded, chain);
}

#[test]
fn chain_step_status_transitions_pending_running_completed() {
    let temp = TempDir::new().expect("tempdir");
    let mut chain = sample_chain(&temp);
    let step = &mut chain.steps[0];

    step.transition_to(ChainStepStatus::Running)
        .expect("running");
    step.transition_to(ChainStepStatus::Completed)
        .expect("completed");

    assert_eq!(step.status, ChainStepStatus::Completed);
}

#[test]
fn chain_paths_match_locks_pattern() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let chain = sample_chain(&temp);

    assert_eq!(
        paths.chain_json(&chain.chain_id),
        paths.chains_dir().join(&chain.chain_id).join("chain.json")
    );
    assert_eq!(
        paths.chain_events(&chain.chain_id),
        paths
            .chains_dir()
            .join(&chain.chain_id)
            .join("chain-events.jsonl")
    );
    assert_eq!(
        paths.conductor_json(&chain.chain_id),
        paths
            .chains_dir()
            .join(&chain.chain_id)
            .join("conductor.json")
    );
}

#[test]
fn chain_lock_task_key_prefix_chain_double_dash() {
    assert_eq!(chain_task_key("chain-a"), "chain--chain-a");
}

#[test]
fn run_promoted_event_emitted_after_atomic_swap() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: "promoted event".to_string(),
            cwd: std::env::current_dir().expect("cwd"),
            sandbox: "none".to_string(),
            provider: None,
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    fs::write(state.working_dir.join("README.md"), "ok").expect("write");
    deadreckon_core::write_acceptance_marker(
        &state.run_root,
        state.run_id.clone(),
        state.working_dir.clone(),
        1,
    )
    .expect("marker");

    let library_dir = promote_completed_run(&paths, &mut state).expect("promote");
    let events = read_run_events(&state);

    assert!(library_dir.join("manifest.json").exists());
    assert!(events.iter().any(|event| {
        matches!(
            &event.event,
            RunEventKind::RunPromoted { library_dir: emitted } if emitted == &library_dir
        )
    }));
}

#[test]
fn run_promoted_event_includes_library_dir() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: "promoted event library dir".to_string(),
            cwd: std::env::current_dir().expect("cwd"),
            sandbox: "none".to_string(),
            provider: None,
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    fs::write(state.working_dir.join("README.md"), "ok").expect("write");
    deadreckon_core::write_acceptance_marker(
        &state.run_root,
        state.run_id.clone(),
        state.working_dir.clone(),
        1,
    )
    .expect("marker");

    let library_dir = promote_completed_run(&paths, &mut state).expect("promote");
    let raw = fs::read_to_string(state.run_root.join("events.jsonl")).expect("events");

    assert!(raw.contains(r#""kind":"run_promoted""#));
    assert!(raw.contains(&library_dir.display().to_string()));
}

#[test]
fn chain_explicit_writes_chain_json_with_n_steps() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--draft",
            "scaffold app",
            "add tests",
            "polish docs",
        ])
        .output()
        .expect("chain draft");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.steps.len(), 3);
    assert_eq!(chain.status, ChainStatus::Pending);
    assert!(stdout(&output).contains("drafted:"));
}

#[test]
fn chain_from_file_parses_newline_separated_goals() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let goals = temp.path().join("goals.txt");
    fs::write(&goals, "one\n\n# skip me\ntwo\nthree\n").expect("goals");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "--draft", "--from-file"])
        .arg(&goals)
        .output()
        .expect("chain from file");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(
        chain
            .steps
            .iter()
            .map(|step| step.goal.as_str())
            .collect::<Vec<_>>(),
        vec!["one", "two", "three"]
    );
}

#[test]
fn chain_from_stdin_parses_when_stdin_is_pipe() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut child = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "--draft", "--from-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"alpha\nbeta\n")
        .expect("write stdin");

    let output = child.wait_with_output().expect("output");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.steps.len(), 2);
    assert_eq!(chain.steps[0].goal, "alpha");
}

#[test]
fn chain_refuses_one_step_with_try_run_hint() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "--draft", "one step only"])
        .output()
        .expect("chain");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("chain must have >= 2 steps"));
    assert!(stderr.contains("try:"));
    assert!(stderr.contains("deadreckon run"));
}

#[test]
fn chain_refuses_more_than_12_steps() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut command = deadreckon(&paths);
    command.current_dir(&repo).arg("chain").arg("--draft");
    for index in 0..13 {
        command.arg(format!("step {index}"));
    }

    let output = command.output().expect("chain");

    assert!(!output.status.success());
    assert!(stderr(&output).contains("chain capped at 12 steps"));
}

#[test]
fn chain_refuses_non_git_cwd_with_try_hint() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(temp.path())
        .args(["chain", "--draft", "one", "two"])
        .output()
        .expect("chain");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("chains require a git repo"));
    assert!(stderr.contains("try:"));
}

#[test]
fn chain_preview_lists_per_step_provider_mode_branch_base_caps() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--draft",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "3",
            "first",
            "second",
        ])
        .output()
        .expect("chain draft");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("chain preview"));
    assert!(stdout.contains("policy branch=stack apply=auto"));
    assert!(stdout.contains("provider smoke"));
    assert!(stdout.contains("sandbox none"));
    assert!(stdout.contains("max-spend $3.00"));
    assert!(stdout.contains("1. first"));
    assert!(stdout.contains("2. second"));
}

#[test]
fn chain_yes_runs_smoke_steps_and_auto_applies() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "4",
            "tiny hello",
            "add goodbye",
        ])
        .output()
        .expect("chain run");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Completed);
    assert!(
        chain
            .steps
            .iter()
            .all(|step| step.status == ChainStepStatus::Applied)
    );
    let log = git_stdout(&repo, &["log", "--oneline"]);
    assert!(log.contains("tiny hello"));
    assert!(log.contains("add goodbye"));
}

#[test]
fn chain_pre_step_hook_can_skip_step() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_hook(
        &repo,
        "pre-step",
        "#!/bin/sh\npayload=$(cat)\necho \"$payload\" | grep -q '\"step_index\":0' && exit 1\nexit 0\n",
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "4",
            "skip this",
            "run this",
        ])
        .output()
        .expect("chain run");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Completed);
    assert_eq!(chain.steps[0].status, ChainStepStatus::Skipped);
    assert_eq!(chain.steps[1].status, ChainStepStatus::Applied);
    let events = fs::read_to_string(paths.chain_events(&chain.chain_id)).expect("events");
    assert!(events.contains(r#""hook":"pre-step""#));
}

#[test]
fn chain_on_promote_hook_refuse_blocks_apply() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_hook(&repo, "on-promote", "#!/bin/sh\necho no promote\nexit 2\n");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "2",
            "blocked promote",
            "never reached",
        ])
        .output()
        .expect("chain run");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Paused);
    assert!(
        chain
            .paused_reason
            .as_deref()
            .unwrap_or_default()
            .contains("apply_refused")
    );
    assert_ne!(chain.steps[0].status, ChainStepStatus::Applied);
    let events = fs::read_to_string(paths.chain_events(&chain.chain_id)).expect("events");
    assert!(events.contains("refused_by_hook_on_promote"));
}

#[test]
fn branch_policy_stack_chains_branches_off_prior_head() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "4",
            "stack one",
            "stack two",
        ])
        .output()
        .expect("chain run");

    assert_success(&output);
    let chain = newest_chain(&paths);
    let first_sha = chain.steps[0].applied_sha.as_deref().expect("sha");
    let second_run = chain.steps[1].run_id.as_deref().expect("run");
    let second_state = deadreckon_core::load_run(&paths, second_run).expect("state");
    let second_record =
        deadreckon_core::read_codebase_record(&second_state.working_dir).expect("record");
    assert_eq!(second_record.base_sha.as_deref(), Some(first_sha));
}

#[test]
fn branch_policy_base_each_step_off_chain_base() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--branch-policy",
            "base",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "4",
            "base one",
            "base two",
        ])
        .output()
        .expect("chain run");

    assert_success(&output);
    let chain = newest_chain(&paths);
    let second_run = chain.steps[1].run_id.as_deref().expect("run");
    let second_state = deadreckon_core::load_run(&paths, second_run).expect("state");
    let second_record =
        deadreckon_core::read_codebase_record(&second_state.working_dir).expect("record");
    assert_eq!(
        second_record.base_sha.as_deref(),
        Some(chain.base_sha.as_str())
    );
}

#[test]
fn apply_mode_auto_refuses_when_file_outside_allowlist() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--apply-allowlist",
            "src/",
            "--max-spend",
            "2",
            "allow one",
            "allow two",
        ])
        .output()
        .expect("chain run");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Paused);
    assert_eq!(chain.steps[0].status, ChainStepStatus::Completed);
    assert_ne!(chain.steps[0].status, ChainStepStatus::Applied);
    assert!(
        chain
            .paused_reason
            .as_deref()
            .unwrap_or_default()
            .contains("outside_allowlist"),
        "{:?}",
        chain.paused_reason
    );
}

#[test]
fn apply_mode_manual_pauses_chain_after_inner_completion() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--apply-mode",
            "manual",
            "--max-spend",
            "2",
            "manual one",
            "manual two",
        ])
        .output()
        .expect("chain run");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Paused);
    assert_eq!(chain.steps[0].status, ChainStepStatus::Completed);
    assert_eq!(chain.paused_reason.as_deref(), Some("apply_mode_manual"));
}

#[test]
fn branch_policy_merge_writes_merge_commit_between_steps() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--branch-policy",
            "merge",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "4",
            "merge one",
            "merge two",
        ])
        .output()
        .expect("chain run");

    assert_success(&output);
    let chain = newest_chain(&paths);
    let first_sha = chain.steps[0].applied_sha.as_deref().expect("sha");
    let second_run = chain.steps[1].run_id.as_deref().expect("run");
    let second_state = deadreckon_core::load_run(&paths, second_run).expect("state");
    let second_record =
        deadreckon_core::read_codebase_record(&second_state.working_dir).expect("record");
    assert_eq!(second_record.base_sha.as_deref(), Some(first_sha));
    let merges = git_stdout(&repo, &["log", "--merges", "--oneline"]);
    assert!(merges.contains("deadreckon run"), "{merges}");
}

#[test]
fn on_fail_stop_pauses_at_first_red() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_hook(&repo, "post-step", "#!/bin/sh\nexit 2\n");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "2",
            "stop one",
            "stop two",
        ])
        .output()
        .expect("chain run");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Paused);
    assert_eq!(chain.steps[0].status, ChainStepStatus::Failed);
    assert_eq!(chain.paused_reason.as_deref(), Some("step_failed"));
}

#[test]
fn on_fail_skip_advances_past_failed_step() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let marker = temp.path().join("fail-once-marker");
    write_hook(
        &repo,
        "post-step",
        &format!(
            "#!/bin/sh\nif [ ! -f '{}' ]; then touch '{}'; exit 2; fi\nexit 0\n",
            marker.display(),
            marker.display()
        ),
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--on-fail",
            "skip",
            "--max-spend",
            "4",
            "skip one",
            "skip two",
        ])
        .output()
        .expect("chain run");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Completed);
    assert_eq!(chain.steps[0].status, ChainStepStatus::Skipped);
    assert_eq!(chain.steps[1].status, ChainStepStatus::Applied);
}

#[test]
fn circuit_breaker_threshold_configurable_via_flag() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_hook(&repo, "post-step", "#!/bin/sh\nexit 2\n");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--on-fail",
            "skip",
            "--circuit-breaker-threshold",
            "1",
            "--max-spend",
            "4",
            "breaker one",
            "breaker two",
        ])
        .output()
        .expect("chain run");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Paused);
    assert_eq!(chain.circuit_breaker_consecutive_failures, 1);
    assert_eq!(chain.paused_reason.as_deref(), Some("circuit_breaker_open"));
}

#[test]
fn chain_per_step_cap_is_remaining_over_remaining_steps() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "4",
            "budget one",
            "budget two",
        ])
        .output()
        .expect("chain run");

    assert_success(&output);
    let chain = newest_chain(&paths);
    let first_state =
        deadreckon_core::load_run(&paths, chain.steps[0].run_id.as_deref().expect("first run"))
            .expect("state");
    let second_state = deadreckon_core::load_run(
        &paths,
        chain.steps[1].run_id.as_deref().expect("second run"),
    )
    .expect("state");
    assert_eq!(first_state.max_spend_usd, Some(2.0));
    assert!(second_state.max_spend_usd.unwrap_or_default() > 1.99);
}

#[test]
fn single_run_show_renders_chain_banner_when_step_json_present() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "2",
            "banner one",
            "banner two",
        ])
        .output()
        .expect("chain run");
    assert_success(&output);
    let chain = newest_chain(&paths);
    let run_id = chain.steps[0].run_id.as_deref().expect("run");

    let show = deadreckon(&paths)
        .current_dir(&repo)
        .args(["show", run_id])
        .output()
        .expect("show");

    assert_success(&show);
    let stdout = stdout(&show);
    assert!(stdout.contains(&format!("chain {}", &chain.chain_id[..8])));
    assert!(stdout.contains("step 1/2"));
    assert!(stdout.contains("policy: stack | apply=auto"));
}

#[test]
fn chain_latest_alias_resolves_to_most_recent_in_scope() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "--draft", "older one", "older two"])
            .output()
            .expect("older"),
    );
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "--draft", "newer one", "newer two"])
            .output()
            .expect("newer"),
    );
    let latest = newest_chain(&paths);

    let show = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "show", "latest"])
        .output()
        .expect("show");

    assert_success(&show);
    assert!(stdout(&show).contains(&latest.chain_id));
}

#[test]
fn chain_resume_runs_pending_draft() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args([
                "chain",
                "--draft",
                "--provider",
                "smoke",
                "--sandbox",
                "none",
                "--max-spend",
                "4",
                "resume one",
                "resume two",
            ])
            .output()
            .expect("draft"),
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "resume", "latest"])
        .output()
        .expect("resume");

    assert_success(&output);
    assert_eq!(newest_chain(&paths).status, ChainStatus::Completed);
}

#[test]
fn chain_extend_appends_step_and_writes_event() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "--draft", "one", "two"])
            .output()
            .expect("draft"),
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "extend", "latest", "three"])
        .output()
        .expect("extend");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.steps.len(), 3);
    assert_eq!(chain.steps[2].goal, "three");
    let events = fs::read_to_string(paths.chain_events(&chain.chain_id)).expect("events");
    assert!(events.contains("chain_step_extended"));
}

#[test]
fn chain_redo_applied_step_requires_reapply_flag() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args([
                "chain",
                "--yes",
                "--provider",
                "smoke",
                "--sandbox",
                "none",
                "--max-spend",
                "2",
                "redo one",
                "redo two",
            ])
            .output()
            .expect("run"),
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "redo", "latest", "--step", "1"])
        .output()
        .expect("redo");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("redo needs --reapply"));
    assert!(stderr.contains("try:"));
}

#[test]
fn chain_undo_records_undone_step_events() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args([
                "chain",
                "--yes",
                "--provider",
                "smoke",
                "--sandbox",
                "none",
                "--max-spend",
                "2",
                "undo one",
                "undo two",
            ])
            .output()
            .expect("run"),
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "undo", "latest", "--no-confirm"])
        .output()
        .expect("undo");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Undone);
    let events = fs::read_to_string(paths.chain_events(&chain.chain_id)).expect("events");
    assert!(events.contains("chain_undone_step"));
}

#[test]
fn chain_pause_refuses_when_status_not_running_with_try() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "--draft", "one", "two"])
            .output()
            .expect("draft"),
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "pause", "latest"])
        .output()
        .expect("pause");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("cannot pause 'pending' chain"));
    assert!(stderr.contains("try:"));
}

#[test]
fn chain_kill_cascade_terminates_inner_run_and_conductor_under_5s() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let fake_codex = temp.path().join("fake-codex-sleep");
    write_sleeping_cli(&fake_codex);
    write_cli_codex_config(&paths, &fake_codex);

    let launch = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--detach",
            "--provider",
            "slow-codex",
            "--sandbox",
            "none",
            "--max-spend",
            "2",
            "slow one",
            "slow two",
        ])
        .output()
        .expect("launch");
    assert_success(&launch);
    let chain_id = newest_chain(&paths).chain_id;
    let (run_id, conductor_pid, child_pid) = wait_for_live_conductor(&paths, &chain_id);

    let started = std::time::Instant::now();
    let kill = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "kill", "latest"])
        .output()
        .expect("kill");

    assert_success(&kill);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "chain kill took {:?}",
        started.elapsed()
    );
    let chain = load_chain(&paths, &chain_id).expect("chain");
    assert_eq!(chain.status, ChainStatus::Killed);
    let run = deadreckon_core::load_run(&paths, &run_id).expect("run");
    assert_eq!(run.status, deadreckon_core::RunStatus::Killed);
    wait_until_pid_dead(conductor_pid);
    wait_until_pid_dead(child_pid);
    assert!(!deadreckon_core::pid_is_alive(conductor_pid));
    assert!(!deadreckon_core::pid_is_alive(child_pid));
}

#[test]
fn chain_hooks_list_emits_resolution_tiers() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_hook(&repo, "pre-step", "#!/bin/sh\nexit 0\n");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "hooks", "list"])
        .output()
        .expect("hooks list");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("pre-step\tproject"));
    assert!(stdout.contains("post-step\tmissing"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chain_plan_writes_chain_json_with_n_steps() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let server = MockServer::start(r#"["scaffold board","add rules","polish ui"]"#).await;
    write_config(temp.path(), &server.base_url());

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "plan",
            "build chess",
            "--n",
            "3",
            "--draft",
            "--provider",
            "mock",
        ])
        .output()
        .expect("chain plan");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(
        chain
            .steps
            .iter()
            .map(|step| step.goal.as_str())
            .collect::<Vec<_>>(),
        vec!["scaffold board", "add rules", "polish ui"]
    );
    assert!(
        paths
            .chain_dir(&chain.chain_id)
            .join("spend.jsonl")
            .exists()
    );
    assert!(
        server.journal()[0]["messages"][0]["content"]
            .as_str()
            .expect("prompt")
            .contains("ordered serial chain")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chain_plan_refuses_single_step_response() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let server = MockServer::start(r#"["only one"]"#).await;
    write_config(temp.path(), &server.base_url());

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "plan",
            "build chess",
            "--draft",
            "--provider",
            "mock",
        ])
        .output()
        .expect("chain plan");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("decomposition produced 1 goals; need >= 2"));
    assert!(stderr.contains("try:"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chain_plan_refuses_duplicate_steps() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let server = MockServer::start(r#"["add tests","add   tests"]"#).await;
    write_config(temp.path(), &server.base_url());

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "plan",
            "build chess",
            "--draft",
            "--provider",
            "mock",
        ])
        .output()
        .expect("chain plan");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("duplicate steps"));
    assert!(stderr.contains("try:"));
}

fn read_run_events(state: &deadreckon_core::PipelineState) -> Vec<RunEvent> {
    fs::read_to_string(state.run_root.join("events.jsonl"))
        .expect("events")
        .lines()
        .map(|line| serde_json::from_str::<RunEvent>(line).expect("event"))
        .collect()
}

fn repo_tempdir() -> TempDir {
    let root = PathBuf::from("/Users/gdc/deadreckon/.test-tmp");
    fs::create_dir_all(&root).expect("test tmp root");
    TempDir::new_in(root).expect("tempdir")
}

fn clean_git_repo(temp: &TempDir) -> PathBuf {
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    git(&repo, &["init"]).expect("git init");
    fs::write(repo.join("README.md"), "hello").expect("readme");
    git(&repo, &["add", "-A"]).expect("add");
    git(&repo, &["commit", "-m", "initial"]).expect("commit");
    repo
}

fn write_hook(repo: &std::path::Path, name: &str, body: &str) {
    let dir = repo.join(".deadreckon/hooks/chain");
    fs::create_dir_all(&dir).expect("hook dir");
    let path = dir.join(name);
    fs::write(&path, body).expect("hook");
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("permissions");
    }
    git(repo, &["add", ".deadreckon/hooks/chain"]).expect("add hook");
    git(repo, &["commit", "-m", &format!("add {name} hook")]).expect("commit hook");
}

fn write_sleeping_cli(path: &std::path::Path) {
    fs::write(
        path,
        r#"#!/bin/sh
trap 'kill "$child" 2>/dev/null; exit 143' TERM INT
sleep 60 &
child=$!
wait "$child"
"#,
    )
    .expect("fake cli");
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("permissions");
    }
}

fn write_cli_codex_config(paths: &DeadreckonPaths, binary: &std::path::Path) {
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        format!(
            r#"
fallback = ["slow-codex"]

[providers.slow-codex]
kind = "cli-codex"
binary = "{}"
model = "cli:codex"
input_cost_per_million = 0.0
output_cost_per_million = 0.0
"#,
            binary.display()
        ),
    )
    .expect("config");
}

fn wait_for_live_conductor(paths: &DeadreckonPaths, chain_id: &str) -> (String, u32, u32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Ok(raw) = fs::read_to_string(paths.conductor_json(chain_id))
            && let Ok(value) = serde_json::from_str::<Value>(&raw)
        {
            let run_id = value
                .get("live_run_id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            let conductor_pid = value
                .get("conductor_pid")
                .and_then(Value::as_u64)
                .map(|pid| pid as u32);
            let child_pid = value
                .get("live_child_pid")
                .and_then(Value::as_u64)
                .map(|pid| pid as u32);
            if let (Some(run_id), Some(conductor_pid), Some(child_pid)) =
                (run_id, conductor_pid, child_pid)
            {
                return (run_id, conductor_pid, child_pid);
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for live conductor"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn wait_until_pid_dead(pid: u32) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while deadreckon_core::pid_is_alive(pid) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn newest_chain(paths: &DeadreckonPaths) -> Chain {
    let mut chains = fs::read_dir(paths.chains_dir())
        .expect("chains dir")
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path().join("chain.json"))
        .filter(|path| path.exists())
        .map(|path| {
            let chain_id = path
                .parent()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .expect("chain id")
                .to_string();
            load_chain(paths, &chain_id).expect("chain")
        })
        .collect::<Vec<_>>();
    chains.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    chains.into_iter().next().expect("chain")
}

fn deadreckon(paths: &DeadreckonPaths) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command.env("DEADRECKON_HOME", paths.home());
    command
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{}{}",
        stdout(output),
        stderr(output)
    );
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

fn git(cwd: &std::path::Path, args: &[&str]) -> std::io::Result<()> {
    let output = Command::new("git").current_dir(cwd).args(args).output()?;
    if args.first() == Some(&"init") && output.status.success() {
        let _ = Command::new("git")
            .current_dir(cwd)
            .args(["config", "user.email", "deadreckon@example.invalid"])
            .output();
        let _ = Command::new("git")
            .current_dir(cwd)
            .args(["config", "user.name", "deadreckon"])
            .output();
    }
    assert!(
        output.status.success(),
        "git {:?}\nstdout:{}\nstderr:{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

fn git_stdout(cwd: &std::path::Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {:?}\nstdout:{}\nstderr:{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn write_config(temp: &std::path::Path, base_url: &str) {
    let home = temp.join("home");
    fs::create_dir_all(&home).expect("home");
    fs::write(
        home.join("config.toml"),
        format!(
            r#"
fallback = ["mock"]

[providers.mock]
kind = "open-ai-compatible"
base_url = "{base_url}"
model = "mock-agent"
api_key = "test"
input_cost_per_million = 1.0
output_cost_per_million = 1.0
"#
        ),
    )
    .expect("config");
}

#[derive(Clone)]
struct MockState {
    content: Arc<Mutex<Option<String>>>,
    journal: Arc<Mutex<Vec<Value>>>,
}

struct MockServer {
    addr: SocketAddr,
    state: MockState,
}

impl MockServer {
    async fn start(content: &str) -> Self {
        let state = MockState {
            content: Arc::new(Mutex::new(Some(content.to_string()))),
            journal: Arc::new(Mutex::new(Vec::new())),
        };
        let app = Router::new()
            .route("/chat/completions", post(mock_chat_completions))
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

async fn mock_chat_completions(
    State(state): State<MockState>,
    Json(request): Json<Value>,
) -> impl IntoResponse {
    state.journal.lock().expect("journal").push(request);
    let content = state.content.lock().expect("content").take();
    let Some(content) = content else {
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
                "message": {"role": "assistant", "content": content},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 25,
                "total_tokens": 125
            }
        })),
    )
}
