#![allow(clippy::expect_used)]

use std::fs;
use std::process::Command;

use chrono::Utc;
use deadreckon_core::paths::workspace_scope;
use deadreckon_core::{
    ApplyMode, ApplyStrategy, BranchPolicy, Chain, ChainNewOptions, ChainStatus, ChainStepStatus,
    DeadreckonPaths, OnFail, Plan, PlanMode, PlanProviders, PlanRole, PlanStatus, PlanTask,
    PlanTaskStatus, RunOptions, RunStatus, TraceRecord, append_trace, create_run, run_status_label,
    save_chain, save_plan, save_state,
};
use serde_json::{Value, json};
use tempfile::TempDir;

#[test]
fn runstatus_executing_renders_running_through_glossary() {
    assert_eq!(run_status_label(RunStatus::Executing), "running");
    assert_eq!(RunStatus::Executing.to_string(), "running");
}

#[test]
fn raw_ansi_escapes_stay_in_ui_module() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(manifest.join("src/main.rs")).expect("main");
    assert!(!main.contains("\\x1b["), "main.rs contains raw ANSI");
    let ui = fs::read_to_string(manifest.join("src/ui.rs")).expect("ui");
    let ui_card = fs::read_to_string(manifest.join("src/ui_card.rs")).expect("ui_card");
    assert!(
        ui.contains("\\x1b[") || ui_card.contains("\\u{1b}["),
        "ui modules own ANSI rendering"
    );
}

#[test]
fn help_uses_new_flag_names_with_alpha_aliases_hidden() {
    let run = help(["run", "--help"]);
    assert!(run.contains("--branch-name"), "{run}");
    assert!(!run.contains("--branch <"), "{run}");

    let kill = help(["kill", "--help"]);
    assert!(kill.contains("--escalate"), "{kill}");

    let doc = help(["doc", "--help"]);
    assert!(doc.contains("--max-spend"), "{doc}");
    assert!(!doc.contains("--budget-cap"), "{doc}");

    let apply = help(["apply", "--help"]);
    assert!(apply.contains("--git-strategy"), "{apply}");
}

#[test]
fn attach_plain_displays_running_for_executing_run() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("repo");
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: "running status".to_string(),
            cwd,
            sandbox: "none".to_string(),
            provider: None,
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");
    state.status = RunStatus::Executing;
    save_state(&state).expect("save");

    let output = deadreckon(&paths)
        .args(["attach", &state.run_id, "--plain"])
        .output()
        .expect("attach");

    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("status        running"), "{stdout}");
    assert!(!stdout.contains("executing"), "{stdout}");
}

#[test]
fn attach_plan_plain_displays_running_for_inflight_child() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let mut child = PlanTask::new(
        0,
        "Build child",
        "Build the child slice",
        PlanRole::Child,
        Some("smoke:child".to_string()),
    );
    child.status = PlanTaskStatus::Running;
    let waiting = PlanTask::new(
        1,
        "Review child",
        "Review the child slice",
        PlanRole::Child,
        Some("smoke:child".to_string()),
    );
    let mut plan = Plan::new(
        "orchestrate running child",
        PlanMode::FullPlan,
        vec![child, waiting],
        PlanProviders {
            planner: Some("smoke:planner".to_string()),
            default_child: Some("smoke:child".to_string()),
            coder: None,
            reviewer: None,
            children: Default::default(),
        },
        Some("scope".to_string()),
        "0.1.0",
    )
    .expect("plan");
    plan.status = PlanStatus::Forked;
    save_plan(&paths, &plan).expect("save plan");

    let output = deadreckon(&paths)
        .args(["attach", &plan.plan_id, "--plain"])
        .output()
        .expect("attach plan");

    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains("status      : running"), "{stdout}");
    assert!(stdout.contains("task-0"), "{stdout}");
}

#[test]
fn why_failed_run_and_chain_share_failure_layout() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("repo");

    let mut state = create_run(
        &paths,
        RunOptions {
            goal: "failed run".to_string(),
            cwd: cwd.clone(),
            sandbox: "none".to_string(),
            provider: None,
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: Some("aaaabbbbccccdddd1111222233334444".to_string()),
            codebase: None,
        },
    )
    .expect("run");
    state.status = RunStatus::Failed;
    state.failure_reason = Some("acceptance failed".to_string());
    save_state(&state).expect("save run");
    append_trace(
        &state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: 1,
            event: "acceptance.failed".to_string(),
            latency_ms: Some(1),
            detail: json!({ "stderr": "failed" }),
        },
    )
    .expect("trace");

    let mut chain = Chain::new(ChainNewOptions {
        root_goal: "failed chain".to_string(),
        goals: vec!["first step".to_string(), "second step".to_string()],
        scope: workspace_scope(&cwd).expect("scope"),
        base_branch: "main".to_string(),
        base_sha: "0123456789abcdef".to_string(),
        cwd: cwd.clone(),
        provider: None,
        model: None,
        sandbox: "none".to_string(),
        branch_policy: BranchPolicy::Stack,
        apply_mode: ApplyMode::Manual,
        apply_strategy: ApplyStrategy::Squash,
        apply_allowlist: Vec::new(),
        on_fail: OnFail::Stop,
        circuit_breaker_threshold: 1,
        max_spend_usd: Some(1.0),
        max_wall_seconds: None,
        deadreckon_version: "0.1.0".to_string(),
    })
    .expect("chain");
    chain.status = ChainStatus::Failed;
    chain.failure_reason = Some("step failed".to_string());
    chain.steps[0].status = ChainStepStatus::Failed;
    chain.steps[0].run_id = Some(state.run_id.clone());
    chain.steps[0].fail_reason = Some("acceptance failed".to_string());
    save_chain(&paths, &chain).expect("save chain");

    let run_output = deadreckon(&paths)
        .current_dir(&cwd)
        .args(["show", &state.run_id[..8], "--why-failed"])
        .output()
        .expect("run why failed");
    assert!(run_output.status.success(), "{}", stderr(&run_output));
    let run_stdout = stdout(&run_output);

    let chain_output = deadreckon(&paths)
        .current_dir(&cwd)
        .args([
            "chain",
            "show",
            &chain.chain_id[..8],
            "--all-scopes",
            "--why-failed",
        ])
        .output()
        .expect("chain why failed");
    assert!(chain_output.status.success(), "{}", stderr(&chain_output));
    let chain_stdout = stdout(&chain_output);

    for label in ["failure summary", "status:", "reason:", "evidence:"] {
        assert!(
            run_stdout.contains(label),
            "missing {label} in {run_stdout}"
        );
        assert!(
            chain_stdout.contains(label),
            "missing {label} in {chain_stdout}"
        );
    }
    assert!(chain_stdout.contains("try: deadreckon show aaaabbbb --why-failed"));
}

#[test]
fn json_inspection_surfaces_emit_named_payloads() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("repo");
    let state = create_run(
        &paths,
        RunOptions {
            goal: "json run".to_string(),
            cwd,
            sandbox: "none".to_string(),
            provider: None,
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: Some("bbbbccccddddaaaa1111222233334444".to_string()),
            codebase: None,
        },
    )
    .expect("run");

    for (args, key) in [
        (vec!["list", "--json"], "runs"),
        (vec!["status", &state.run_id[..8], "--json"], "run"),
        (vec!["show", &state.run_id[..8], "--json"], "run"),
        (vec!["doctor", "--json"], "sandboxes"),
        (vec!["providers", "list", "--json"], "providers"),
        (vec!["chain", "--json", "list"], "chains"),
    ] {
        let output = deadreckon(&paths)
            .args(args)
            .output()
            .expect("json command");
        assert!(output.status.success(), "{}", stderr(&output));
        let value: Value = serde_json::from_str(&stdout(&output)).expect("valid json");
        assert!(value.get(key).is_some(), "missing {key}: {value}");
        assert!(
            value.get("try_lines").is_some(),
            "missing try_lines: {value}"
        );
    }
}

fn help<const N: usize>(args: [&str; N]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .args(args)
        .output()
        .expect("help");
    assert!(output.status.success(), "{}", stderr(&output));
    stdout(&output)
}

fn deadreckon(paths: &DeadreckonPaths) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command.env("DEADRECKON_HOME", paths.home());
    command
}

fn repo_tempdir() -> TempDir {
    let root = std::path::Path::new("/Users/gdc/deadreckon/.test-tmp");
    fs::create_dir_all(root).expect("test tmp root");
    TempDir::new_in(root).expect("tempdir")
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
