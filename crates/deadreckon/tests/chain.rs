use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use deadreckon_core::{
    ApplyMode, ApplyStrategy, BranchPolicy, Chain, ChainNewOptions, ChainStatus, ChainStepStatus,
    DeadreckonPaths, OnFail, RunEvent, RunEventKind, RunOptions, chain_task_key, create_run,
    load_chain, promote_completed_run,
};
use tempfile::TempDir;

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
