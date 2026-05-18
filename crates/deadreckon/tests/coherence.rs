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
    for entry in fs::read_dir(manifest.join("src")).expect("src dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("source");
        if path.file_name().and_then(|name| name.to_str()) == Some("ui.rs") {
            assert!(text.contains("\\x1b["), "ui.rs owns raw ANSI rendering");
            continue;
        }
        assert!(
            !text.contains("\\x1b[") && !text.contains("\\u{1b}["),
            "{} contains raw ANSI escape construction",
            path.display()
        );
    }
}

#[test]
fn error_lines_use_shared_stderr_helper() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(manifest.join("src/main.rs")).expect("main");
    assert!(!main.contains("eprintln!(\"error:"), "raw error printer");
    assert!(!main.contains("eprintln!(\"  hint:"), "raw hint printer");
    assert!(
        !main.contains("println!(\"hint:"),
        "raw stdout hint printer"
    );
    assert!(main.contains("fn print_error("), "shared error helper");
}

#[test]
fn help_uses_new_flag_names_with_alpha_aliases_hidden() {
    let run = help(["run", "--help"]);
    assert!(run.contains("--branch-name"), "{run}");
    assert!(!run.contains("--branch <"), "{run}");

    let finish = help(["finish", "--help"]);
    assert!(finish.contains("--into"), "{finish}");
    assert!(
        finish.contains("Target branch for apply; defaults to the current branch"),
        "{finish}"
    );
    assert!(!finish.contains("--branch <"), "{finish}");

    let kill = help(["kill", "--help"]);
    assert!(kill.contains("--escalate"), "{kill}");
    assert!(kill.contains("deadreckon kill <chain-id>"), "{kill}");
    assert!(kill.contains("deadreckon kill <plan-id>"), "{kill}");
    assert!(
        kill.contains("Kill cancels a run, chain, or plan,"),
        "{kill}"
    );

    let doc = help(["doc", "--help"]);
    assert!(doc.contains("--max-spend"), "{doc}");
    assert!(!doc.contains("--budget-cap"), "{doc}");

    let apply = help(["apply", "--help"]);
    assert!(apply.contains("--git-strategy"), "{apply}");
    assert!(apply.contains("--into"), "{apply}");
    assert!(
        apply.contains("Target branch for apply; defaults to the current branch"),
        "{apply}"
    );
    assert!(!apply.contains("--branch <"), "{apply}");
}

#[test]
fn force_is_hidden_behind_intent_specific_flags() {
    for (args, visible) in [
        (&["kill", "--help"][..], "--escalate"),
        (&["chain", "--help"][..], "--escalate"),
        (&["cleanup", "--help"][..], "--escalate"),
        (&["finish", "--help"][..], "--overwrite"),
        (&["apply", "--help"][..], "--git-strategy"),
        (&["export", "--help"][..], "--overwrite"),
        (&["abandon", "--help"][..], "--anyway"),
        (&["doc", "--help"][..], "--overwrite"),
        (&["def-done", "--help"][..], "--overwrite"),
        (&["acceptance", "setup", "--help"][..], "--overwrite"),
        (&["acceptance", "add", "--help"][..], "--overwrite"),
        (&["acceptance", "init", "--help"][..], "--overwrite"),
        (&["acceptance", "draft", "--help"][..], "--overwrite"),
        (&["acceptance", "refine", "--help"][..], "--overwrite"),
        (&["update", "--help"][..], "--anyway"),
    ] {
        let out = help_slice(args);
        assert!(out.contains(visible), "{args:?}\n{out}");
        assert!(
            !out.contains("--force"),
            "{args:?} should keep --force as a hidden alpha alias:\n{out}"
        );
    }
}

#[test]
fn all_scope_flag_help_uses_scope_vocabulary() {
    for args in [
        &["chain", "--help"][..],
        &["list", "--help"][..],
        &["cleanup", "--help"][..],
        &["history", "grep", "--help"][..],
        &["library", "list", "--help"][..],
        &["library", "search", "--help"][..],
    ] {
        let out = help_slice(args);
        assert!(
            out.contains("all project scopes"),
            "{args:?} should describe cross-project scope:\n{out}"
        );
        assert!(
            !out.contains("all projects") && !out.contains("all scopes"),
            "{args:?} should not use stale cross-scope wording:\n{out}"
        );
    }

    let providers = help(["providers", "list", "--help"]);
    assert!(providers.contains("--all"), "{providers}");
    assert!(
        providers.contains("built-in") || providers.contains("override"),
        "provider --all is provider inventory, not project scope:\n{providers}"
    );
}

#[test]
fn top_help_uses_canonical_discovery_words() {
    let top = help(["--help"]);
    for command in ["detect", "providers", "update", "history"] {
        assert!(
            top.contains(command),
            "top help should include {command} in More help:\n{top}"
        );
    }
    assert!(
        top.contains("watch a run, chain, or plan in the TUI"),
        "{top}"
    );
    assert!(top.contains("cancel a run, chain, or plan"), "{top}");
    assert!(top.contains("show runs and plans"), "{top}");
    assert!(
        top.contains("show every command, including advanced commands (alias: commands)"),
        "{top}"
    );
    assert!(
        !top.contains("commands    alias for help-all"),
        "aliases should be inline, not separate commands:\n{top}"
    );
    assert!(
        !top.contains("orchestration jobs"),
        "plan-facing help should not say jobs:\n{top}"
    );
    assert!(
        top.contains(
            "Run, chain, and plan ids accept unique prefixes where that command accepts the kind."
        ),
        "{top}"
    );
}

#[test]
fn help_all_keeps_aliases_inline() {
    let all = help(["help-all"]);
    assert!(all.contains("show runs and plans"), "{all}");
    assert!(
        all.contains("copy a completed fresh/copy run (alias: materialize)"),
        "{all}"
    );
    assert!(
        !all.contains("materialize alias for export"),
        "materialize should not get a separate row:\n{all}"
    );
    assert!(
        !all.contains("orchestration jobs"),
        "plan-facing help should not say jobs:\n{all}"
    );
}

#[test]
fn command_help_prefers_status_finish_export_and_cleanup() {
    let run = help(["run", "--help"]);
    assert!(run.contains("deadreckon status latest"), "{run}");
    assert!(
        !run.contains("deadreckon next"),
        "run help should reserve next for alias notes:\n{run}"
    );

    let attach = help(["attach", "--help"]);
    assert!(attach.contains("deadreckon status latest"), "{attach}");
    assert!(attach.contains("deadreckon attach <chain-id>"), "{attach}");
    assert!(attach.contains("deadreckon attach <plan-id>"), "{attach}");
    assert!(
        attach.contains("deadreckon attach <plan-id>:task-0"),
        "{attach}"
    );
    assert!(
        attach.contains("Attach opens the live TUI for a run, chain, or plan."),
        "{attach}"
    );
    assert!(
        !attach.contains("deadreckon next"),
        "attach help should reserve next for alias notes:\n{attach}"
    );

    let status = help(["status", "--help"]);
    assert!(status.contains("deadreckon status"), "{status}");
    assert!(status.contains("`next` is an alias."), "{status}");
    assert!(
        !status.contains("deadreckon next"),
        "status help should lead with canonical status command:\n{status}"
    );

    let show = help(["show", "--help"]);
    assert!(show.contains("deadreckon show <plan-id>"), "{show}");
    assert!(show.contains("deadreckon show <plan-id>:task-0"), "{show}");
    assert!(
        show.contains("Show prints run, plan, or plan-child state"),
        "{show}"
    );
    assert!(show.contains("Run id, plan id, plan-id:task-id"), "{show}");

    let detect = help(["detect", "--help"]);
    assert!(detect.contains("registered provider routes"), "{detect}");
    assert!(!detect.contains("descriptor data"), "{detect}");

    let providers = help(["providers", "list", "--help"]);
    assert!(
        providers.contains("List registered provider routes"),
        "{providers}"
    );
    assert!(!providers.contains("descriptor registry"), "{providers}");

    let merge = help(["merge", "--help"]);
    assert!(merge.contains("deadreckon finish <plan-id>"), "{merge}");
    assert!(merge.contains("deadreckon apply <plan-id>"), "{merge}");
    assert!(
        merge.contains("deadreckon export <plan-id> --dest ./result"),
        "{merge}"
    );
    assert!(!merge.contains("merged-run-id"), "{merge}");
    assert!(!merge.contains("deadreckon materialize"), "{merge}");

    let apply = help(["apply", "--help"]);
    assert!(apply.contains("deadreckon cleanup latest"), "{apply}");
    assert!(
        !apply.contains("deadreckon discard latest"),
        "apply help should prefer cleanup:\n{apply}"
    );
}

#[test]
fn plain_flag_help_uses_one_definition() {
    const PLAIN_HELP: &str = "Plain output without TUI, spinner, or ANSI affordances";
    const STALE_PLAIN_HELP: &[&str] = &[
        "Plain output without TUI or ANSI affordances",
        "Plain output without ANSI affordances",
        "Plain output without opening the TUI",
    ];

    for args in [
        ["run", "--help"],
        ["orchestrate", "--help"],
        ["plan", "--help"],
        ["fork", "--help"],
        ["merge", "--help"],
        ["chain", "--help"],
        ["list", "--help"],
        ["update", "--help"],
        ["apply", "--help"],
        ["attach", "--help"],
        ["kill", "--help"],
        ["resume", "--help"],
        ["show", "--help"],
        ["status", "--help"],
    ] {
        let out = help(args);
        assert!(out.contains(PLAIN_HELP), "{args:?}\n{out}");
        for stale in STALE_PLAIN_HELP {
            assert!(!out.contains(stale), "{args:?}\n{out}");
        }
    }

    for args in [
        ["orchestrate", "review", "--help"],
        ["orchestrate", "full-plan", "--help"],
    ] {
        let out = help(args);
        assert!(out.contains(PLAIN_HELP), "{args:?}\n{out}");
        for stale in STALE_PLAIN_HELP {
            assert!(!out.contains(stale), "{args:?}\n{out}");
        }
    }
}

#[test]
fn current_docs_do_not_teach_stale_primary_aliases() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest
        .parent()
        .and_then(|path| path.parent())
        .expect("repo");
    for relative in ["README.md", "HOWTO.md", "docs/DEVELOPMENT-README.md"] {
        let text = fs::read_to_string(repo.join(relative)).expect(relative);
        for stale in [
            "deadreckon materialize",
            "deadreckon next",
            "deadreckon discard",
            "m materialize",
            "materialize/export",
        ] {
            assert!(
                !text.contains(stale),
                "{relative} should not teach stale primary alias `{stale}`"
            );
        }
    }
}

#[test]
fn orchestration_help_uses_plan_child_provider_language() {
    for (args, label) in [
        (["orchestrate", "--help"], "orchestrate"),
        (["plan", "--help"], "plan"),
        (["fork", "--help"], "fork"),
        (["merge", "--help"], "merge"),
    ] {
        let out = help(args);
        assert!(
            !out.contains("job"),
            "{label} help should not say job:\n{out}"
        );
        assert!(
            !out.contains("descriptor"),
            "{label} help should reserve descriptor for technical docs:\n{out}"
        );
        assert!(
            out.contains("plan"),
            "{label} help should name plans:\n{out}"
        );
        assert!(
            out.contains("child"),
            "{label} help should name children:\n{out}"
        );
        if label != "merge" {
            assert!(
                out.contains("provider"),
                "{label} help should surface provider selection:\n{out}"
            );
        }
    }
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
    assert!(stdout.contains("status: running"), "{stdout}");
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
    child.child_run_id = Some("aaaabbbbccccdddd1111222233334444".to_string());
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
    assert!(
        stdout.contains(&format!(
            "drill: deadreckon attach {}:task-0",
            &plan.plan_id[..8]
        )),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!(
            "show: deadreckon show {}:task-0",
            &plan.plan_id[..8]
        )),
        "{stdout}"
    );
    assert!(stdout.contains("run id aaaabbbb"), "{stdout}");
}

#[test]
fn top_level_attach_plain_accepts_chain_id() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("repo");
    let mut chain = Chain::new(ChainNewOptions {
        root_goal: "top-level chain attach".to_string(),
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
    chain.status = ChainStatus::Running;
    chain.steps[0].status = ChainStepStatus::Running;
    save_chain(&paths, &chain).expect("save chain");

    let output = deadreckon(&paths)
        .current_dir(&cwd)
        .args(["attach", &chain.chain_id[..8], "--plain"])
        .output()
        .expect("attach chain");

    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(stdout.contains(&chain.chain_id[..8]), "{stdout}");
    assert!(stdout.contains("step 1"), "{stdout}");
    assert!(stdout.contains("[r] redo"), "{stdout}");
}

#[test]
fn chain_show_and_attach_plain_share_header_precision() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("repo");
    let mut chain = Chain::new(ChainNewOptions {
        root_goal: "chain header precision".to_string(),
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
        max_spend_usd: Some(5.0),
        max_wall_seconds: None,
        deadreckon_version: "0.1.0".to_string(),
    })
    .expect("chain");
    chain.status = ChainStatus::Running;
    chain.total_spend_usd = 1.25;
    save_chain(&paths, &chain).expect("save chain");

    let show = deadreckon(&paths)
        .current_dir(&cwd)
        .env("NO_COLOR", "1")
        .args(["chain", "show", &chain.chain_id[..8]])
        .output()
        .expect("chain show");
    assert!(show.status.success(), "{}", stderr(&show));
    let show_stdout = stdout(&show);

    let attach = deadreckon(&paths)
        .current_dir(&cwd)
        .env("NO_COLOR", "1")
        .args(["attach", &chain.chain_id[..8], "--plain"])
        .output()
        .expect("chain attach");
    assert!(attach.status.success(), "{}", stderr(&attach));
    let attach_stdout = stdout(&attach);

    let show_header = show_stdout.lines().take(8).collect::<Vec<_>>();
    let attach_header = attach_stdout.lines().take(8).collect::<Vec<_>>();
    assert_eq!(
        show_header, attach_header,
        "{show_stdout}\n---\n{attach_stdout}"
    );
    assert!(
        show_stdout.contains("spend : $1.250000 / $5.000000"),
        "{show_stdout}"
    );
}

#[test]
fn top_level_kill_accepts_chain_id_and_uses_shared_banner() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("repo");
    let mut chain = Chain::new(ChainNewOptions {
        root_goal: "top-level chain kill".to_string(),
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
    chain.status = ChainStatus::Running;
    chain.steps[0].status = ChainStepStatus::Running;
    save_chain(&paths, &chain).expect("save chain");

    let output = deadreckon(&paths)
        .current_dir(&cwd)
        .args(["kill", &chain.chain_id[..8], "--escalate", "--plain"])
        .output()
        .expect("kill chain");

    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(
        stdout.contains(&format!("killed chain {} forcefully", &chain.chain_id[..8])),
        "{stdout}"
    );
}

#[test]
fn doctor_detect_and_providers_share_kind_token() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin).expect("empty bin");
    fs::write(
        paths.config_path(),
        r#"
default_provider = "cli:codex"

[providers."cli:codex"]
kind = "cli-codex"
extra_args = []
"#,
    )
    .expect("config");

    for (args, label) in [
        (vec!["doctor"], "doctor"),
        (vec!["detect"], "detect"),
        (vec!["providers", "list"], "providers list"),
    ] {
        let output = deadreckon(&paths)
            .env("PATH", &empty_bin)
            .args(args)
            .output()
            .unwrap_or_else(|err| panic!("{label}: {err}"));
        assert!(output.status.success(), "{}: {}", label, stderr(&output));
        let stdout = stdout(&output);
        assert!(
            stdout.contains("kind=cli"),
            "{label} should print provider kind tokens:\n{stdout}"
        );
        assert!(
            !stdout.contains("descriptor="),
            "{label} should not expose descriptor as a normal output noun:\n{stdout}"
        );
    }
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
    help_slice(&args)
}

fn help_slice(args: &[&str]) -> String {
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
