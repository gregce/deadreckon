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

mod common;

use common::{deadreckon, repo_tempdir, stderr, stdout};

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
fn style_facade_and_status_tones_live_in_ui_module() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(manifest.join("src/main.rs")).expect("main");
    let ui = fs::read_to_string(manifest.join("src/ui.rs")).expect("ui");

    for helper in [
        "ui_heading",
        "ui_muted",
        "ui_id",
        "ui_command",
        "ui_ok",
        "ui_warn",
        "ui_note",
        "ui_status",
        "ui_error",
    ] {
        assert!(
            !main.contains(&format!("fn {helper}(")),
            "main.rs should not own {helper}"
        );
        assert!(
            ui.contains(&format!("fn {helper}(")),
            "ui.rs should own {helper}"
        );
    }

    assert!(
        ui.contains("fn status_tone(") && ui.contains("render_status("),
        "ui.rs should own status tone mapping"
    );
}

#[test]
fn terminal_status_lines_do_not_use_warning_tone_for_failures() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(manifest.join("src/main.rs")).expect("main");
    let lifecycle =
        fs::read_to_string(manifest.join("src/commands/lifecycle.rs")).expect("lifecycle");
    let ui = fs::read_to_string(manifest.join("src/ui.rs")).expect("ui");

    for stale in [
        "ui_warn(\"paused extended run\")",
        "ui_warn(\"killed extended run\")",
        "ui_warn(\"failed extended run\")",
        "ui_warn(\"done criteria failed\")",
    ] {
        assert!(!main.contains(stale), "stale warning tone in main: {stale}");
        assert!(
            !lifecycle.contains(stale),
            "stale warning tone in lifecycle: {stale}"
        );
    }
    assert!(lifecycle.contains("fn print_extended_run_outcome("));
    assert!(lifecycle.contains("ui_status(status)"));
    assert!(ui.contains("Paused"));
    assert!(ui.contains("Note"));
}

#[test]
fn run_start_summaries_use_shared_key_value_layout() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(manifest.join("src/main.rs")).expect("main");
    let start = main
        .split("fn print_run_started_with_label(")
        .nth(1)
        .and_then(|rest| rest.split("fn print_lifecycle_hints(").next())
        .expect("print_run_started_with_label body");

    assert!(start.contains("print_kv_block(&item_refs)"), "{start}");
    assert!(!start.contains("println!(\"provider {}\""), "{start}");
    assert!(!start.contains("println!(\"state {}\""), "{start}");
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
fn strategy_flags_use_scoped_vocabulary() {
    let merge = help(["merge", "--help"]);
    assert!(merge.contains("--strategy"), "{merge}");
    assert!(
        merge.contains("Plan merge strategy: dag-aware, fail-on-conflict, or prefer-child"),
        "{merge}"
    );
    assert!(
        merge.contains("merge --strategy is the plan merge strategy"),
        "{merge}"
    );
    assert!(
        merge.contains("apply/finish use --git-strategy for git operations"),
        "{merge}"
    );

    for args in [["apply", "--help"], ["finish", "--help"]] {
        let out = help(args);
        assert!(out.contains("--git-strategy"), "{args:?}\n{out}");
        assert!(
            out.contains("Git apply strategy: squash, merge, or cherry-pick"),
            "{args:?}\n{out}"
        );
    }

    let chain = help(["chain", "--help"]);
    assert!(
        chain.contains("Chain apply mode: auto, preview, or manual"),
        "{chain}"
    );
    assert!(
        chain.contains("Git apply strategy for chain steps: squash, merge, or cherry-pick"),
        "{chain}"
    );
    assert!(
        !chain.contains("Apply strategy: squash, merge, or cherry-pick"),
        "chain help should not use the unscoped strategy phrase:\n{chain}"
    );
}

#[test]
fn prompt_output_and_machine_flags_have_one_policy() {
    let all = help(["help-all"]);
    assert!(all.contains("output and scripting policy"), "{all}");
    for phrase in [
        "--yes",
        "confirms preflight previews for start/update-style commands",
        "--no-confirm",
        "skips destructive or follow-up confirmations after a target is known",
        "--quiet",
        "suppresses success chatter and post-action hints, never requested data or errors",
        "--plain",
        "disables TUI, spinner, and ANSI affordances; it does not imply quiet",
        "--json",
        "JSON wins over styling and hints",
        "--no-hints",
        "DEADRECKON_HINTS=0 also disables them",
    ] {
        assert!(all.contains(phrase), "missing `{phrase}`:\n{all}");
    }

    for args in [
        &["run", "--help"][..],
        &["orchestrate", "--help"][..],
        &["orchestrate", "review", "--help"][..],
        &["orchestrate", "full-plan", "--help"][..],
        &["chain", "--help"][..],
    ] {
        let out = help_slice(args);
        assert!(
            out.contains("preflight") && out.contains("without prompting"),
            "{args:?}\n{out}"
        );
    }

    let update = help(["update", "--help"]);
    assert!(
        update.contains("Confirm shell-channel update preview without prompting"),
        "{update}"
    );

    for args in [
        &["run", "--help"][..],
        &["orchestrate", "--help"][..],
        &["plan", "--help"][..],
        &["fork", "--help"][..],
        &["merge", "--help"][..],
        &["chain", "--help"][..],
        &["update", "--help"][..],
    ] {
        let out = help_slice(args);
        assert!(
            out.contains("Suppress success chatter and post-action hints"),
            "{args:?}\n{out}"
        );
        assert!(
            !out.contains("Suppress success stdout"),
            "{args:?} should not use stale quiet wording:\n{out}"
        );
    }

    for args in [
        &["finish", "--help"][..],
        &["apply", "--help"][..],
        &["cleanup", "--help"][..],
        &["doc", "--help"][..],
        &["chain", "--help"][..],
    ] {
        let out = help_slice(args);
        assert!(
            out.contains("Skip destructive or follow-up confirmations"),
            "{args:?}\n{out}"
        );
        assert!(
            !out.contains("Skip interactive confirmation"),
            "{args:?} should not use stale confirmation wording:\n{out}"
        );
    }
}

#[test]
fn provider_role_help_uses_one_glossary() {
    let all = help(["help-all"]);
    assert!(all.contains("provider roles"), "{all}");
    for phrase in [
        "provider route/model/kind",
        "--provider",
        "primary run provider route",
        "--planner-provider",
        "writes the child graph before fork",
        "--child-provider IDX=PROVIDER",
        "per-child route override",
        "--coder-provider",
        "implementation pass",
        "--reviewer-provider",
        "independently reviews or fixes",
        "--doc-provider",
        "documentation polish route",
        "--repair-provider",
        "merge repair planning",
    ] {
        assert!(all.contains(phrase), "missing `{phrase}`:\n{all}");
    }

    for args in [
        &["orchestrate", "--help"][..],
        &["orchestrate", "review", "--help"][..],
        &["orchestrate", "full-plan", "--help"][..],
        &["plan", "--help"][..],
        &["fork", "--help"][..],
        &["merge", "--help"][..],
        &["doc", "--help"][..],
    ] {
        let out = help_slice(args);
        assert!(
            !out.contains("descriptor"),
            "{args:?} should reserve descriptor for registry docs:\n{out}"
        );
    }

    let plan = help(["plan", "--help"]);
    assert!(
        plan.contains("Planner provider route for full-plan decomposition"),
        "{plan}"
    );
    assert!(
        plan.contains("Default child provider route for full-plan work"),
        "{plan}"
    );
    assert!(plan.contains("Per-child provider route override"), "{plan}");
    assert!(
        plan.contains("Coder provider route for review mode"),
        "{plan}"
    );
    assert!(
        plan.contains("Reviewer provider route for review mode"),
        "{plan}"
    );

    let doc = help(["doc", "--help"]);
    assert!(
        doc.contains("Documentation provider route for polish"),
        "{doc}"
    );
}

#[test]
fn spend_cap_help_uses_one_glossary() {
    let all = help(["help-all"]);
    assert!(all.contains("spend cap glossary"), "{all}");
    for phrase in [
        "run cap",
        "`run --max-spend` limits one run's provider spend",
        "per-child cap",
        "`orchestrate`/`fork --max-spend` limits each child run",
        "aggregate chain cap",
        "`chain --max-spend` limits cumulative chain spend",
        "doc polish cap",
        "`doc --max-spend` limits documentation polish spend",
    ] {
        assert!(all.contains(phrase), "missing `{phrase}`:\n{all}");
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
fn top_help_names_audience_and_start_path() {
    let top = help(["--help"]);

    assert!(
        top.contains("already use agent CLIs"),
        "top help should name the intended audience:\n{top}"
    );
    assert!(
        top.contains("unattended, sandboxed, auditable work"),
        "top help should say why DeadReckon exists:\n{top}"
    );
    assert!(
        top.contains("deadreckon start \"build the app\""),
        "top help should point first-time users at start:\n{top}"
    );
}

#[test]
fn start_command_is_visible_in_top_help_and_help_all() {
    let top = help(["--help"]);
    let all = help(["help-all"]);

    for text in [&top, &all] {
        assert!(
            text.contains("start"),
            "guided start command should be discoverable:\n{text}"
        );
        assert!(
            text.contains("deadreckon start \"build the app\""),
            "guided first command should be shown:\n{text}"
        );
    }
}

#[test]
fn start_mode_values_parse_and_reject_unknown_modes() {
    let start_help = help(["start", "--help"]);
    for mode in ["auto", "run", "review", "full-plan"] {
        assert!(
            start_help.contains(mode),
            "missing mode {mode}:\n{start_help}"
        );
    }
    assert!(start_help.contains("selection prompts"), "{start_help}");
    assert!(start_help.contains("--plain"), "{start_help}");
    assert!(start_help.contains("--json"), "{start_help}");

    let output = Command::new(env!("CARGO_BIN_EXE_deadreckon"))
        .args(["start", "build the app", "--mode", "unknown", "--preview"])
        .output()
        .expect("start invalid mode");
    assert!(!output.status.success(), "unknown mode should fail");
    let stderr = stderr(&output);
    assert!(
        stderr.contains("unknown") && stderr.contains("possible values"),
        "unknown mode should show valid values:\n{stderr}"
    );
}

#[test]
fn start_help_points_to_run_or_orchestrate_for_power_users() {
    let start_help = help(["start", "--help"]);

    assert!(
        start_help.contains("deadreckon run \"build the app\""),
        "start help should point power users at run:\n{start_help}"
    );
    assert!(
        start_help.contains("deadreckon orchestrate \"build and review the app\""),
        "start help should point power users at orchestrate:\n{start_help}"
    );
}

#[test]
fn public_docs_first_screen_explains_harness_of_harnesses() {
    for relative in ["README.md", "HOWTO.md"] {
        let first_screen = repo_file_text(relative)
            .lines()
            .take(35)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            first_screen.contains("harness around"),
            "{relative} first screen should describe DeadReckon as a harness:\n{first_screen}"
        );
        assert!(
            first_screen.contains("agent CLI") || first_screen.contains("agent CLIs"),
            "{relative} first screen should name agent CLI users:\n{first_screen}"
        );
        assert!(
            first_screen.contains("deadreckon start \"build the app\""),
            "{relative} first screen should show the guided first command:\n{first_screen}"
        );
    }
}

#[test]
fn readme_first_screen_mentions_start_watch_finish() {
    let first_screen = repo_file_text("README.md")
        .lines()
        .take(35)
        .collect::<Vec<_>>()
        .join("\n");
    for text in [
        "deadreckon start \"build the app\"",
        "deadreckon attach latest",
        "deadreckon status",
        "deadreckon list",
        "deadreckon finish latest",
    ] {
        assert!(
            first_screen.contains(text),
            "missing {text}:\n{first_screen}"
        );
    }
}

#[test]
fn howto_new_user_path_does_not_require_provider_flags() {
    let howto = repo_file_text("HOWTO.md");
    let new_user = section_between(&howto, "## New User Path", "## Normal Single Run")
        .expect("new user section");
    assert!(
        new_user.contains("deadreckon start \"build the app\""),
        "{new_user}"
    );
    assert!(
        new_user.contains("deadreckon attach latest")
            && new_user.contains("deadreckon status latest")
            && new_user.contains("deadreckon list")
            && new_user.contains("deadreckon finish latest"),
        "{new_user}"
    );
    assert!(!new_user.contains("--provider"), "{new_user}");
    assert!(!new_user.contains("cli:"), "{new_user}");
}

#[test]
fn docs_still_document_run_and_orchestrate_directly() {
    let docs = format!(
        "{}\n{}",
        repo_file_text("README.md"),
        repo_file_text("HOWTO.md")
    );
    assert!(docs.contains("deadreckon run \"goal\""), "{docs}");
    assert!(docs.contains("deadreckon orchestrate"), "{docs}");
}

#[test]
fn start_as_built_documents_guided_front_door() {
    let as_built = repo_file_text("docs/AS-BUILT-ARCHITECTURE.md");
    let cli_surface = section_between(&as_built, "## 17. CLI Surface", "## 18. TUI (`attach`)")
        .expect("CLI surface section");

    assert!(
        cli_surface.contains("| `start` | Guided production front door"),
        "{cli_surface}"
    );
    for required in [
        "Guided first use",
        "`deadreckon start \"<goal>\"`",
        "ephemeral launch decision",
        "No `PipelineState` schema",
        "provider setup, done contract, source mode, history, and run-vs-orchestrate/campaign mode",
        "dispatches to the existing `run`, `extend`, and `orchestrate` handlers",
        "History-aware `start`",
        "keep/view/check/update/cancel",
        "previews remain state-free",
    ] {
        assert!(
            as_built.contains(required),
            "missing {required}:\n{as_built}"
        );
    }
}

#[test]
fn start_v1_candidates_track_guided_deferrals() {
    let v1 = repo_file_text("docs/V1-CANDIDATES.md");
    let guided = section_between(&v1, "## Guided first-use follow-ups", "## ")
        .unwrap_or_else(|| panic!("missing guided deferral section:\n{v1}"));

    for required in [
        "Durable launch profiles",
        "Richer multi-piece goal classification",
        "provider-backed goal-shape recommendation",
        "Personalized onboarding",
        "Provider-specific setup wizards",
    ] {
        assert!(guided.contains(required), "missing {required}:\n{guided}");
    }
}

#[test]
fn start_changelog_records_architecture_and_deferral_closeout() {
    let changelog = repo_file_text("CHANGELOG.md");
    let production = section_between(
        &changelog,
        "## Production command model (alpha) - 2026-05-27",
        "## Start picker",
    )
    .expect("production command changelog section");
    for required in [
        "default help",
        "`deadreckon help-all`",
        "history-aware",
        "done-criteria transparency",
    ] {
        assert!(
            production.contains(required),
            "missing {required}:\n{production}"
        );
    }

    let guided = section_between(
        &changelog,
        "## Guided first use (alpha) - 2026-05-26",
        "## TUI Responsiveness",
    )
    .expect("guided changelog section");

    assert!(
        guided.contains("Documented the guided first-use architecture and V1 deferrals"),
        "{guided}"
    );
}

#[test]
fn audience_copy_does_not_call_deadreckon_a_provider_replacement() {
    let docs = [
        help(["--help"]),
        repo_file_text("README.md"),
        repo_file_text("HOWTO.md"),
        repo_file_text("docs/AS-BUILT-ARCHITECTURE.md"),
    ]
    .join("\n");

    for stale in [
        "provider replacement",
        "replaces your provider",
        "replaces provider CLIs",
        "replacement for Claude",
        "replacement for Codex",
        "autonomous employee",
        "cloud IDE",
    ] {
        assert!(
            !docs.to_lowercase().contains(&stale.to_lowercase()),
            "public copy should avoid overclaim `{stale}`"
        );
    }
    assert!(
        docs.contains("instead of replacing them") || docs.contains("not a provider replacement"),
        "public copy should explicitly frame providers as supervised, not replaced"
    );
}

#[test]
fn top_help_uses_canonical_discovery_words() {
    let top = help(["--help"]);
    for command in [
        "start", "attach", "status", "list", "finish", "doctor", "kill", "resume", "cleanup",
    ] {
        assert!(
            top.contains(command),
            "top help should include {command} in the production model:\n{top}"
        );
    }
    for advanced in [
        "run",
        "orchestrate",
        "chain",
        "plan",
        "fork",
        "merge",
        "apply",
        "export",
        "history",
        "learn",
        "improve",
    ] {
        assert!(
            !top.contains(&format!("{advanced}    ")),
            "short help should leave {advanced} to help-all:\n{top}"
        );
    }
    assert!(top.contains("Production flow"), "{top}");
    assert!(top.contains("Start, watch, keep"), "{top}");
    assert!(
        top.contains("watch and understand a run, chain, or plan"),
        "{top}"
    );
    assert!(top.contains("stop a run, chain, or plan"), "{top}");
    assert!(top.contains("find runs and plans"), "{top}");
    assert!(
        top.contains("show every command, including advanced commands"),
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
    assert!(all.contains("deadreckon full command map"), "{all}");
    assert!(all.contains("production flow"), "{all}");
    assert!(all.contains("power-user launch paths"), "{all}");
    assert!(all.contains("run"), "{all}");
    assert!(all.contains("orchestrate"), "{all}");
    assert!(all.contains("chain"), "{all}");
    assert!(all.contains("find runs and plans"), "{all}");
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

    let cleanup = help(["cleanup", "--help"]);
    assert!(
        cleanup.contains("temporary run worktrees and branches"),
        "{cleanup}"
    );
    assert!(
        cleanup.contains("does not delete plan state, promoted library"),
        "{cleanup}"
    );
    assert!(
        cleanup.contains("directories exported with `deadreckon export`"),
        "{cleanup}"
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
            "deadreckon done",
            "m materialize",
            "materialize/export",
            "--force",
            "--budget-cap",
            "cleanup --all ",
            "apply <run-id> --strategy",
            "apply latest --strategy",
            "run \"goal\" --branch ",
        ] {
            assert!(
                !text.contains(stale),
                "{relative} should not teach stale primary alias `{stale}`"
            );
        }
    }
}

#[test]
fn attach_help_explains_narrative_without_log_jargon() {
    let out = help(["attach", "--help"]);

    assert!(out.contains("[default: activity]"), "{out}");
    assert!(
        out.contains("possible values: activity, narrative, split"),
        "{out}"
    );
    assert!(
        out.contains("possible values: architecture, agents, files, evidence, none"),
        "{out}"
    );
    assert!(
        out.contains("cited prose plus an evidence-backed visual map"),
        "{out}"
    );
    for jargon in ["JSONL", "provider transcript", "raw diff", "trace DAG"] {
        assert!(
            !out.contains(jargon),
            "attach help should not lead with internal narrative jargon `{jargon}`:\n{out}"
        );
    }
}

#[test]
fn docs_include_narrative_attach_example_without_provider_brand_lock() {
    let readme = repo_file_text("README.md");
    let example = readme
        .lines()
        .find(|line| line.contains("deadreckon attach latest --view narrative"))
        .expect("README narrative attach example");

    assert!(!example.contains("cli:"), "{example}");
    assert!(!example.contains("--provider"), "{example}");
    assert!(!example.contains("--narrative-provider"), "{example}");
}

#[test]
fn docs_explain_visual_map_evidence_and_color_fallback() {
    let readme = repo_file_text("README.md");
    let as_built = repo_file_text("docs/AS-BUILT-ARCHITECTURE.md");

    assert!(readme.contains("evidence-backed visual map"), "{readme}");
    assert!(
        as_built.contains("ASCII-compatible labels and color-independent badges"),
        "{as_built}"
    );
    assert!(
        as_built.contains("The TUI renders architecture, agent, file, and evidence views"),
        "{as_built}"
    );
}

#[test]
fn privacy_docs_state_summarizer_redaction_limits() {
    let as_built = repo_file_text("docs/AS-BUILT-ARCHITECTURE.md");

    assert!(as_built.contains("redacted prompt"), "{as_built}");
    assert!(
        as_built.contains("send only bounded evidence summaries"),
        "{as_built}"
    );
    assert!(
        as_built.contains("cites unknown evidence")
            && as_built.contains("emits secret-like text")
            && as_built.contains("fails, attach keeps running"),
        "{as_built}"
    );
}

#[test]
fn plan_result_wording_keeps_plan_primary() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let main = fs::read_to_string(manifest.join("src/main.rs")).expect("main");
    assert!(
        main.contains("completed plan"),
        "merge completion should name the plan"
    );
    assert!(
        main.contains("secondary run"),
        "result run should be presented as a secondary detail"
    );
    assert!(
        main.contains("artifact library"),
        "library path should be labeled as the plan artifact detail"
    );
    assert!(
        !main.contains("completed orchestration"),
        "merge completion should not hide the primary plan behind orchestration wording"
    );
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
    assert!(
        stdout.contains(&format!("paused plan {}", &plan.plan_id[..8])),
        "{stdout}"
    );
    assert!(stdout.contains("Explanation\n"), "{stdout}");
    assert!(stdout.contains("Evidence\n"), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(
        stdout.contains(&format!(
            "Recommended\ndeadreckon attach {}",
            &plan.plan_id[..8]
        )),
        "{stdout}"
    );
    assert!(stdout.contains("status      : running"), "{stdout}");
    assert!(stdout.contains("task-0"), "{stdout}");
    assert!(
        stdout.contains(&format!("deadreckon attach {}:task-0", &plan.plan_id[..8])),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("deadreckon show {}:task-0", &plan.plan_id[..8])),
        "{stdout}"
    );
    assert!(!stdout.contains("attach:"), "{stdout}");
    assert!(!stdout.contains("show:"), "{stdout}");
    assert!(!stdout.contains("finish:"), "{stdout}");
    assert!(!stdout.contains("apply:"), "{stdout}");
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

    assert!(show_stdout.starts_with("paused chain "), "{show_stdout}");
    assert!(show_stdout.contains("Explanation"), "{show_stdout}");
    assert!(show_stdout.contains("Recommended"), "{show_stdout}");
    for fragment in [
        &format!("chain : {} ({})", &chain.chain_id[..8], chain.chain_id),
        "status: running",
        "steps : 0/2",
        "spend : $1.250000 / $5.000000",
    ] {
        assert!(
            show_stdout.contains(fragment),
            "chain show lost shared header fragment {fragment}:\n{show_stdout}"
        );
        assert!(
            attach_stdout.contains(fragment),
            "chain attach lost shared header fragment {fragment}:\n{attach_stdout}"
        );
    }
}

#[test]
fn kill_surface_explains_state_transition_and_next_inspection() {
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
    assert!(stdout.starts_with("killed chain "), "{stdout}");
    assert!(stdout.contains("Explanation"), "{stdout}");
    assert!(stdout.contains("Evidence"), "{stdout}");
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

    for label in ["failed run", "Explanation", "Evidence", "Recommended"] {
        assert!(
            run_stdout.contains(label),
            "missing {label} in {run_stdout}"
        );
    }
    assert_eq!(
        run_stdout.matches("\nRecommended\n").count(),
        1,
        "{run_stdout}"
    );
    assert!(
        run_stdout.contains("Recommended\ndeadreckon show aaaabbbb --why-failed"),
        "{run_stdout}"
    );
    assert!(!run_stdout.contains("try:"), "{run_stdout}");
    for label in ["failed chain", "Explanation", "Evidence", "Recommended"] {
        assert!(
            chain_stdout.contains(label),
            "missing {label} in {chain_stdout}"
        );
    }
    assert!(chain_stdout.contains("failed run: aaaabbbb"));
    assert_eq!(
        chain_stdout
            .matches(&format!(
                "deadreckon chain show {} --why-failed",
                &chain.chain_id[..8]
            ))
            .count(),
        1,
        "{chain_stdout}"
    );
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
            cwd: cwd.clone(),
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
        (vec!["detect", "--json"], "providers"),
        (vec!["doctor", "--json"], "sandboxes"),
        (vec!["providers", "list", "--json"], "providers"),
        (vec!["chain", "--json", "list"], "chains"),
    ] {
        let output = deadreckon(&paths)
            .args(args)
            .output()
            .expect("json command");
        assert!(output.status.success(), "{}", stderr(&output));
        let stdout = stdout(&output);
        assert!(!stdout.contains("\u{1b}["), "{stdout}");
        assert!(!stdout.contains("hint:"), "{stdout}");
        let value: Value = serde_json::from_str(&stdout).expect("valid json");
        assert!(value.get(key).is_some(), "missing {key}: {value}");
        for field in ["kind", "id", "status", "next_actions", "try_lines", "paths"] {
            assert!(value.get(field).is_some(), "missing {field}: {value}");
        }
        assert!(
            value.get("next_actions").is_some_and(Value::is_array),
            "next_actions should be an array: {value}"
        );
        assert!(
            value.get("try_lines").is_some_and(Value::is_array),
            "try_lines should be an array: {value}"
        );
    }

    let output = deadreckon(&paths)
        .current_dir(&cwd)
        .args([
            "plan",
            "json plan",
            "--planner-provider",
            "smoke",
            "--provider",
            "smoke",
            "--quiet",
            "--json",
        ])
        .output()
        .expect("plan json command");
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = stdout(&output);
    assert!(!stdout.contains("\u{1b}["), "{stdout}");
    assert!(!stdout.contains("hint:"), "{stdout}");
    let value: Value = serde_json::from_str(&stdout).expect("valid plan json");
    assert!(value.get("plan").is_some(), "{value}");
    for field in ["kind", "id", "status", "next_actions", "try_lines", "paths"] {
        assert!(value.get(field).is_some(), "missing {field}: {value}");
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

fn repo_file_text(relative: &str) -> String {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo = manifest
        .parent()
        .and_then(|path| path.parent())
        .expect("repo");
    fs::read_to_string(repo.join(relative)).expect(relative)
}

fn section_between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let after_start = text.split_once(start)?.1;
    Some(
        after_start
            .split_once(end)
            .map_or(after_start, |(section, _)| section),
    )
}
