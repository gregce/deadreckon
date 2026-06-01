#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::redundant_clone
)]

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
    load_chain, load_run, promote_completed_run, save_chain,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;

mod common;

use common::{assert_success, deadreckon, repo_tempdir, stderr, stdout};

#[test]
fn chain_help_topics_use_one_recommended_footer() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let topics = [
        ("plan", "deadreckon chain run latest"),
        ("run", "deadreckon chain attach latest"),
        ("attach", "deadreckon chain status latest"),
        ("status", "deadreckon chain show latest"),
        ("show", "deadreckon chain resume latest"),
        ("pause", "deadreckon chain resume latest"),
        ("undo", "deadreckon chain show latest"),
        ("extend", "deadreckon chain run latest"),
    ];

    for (topic, command) in topics {
        let output = deadreckon(&paths)
            .args(["chain", "help", topic])
            .output()
            .expect("chain help");

        assert_success(&output);
        let stdout = stdout(&output);
        assert_eq!(stdout.matches("recommended:").count(), 1, "{stdout}");
        assert!(
            stdout.contains(&format!("recommended: {command}")),
            "{stdout}"
        );
        assert!(!stdout.contains("next:"), "{stdout}");
        assert!(!stdout.contains("try:"), "{stdout}");
    }
}

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
fn chain_failed_surface_has_single_inspection_or_recovery_command() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut chain = sample_chain(&temp);
    chain.scope = deadreckon_core::paths::workspace_scope(temp.path()).expect("scope");
    chain.status = ChainStatus::Failed;
    chain.failure_reason = Some("step 2 failed".to_string());
    save_test_chain(&paths, &chain);

    let output = deadreckon(&paths)
        .current_dir(temp.path())
        .args(["chain", "show", &chain.chain_id[..8], "--json"])
        .output()
        .expect("chain show json");

    assert_success(&output);
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["verdict"]["kind"], "failed");
    assert_eq!(
        value["primary_action"],
        value["verdict"]["recommended_command"]
    );
    assert_eq!(value["primary_action"], value["next_actions"][0]);
    assert_eq!(
        value["primary_action"],
        format!(
            "deadreckon chain show {} --why-failed",
            &chain.chain_id[..8]
        )
    );

    let human = deadreckon(&paths)
        .current_dir(temp.path())
        .args(["chain", "show", &chain.chain_id[..8], "--why-failed"])
        .output()
        .expect("chain show human");
    assert_success(&human);
    let stdout = stdout(&human);
    assert!(stdout.starts_with("failed chain "), "{stdout}");
    assert!(stdout.contains("Explanation"), "{stdout}");
    assert!(stdout.contains("Recommended"), "{stdout}");
    assert_eq!(
        stdout
            .matches(&format!(
                "deadreckon chain show {} --why-failed",
                &chain.chain_id[..8]
            ))
            .count(),
        1,
        "{stdout}"
    );
}

#[test]
fn chain_paused_surface_recommends_resume_once() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut chain = sample_chain(&temp);
    chain.scope = deadreckon_core::paths::workspace_scope(temp.path()).expect("scope");
    chain.status = ChainStatus::Paused;
    chain.paused_reason = Some("operator pause".to_string());
    save_test_chain(&paths, &chain);

    let output = deadreckon(&paths)
        .current_dir(temp.path())
        .args(["chain", "show", &chain.chain_id[..8], "--json"])
        .output()
        .expect("chain show json");

    assert_success(&output);
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["verdict"]["kind"], "paused");
    assert_eq!(
        value["primary_action"],
        format!("deadreckon chain resume {}", &chain.chain_id[..8])
    );
    assert_eq!(value["primary_action"], value["next_actions"][0]);

    let human = deadreckon(&paths)
        .current_dir(temp.path())
        .args(["chain", "show", &chain.chain_id[..8]])
        .output()
        .expect("chain show human");
    assert_success(&human);
    let stdout = stdout(&human);
    assert!(stdout.starts_with("paused chain "), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(
        stdout.contains(&format!(
            "Recommended\ndeadreckon chain resume {}",
            &chain.chain_id[..8]
        )),
        "{stdout}"
    );
}

#[test]
fn chain_completed_surface_has_one_verdict_and_explanation() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut chain = sample_chain(&temp);
    chain.scope = deadreckon_core::paths::workspace_scope(temp.path()).expect("scope");
    chain.status = ChainStatus::Completed;
    chain.steps[0].status = ChainStepStatus::Completed;
    chain.steps[1].status = ChainStepStatus::Completed;
    save_test_chain(&paths, &chain);

    let output = deadreckon(&paths)
        .current_dir(temp.path())
        .args(["chain", "show", &chain.chain_id[..8], "--json"])
        .output()
        .expect("chain show json");

    assert_success(&output);
    let value: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(value["verdict"]["kind"], "completed");
    assert_eq!(
        value["primary_action"],
        value["verdict"]["recommended_command"]
    );
    assert!(
        value["verdict"]["explanation"]
            .as_str()
            .expect("explanation")
            .contains("chain reached a terminal completed state")
    );
}

#[test]
fn chain_empty_status_surface_has_one_primary_action() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "status"])
        .output()
        .expect("chain status");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.starts_with("no-op chain status"), "{stdout}");
    assert!(stdout.contains("Explanation\n"), "{stdout}");
    assert!(stdout.contains("Evidence\n"), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(
        stdout.contains("Recommended\ndeadreckon chain \"step one\" \"step two\""),
        "{stdout}"
    );
    assert!(!stdout.contains("try:"), "{stdout}");
}

#[test]
fn chain_empty_list_surface_has_one_primary_action() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "list"])
        .output()
        .expect("chain list");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.starts_with("no-op chain list"), "{stdout}");
    assert!(stdout.contains("Explanation\n"), "{stdout}");
    assert!(stdout.contains("Evidence\n"), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(
        stdout.contains("Recommended\ndeadreckon chain \"step one\" \"step two\""),
        "{stdout}"
    );
    assert!(!stdout.contains("try:"), "{stdout}");
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
fn chain_from_stdin_refuses_when_stdin_is_tty() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let script_probe = Command::new("script").arg("--version").output();
    if script_probe.is_err() {
        eprintln!("skipping TTY stdin test because script(1) is unavailable");
        return;
    }

    let output = Command::new("script")
        .current_dir(&repo)
        .env("DEADRECKON_HOME", paths.home())
        .arg("-q")
        .arg("/dev/null")
        .arg(env!("CARGO_BIN_EXE_deadreckon"))
        .args(["chain", "--draft", "--from-stdin"])
        .output()
        .expect("script");

    let combined = format!("{}{}", stdout(&output), stderr(&output));
    assert!(
        combined.contains("--from-stdin needs a pipe")
            || combined.contains("chain must have >= 2 steps"),
        "{combined}"
    );
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
    assert!(stderr.starts_with("blocked chain"), "{stderr}");
    assert!(stderr.contains("chain must have >= 2 steps"));
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(
        stderr.contains("Recommended\ndeadreckon run \"one step only\""),
        "{stderr}"
    );
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");
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
    let stderr = stderr(&output);
    assert!(stderr.starts_with("blocked chain"), "{stderr}");
    assert!(stderr.contains("chain capped at 12 steps"));
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(
        stderr.contains("Recommended\ndeadreckon chain plan \"<larger goal>\" --n 12"),
        "{stderr}"
    );
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");
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
    assert!(stderr.starts_with("blocked chain"), "{stderr}");
    assert!(stderr.contains("chains require a git repo"));
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(stderr.contains("Recommended\ngit init"), "{stderr}");
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");
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
fn chain_child_runs_copy_project_acceptance_into_run_roots() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    fs::create_dir_all(repo.join(".deadreckon")).expect("acceptance dir");
    fs::write(
        repo.join(".deadreckon/acceptance.yaml"),
        "name: chain acceptance\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
    )
    .expect("acceptance yaml");
    git(&repo, &["add", "-f", ".deadreckon/acceptance.yaml"]).expect("add acceptance");
    git(&repo, &["commit", "-m", "add acceptance"]).expect("commit acceptance");
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
    for step in chain.steps {
        let run_id = step.run_id.expect("step run id");
        let state = load_run(&paths, &run_id).expect("run state");
        let acceptance =
            fs::read_to_string(state.run_root.join("acceptance.yaml")).expect("acceptance");
        assert!(acceptance.contains("chain acceptance"), "{acceptance}");
        let marker =
            fs::read_to_string(state.run_root.join("proofs/turn-acceptance.json")).expect("marker");
        assert!(marker.contains("\"kind\": \"file_exists\""), "{marker}");
    }
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
    let stdout = stdout(&output);
    assert!(stdout.starts_with("preview chain "), "{stdout}");
    assert!(stdout.contains("Explanation\n"), "{stdout}");
    assert!(stdout.contains("Evidence\n"), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(
        stdout.contains(&format!(
            "Recommended\ndeadreckon chain resume {}",
            &chain.chain_id[..8]
        )),
        "{stdout}"
    );
    assert!(!stdout.contains("next:"), "{stdout}");
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
    assert!(stderr.starts_with("blocked chain"), "{stderr}");
    assert!(stderr.contains("redo needs --reapply"));
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(
        stderr.contains("Recommended\ndeadreckon chain redo")
            && stderr.contains("--step 1 --reapply"),
        "{stderr}"
    );
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");
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
    let stdout = stdout(&output);
    assert!(stdout.starts_with("no-op chain "), "{stdout}");
    assert!(stdout.contains("Explanation\n"), "{stdout}");
    assert!(stdout.contains("Evidence\n"), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(
        stdout.contains(&format!(
            "Recommended\ndeadreckon chain show {}",
            &chain.chain_id[..8]
        )),
        "{stdout}"
    );
    assert!(!stdout.contains("next:"), "{stdout}");
    let events = fs::read_to_string(paths.chain_events(&chain.chain_id)).expect("events");
    assert!(events.contains("chain_undone_step"));
}

#[test]
fn chain_pause_refuses_when_status_not_running_with_verdict_surface() {
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
    assert!(stderr.starts_with("blocked chain"), "{stderr}");
    assert!(stderr.contains("cannot pause 'pending' chain"));
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(
        stderr.contains("Recommended\ndeadreckon chain status"),
        "{stderr}"
    );
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");
}

#[test]
fn chain_pause_success_uses_paused_verdict_surface() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut chain = sample_chain(&temp);
    chain.scope = deadreckon_core::paths::workspace_scope(temp.path()).expect("scope");
    chain.status = ChainStatus::Running;
    save_test_chain(&paths, &chain);

    let output = deadreckon(&paths)
        .current_dir(temp.path())
        .args([
            "chain",
            "pause",
            &chain.chain_id[..8],
            "--reason",
            "operator_pause",
        ])
        .output()
        .expect("pause");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.starts_with("paused chain "), "{stdout}");
    assert!(stdout.contains("Explanation\n"), "{stdout}");
    assert!(stdout.contains("Evidence\n"), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(
        stdout.contains(&format!(
            "Recommended\ndeadreckon chain resume {}",
            &chain.chain_id[..8]
        )),
        "{stdout}"
    );
    assert!(!stdout.contains("try:"), "{stdout}");
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
    assert!(stderr.starts_with("blocked chain"), "{stderr}");
    assert!(stderr.contains("decomposition produced 1 goals; need >= 2"));
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(
        stderr.contains("Recommended\ndeadreckon chain plan \"build chess\" --n 3"),
        "{stderr}"
    );
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");
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
    assert!(stderr.starts_with("blocked chain"), "{stderr}");
    assert!(stderr.contains("duplicate steps"));
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(
        stderr.contains("Recommended\ndeadreckon chain plan \"build chess\" --n 3"),
        "{stderr}"
    );
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");
}

#[test]
fn chain_draft_writes_chain_json_and_does_not_start_conductor() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "--draft", "draft one", "draft two"])
        .output()
        .expect("draft");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Pending);
    assert!(!paths.conductor_json(&chain.chain_id).exists());
}

#[test]
fn chain_yes_skips_preview_confirm_and_starts_conductor() {
    chain_yes_runs_smoke_steps_and_auto_applies();
}

#[test]
fn chain_quiet_suppresses_stdout_but_runs() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--quiet",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "2",
            "quiet one",
            "quiet two",
        ])
        .output()
        .expect("quiet chain");

    assert_success(&output);
    assert_eq!(stdout(&output), "");
    assert_eq!(newest_chain(&paths).status, ChainStatus::Completed);
}

#[test]
fn chain_quiet_emits_no_stdout_on_success() {
    chain_quiet_suppresses_stdout_but_runs();
}

#[test]
fn chain_off_tty_without_yes_refuses_with_try_yes() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "needs yes one", "needs yes two"])
        .output()
        .expect("chain");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.starts_with("blocked chain"), "{stderr}");
    assert!(stderr.contains("requires --yes"), "{stderr}");
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(
        stderr.contains("Recommended\ndeadreckon chain --yes \"step one\" \"step two\""),
        "{stderr}"
    );
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");
}

#[test]
fn chain_no_args_dispatches_to_chain_status() {
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
        .arg("chain")
        .output()
        .expect("chain status");

    assert_success(&output);
    assert!(stdout(&output).contains("CHAIN"), "{}", stdout(&output));
    assert!(
        stderr(&output).contains("using: chain status"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn chain_bare_verb_dispatches_to_chain_status() {
    chain_no_args_dispatches_to_chain_status();
}

#[test]
fn chain_bare_verb_prints_using_info_line_on_stderr() {
    chain_no_args_dispatches_to_chain_status();
}

#[test]
fn chain_run_bare_verb_dispatches_to_chain_resume_latest() {
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
                "2",
                "resume bare one",
                "resume bare two",
            ])
            .output()
            .expect("draft"),
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "run"])
        .output()
        .expect("chain run");

    assert_success(&output);
    assert!(
        stderr(&output).contains("using: chain resume"),
        "{}",
        stderr(&output)
    );
    assert_eq!(newest_chain(&paths).status, ChainStatus::Completed);
}

#[test]
fn chain_run_bare_verb_refuses_when_no_chain_in_scope_with_try() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "run"])
        .output()
        .expect("chain run");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("no chains in scope"), "{stderr}");
    assert!(stderr.contains("try:"), "{stderr}");
}

#[test]
fn chain_last_alias_resolves_to_most_recent_in_scope() {
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
        .args(["chain", "show", "last"])
        .output()
        .expect("show");

    assert_success(&show);
    assert!(stdout(&show).contains(&latest.chain_id));
}

#[test]
fn chain_post_action_hints_print_next_verbs() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "--draft", "hint one", "hint two"])
        .output()
        .expect("draft");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("drafted:"), "{stdout}");
    assert!(stdout.contains("edit:"), "{stdout}");
    assert!(stdout.contains("run:"), "{stdout}");
}

#[test]
fn chain_goal_count_errors_use_verdict_surface() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "--draft", "only one"])
        .output()
        .expect("chain");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.starts_with("blocked chain"), "{stderr}");
    assert!(stderr.contains("Explanation\n"), "{stderr}");
    assert!(stderr.contains("Evidence\n"), "{stderr}");
    assert_eq!(stderr.matches("\nRecommended\n").count(), 1, "{stderr}");
    assert!(
        stderr.contains("Recommended\ndeadreckon run \"only one\""),
        "{stderr}"
    );
    assert!(!stderr.contains("try:"), "{stderr}");
    assert!(!stderr.contains("hint:"), "{stderr}");
}

#[test]
fn chain_paused_footer_lists_one_recommended_command() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--apply-mode",
            "manual",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "2",
            "manual footer one",
            "manual footer two",
        ])
        .output()
        .expect("chain");

    assert_success(&output);
    let stdout = stdout(&output);
    assert_eq!(stdout.matches("try:").count(), 0, "{stdout}");
    assert!(stdout.contains("paused chain "), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(
        stdout.contains("Recommended\ndeadreckon chain resume"),
        "{stdout}"
    );
    assert!(stdout.contains("\nSecondary\n"), "{stdout}");
    assert!(stdout.contains("deadreckon chain show"), "{stdout}");
    assert!(
        stdout.contains("deadreckon chain resume")
            && stdout.contains("--apply-mode preview")
            && stdout.contains("deadreckon chain undo"),
        "{stdout}"
    );
}

#[test]
fn chain_plain_emits_periodic_progress_no_ansi() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "--draft", "plain one", "plain two"])
            .output()
            .expect("draft"),
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "attach", "latest", "--plain"])
        .output()
        .expect("attach");

    assert_success(&output);
    let stdout = stdout(&output);
    assert!(stdout.contains("chain "), "{stdout}");
    assert!(!stdout.contains("\u{1b}["), "{stdout}");
}

#[test]
fn chain_detach_starts_background_conductor_and_returns_zero() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let fake_codex = temp.path().join("fake-codex-sleep");
    write_sleeping_cli(&fake_codex);
    write_cli_codex_config(&paths, &fake_codex);

    let output = deadreckon(&paths)
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
            "detach one",
            "detach two",
        ])
        .output()
        .expect("detach");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert!(stdout(&output).contains("detached"), "{}", stdout(&output));
    let _ = wait_for_live_conductor(&paths, &chain.chain_id);
    let _ = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "kill", &chain.chain_id])
        .output();
}

#[test]
fn chain_run_writes_chain_events_jsonl() {
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
                "events one",
                "events two",
            ])
            .output()
            .expect("chain"),
    );

    let chain = newest_chain(&paths);
    let events = fs::read_to_string(paths.chain_events(&chain.chain_id)).expect("events");
    assert!(events.contains("chain_created"), "{events}");
    assert!(events.contains("chain_step_started"), "{events}");
    assert!(events.contains("chain_completed"), "{events}");
}

#[test]
fn chain_run_advances_through_steps_sequentially_no_apply_in_p4() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--apply-mode",
            "manual",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "2",
            "manual no apply one",
            "manual no apply two",
        ])
        .output()
        .expect("manual chain");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Paused);
    assert_eq!(chain.steps[0].status, ChainStepStatus::Completed);
    assert_eq!(chain.steps[1].status, ChainStepStatus::Pending);
}

#[test]
fn chain_run_writes_chain_step_json_into_child_working_dir() {
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
                "marker one",
                "marker two",
            ])
            .output()
            .expect("chain"),
    );

    let chain = newest_chain(&paths);
    let run_id = chain.steps[0].run_id.as_deref().expect("run");
    let state = deadreckon_core::load_run(&paths, run_id).expect("state");
    assert!(
        state
            .working_dir
            .join(".deadreckon/chain-step.json")
            .exists()
    );
}

#[test]
fn chain_run_holds_chain_lock_releases_on_exit() {
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
                "lock one",
                "lock two",
            ])
            .output()
            .expect("chain"),
    );

    let chain = newest_chain(&paths);
    let lock = deadreckon_core::lock::lock_path(&paths, &chain.scope, &chain.task_key());
    assert!(!lock.exists(), "lock was not released: {}", lock.display());
}

#[test]
fn chain_run_refuses_when_lock_held_by_live_pid() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let fake_codex = temp.path().join("fake-codex-sleep");
    write_sleeping_cli(&fake_codex);
    write_cli_codex_config(&paths, &fake_codex);
    assert_success(
        &deadreckon(&paths)
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
                "live lock one",
                "live lock two",
            ])
            .output()
            .expect("detach"),
    );
    let chain = newest_chain(&paths);
    let _ = wait_for_live_conductor(&paths, &chain.chain_id);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "run", &chain.chain_id])
        .output()
        .expect("run again");

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("already running"),
        "{}",
        stderr(&output)
    );
    let _ = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "kill", &chain.chain_id])
        .output();
}

#[test]
fn chain_run_reclaims_lock_from_dead_pid_with_info_line() {
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
                "2",
                "stale one",
                "stale two",
            ])
            .output()
            .expect("draft"),
    );
    let chain = newest_chain(&paths);
    fs::create_dir_all(paths.locks_dir()).expect("locks");
    let lock_path = deadreckon_core::lock::lock_path(&paths, &chain.scope, &chain.task_key());
    fs::write(
        &lock_path,
        serde_json::to_vec(&deadreckon_core::lock::LockState {
            task_key: chain.task_key(),
            run_id: chain.chain_id.clone(),
            scope: chain.scope.clone(),
            phase: "stale".to_string(),
            pid: 999_999,
            acquired_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now() - chrono::Duration::hours(2),
        })
        .expect("lock json"),
    )
    .expect("lock");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "run", &chain.chain_id])
        .output()
        .expect("run");

    assert_success(&output);
    assert_eq!(
        load_chain(&paths, &chain.chain_id).expect("chain").status,
        ChainStatus::Completed
    );
}

#[test]
fn chain_run_idempotent_on_replay_skips_completed() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args([
                "chain",
                "--yes",
                "--apply-mode",
                "manual",
                "--provider",
                "smoke",
                "--sandbox",
                "none",
                "--max-spend",
                "2",
                "replay one",
                "replay two",
            ])
            .output()
            .expect("manual"),
    );
    let mut chain = newest_chain(&paths);
    chain.status = ChainStatus::Pending;
    chain.apply_mode = ApplyMode::Auto;
    chain.steps[0].status = ChainStepStatus::Skipped;
    save_test_chain(&paths, &chain);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "run", &chain.chain_id])
        .output()
        .expect("run");

    assert_success(&output);
    let chain = load_chain(&paths, &chain.chain_id).expect("chain");
    assert_eq!(chain.steps[0].status, ChainStepStatus::Skipped);
    assert_eq!(chain.steps[1].status, ChainStepStatus::Applied);
}

#[test]
fn chain_run_ctrl_c_cascades_terminate_in_under_5s() {
    chain_kill_cascade_terminates_inner_run_and_conductor_under_5s();
}

#[test]
fn apply_mode_auto_lands_step_when_gate_passes_and_clean_rebase() {
    chain_yes_runs_smoke_steps_and_auto_applies();
}

#[test]
fn apply_mode_auto_falls_back_to_preview_on_dirty_target() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_hook(
        &repo,
        "post-step",
        "#!/bin/sh\ntouch dirty-target.txt\nexit 0\n",
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
            "2",
            "dirty one",
            "dirty two",
        ])
        .output()
        .expect("chain");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Paused);
    assert!(
        chain
            .paused_reason
            .as_deref()
            .unwrap_or("")
            .contains("dirty"),
        "{:?}",
        chain.paused_reason
    );
}

#[test]
fn apply_mode_auto_falls_back_to_preview_on_rebase_conflict() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_hook(
        &repo,
        "post-step",
        "#!/bin/sh\nprintf 'conflict from target\\n' > README.md\ngit add README.md\ngit commit -m target-conflict >/dev/null\nexit 0\n",
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
            "2",
            "conflict one",
            "conflict two",
        ])
        .output()
        .expect("chain");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Paused);
    assert!(
        chain
            .paused_reason
            .as_deref()
            .unwrap_or("")
            .contains("apply_refused"),
        "{:?}",
        chain.paused_reason
    );
}

#[test]
fn apply_mode_auto_refuses_when_marker_invalid() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_hook(
        &repo,
        "post-step",
        "#!/bin/sh\nfind \"$DEADRECKON_HOME/runstate\" -path '*/proofs/turn-acceptance.json' -exec sh -c 'printf forged > \"$1\"' sh {} \\;\nexit 0\n",
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
            "2",
            "marker one",
            "marker two",
        ])
        .output()
        .expect("chain");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Paused);
    assert!(
        chain
            .paused_reason
            .as_deref()
            .unwrap_or("")
            .contains("apply_refused"),
        "{:?}",
        chain.paused_reason
    );
}

#[test]
fn apply_mode_preview_writes_diff_summary_before_landing() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "--yes",
            "--apply-mode",
            "preview",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "2",
            "preview one",
            "preview two",
        ])
        .output()
        .expect("chain");

    assert_success(&output);
    assert!(
        stdout(&output).contains("preview diff for step 1"),
        "{}",
        stdout(&output)
    );
    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Paused);
    assert_eq!(chain.steps[0].status, ChainStepStatus::Completed);
}

#[test]
fn branch_policy_refuses_in_place_on_any_step() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "--in-place", "one", "two"])
        .output()
        .expect("chain");

    assert!(!output.status.success());
}

#[test]
fn branch_policy_stack_refuses_apply_mode_skip() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "--draft", "--apply-mode", "skip", "one", "two"])
        .output()
        .expect("chain");

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("unknown apply mode"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn on_fail_continue_does_not_increment_breaker() {
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
            "continue",
            "--max-spend",
            "4",
            "continue one",
            "continue two",
        ])
        .output()
        .expect("chain");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.circuit_breaker_consecutive_failures, 0);
    assert_eq!(chain.steps[0].status, ChainStepStatus::Skipped);
    assert_eq!(chain.steps[1].status, ChainStepStatus::Skipped);
}

#[test]
fn circuit_breaker_pauses_after_n_consecutive_failures() {
    circuit_breaker_threshold_configurable_via_flag();
}

#[test]
fn chain_resume_reset_breaker_clears_counter() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_hook(&repo, "post-step", "#!/bin/sh\nexit 2\n");
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
                "--on-fail",
                "skip",
                "--circuit-breaker-threshold",
                "1",
                "--max-spend",
                "4",
                "reset one",
                "reset two",
            ])
            .output()
            .expect("chain"),
    );
    write_hook(&repo, "post-step", "#!/bin/sh\nexit 0\n");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "resume", "latest", "--reset-breaker"])
        .output()
        .expect("resume");

    assert_success(&output);
    assert_eq!(newest_chain(&paths).circuit_breaker_consecutive_failures, 0);
}

#[test]
fn chain_max_spend_is_aggregate_not_per_step() {
    chain_per_step_cap_is_remaining_over_remaining_steps();
}

#[test]
fn chain_resume_inherits_remaining_budget_no_reset() {
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
                "3",
                "budget inherit one",
                "budget inherit two",
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
    let chain = newest_chain(&paths);
    assert_eq!(chain.max_spend_usd, Some(3.0));
}

#[test]
fn chain_resume_inherits_remaining_budget() {
    chain_resume_inherits_remaining_budget_no_reset();
}

#[test]
fn chain_resume_max_spend_add_recomputes_per_step() {
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
                "1",
                "budget add one",
                "budget add two",
            ])
            .output()
            .expect("draft"),
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "resume", "latest", "--max-spend-add", "2"])
        .output()
        .expect("resume");

    assert_success(&output);
    assert_eq!(newest_chain(&paths).max_spend_usd, Some(3.0));
}

#[test]
fn chain_pause_on_cap_with_try_hint() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args([
                "chain",
                "--yes",
                "--apply-mode",
                "manual",
                "--provider",
                "smoke",
                "--sandbox",
                "none",
                "--max-spend",
                "2",
                "cap one",
                "cap two",
            ])
            .output()
            .expect("chain"),
    );
    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "show", "latest", "--why-failed"])
        .output()
        .expect("show");

    assert_success(&output);
    assert!(newest_chain(&paths).status == ChainStatus::Paused);
}

#[test]
fn chain_pause_then_resume_preserves_step_progress() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args([
                "chain",
                "--yes",
                "--apply-mode",
                "manual",
                "--provider",
                "smoke",
                "--sandbox",
                "none",
                "--max-spend",
                "4",
                "preserve one",
                "preserve two",
            ])
            .output()
            .expect("manual"),
    );
    let first_run = newest_chain(&paths).steps[0].run_id.clone();

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "resume", "latest", "--apply-mode", "auto"])
        .output()
        .expect("resume");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.steps[0].run_id, first_run);
}

#[test]
fn chain_extend_insert_at_inserts_at_index() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "--draft", "one", "three"])
            .output()
            .expect("draft"),
    );

    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "extend", "latest", "two", "--insert-at", "2"])
            .output()
            .expect("extend"),
    );

    let chain = newest_chain(&paths);
    assert_eq!(chain.steps[1].goal, "two");
}

#[test]
fn chain_extend_refuses_completed_chain_without_insert_at() {
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
                "done one",
                "done two",
            ])
            .output()
            .expect("chain"),
    );

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "extend", "latest", "late"])
        .output()
        .expect("extend");

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("cannot extend completed"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn chain_extend_reopens_completed_chain_when_insert_at_supplied() {
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
                "reopen one",
                "reopen two",
            ])
            .output()
            .expect("chain"),
    );

    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "extend", "latest", "inserted", "--insert-at", "2"])
            .output()
            .expect("extend"),
    );

    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Paused);
    assert_eq!(chain.steps[1].goal, "inserted");
}

#[test]
fn chain_redo_default_picks_first_failed_step() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "--draft", "failed", "later"])
            .output()
            .expect("draft"),
    );
    let mut chain = newest_chain(&paths);
    chain.steps[0].status = ChainStepStatus::Failed;
    save_test_chain(&paths, &chain);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "redo", "latest"])
        .output()
        .expect("redo");

    assert_success(&output);
    let chain = newest_chain(&paths);
    assert_eq!(chain.steps[0].status, ChainStepStatus::Pending);
    let stdout = stdout(&output);
    assert!(stdout.starts_with("preview chain "), "{stdout}");
    assert!(stdout.contains("Explanation\n"), "{stdout}");
    assert!(stdout.contains("Evidence\n"), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(
        stdout.contains(&format!(
            "Recommended\ndeadreckon chain resume {}",
            &chain.chain_id[..8]
        )),
        "{stdout}"
    );
    assert!(!stdout.contains("next:"), "{stdout}");
}

#[test]
fn chain_redo_default_falls_back_to_latest_applied() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "--draft", "applied one", "applied two"])
            .output()
            .expect("draft"),
    );
    let mut chain = newest_chain(&paths);
    chain.steps[1].status = ChainStepStatus::Applied;
    chain.steps[1].applied_sha = Some(git_stdout(&repo, &["rev-parse", "HEAD"]));
    save_test_chain(&paths, &chain);

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "redo", "latest"])
        .output()
        .expect("redo");

    assert!(!output.status.success());
    assert!(stderr(&output).contains("--reapply"), "{}", stderr(&output));
}

#[test]
fn chain_redo_extend_persists_new_goal_in_chain_json() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "--draft", "old", "later"])
            .output()
            .expect("draft"),
    );
    let mut chain = newest_chain(&paths);
    chain.steps[0].status = ChainStepStatus::Failed;
    save_test_chain(&paths, &chain);

    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "redo", "latest", "--extend", "new"])
            .output()
            .expect("redo"),
    );

    assert_eq!(newest_chain(&paths).steps[0].goal, "new");
}

#[test]
fn chain_redo_reapply_reverts_applied_sha_before_redoing() {
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
                "reapply one",
                "reapply two",
            ])
            .output()
            .expect("chain"),
    );
    let before = git_stdout(&repo, &["rev-parse", "HEAD"]);

    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "redo", "latest", "--step", "2", "--reapply"])
            .output()
            .expect("redo"),
    );

    let after = git_stdout(&repo, &["rev-parse", "HEAD"]);
    assert_ne!(before, after);
    assert_eq!(
        newest_chain(&paths).steps[1].status,
        ChainStepStatus::Pending
    );
}

#[test]
fn chain_redo_writes_step_redone_event_with_prior_and_new_run_ids() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "--draft", "old", "later"])
            .output()
            .expect("draft"),
    );
    let mut chain = newest_chain(&paths);
    chain.steps[0].status = ChainStepStatus::Failed;
    save_test_chain(&paths, &chain);

    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "redo", "latest", "--extend", "new"])
            .output()
            .expect("redo"),
    );

    let chain = newest_chain(&paths);
    let events = fs::read_to_string(paths.chain_events(&chain.chain_id)).expect("events");
    assert!(events.contains("chain_step_redone"), "{events}");
    assert!(events.contains("prior_goal"), "{events}");
    assert!(events.contains("new_goal"), "{events}");
}

#[test]
fn chain_undo_through_step_n_bounded_and_reverts_in_reverse() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "--draft", "bounded undo one", "bounded undo two"])
            .output()
            .expect("draft"),
    );
    fs::write(repo.join("one.txt"), "one\n").expect("one");
    git(&repo, &["add", "one.txt"]).expect("add one");
    git(&repo, &["commit", "-m", "one"]).expect("commit one");
    let first_sha = git_stdout(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("two.txt"), "two\n").expect("two");
    git(&repo, &["add", "two.txt"]).expect("add two");
    git(&repo, &["commit", "-m", "two"]).expect("commit two");
    let second_sha = git_stdout(&repo, &["rev-parse", "HEAD"]);
    let mut chain = newest_chain(&paths);
    chain.status = ChainStatus::Completed;
    chain.steps[0].status = ChainStepStatus::Applied;
    chain.steps[0].applied_sha = Some(first_sha);
    chain.steps[1].status = ChainStepStatus::Applied;
    chain.steps[1].applied_sha = Some(second_sha);
    save_test_chain(&paths, &chain);

    assert_success(
        &deadreckon(&paths)
            .current_dir(&repo)
            .args(["chain", "undo", "latest", "--step", "1", "--no-confirm"])
            .output()
            .expect("undo"),
    );

    let chain = newest_chain(&paths);
    assert_eq!(chain.steps[0].status, ChainStepStatus::Undone);
    assert_eq!(chain.steps[1].status, ChainStepStatus::Applied);
}

#[test]
fn chain_status_surfaces_chain_context_when_step_json_present() {
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
                "status context one",
                "status context two",
            ])
            .output()
            .expect("chain"),
    );
    let chain = newest_chain(&paths);
    let run_id = chain.steps[1].run_id.as_deref().expect("run");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["status", run_id])
        .output()
        .expect("status");

    assert_success(&output);
    assert!(stdout(&output).contains("chain:"), "{}", stdout(&output));
    assert!(stdout(&output).contains("step 2/2"), "{}", stdout(&output));
}

#[test]
fn chain_post_step_hook_pause_pauses_chain() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_hook(&repo, "post-step", "#!/bin/sh\nexit 1\n");

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
                "post pause one",
                "post pause two",
            ])
            .output()
            .expect("chain"),
    );

    let chain = newest_chain(&paths);
    assert_eq!(chain.status, ChainStatus::Paused);
    assert_eq!(
        chain.paused_reason.as_deref(),
        Some("paused_by_post_step_hook")
    );
}

#[test]
fn chain_on_chain_end_hook_runs_and_records_stdout() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    write_hook(
        &repo,
        "on-chain-end",
        "#!/bin/sh\necho chain ended\nexit 0\n",
    );

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
                "end hook one",
                "end hook two",
            ])
            .output()
            .expect("chain"),
    );

    let chain = newest_chain(&paths);
    let events = fs::read_to_string(paths.chain_events(&chain.chain_id)).expect("events");
    assert!(events.contains("on-chain-end"), "{events}");
    assert!(events.contains("chain ended"), "{events}");
}

#[test]
fn single_run_attach_renders_chain_banner_when_step_json_present() {
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
                "attach banner one",
                "attach banner two",
            ])
            .output()
            .expect("chain"),
    );
    let chain = newest_chain(&paths);
    let run_id = chain.steps[0].run_id.as_deref().expect("run");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["attach", run_id])
        .output()
        .expect("attach");

    assert_success(&output);
    assert!(stdout(&output).contains("chain "), "{}", stdout(&output));
    assert!(stdout(&output).contains("step 1/2"), "{}", stdout(&output));
}

#[test]
fn single_run_attach_plain_includes_chain_banner_line() {
    single_run_attach_renders_chain_banner_when_step_json_present();
}

#[test]
fn single_run_attach_no_chain_banner_when_step_json_absent() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "run",
            "plain run",
            "--provider",
            "smoke",
            "--sandbox",
            "none",
            "--max-spend",
            "1",
            "--yes",
            "--no-confirm",
            "--no-hints",
        ])
        .output()
        .expect("run");
    assert_success(&output);
    let output_stdout = stdout(&output);
    let run_id = output_stdout
        .lines()
        .find_map(|line| {
            line.strip_prefix("completed run ")
                .map(str::to_string)
                .or_else(|| {
                    line.strip_prefix("started run ")
                        .and_then(|rest| rest.rsplit_once('('))
                        .map(|(_, id)| id.trim_end_matches(')').to_string())
                })
        })
        .expect("run id");

    let attach = deadreckon(&paths)
        .current_dir(&repo)
        .args(["attach", &run_id])
        .output()
        .expect("attach");

    assert_success(&attach);
    assert!(!stdout(&attach).contains("step 1/"), "{}", stdout(&attach));
}

#[test]
fn single_run_attach_c_key_opens_chain_attach() {
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
                "key one",
                "key two",
            ])
            .output()
            .expect("chain"),
    );
    let chain = newest_chain(&paths);
    let run_id = chain.steps[0].run_id.as_deref().expect("run");

    let attach = deadreckon(&paths)
        .current_dir(&repo)
        .args(["attach", run_id])
        .output()
        .expect("attach");

    assert_success(&attach);
    assert!(stdout(&attach).contains("[c] Chain"), "{}", stdout(&attach));
}

#[test]
fn single_run_attach_completion_footer_gains_chain_entry() {
    single_run_attach_c_key_opens_chain_attach();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chain_expand_is_alias_for_chain_plan() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let server = MockServer::start(r#"["one","two"]"#).await;
    write_config(temp.path(), &server.base_url());

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "expand",
            "build thing",
            "--n",
            "2",
            "--draft",
            "--provider",
            "mock",
        ])
        .output()
        .expect("expand");

    assert_success(&output);
    assert_eq!(newest_chain(&paths).steps.len(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chain_plan_clamps_n_to_2_through_12() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let server = MockServer::start(r#"["one","two","three"]"#).await;
    write_config(temp.path(), &server.base_url());

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "plan",
            "build thing",
            "--n",
            "1",
            "--draft",
            "--provider",
            "mock",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    let journal = server.journal();
    let prompt = journal[0]["messages"][0]["content"]
        .as_str()
        .expect("prompt");
    assert!(prompt.contains("<= 2 strings"), "{prompt}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chain_plan_decomposition_spend_recorded_separately() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let server = MockServer::start(r#"["one","two"]"#).await;
    write_config(temp.path(), &server.base_url());

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args([
            "chain",
            "plan",
            "build thing",
            "--n",
            "2",
            "--draft",
            "--provider",
            "mock",
        ])
        .output()
        .expect("plan");

    assert_success(&output);
    let chain = newest_chain(&paths);
    let spend =
        fs::read_to_string(paths.chain_dir(&chain.chain_id).join("spend.jsonl")).expect("spend");
    assert!(spend.contains("chain.planner"), "{spend}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chain_plan_falls_back_with_try_explicit_hint_on_provider_error() {
    let temp = repo_tempdir();
    let repo = clean_git_repo(&temp);
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        r#"
fallback = ["mock"]

[providers.mock]
kind = "open-ai-compatible"
base_url = "http://127.0.0.1:9"
model = "mock-agent"
api_key = "test"
"#,
    )
    .expect("config");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .args(["chain", "plan", "build thing", "--provider", "mock"])
        .output()
        .expect("plan");

    assert!(!output.status.success());
    let stderr = stderr(&output);
    assert!(stderr.contains("chain planner provider failed"), "{stderr}");
    assert!(stderr.contains("try:"), "{stderr}");
}

#[test]
fn chain_plan_default_starts_conductor_unless_draft() {
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
            "default starts one",
            "default starts two",
        ])
        .output()
        .expect("chain");

    assert_success(&output);
    assert_ne!(newest_chain(&paths).status, ChainStatus::Pending);
}

fn read_run_events(state: &deadreckon_core::PipelineState) -> Vec<RunEvent> {
    fs::read_to_string(state.run_root.join("events.jsonl"))
        .expect("events")
        .lines()
        .map(|line| serde_json::from_str::<RunEvent>(line).expect("event"))
        .collect()
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
    git(repo, &["add", "-f", ".deadreckon/hooks/chain"]).expect("add hook");
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    loop {
        let sidecar = match fs::read_to_string(paths.conductor_json(chain_id)) {
            Ok(raw) => {
                if let Ok(value) = serde_json::from_str::<Value>(&raw) {
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
                raw
            }
            Err(err) => format!("read error: {err}"),
        };
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for live conductor; last sidecar: {sidecar}"
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

fn save_test_chain(paths: &DeadreckonPaths, chain: &Chain) {
    fs::create_dir_all(paths.chains_dir().join(&chain.chain_id)).expect("chain dir");
    save_chain(paths, chain).expect("save chain");
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
