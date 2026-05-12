use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use deadreckon_core::DocKind;

const TOP_LEVEL_HELP: &str = "\
Core lifecycle:
  deadreckon init
  deadreckon doctor
  deadreckon done \"builds, tests pass, and opens in a browser\"
  deadreckon run \"build the thing\"
  deadreckon attach latest
  deadreckon status
  deadreckon finish latest

Continue or recover:
  deadreckon extend latest \"add tests\"
  deadreckon resume latest
  deadreckon kill latest
  deadreckon cleanup --completed

More help:
  deadreckon help-all
  deadreckon <command> --help

Run ids accept unique prefixes. `latest` means the newest run for the current project.";

const INIT_HELP: &str = "\
Lifecycle:
  deadreckon init
  deadreckon doctor
  deadreckon run \"build the thing\"

Use `deadreckon config provider` and `deadreckon config model` later to see or change defaults.";

const CONFIG_HELP: &str = "\
Subcommands:
  deadreckon config get defaults.provider
  deadreckon config set defaults.max_spend 15
  deadreckon config provider
  deadreckon config provider cli:codex
  deadreckon config model
  deadreckon config model gpt-5.1-codex --provider cli:codex

Lifecycle:
  Configure once, then `deadreckon run \"goal\"`. Per-run flags override these defaults.";

const RUN_HELP: &str = "\
Lifecycle:
  deadreckon run \"build the thing\"
  deadreckon attach latest
  deadreckon next
  deadreckon finish latest

Common provider/model changes:
  deadreckon run \"goal\" --provider cli:codex
  deadreckon run \"goal\" --provider cli:codex --model gpt-5.1-codex
  deadreckon run \"goal\" --doc-provider cli:codex
  deadreckon config provider cli:codex
  deadreckon config model gpt-5.1-codex --provider cli:codex

Done criteria:
  deadreckon done \"build, test, and open in a browser\"
  deadreckon done add \"users can save drawings\"
  deadreckon done check
  deadreckon done show

Modes:
  In a git repo, the default is an isolated worktree.
  Use `--fresh`, `--from <dir>`, `--worktree`, or `--in-place --i-know-its-a-lot` to force a mode.";

const DONE_HELP: &str = "\
Lifecycle:
  deadreckon done \"builds, opens in a browser, and has no console errors\"
  deadreckon run \"finish the app\"

Common actions:
  deadreckon done \"plain-English definition of done\"
  deadreckon done add \"one more thing that must be true\"
  deadreckon done add browser
  deadreckon done check
  deadreckon done show

What it means:
  Write done criteria in English. deadreckon compiles them into checks for dr-gate.
  `deadreckon run` and `deadreckon chain run` prompt for this when criteria are missing.";

const ACCEPTANCE_HELP: &str = "\
Advanced compatibility command. Most users should use `deadreckon done`.

Subcommands:
  deadreckon acceptance setup
  deadreckon acceptance setup \"build, load in a browser, and show no console errors\"
  deadreckon acceptance add browser
  deadreckon acceptance add \"users can add artwork and browse the gallery\"
  deadreckon acceptance init --preset auto
  deadreckon acceptance draft \"generate acceptance for this app\"
  deadreckon acceptance refine \"also require websocket reconnect tests\"
  deadreckon acceptance explain
  deadreckon acceptance check

Lifecycle:
  Write criteria in English; deadreckon compiles them to `.deadreckon/acceptance.yaml`.
  `deadreckon run` automatically uses it, or pass `--acceptance <path>`.
  `dr-gate` remains the pass/fail authority at completion.

Packs:
  auto, basic, build, test, rust, node, static-site, browser, playwright, vite, nextjs, python";

pub(crate) const CHAIN_HELP: &str = "\
Chain subcommands:
  deadreckon chain plan \"large goal\" --n 4
  deadreckon chain \"step one\" \"step two\" --yes
  deadreckon chain run latest
  deadreckon chain attach latest
  deadreckon chain status latest
  deadreckon chain show latest --why-failed
  deadreckon chain pause latest --reason \"waiting on review\"
  deadreckon chain resume latest
  deadreckon chain kill latest
  deadreckon chain undo latest --step 2
  deadreckon chain extend latest \"new step goal\"
  deadreckon chain redo latest --step 2
  deadreckon chain hooks list

Lifecycle:
  plan/expand drafts provider-generated steps.
  run/resume executes the conductor.
  attach watches the chain TUI.
  pause/kill/undo/redo recover specific steps.
  extend adds a new step to an existing chain.";

const DOCTOR_HELP: &str = "\
Lifecycle:
  deadreckon doctor
  deadreckon init
  deadreckon run \"goal\"

Doctor checks providers, CLI binaries, sandboxes, disk space, write permissions, and OS details.";

const LIST_HELP: &str = "\
Lifecycle:
  deadreckon list
  deadreckon list --all
  deadreckon list --full
  deadreckon attach <short-id>
  deadreckon finish <short-id>

The default view is compact and scoped to the current project. Use `--full` for scripts.";

const LIBRARY_HELP: &str = "\
Subcommands:
  deadreckon library list
  deadreckon library search gallery
  deadreckon library show latest

Lifecycle:
  Accepted runs are promoted into the library. Use this to inspect artifacts after completion.";

const FINISH_HELP: &str = "\
Lifecycle:
  deadreckon finish latest
  deadreckon finish latest --autostash --cleanup
  deadreckon finish latest --dest ./finished-project

Finish chooses the right completed-run action:
  worktree run -> apply
  fresh/copy run -> export
  in-place run -> show review guidance

It still respects confirmations unless you pass `--no-confirm`.";

const MATERIALIZE_HELP: &str = "\
Lifecycle:
  deadreckon export latest --dest ./finished-project
  deadreckon show latest
  deadreckon extend latest \"follow-up goal\"

Use export/materialize for completed fresh or copy runs. Worktree runs use `deadreckon apply` instead.";

const APPLY_HELP: &str = "\
Lifecycle:
  deadreckon show latest
  deadreckon apply latest --autostash --cleanup
  deadreckon discard latest

Use apply for completed worktree runs. It merges the temporary `dr/...` branch back into your checkout.";

const ABANDON_HELP: &str = "\
Lifecycle:
  deadreckon discard latest
  deadreckon cleanup --completed

Discard removes a run's temporary worktree and branch after you decide not to keep it.";

const CLEANUP_HELP: &str = "\
Lifecycle:
  deadreckon cleanup --completed
  deadreckon cleanup --stale --force
  deadreckon cleanup <run-id>

Cleanup handles abandoned, stale, or completed temporary worktrees. It does not delete promoted library artifacts.";

const EXTEND_HELP: &str = "\
Lifecycle:
  deadreckon extend latest \"add tests\"
  deadreckon attach latest
  deadreckon finish latest

Extend creates a new run from a completed parent artifact and includes parent context by default.";

const DOC_HELP: &str = "\
Lifecycle:
  deadreckon doc latest
  deadreckon doc latest --kind as-built
  deadreckon doc latest --kind decisions
  deadreckon doc latest --export ./RUN-NARRATIVE.md

Docs are generated as part of accepted runs and are also shown in the TUI after completion.
Docs can be regenerated with a provider-backed polish pass:
  deadreckon doc latest --polish
  deadreckon doc latest --polish --doc-provider cli:codex --force
  deadreckon doc latest --polish --budget-cap 0.25 --no-confirm";

const ATTACH_HELP: &str = "\
Lifecycle:
  deadreckon attach latest
  deadreckon next
  deadreckon finish latest

Attach opens the live TUI. `q`, Esc, and Ctrl-D detach without killing the run.";

const KILL_HELP: &str = "\
Lifecycle:
  deadreckon kill latest
  deadreckon resume latest
  deadreckon cleanup --stale

Kill cancels the run, writes durable state, and terminates supervised child processes.";

const RESUME_HELP: &str = "\
Lifecycle:
  deadreckon resume latest
  deadreckon resume latest --from-turn 2
  deadreckon attach latest

Resume reconstructs history from traces, skips partial trailing records, and continues the run.";

const UNDO_HELP: &str = "\
Lifecycle:
  deadreckon undo --run latest
  deadreckon show latest

Undo restores a run snapshot. It is mainly for in-place runs or recovery inside a run working directory.";

const SHOW_HELP: &str = "\
Lifecycle:
  deadreckon show latest
  deadreckon doc latest
  deadreckon finish latest

Show prints state, mode, lineage, traces, provenance, docs, and suggested next actions.";

const STATUS_HELP: &str = "\
Lifecycle:
  deadreckon next
  deadreckon attach latest
  deadreckon finish latest

Status explains the latest run and what to do next. `next` is the same command.";

const IMPORT_HELP: &str = "\
Lifecycle:
  deadreckon import codex
  deadreckon import claude-code
  deadreckon import cursor
  deadreckon show <imported-run-id>

Import is read-only and normalizes other tool histories into deadreckon trace/provenance shape.";

#[derive(Parser)]
#[command(
    name = "deadreckon",
    version,
    about = "Unattended agentic coding harness",
    long_about = "deadreckon runs long coding tasks in an isolated worktree or sandbox, tracks durable state, and gives you explicit apply/export/cleanup steps.",
    after_help = TOP_LEVEL_HELP
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    #[command(
        next_help_heading = "Setup",
        visible_alias = "setup",
        about = "Create ~/.deadreckon/config.toml and check the local setup",
        after_help = INIT_HELP
    )]
    Init {
        #[arg(
            long,
            help = "Default provider route, for example cli:codex or cli:claude-code"
        )]
        provider: Option<String>,
        #[arg(long, help = "Provider API key for API-backed providers")]
        api_key: Option<String>,
        #[arg(long, help = "Base URL for OpenAI-compatible providers")]
        base_url: Option<String>,
        #[arg(long, default_value_t = 10.0, help = "Default spend cap in USD")]
        max_spend: f64,
        #[arg(long, default_value = "auto", help = "Default sandbox backend")]
        sandbox: String,
        #[arg(long, help = "Use detected defaults without interactive prompts")]
        no_confirm: bool,
    },
    #[command(
        next_help_heading = "Setup",
        visible_alias = "settings",
        about = "Read or update ~/.deadreckon/config.toml",
        after_help = CONFIG_HELP
    )]
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    #[command(
        next_help_heading = "Setup",
        visible_alias = "commands",
        about = "Show every command, including advanced commands",
        after_help = "Lifecycle:\n  deadreckon help-all\n  deadreckon <command> --help"
    )]
    HelpAll,
    #[command(
        next_help_heading = "Acceptance",
        hide = true,
        about = "Create, refine, explain, and check project acceptance criteria",
        after_help = ACCEPTANCE_HELP
    )]
    Acceptance {
        #[command(subcommand)]
        command: AcceptanceCommand,
    },
    #[command(
        next_help_heading = "Done Criteria",
        about = "Write, add, show, and check done criteria in English",
        after_help = DONE_HELP
    )]
    Done {
        #[arg(
            value_name = "TEXT_OR_COMMAND",
            num_args = 0..,
            help = "Plain-English criteria, or add/check/show"
        )]
        args: Vec<String>,
        #[arg(long, help = "Provider route override for drafting")]
        provider: Option<String>,
        #[arg(long, help = "Model override for drafting")]
        model: Option<String>,
        #[arg(long, help = "Overwrite generated criteria/helper files")]
        force: bool,
        #[arg(
            long,
            value_name = "PATH",
            help = "Compiled criteria file to show/check"
        )]
        spec: Option<PathBuf>,
        #[arg(
            long,
            value_name = "PATH",
            help = "Working directory to check; defaults to current directory"
        )]
        against: Option<PathBuf>,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Start an unattended coding run",
        after_help = RUN_HELP
    )]
    Run {
        #[arg(help = "Natural-language coding goal")]
        goal: String,
        #[arg(long, help = "Start from an empty workspace")]
        fresh: bool,
        #[arg(long, help = "Force a git worktree run")]
        worktree: bool,
        #[arg(
            long = "from",
            help = "Copy this source directory into runstate before running"
        )]
        from: Option<PathBuf>,
        #[arg(
            long,
            help = "Edit the current directory directly; requires explicit acknowledgement"
        )]
        in_place: bool,
        #[arg(long, help = "Base git ref for worktree runs")]
        base: Option<String>,
        #[arg(long, help = "Branch name for worktree runs")]
        branch: Option<String>,
        #[arg(long, help = "Seed current dirty/untracked files into the worktree")]
        allow_dirty: bool,
        #[arg(long, help = "Initialize git in a plain directory before running")]
        init_git: bool,
        #[arg(long, help = "Skip the run preview confirmation")]
        yes: bool,
        #[arg(long, help = "Show the planned run without creating state")]
        preview: bool,
        #[arg(long, help = "Print a single-line preview")]
        brief: bool,
        #[arg(long, help = "Spend cap in USD")]
        max_spend: Option<f64>,
        #[arg(long, help = "Wall-clock cap for CLI-backed turns")]
        max_wall_seconds: Option<f64>,
        #[arg(
            long,
            help = "Sandbox backend: auto, sandbox-exec, bwrap, docker, or none"
        )]
        sandbox: Option<String>,
        #[arg(long, help = "Provider route override")]
        provider: Option<String>,
        #[arg(long, help = "Model override for this run")]
        model: Option<String>,
        #[arg(long, help = "Provider route for generated documentation polish")]
        doc_provider: Option<String>,
        #[arg(
            long,
            value_name = "PATH",
            help = "Acceptance spec for this run; defaults to .deadreckon/acceptance.yaml when present"
        )]
        acceptance: Option<PathBuf>,
        #[arg(
            long,
            default_value = "default-coding",
            help = "Skill name under skills/"
        )]
        skill: String,
        #[arg(long, help = "Use the deterministic smoke provider")]
        smoke: bool,
        #[arg(long, help = "Allow high spend or in-place edits")]
        i_know_its_a_lot: bool,
        #[arg(long, help = "Skip safety prompts in scripts")]
        no_confirm: bool,
        #[arg(long, help = "Suppress post-completion action prompts")]
        no_hints: bool,
        #[arg(long, help = "Skip generated run documentation")]
        no_docs: bool,
        #[arg(long, help = "Documentation skill name")]
        doc_skill: Option<String>,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Run a serial chain of coding goals",
        after_help = CHAIN_HELP
    )]
    Chain {
        #[arg(value_name = "ARG", num_args = 0.., help = "Step goals or chain subcommand")]
        args: Vec<String>,
        #[arg(long, help = "Read newline-separated step goals from a file")]
        from_file: Option<PathBuf>,
        #[arg(long, help = "Read newline-separated step goals from stdin")]
        from_stdin: bool,
        #[arg(long, help = "Write chain.json only; do not start the conductor")]
        draft: bool,
        #[arg(long, help = "Skip the interactive chain preview confirmation")]
        yes: bool,
        #[arg(long, help = "Start the conductor in the background")]
        detach: bool,
        #[arg(
            long,
            default_value = "stack",
            help = "Branch policy: stack, base, or merge"
        )]
        branch_policy: String,
        #[arg(
            long,
            default_value = "auto",
            help = "Apply mode: auto, preview, or manual"
        )]
        apply_mode: String,
        #[arg(
            long,
            default_value = "squash",
            help = "Apply strategy: squash, merge, or cherry-pick"
        )]
        apply_strategy: String,
        #[arg(long, help = "Glob allowlist for auto-apply")]
        apply_allowlist: Vec<String>,
        #[arg(
            long,
            default_value = "stop",
            help = "Failure policy: stop, skip, or continue"
        )]
        on_fail: String,
        #[arg(
            long,
            default_value_t = 2,
            help = "Consecutive failed steps before pausing"
        )]
        circuit_breaker_threshold: u32,
        #[arg(long, help = "Aggregate chain spend cap in USD")]
        max_spend: Option<f64>,
        #[arg(long, help = "Aggregate chain wall-clock cap in seconds")]
        max_wall_seconds: Option<f64>,
        #[arg(long, help = "Provider route override")]
        provider: Option<String>,
        #[arg(long, help = "Model override")]
        model: Option<String>,
        #[arg(long, default_value = "auto", help = "Sandbox backend")]
        sandbox: String,
        #[arg(long, help = "Base git ref; defaults to current HEAD")]
        base: Option<String>,
        #[arg(
            long,
            default_value_t = 4,
            help = "Number of planner steps for chain plan"
        )]
        n: u8,
        #[arg(long, help = "Suppress post-action hints")]
        no_hints: bool,
        #[arg(long, help = "Suppress success stdout")]
        quiet: bool,
        #[arg(long, help = "Plain output without TUI or ANSI affordances")]
        plain: bool,
        #[arg(long, help = "Reason for pause")]
        reason: Option<String>,
        #[arg(long, help = "Resume from this step index")]
        from_step: Option<u32>,
        #[arg(long, help = "Add to the aggregate spend cap on resume")]
        max_spend_add: Option<f64>,
        #[arg(long, help = "Reset the circuit breaker on resume")]
        reset_breaker: bool,
        #[arg(long, help = "Force kill without SIGTERM grace")]
        force: bool,
        #[arg(long, help = "Step index for redo")]
        step: Option<u32>,
        #[arg(long, help = "Patch the selected redo step goal")]
        extend: Option<String>,
        #[arg(long, help = "Allow redo of an already-applied step")]
        reapply: bool,
        #[arg(long, help = "Insert extension at this step index")]
        insert_at: Option<u32>,
        #[arg(long, help = "Skip confirmation for destructive chain actions")]
        no_confirm: bool,
        #[arg(long, help = "Print exact IDs and paths")]
        full: bool,
        #[arg(long, help = "Show chains from all scopes")]
        all: bool,
        #[arg(long, help = "Explain the failure reason in chain show")]
        why_failed: bool,
    },
    #[command(
        next_help_heading = "Setup",
        visible_alias = "check",
        about = "Check providers, sandboxing, disk, and local prerequisites",
        after_help = DOCTOR_HELP
    )]
    Doctor,
    #[command(
        next_help_heading = "Run Lifecycle",
        visible_alias = "runs",
        about = "Show runs for the current project by default",
        after_help = LIST_HELP
    )]
    List {
        #[arg(long, help = "Filter to a specific scope key")]
        scope: Option<String>,
        #[arg(long, help = "Show runs from all projects")]
        all: bool,
        #[arg(long, help = "Print full TSV-style values for scripts")]
        full: bool,
    },
    #[command(
        next_help_heading = "Completed Run Actions",
        hide = true,
        visible_alias = "artifacts",
        about = "Inspect promoted run artifacts in the deadreckon library",
        after_help = LIBRARY_HELP
    )]
    Library {
        #[command(subcommand)]
        command: LibraryCommand,
    },
    #[command(
        next_help_heading = "Completed Run Actions",
        about = "Route a completed run to the right finish action",
        after_help = FINISH_HELP
    )]
    Finish {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: Option<String>,
        #[arg(long, help = "Destination directory for fresh/copy exports")]
        dest: Option<PathBuf>,
        #[arg(long, help = "Overwrite a non-empty export destination")]
        force: bool,
        #[arg(long, help = "Keep manifest.json in exported output")]
        include_manifest: bool,
        #[arg(
            long,
            default_value = "squash",
            help = "Worktree apply strategy: squash, merge, or cherry-pick"
        )]
        strategy: String,
        #[arg(long, help = "Apply target branch; defaults to the current branch")]
        branch: Option<String>,
        #[arg(
            long,
            help = "Temporarily stash local changes and restore them after worktree apply"
        )]
        autostash: bool,
        #[arg(
            long,
            help = "Remove the temporary worktree/branch after a successful worktree apply"
        )]
        cleanup: bool,
        #[arg(long, help = "Skip interactive confirmations")]
        no_confirm: bool,
        #[arg(long, help = "Commit message override for worktree apply")]
        message: Option<String>,
    },
    #[command(
        next_help_heading = "Completed Run Actions",
        hide = true,
        visible_aliases = ["export", "copy-out"],
        about = "Copy a completed fresh/copy run into a chosen directory",
        after_help = MATERIALIZE_HELP
    )]
    Materialize {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Destination directory")]
        dest: Option<PathBuf>,
        #[arg(long, help = "Overwrite a non-empty destination")]
        force: bool,
        #[arg(long, help = "Keep manifest.json in the exported output")]
        include_manifest: bool,
    },
    #[command(
        next_help_heading = "Completed Run Actions",
        hide = true,
        visible_alias = "keep",
        about = "Merge a completed worktree run back into the source checkout",
        after_help = APPLY_HELP
    )]
    Apply {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(
            long,
            default_value = "squash",
            help = "Apply strategy: squash, merge, or cherry-pick"
        )]
        strategy: String,
        #[arg(long, help = "Target branch; defaults to the current branch")]
        branch: Option<String>,
        #[arg(long, help = "Skip interactive confirmation")]
        no_confirm: bool,
        #[arg(
            long,
            help = "Temporarily stash local changes and restore them after apply"
        )]
        autostash: bool,
        #[arg(
            long,
            help = "Remove the temporary worktree/branch after a successful apply"
        )]
        cleanup: bool,
        #[arg(long, help = "Commit message override")]
        message: Option<String>,
    },
    #[command(
        next_help_heading = "Cleanup And Recovery",
        hide = true,
        visible_alias = "discard",
        about = "Remove a run's temporary worktree and branch",
        after_help = ABANDON_HELP
    )]
    Abandon {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Keep the temporary branch")]
        keep_branch: bool,
        #[arg(long, help = "Clean even if the run is still marked executing")]
        force: bool,
    },
    #[command(
        next_help_heading = "Cleanup And Recovery",
        visible_aliases = ["prune", "clean"],
        about = "Clean stale or temporary deadreckon worktrees",
        after_help = CLEANUP_HELP
    )]
    Cleanup {
        #[arg(help = "Optional run id, unique prefix, or latest")]
        run_id: Option<String>,
        #[arg(long, help = "Search all project scopes")]
        all: bool,
        #[arg(long, help = "Include completed worktree runs not already abandoned")]
        completed: bool,
        #[arg(long, help = "Include stale executing runs")]
        stale: bool,
        #[arg(long, help = "Skip interactive confirmation")]
        no_confirm: bool,
        #[arg(long, help = "Pass --force to git worktree remove and kill stale runs")]
        force: bool,
        #[arg(long, help = "Keep temporary branches")]
        keep_branch: bool,
    },
    #[command(
        next_help_heading = "Completed Run Actions",
        visible_alias = "follow-up",
        about = "Continue from a completed run with a follow-up goal",
        after_help = EXTEND_HELP
    )]
    Extend {
        #[arg(help = "Parent run id, unique prefix, or latest")]
        parent_run_id: String,
        #[arg(help = "Follow-up coding goal")]
        new_goal: String,
        #[arg(long, help = "Destination working directory for copy/fresh extensions")]
        dest: Option<PathBuf>,
        #[arg(long, help = "Maximum parent turns to include in context")]
        max_context_turns: Option<u32>,
        #[arg(long, help = "Do not include parent context")]
        no_context: bool,
        #[arg(long, help = "Spend cap in USD")]
        max_spend: Option<f64>,
        #[arg(long, help = "Wall-clock cap for CLI-backed turns")]
        max_wall_seconds: Option<f64>,
        #[arg(long, help = "Provider route override")]
        provider: Option<String>,
        #[arg(long, help = "Model override for this extended run")]
        model: Option<String>,
        #[arg(long, help = "Sandbox backend override")]
        sandbox: Option<String>,
        #[arg(long, help = "Skip generated run documentation")]
        no_docs: bool,
        #[arg(long, help = "Documentation skill name")]
        doc_skill: Option<String>,
    },
    #[command(
        next_help_heading = "Completed Run Actions",
        hide = true,
        visible_alias = "docs",
        about = "Print or regenerate generated run documentation",
        after_help = DOC_HELP
    )]
    Doc {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, value_enum, default_value_t = CliDocKind::Narrative, help = "Document kind")]
        kind: CliDocKind,
        #[arg(long, help = "Write the document to this path instead of stdout")]
        export: Option<PathBuf>,
        #[arg(long, help = "Use the doc provider to polish generated docs")]
        polish: bool,
        #[arg(long, help = "Skip polish confirmation")]
        no_confirm: bool,
        #[arg(long, help = "Overwrite existing export or polish output")]
        force: bool,
        #[arg(long, help = "Documentation skill name")]
        doc_skill: Option<String>,
        #[arg(long, help = "Provider route for documentation polish")]
        doc_provider: Option<String>,
        #[arg(long, help = "Budget cap in USD for documentation polish")]
        budget_cap: Option<f64>,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        visible_alias = "watch",
        about = "Attach the live terminal UI to a run",
        after_help = ATTACH_HELP
    )]
    Attach {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Suppress post-completion action hints")]
        no_hints: bool,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        visible_alias = "stop",
        about = "Cancel a running task",
        after_help = KILL_HELP
    )]
    Kill {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Escalate subprocess termination")]
        force: bool,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        visible_alias = "continue",
        about = "Resume an incomplete run",
        after_help = RESUME_HELP
    )]
    Resume {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Resume from this turn number")]
        from_turn: Option<u32>,
        #[arg(long, help = "Override wall-clock cap")]
        max_wall_seconds: Option<f64>,
    },
    #[command(
        next_help_heading = "Cleanup And Recovery",
        hide = true,
        visible_alias = "restore",
        about = "Restore an in-place run snapshot",
        after_help = UNDO_HELP
    )]
    Undo {
        #[arg(
            long,
            help = "Run id, unique prefix, or latest; defaults to current project's latest"
        )]
        run: Option<String>,
        #[arg(long, help = "Snapshot turn to restore")]
        turn: Option<u32>,
    },
    #[command(
        next_help_heading = "Inspect And Import",
        hide = true,
        visible_alias = "inspect",
        about = "Show full state, provenance, and trace details for a run",
        after_help = SHOW_HELP
    )]
    Show {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Only show trace/provenance records for this turn")]
        turn: Option<u32>,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        visible_alias = "next",
        about = "Explain the current project's latest run and next action",
        after_help = STATUS_HELP
    )]
    Status {
        #[arg(help = "Optional run id, unique prefix, or latest")]
        run_id: Option<String>,
        #[arg(
            long,
            help = "Use the global latest run instead of the current project"
        )]
        all: bool,
    },
    #[command(
        next_help_heading = "Inspect And Import",
        hide = true,
        about = "Import read-only history from another coding tool",
        after_help = IMPORT_HELP
    )]
    Import {
        #[arg(help = "Source: claude-code, codex, or cursor")]
        source: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliDocKind {
    Narrative,
    AsBuilt,
    Decisions,
    Delta,
}

impl From<CliDocKind> for DocKind {
    fn from(value: CliDocKind) -> Self {
        match value {
            CliDocKind::Narrative => DocKind::Narrative,
            CliDocKind::AsBuilt => DocKind::AsBuilt,
            CliDocKind::Decisions => DocKind::Decisions,
            CliDocKind::Delta => DocKind::Delta,
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum AcceptanceCommand {
    #[command(
        about = "Create acceptance criteria from plain English",
        after_help = ACCEPTANCE_HELP
    )]
    Setup {
        #[arg(value_name = "REQUEST", num_args = 0.., help = "Plain-English definition of done")]
        request: Vec<String>,
        #[arg(long, help = "Provider route override for drafting")]
        provider: Option<String>,
        #[arg(long, help = "Model override for drafting")]
        model: Option<String>,
        #[arg(long, help = "Overwrite existing .deadreckon/acceptance files")]
        force: bool,
    },
    #[command(
        about = "Add an acceptance pack or plain-English criterion",
        after_help = ACCEPTANCE_HELP
    )]
    Add {
        #[arg(
            value_name = "PACK_OR_REQUEST",
            num_args = 1..,
            help = "Pack name like browser/node/playwright, or an English criterion"
        )]
        request: Vec<String>,
        #[arg(long, help = "Provider route override for English criteria")]
        provider: Option<String>,
        #[arg(long, help = "Model override for English criteria")]
        model: Option<String>,
        #[arg(long, help = "Overwrite generated helper files")]
        force: bool,
    },
    #[command(about = "Write a local acceptance template", after_help = ACCEPTANCE_HELP)]
    Init {
        #[arg(
            long,
            default_value = "auto",
            help = "Template: auto, rust, node, static-site, or basic"
        )]
        preset: AcceptancePreset,
        #[arg(long, help = "Overwrite existing .deadreckon/acceptance files")]
        force: bool,
    },
    #[command(
        about = "Ask the configured provider to draft acceptance criteria",
        after_help = ACCEPTANCE_HELP
    )]
    Draft {
        #[arg(value_name = "REQUEST", num_args = 0.., help = "What should count as done")]
        request: Vec<String>,
        #[arg(long, help = "Provider route override for drafting")]
        provider: Option<String>,
        #[arg(long, help = "Model override for drafting")]
        model: Option<String>,
        #[arg(long, help = "Overwrite existing .deadreckon/acceptance files")]
        force: bool,
    },
    #[command(
        about = "Ask the configured provider to improve existing acceptance criteria",
        after_help = ACCEPTANCE_HELP
    )]
    Refine {
        #[arg(value_name = "REQUEST", num_args = 0.., help = "Requested acceptance change")]
        request: Vec<String>,
        #[arg(long, help = "Provider route override for drafting")]
        provider: Option<String>,
        #[arg(long, help = "Model override for drafting")]
        model: Option<String>,
        #[arg(long, help = "Overwrite existing .deadreckon/acceptance files")]
        force: bool,
    },
    #[command(about = "Explain the active acceptance criteria", after_help = ACCEPTANCE_HELP)]
    Explain {
        #[arg(long, value_name = "PATH", help = "Acceptance spec to explain")]
        spec: Option<PathBuf>,
    },
    #[command(
        about = "Dry-run acceptance checks against a working directory",
        after_help = ACCEPTANCE_HELP
    )]
    Check {
        #[arg(long, value_name = "PATH", help = "Acceptance spec to check")]
        spec: Option<PathBuf>,
        #[arg(
            long,
            value_name = "PATH",
            help = "Working directory to evaluate; defaults to current directory"
        )]
        against: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum AcceptancePreset {
    Auto,
    Rust,
    Node,
    StaticSite,
    Basic,
}

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    #[command(about = "Print one config value")]
    Get {
        #[arg(help = "Dotted key, for example defaults.provider")]
        key: String,
    },
    #[command(about = "Set one config value")]
    Set {
        #[arg(help = "Dotted key, for example defaults.provider")]
        key: String,
        #[arg(help = "TOML value or plain string")]
        value: String,
    },
    #[command(about = "Show or set the default provider route")]
    Provider {
        #[arg(help = "Provider route to make default, for example cli:codex")]
        provider: Option<String>,
    },
    #[command(about = "Show or set the model for a provider route")]
    Model {
        #[arg(help = "Model to set; omit to show the active route/model")]
        model: Option<String>,
        #[arg(
            long,
            help = "Provider route to update; defaults to the active provider"
        )]
        provider: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum LibraryCommand {
    #[command(about = "List promoted artifacts for the current project")]
    List {
        #[arg(long, help = "Filter to a specific scope key")]
        scope: Option<String>,
        #[arg(long, help = "Show artifacts from all projects")]
        all: bool,
        #[arg(long, help = "Filter promoted goals by case-insensitive text")]
        goal: Option<String>,
        #[arg(
            long,
            help = "Only show artifacts promoted on or after YYYY-MM-DD or RFC3339"
        )]
        since: Option<String>,
        #[arg(
            long,
            help = "Only show artifacts promoted on or before YYYY-MM-DD or RFC3339"
        )]
        until: Option<String>,
        #[arg(long, help = "Print full TSV-style values for scripts")]
        full: bool,
    },
    #[command(about = "Search promoted artifact goals, scopes, and run ids")]
    Search {
        #[arg(help = "Case-insensitive search text")]
        query: String,
        #[arg(long, help = "Filter to a specific scope key")]
        scope: Option<String>,
        #[arg(long, help = "Search artifacts from all projects")]
        all: bool,
        #[arg(
            long,
            help = "Only search artifacts promoted on or after YYYY-MM-DD or RFC3339"
        )]
        since: Option<String>,
        #[arg(
            long,
            help = "Only search artifacts promoted on or before YYYY-MM-DD or RFC3339"
        )]
        until: Option<String>,
    },
    #[command(about = "Show manifest and materialization details for one artifact")]
    Show {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Filter to a specific scope key")]
        scope: Option<String>,
    },
}

pub(crate) struct RunCommandArgs {
    pub(crate) goal: String,
    pub(crate) fresh: bool,
    pub(crate) worktree: bool,
    pub(crate) from: Option<PathBuf>,
    pub(crate) in_place: bool,
    pub(crate) base: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) allow_dirty: bool,
    pub(crate) init_git: bool,
    pub(crate) yes: bool,
    pub(crate) preview: bool,
    pub(crate) brief: bool,
    pub(crate) max_spend: Option<f64>,
    pub(crate) max_wall_seconds: Option<f64>,
    pub(crate) sandbox: Option<String>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) doc_provider: Option<String>,
    pub(crate) acceptance: Option<PathBuf>,
    pub(crate) skill: String,
    pub(crate) smoke: bool,
    pub(crate) i_know_its_a_lot: bool,
    pub(crate) no_confirm: bool,
    pub(crate) no_hints: bool,
    pub(crate) no_docs: bool,
    pub(crate) doc_skill: Option<String>,
}

pub(crate) struct ChainCommandArgs {
    pub(crate) args: Vec<String>,
    pub(crate) from_file: Option<PathBuf>,
    pub(crate) from_stdin: bool,
    pub(crate) draft: bool,
    pub(crate) yes: bool,
    pub(crate) detach: bool,
    pub(crate) branch_policy: String,
    pub(crate) apply_mode: String,
    pub(crate) apply_strategy: String,
    pub(crate) apply_allowlist: Vec<String>,
    pub(crate) on_fail: String,
    pub(crate) circuit_breaker_threshold: u32,
    pub(crate) max_spend: Option<f64>,
    pub(crate) max_wall_seconds: Option<f64>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) sandbox: String,
    pub(crate) base: Option<String>,
    pub(crate) n: u8,
    pub(crate) no_hints: bool,
    pub(crate) quiet: bool,
    pub(crate) plain: bool,
    pub(crate) reason: Option<String>,
    pub(crate) from_step: Option<u32>,
    pub(crate) max_spend_add: Option<f64>,
    pub(crate) reset_breaker: bool,
    pub(crate) force: bool,
    pub(crate) step: Option<u32>,
    pub(crate) extend: Option<String>,
    pub(crate) reapply: bool,
    pub(crate) insert_at: Option<u32>,
    pub(crate) no_confirm: bool,
    pub(crate) full: bool,
    pub(crate) all: bool,
    pub(crate) why_failed: bool,
}

pub(crate) struct ExtendCommandArgs {
    pub(crate) parent_run_id: String,
    pub(crate) new_goal: String,
    pub(crate) dest: Option<PathBuf>,
    pub(crate) max_context_turns: Option<u32>,
    pub(crate) no_context: bool,
    pub(crate) max_spend: Option<f64>,
    pub(crate) max_wall_seconds: Option<f64>,
    pub(crate) provider: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) sandbox: Option<String>,
    pub(crate) no_docs: bool,
    pub(crate) doc_skill: Option<String>,
    pub(crate) post_actions: bool,
}
