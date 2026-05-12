use std::collections::{BTreeSet, hash_map::DefaultHasher};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use deadreckon_core::paths::workspace_scope;
use deadreckon_core::{
    ApplyMode, ApplyStrategy, BranchPolicy, Chain, ChainEventKind, ChainNewOptions, ChainStatus,
    ChainStepMarker, ChainStepStatus, CodebaseMode, CodebaseRecord, ConductorState,
    DeadreckonError, DeadreckonPaths, DocKind, ModeFlags, OnFail, PhaseId, PhaseStatus,
    PolishConfig, PromotionManifest, ProvenanceRecord, RUN_EVENTS_JSONL, ResolvedMode, RunEvent,
    RunLoopConfig, RunLoopDocsConfig, RunLoopOutcome, RunOptions, RunStatus, SpendRecord,
    TraceRecord, WorktreeOptions, acquire_lock, append_chain_event, append_parent_narrative_update,
    append_provenance, append_trace, apply_commit_body, clear_cancel_marker,
    copy_source_to_working, copy_tree, create_run, create_worktree, doc_path_for_kind,
    docs_status_for_state, emit_event, inventory_files, list_runs, load_chain, load_run,
    pid_is_alive, polish_run_docs, prepare_worktree_record, preview_git_state,
    read_chain_step_marker, read_codebase_record, record_for_resolved_mode, release_lock_file,
    resolve_mode, restore_snapshot, run_turn_loop, save_chain, save_state, terminate_pid,
    validate_acceptance_marker, write_cancel_marker, write_chain_step_marker,
};
use deadreckon_providers::{ProviderRequest, ProviderRouteInfo, ProviderRouter};
use deadreckon_sandbox::SandboxBackend;
use pulldown_cmark::{
    CodeBlockKind, Event as MarkdownEvent, HeadingLevel, Options as MarkdownOptions,
    Parser as MarkdownParser, Tag, TagEnd,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

mod tui_events;

#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error(transparent)]
    Core(#[from] DeadreckonError),
    #[error(transparent)]
    Provider(#[from] deadreckon_providers::ProviderError),
    #[error(transparent)]
    Sandbox(#[from] deadreckon_sandbox::SandboxError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("TOML decode error: {0}")]
    TomlDe(#[from] toml::de::Error),
    #[error("TOML encode error: {0}")]
    TomlSer(#[from] toml::ser::Error),
}

type Result<T> = std::result::Result<T, CliError>;

fn error_hint(err: &CliError) -> Option<&'static str> {
    match err {
        CliError::Provider(deadreckon_providers::ProviderError::MissingCredential(_))
        | CliError::Provider(deadreckon_providers::ProviderError::NoRoute(_)) => Some(
            "run `deadreckon init` or `deadreckon config set providers.anthropic.api_key <KEY>`",
        ),
        CliError::Core(DeadreckonError::InvalidInput(message))
            if message.contains("max spend above $50") =>
        {
            Some("rerun with `--i-know-its-a-lot` or lower `--max-spend`")
        }
        CliError::Core(DeadreckonError::NotFound(_)) => {
            Some("run `deadreckon list` to find valid run ids or config keys")
        }
        CliError::Core(DeadreckonError::LockHeld { .. }) => Some(
            "run `deadreckon list`, then `deadreckon attach <run-id>` or `deadreckon kill <run-id>`",
        ),
        CliError::Sandbox(_) => Some("run `deadreckon doctor` to inspect sandbox availability"),
        CliError::TomlDe(_) | CliError::TomlSer(_) => {
            Some("check /Users/gdc/.deadreckon/config.toml or rerun `deadreckon init`")
        }
        CliError::Io(_) => Some("check that the referenced path exists and is writable"),
        CliError::Json(_) => Some("inspect the referenced JSON file for invalid syntax"),
        CliError::Core(_) | CliError::Provider(_) => None,
    }
}

#[derive(Clone, Copy)]
enum UiStream {
    Stdout,
    Stderr,
}

fn ui_enabled(stream: UiStream) -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if std::env::var("TERM").is_ok_and(|term| term == "dumb") {
        return false;
    }
    match stream {
        UiStream::Stdout => io::stdout().is_terminal(),
        UiStream::Stderr => io::stderr().is_terminal(),
    }
}

fn ui_style(text: impl AsRef<str>, code: &str, stream: UiStream) -> String {
    let text = text.as_ref();
    if ui_enabled(stream) {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn ui_heading(text: impl AsRef<str>) -> String {
    ui_style(text, "1;36", UiStream::Stdout)
}

fn ui_muted(text: impl AsRef<str>) -> String {
    ui_style(text, "2", UiStream::Stdout)
}

fn ui_id(text: impl AsRef<str>) -> String {
    ui_style(text, "1;35", UiStream::Stdout)
}

fn ui_command(text: impl AsRef<str>) -> String {
    ui_style(text, "1;34", UiStream::Stdout)
}

fn ui_ok(text: impl AsRef<str>) -> String {
    ui_style(text, "1;32", UiStream::Stdout)
}

fn ui_warn(text: impl AsRef<str>) -> String {
    ui_style(text, "1;33", UiStream::Stdout)
}

fn ui_error(text: impl AsRef<str>) -> String {
    ui_style(text, "1;31", UiStream::Stderr)
}

#[derive(Parser)]
#[command(
    name = "deadreckon",
    version,
    about = "Unattended agentic coding harness",
    long_about = "deadreckon runs long coding tasks in an isolated worktree or sandbox, tracks durable state, and gives you explicit apply/export/cleanup steps.",
    after_help = "Command groups:\n  Setup: init, config, doctor\n  Run Lifecycle: run, attach, status, list, kill, resume\n  Completed Run Actions: apply, materialize/export, extend, doc, library\n  Cleanup And Recovery: abandon/discard, cleanup/prune, undo\n  Inspect And Import: show, import\n\nLifecycle:\n  deadreckon run \"build the thing\"\n  deadreckon attach latest\n  deadreckon status\n  deadreckon apply latest --autostash --cleanup\n  deadreckon cleanup --completed\n\nRun ids accept unique prefixes. `latest` means the newest run for the current project."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(
        next_help_heading = "Setup",
        about = "Create ~/.deadreckon/config.toml and check the local setup"
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
        about = "Read or update ~/.deadreckon/config.toml"
    )]
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Start an unattended coding run"
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
        about = "Run a serial chain of coding goals"
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
        about = "Check providers, sandboxing, disk, and local prerequisites"
    )]
    Doctor,
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Show runs for the current project by default"
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
        about = "Inspect promoted run artifacts in the deadreckon library"
    )]
    Library {
        #[command(subcommand)]
        command: LibraryCommand,
    },
    #[command(
        next_help_heading = "Completed Run Actions",
        visible_alias = "export",
        about = "Copy a completed fresh/copy run into a chosen directory"
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
        about = "Merge a completed worktree run back into the source checkout"
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
        visible_alias = "discard",
        about = "Remove a run's temporary worktree and branch"
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
        visible_alias = "prune",
        about = "Clean stale or temporary deadreckon worktrees"
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
        about = "Continue from a completed run with a follow-up goal"
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
        about = "Print or regenerate generated run documentation"
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
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Attach the live terminal UI to a run"
    )]
    Attach {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Suppress post-completion action hints")]
        no_hints: bool,
    },
    #[command(next_help_heading = "Run Lifecycle", about = "Cancel a running task")]
    Kill {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Escalate subprocess termination")]
        force: bool,
    },
    #[command(
        next_help_heading = "Run Lifecycle",
        about = "Resume an incomplete run"
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
        about = "Restore an in-place run snapshot"
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
        about = "Show full state, provenance, and trace details for a run"
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
        about = "Explain the current project's latest run and next action"
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
        about = "Import read-only history from another coding tool"
    )]
    Import {
        #[arg(help = "Source: claude-code, codex, or cursor")]
        source: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CliDocKind {
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
enum ConfigCommand {
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
enum LibraryCommand {
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

#[tokio::main]
async fn main() {
    if let Err(err) = main_inner().await {
        eprintln!("{} {err}", ui_error("error:"));
        if let Some(hint) = error_hint(&err) {
            eprintln!("  {} {hint}", ui_style("hint:", "1;34", UiStream::Stderr));
        }
        std::process::exit(1);
    }
}

async fn main_inner() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();
    let command = cli.command.unwrap_or(Commands::Status {
        run_id: None,
        all: false,
    });
    match command {
        Commands::Init {
            provider,
            api_key,
            base_url,
            max_spend,
            sandbox,
            no_confirm,
        } => init_command(provider, api_key, base_url, max_spend, sandbox, no_confirm).await,
        Commands::Config { command } => config_command(command),
        Commands::Run {
            goal,
            fresh,
            worktree,
            from,
            in_place,
            base,
            branch,
            allow_dirty,
            init_git,
            yes,
            preview,
            brief,
            max_spend,
            max_wall_seconds,
            sandbox,
            provider,
            model,
            skill,
            smoke,
            i_know_its_a_lot,
            no_confirm,
            no_hints,
            no_docs,
            doc_skill,
        } => {
            run_command(RunCommandArgs {
                goal,
                fresh,
                worktree,
                from,
                in_place,
                base,
                branch,
                allow_dirty,
                init_git,
                yes,
                preview,
                brief,
                max_spend,
                max_wall_seconds,
                sandbox,
                provider,
                model,
                skill,
                smoke,
                i_know_its_a_lot,
                no_confirm,
                no_hints,
                no_docs,
                doc_skill,
            })
            .await
        }
        Commands::Chain {
            args,
            from_file,
            from_stdin,
            draft,
            yes,
            detach,
            branch_policy,
            apply_mode,
            apply_strategy,
            apply_allowlist,
            on_fail,
            circuit_breaker_threshold,
            max_spend,
            max_wall_seconds,
            provider,
            model,
            sandbox,
            base,
            n,
            no_hints,
            quiet,
            plain,
            reason,
            from_step,
            max_spend_add,
            reset_breaker,
            force,
            step,
            extend,
            reapply,
            insert_at,
            no_confirm,
            full,
            all,
            why_failed,
        } => {
            chain_command(ChainCommandArgs {
                args,
                from_file,
                from_stdin,
                draft,
                yes,
                detach,
                branch_policy,
                apply_mode,
                apply_strategy,
                apply_allowlist,
                on_fail,
                circuit_breaker_threshold,
                max_spend,
                max_wall_seconds,
                provider,
                model,
                sandbox,
                base,
                n,
                no_hints,
                quiet,
                plain,
                reason,
                from_step,
                max_spend_add,
                reset_breaker,
                force,
                step,
                extend,
                reapply,
                insert_at,
                no_confirm,
                full,
                all,
                why_failed,
            })
            .await
        }
        Commands::Doctor => doctor_command().await,
        Commands::List { scope, all, full } => list_command(scope, all, full),
        Commands::Library { command } => library_command(command),
        Commands::Materialize {
            run_id,
            dest,
            force,
            include_manifest,
        } => materialize_command(run_id, dest, force, include_manifest),
        Commands::Apply {
            run_id,
            strategy,
            branch,
            no_confirm,
            autostash,
            cleanup,
            message,
        } => apply_command(
            run_id, strategy, branch, no_confirm, autostash, cleanup, message,
        ),
        Commands::Abandon {
            run_id,
            keep_branch,
            force,
        } => abandon_command(run_id, keep_branch, force),
        Commands::Cleanup {
            run_id,
            all,
            completed,
            stale,
            no_confirm,
            force,
            keep_branch,
        } => cleanup_command(
            run_id,
            all,
            completed,
            stale,
            no_confirm,
            force,
            keep_branch,
        ),
        Commands::Extend {
            parent_run_id,
            new_goal,
            dest,
            max_context_turns,
            no_context,
            max_spend,
            max_wall_seconds,
            provider,
            model,
            sandbox,
            no_docs,
            doc_skill,
        } => {
            extend_command(ExtendCommandArgs {
                parent_run_id,
                new_goal,
                dest,
                max_context_turns,
                no_context,
                max_spend,
                max_wall_seconds,
                provider,
                model,
                sandbox,
                no_docs,
                doc_skill,
                post_actions: completion_hints_enabled(false),
            })
            .await
        }
        Commands::Doc {
            run_id,
            kind,
            export,
            polish,
            no_confirm,
            force,
            doc_skill,
        } => {
            doc_command(
                run_id,
                kind.into(),
                export,
                polish,
                no_confirm,
                force,
                doc_skill,
            )
            .await
        }
        Commands::Attach { run_id, no_hints } => attach_command(run_id, no_hints).await,
        Commands::Kill { run_id, force } => kill_command(run_id, force),
        Commands::Resume {
            run_id,
            from_turn,
            max_wall_seconds,
        } => resume_command(run_id, from_turn, max_wall_seconds).await,
        Commands::Undo { run, turn } => undo_command(run, turn),
        Commands::Show { run_id, turn } => show_command(run_id, turn),
        Commands::Status { run_id, all } => status_command(run_id, all),
        Commands::Import { source } => import_command(source),
    }
}

async fn init_command(
    provider: Option<String>,
    api_key: Option<String>,
    base_url: Option<String>,
    max_spend: f64,
    sandbox: String,
    no_confirm: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    fs::create_dir_all(paths.home())?;
    let provider = match provider {
        Some(provider) => provider,
        None if no_confirm && command_exists("claude") => "cli:claude-code".to_string(),
        None if no_confirm && command_exists("codex") => "cli:codex".to_string(),
        None if no_confirm => "anthropic".to_string(),
        None => prompt_provider()?,
    };
    let api_key = api_key.or_else(|| {
        if provider.starts_with("cli:") {
            None
        } else {
            prompt("provider API key (leave blank to use env var): ").ok()
        }
    });
    let config = init_config_text(
        &provider,
        api_key.as_deref(),
        base_url.as_deref(),
        max_spend,
        &sandbox,
    );
    fs::write(paths.config_path(), config)?;
    println!("{} {}", ui_ok("wrote"), paths.config_path().display());
    doctor_command().await?;
    println!(
        "{} {}",
        ui_command("next:"),
        ui_command("deadreckon run \"describe the coding task\"")
    );
    Ok(())
}

fn config_command(command: ConfigCommand) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    match command {
        ConfigCommand::Get { key } => {
            let root = load_config_value(&paths)?;
            match get_toml_path(&root, &key) {
                Some(value) => println!("{}", value_to_display(value)),
                None => {
                    return Err(CliError::Core(DeadreckonError::NotFound(format!(
                        "config key {key}"
                    ))));
                }
            }
        }
        ConfigCommand::Set { key, value } => {
            fs::create_dir_all(paths.home())?;
            let mut root = load_config_value(&paths)?;
            set_toml_path(&mut root, &key, parse_config_value(&value));
            fs::write(paths.config_path(), toml::to_string_pretty(&root)?)?;
            println!("{} {key}", ui_ok("set"));
        }
        ConfigCommand::Provider { provider } => match provider {
            Some(provider) => {
                fs::create_dir_all(paths.home())?;
                let mut root = load_config_value(&paths)?;
                set_toml_path(
                    &mut root,
                    "defaults.provider",
                    toml::Value::String(provider.clone()),
                );
                set_toml_path(
                    &mut root,
                    "default_provider",
                    toml::Value::String(provider.clone()),
                );
                fs::write(paths.config_path(), toml::to_string_pretty(&root)?)?;
                println!("{} default provider {}", ui_ok("set"), ui_id(provider));
            }
            None => print_provider_selection(&paths, None)?,
        },
        ConfigCommand::Model { model, provider } => match model {
            Some(model) => {
                fs::create_dir_all(paths.home())?;
                let mut root = load_config_value(&paths)?;
                let provider = provider
                    .or_else(|| active_provider_from_config(&root))
                    .ok_or_else(|| {
                        CliError::Core(deadreckon_core::user_error(
                            "no active provider configured",
                            "deadreckon config provider cli:codex",
                        ))
                    })?;
                set_provider_model(&mut root, &provider, &model);
                fs::write(paths.config_path(), toml::to_string_pretty(&root)?)?;
                println!(
                    "{} model for {} -> {}",
                    ui_ok("set"),
                    ui_id(provider),
                    ui_id(model)
                );
            }
            None => print_provider_selection(&paths, provider.as_deref())?,
        },
    }
    Ok(())
}

fn print_provider_selection(paths: &DeadreckonPaths, provider: Option<&str>) -> Result<()> {
    let router = ProviderRouter::from_config_path(&paths.config_path(), provider)?;
    let routes = router.route_info();
    let selected = router.selected_route_info();
    println!("{}", ui_heading("provider selection"));
    for route in routes {
        let marker = if selected
            .as_ref()
            .is_some_and(|selected| selected.name == route.name)
        {
            "*"
        } else {
            " "
        };
        let credential = if route.has_credential {
            "ready"
        } else {
            "missing"
        };
        println!(
            "{marker} {}  kind={}  model={}  credential={credential}",
            ui_id(route.name),
            format_provider_kind(route.kind),
            route.model
        );
    }
    if let Some(selected) = selected {
        println!(
            "{} {}",
            ui_command("try:"),
            ui_command(format!(
                "deadreckon run \"goal\" --provider {} --model <model>",
                selected.name
            ))
        );
        println!(
            "{} {}",
            ui_command("default model:"),
            ui_command(format!(
                "deadreckon config model <model> --provider {}",
                selected.name
            ))
        );
    }
    Ok(())
}

fn active_provider_from_config(root: &toml::Value) -> Option<String> {
    get_toml_path(root, "defaults.provider")
        .or_else(|| get_toml_path(root, "default_provider"))
        .and_then(toml::Value::as_str)
        .map(ToString::to_string)
}

fn set_provider_model(root: &mut toml::Value, provider: &str, model: &str) {
    if !root.is_table() {
        *root = toml::Value::Table(Default::default());
    }
    let root_table = root.as_table_mut().expect("table after initialization");
    let providers = root_table
        .entry("providers".to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()));
    if !providers.is_table() {
        *providers = toml::Value::Table(Default::default());
    }
    let providers_table = providers.as_table_mut().expect("providers table");
    let provider_entry = providers_table
        .entry(provider.to_string())
        .or_insert_with(|| toml::Value::Table(Default::default()));
    if !provider_entry.is_table() {
        *provider_entry = toml::Value::Table(Default::default());
    }
    provider_entry
        .as_table_mut()
        .expect("provider table")
        .insert("model".to_string(), toml::Value::String(model.to_string()));
}

fn format_provider_kind(kind: deadreckon_providers::ProviderKind) -> &'static str {
    match kind {
        deadreckon_providers::ProviderKind::Anthropic => "anthropic",
        deadreckon_providers::ProviderKind::OpenAi => "openai",
        deadreckon_providers::ProviderKind::OpenAiCompatible => "openai-compatible",
        deadreckon_providers::ProviderKind::CliClaudeCode => "cli-claude-code",
        deadreckon_providers::ProviderKind::CliCodex => "cli-codex",
        deadreckon_providers::ProviderKind::ScriptedSmoke => "smoke",
    }
}

struct RunCommandArgs {
    goal: String,
    fresh: bool,
    worktree: bool,
    from: Option<PathBuf>,
    in_place: bool,
    base: Option<String>,
    branch: Option<String>,
    allow_dirty: bool,
    init_git: bool,
    yes: bool,
    preview: bool,
    brief: bool,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    sandbox: Option<String>,
    provider: Option<String>,
    model: Option<String>,
    skill: String,
    smoke: bool,
    i_know_its_a_lot: bool,
    no_confirm: bool,
    no_hints: bool,
    no_docs: bool,
    doc_skill: Option<String>,
}

struct ChainCommandArgs {
    args: Vec<String>,
    from_file: Option<PathBuf>,
    from_stdin: bool,
    draft: bool,
    yes: bool,
    detach: bool,
    branch_policy: String,
    apply_mode: String,
    apply_strategy: String,
    apply_allowlist: Vec<String>,
    on_fail: String,
    circuit_breaker_threshold: u32,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    provider: Option<String>,
    model: Option<String>,
    sandbox: String,
    base: Option<String>,
    n: u8,
    no_hints: bool,
    quiet: bool,
    plain: bool,
    reason: Option<String>,
    from_step: Option<u32>,
    max_spend_add: Option<f64>,
    reset_breaker: bool,
    force: bool,
    step: Option<u32>,
    extend: Option<String>,
    reapply: bool,
    insert_at: Option<u32>,
    no_confirm: bool,
    full: bool,
    all: bool,
    why_failed: bool,
}

struct ExtendCommandArgs {
    parent_run_id: String,
    new_goal: String,
    dest: Option<PathBuf>,
    max_context_turns: Option<u32>,
    no_context: bool,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    provider: Option<String>,
    model: Option<String>,
    sandbox: Option<String>,
    no_docs: bool,
    doc_skill: Option<String>,
    post_actions: bool,
}

async fn chain_command(args: ChainCommandArgs) -> Result<()> {
    let ChainCommandArgs {
        args,
        from_file,
        from_stdin,
        draft,
        yes,
        detach,
        branch_policy,
        apply_mode,
        apply_strategy,
        apply_allowlist,
        on_fail,
        circuit_breaker_threshold,
        max_spend,
        max_wall_seconds,
        provider,
        model,
        sandbox,
        base,
        n,
        no_hints,
        quiet,
        plain,
        reason,
        from_step,
        max_spend_add,
        reset_breaker,
        force,
        step,
        extend,
        reapply,
        insert_at,
        no_confirm,
        full,
        all,
        why_failed,
    } = args;
    let paths = DeadreckonPaths::discover();
    let Some(first) = args.first().map(String::as_str) else {
        if from_file.is_some() || from_stdin {
            let goals = collect_chain_goals(&[], from_file, from_stdin)?;
            return chain_create_command(ChainCreateOptions {
                paths,
                root_goal: format!("manual: {} steps", goals.len()),
                goals,
                from_file: None,
                from_stdin: false,
                draft,
                yes,
                detach,
                branch_policy,
                apply_mode,
                apply_strategy,
                apply_allowlist,
                on_fail,
                circuit_breaker_threshold,
                max_spend,
                max_wall_seconds,
                provider,
                model,
                sandbox,
                base,
                n,
                no_hints,
                quiet,
                plain,
            })
            .await
            .map(|_| ());
        }
        eprintln!("using: chain status (scope: {})", current_scope()?);
        return chain_status_command(None, all, full, plain);
    };

    match first {
        "plan" | "expand" => {
            let root_goal = args.get(1).cloned().ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    "chain plan needs a goal",
                    "deadreckon chain plan \"build the app\" --n 4",
                ))
            })?;
            chain_plan_command(ChainCreateOptions {
                paths,
                root_goal,
                goals: Vec::new(),
                from_file,
                from_stdin,
                draft,
                yes,
                detach,
                branch_policy,
                apply_mode,
                apply_strategy,
                apply_allowlist,
                on_fail,
                circuit_breaker_threshold,
                max_spend,
                max_wall_seconds,
                provider,
                model,
                sandbox,
                base,
                n,
                no_hints,
                quiet,
                plain,
            })
            .await
        }
        "run" => {
            let id = args.get(1).map(String::as_str).unwrap_or("latest");
            if id == "latest" || id == "last" {
                let latest = resolve_chain_id(&paths, id, all)?;
                eprintln!("using: chain resume {}", chain_prefix(&latest));
                return chain_run_command(
                    &paths,
                    &latest,
                    ChainRunOptions {
                        detach,
                        quiet,
                        plain,
                        from_step,
                        max_spend_add,
                        reset_breaker,
                        apply_mode: Some(apply_mode),
                    },
                )
                .await;
            }
            let id = resolve_chain_id(&paths, id, all)?;
            chain_run_command(
                &paths,
                &id,
                ChainRunOptions {
                    detach,
                    quiet,
                    plain,
                    from_step,
                    max_spend_add,
                    reset_breaker,
                    apply_mode: Some(apply_mode),
                },
            )
            .await
        }
        "resume" => {
            let id = resolve_chain_id(
                &paths,
                args.get(1).map(String::as_str).unwrap_or("latest"),
                all,
            )?;
            chain_run_command(
                &paths,
                &id,
                ChainRunOptions {
                    detach,
                    quiet,
                    plain,
                    from_step,
                    max_spend_add,
                    reset_breaker,
                    apply_mode: Some(apply_mode),
                },
            )
            .await
        }
        "status" => chain_status_command(args.get(1).map(String::as_str), all, full, plain),
        "list" => chain_list_command(all, full),
        "show" => chain_show_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            why_failed,
        ),
        "attach" => chain_attach_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            plain,
        ),
        "pause" => chain_pause_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            reason,
        ),
        "kill" => chain_kill_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            force,
        ),
        "undo" => chain_undo_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            step,
            no_confirm,
        ),
        "extend" => {
            let id = args.get(1).map(String::as_str).unwrap_or("latest");
            let step_goal = args.get(2).cloned().or(extend).ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    "chain extend needs a step goal",
                    "deadreckon chain extend latest \"add tests\"",
                ))
            })?;
            chain_extend_command(&paths, id, step_goal, insert_at, max_spend_add)
        }
        "redo" => chain_redo_command(
            &paths,
            args.get(1).map(String::as_str).unwrap_or("latest"),
            step,
            extend,
            reapply,
        ),
        "hooks" if args.get(1).is_some_and(|arg| arg == "list") => chain_hooks_list_command(),
        maybe_id if args.len() == 1 && looks_like_chain_id(maybe_id) => {
            Err(CliError::Core(deadreckon_core::user_error(
                &format!("did you mean `chain run {maybe_id}`?"),
                &format!("deadreckon chain run {maybe_id}"),
            )))
        }
        _ => {
            let goals = collect_chain_goals(&args, from_file, from_stdin)?;
            chain_create_command(ChainCreateOptions {
                paths,
                root_goal: format!("manual: {} steps", goals.len()),
                goals,
                from_file: None,
                from_stdin: false,
                draft,
                yes,
                detach,
                branch_policy,
                apply_mode,
                apply_strategy,
                apply_allowlist,
                on_fail,
                circuit_breaker_threshold,
                max_spend,
                max_wall_seconds,
                provider,
                model,
                sandbox,
                base,
                n,
                no_hints,
                quiet,
                plain,
            })
            .await
            .map(|_| ())
        }
    }
}

struct ChainCreateOptions {
    paths: DeadreckonPaths,
    root_goal: String,
    goals: Vec<String>,
    from_file: Option<PathBuf>,
    from_stdin: bool,
    draft: bool,
    yes: bool,
    detach: bool,
    branch_policy: String,
    apply_mode: String,
    apply_strategy: String,
    apply_allowlist: Vec<String>,
    on_fail: String,
    circuit_breaker_threshold: u32,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    provider: Option<String>,
    model: Option<String>,
    sandbox: String,
    base: Option<String>,
    n: u8,
    no_hints: bool,
    quiet: bool,
    plain: bool,
}

struct ChainRunOptions {
    detach: bool,
    quiet: bool,
    plain: bool,
    from_step: Option<u32>,
    max_spend_add: Option<f64>,
    reset_breaker: bool,
    apply_mode: Option<String>,
}

async fn chain_plan_command(options: ChainCreateOptions) -> Result<()> {
    let n = options.n.clamp(2, 12);
    let paths = options.paths.clone();
    let router = ProviderRouter::from_config_path_with_model(
        &paths.config_path(),
        options.provider.as_deref(),
        options.model.as_deref(),
    )?;
    let prompt = chain_planner_prompt(&options.root_goal, n);
    let response = router
        .complete(&ProviderRequest {
            prompt,
            max_output_tokens: u32::from(n) * 96,
            cwd: Some(std::env::current_dir()?),
            output_path: None,
            sandbox_backend: None,
            pid_file: None,
            cancellation_token: None,
        })
        .await
        .map_err(|err| {
            CliError::Core(deadreckon_core::user_error(
                &format!("chain planner provider failed: {err}"),
                "deadreckon chain \"step one\" \"step two\"",
            ))
        })?;
    let goals = parse_planner_goals(&response.content, n)?;
    let chain_id = chain_create_command(ChainCreateOptions { goals, ..options }).await?;
    append_chain_planner_spend(&paths, &chain_id, &response)?;
    Ok(())
}

async fn chain_create_command(options: ChainCreateOptions) -> Result<String> {
    let ChainCreateOptions {
        paths,
        root_goal,
        mut goals,
        from_file,
        from_stdin,
        draft,
        yes,
        detach,
        branch_policy,
        apply_mode,
        apply_strategy,
        apply_allowlist,
        on_fail,
        circuit_breaker_threshold,
        max_spend,
        max_wall_seconds,
        provider,
        model,
        sandbox,
        base,
        n: _,
        no_hints,
        quiet,
        plain,
    } = options;
    if goals.is_empty() {
        goals = collect_chain_goals(&[], from_file, from_stdin)?;
    }
    deadreckon_core::validate_goal_count(goals.len()).map_err(CliError::from)?;
    let cwd = std::env::current_dir()?;
    let git_root = deadreckon_core::find_git_root(&cwd)?.ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            "chains require a git repo",
            "cd into a repo or initialize one with git init",
        ))
    })?;
    let scope = workspace_scope(&cwd).map_err(CliError::from)?;
    let base_ref = base.unwrap_or_else(|| "HEAD".to_string());
    let base_sha = git_stdout(&git_root, &["rev-parse", &base_ref])?;
    let base_branch = git_stdout(&git_root, &["symbolic-ref", "--short", "HEAD"])
        .unwrap_or_else(|_| base_ref.clone());
    let chain = Chain::new(ChainNewOptions {
        root_goal,
        goals,
        scope,
        base_branch,
        base_sha,
        cwd: git_root.clone(),
        provider,
        model,
        sandbox,
        branch_policy: parse_branch_policy(&branch_policy)?,
        apply_mode: parse_apply_mode(&apply_mode)?,
        apply_strategy: parse_apply_strategy(&apply_strategy)?,
        apply_allowlist,
        on_fail: parse_on_fail(&on_fail)?,
        circuit_breaker_threshold,
        max_spend_usd: max_spend,
        max_wall_seconds,
        deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
    })
    .map_err(CliError::from)?;
    save_chain(&paths, &chain)?;
    append_chain_event(
        &paths,
        &chain.chain_id,
        ChainEventKind::ChainCreated,
        None,
        json!({ "steps": chain.steps.len(), "draft": draft }),
    )?;
    if !quiet {
        println!("{}", chain_preview(&chain));
    }
    if draft {
        if completion_hints_enabled(no_hints) && !quiet {
            println!(
                "drafted: {} with {} steps",
                chain.chain_id,
                chain.steps.len()
            );
            println!(
                "edit:    vim {}",
                paths.chain_json(&chain.chain_id).display()
            );
            println!(
                "run:     deadreckon chain run {}",
                chain_prefix(&chain.chain_id)
            );
        }
        return Ok(chain.chain_id);
    }
    if !yes {
        if !io::stdin().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "non-interactive chain start requires --yes",
                "deadreckon chain --yes \"step one\" \"step two\"",
            )));
        }
        let answer = prompt("start the chain? [Y/n]: ")?;
        if matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no") {
            println!("cancelled");
            return Ok(chain.chain_id);
        }
    }
    let chain_id = chain.chain_id.clone();
    chain_run_command(
        &paths,
        &chain_id,
        ChainRunOptions {
            detach,
            quiet,
            plain,
            from_step: None,
            max_spend_add: None,
            reset_breaker: false,
            apply_mode: None,
        },
    )
    .await?;
    Ok(chain_id)
}

async fn chain_run_command(
    paths: &DeadreckonPaths,
    chain_id: &str,
    options: ChainRunOptions,
) -> Result<()> {
    let chain_id = resolve_chain_id(paths, chain_id, false)?;
    if options.detach {
        return detach_chain_conductor(paths, &chain_id, &options);
    }
    run_chain_conductor(paths, &chain_id, options).await
}

async fn run_chain_conductor(
    paths: &DeadreckonPaths,
    chain_id: &str,
    options: ChainRunOptions,
) -> Result<()> {
    let mut chain = load_chain(paths, chain_id)?;
    if let Some(add) = options.max_spend_add {
        chain.max_spend_usd = Some(chain.max_spend_usd.unwrap_or(0.0) + add);
    }
    if options.reset_breaker {
        chain.circuit_breaker_consecutive_failures = 0;
    }
    if let Some(mode) = options.apply_mode.as_deref()
        && mode != "auto"
    {
        chain.apply_mode = parse_apply_mode(mode)?;
    }
    match chain.status {
        ChainStatus::Completed => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("chain '{}' is completed", chain_prefix(&chain.chain_id)),
                &format!("deadreckon chain show {}", chain_prefix(&chain.chain_id)),
            )));
        }
        ChainStatus::Running if chain.conductor_pid.is_some_and(pid_is_alive) => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "chain '{}' is already running (pid {})",
                    chain_prefix(&chain.chain_id),
                    chain.conductor_pid.unwrap_or_default()
                ),
                &format!("deadreckon chain attach {}", chain_prefix(&chain.chain_id)),
            )));
        }
        _ => {}
    }
    let mut lock = acquire_lock(
        paths,
        &chain.task_key(),
        &chain.chain_id,
        &chain.scope,
        "chain",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    )?;
    chain.status = ChainStatus::Running;
    chain.started_at.get_or_insert_with(Utc::now);
    chain.paused_reason = None;
    chain.conductor_pid = Some(std::process::id());
    save_chain(paths, &chain)?;
    let conductor = ConductorState {
        schema_version: 1,
        chain_id: chain.chain_id.clone(),
        conductor_pid: std::process::id(),
        started_at: Utc::now(),
        live_step: None,
        live_run_id: None,
    };
    fs::create_dir_all(paths.chain_dir(&chain.chain_id))?;
    fs::write(
        paths.conductor_json(&chain.chain_id),
        serde_json::to_vec_pretty(&conductor)?,
    )?;

    let start_index = options.from_step.unwrap_or(0);
    let mut completed = true;
    for index in 0..chain.steps.len() {
        if (index as u32) < start_index {
            continue;
        }
        if matches!(
            chain.steps[index].status,
            ChainStepStatus::Applied | ChainStepStatus::Skipped
        ) {
            continue;
        }
        lock.heartbeat(format!("step-{index}"))?;
        let step_cap = per_step_spend_cap(&chain, index);
        let base_ref = chain_step_base_ref(&chain)?;
        match invoke_chain_hook(
            paths,
            &chain,
            "pre-step",
            Some(index as u32),
            json!({
                "chain_id": chain.chain_id,
                "step_index": index,
                "step_goal": chain.steps[index].goal,
                "base_ref": base_ref
            }),
        )? {
            1 => {
                chain.steps[index].status = ChainStepStatus::Skipped;
                append_chain_event(
                    paths,
                    &chain.chain_id,
                    ChainEventKind::ChainStepFailed,
                    Some(index as u32),
                    json!({ "reason": "skipped_by_pre_step_hook" }),
                )?;
                save_chain(paths, &chain)?;
                continue;
            }
            2 => {
                pause_chain_at_step(
                    paths,
                    &mut chain,
                    index,
                    "paused_by_pre_step_hook".to_string(),
                )?;
                completed = false;
                break;
            }
            _ => {}
        }
        chain.steps[index].status = ChainStepStatus::Running;
        append_chain_event(
            paths,
            &chain.chain_id,
            ChainEventKind::ChainStepStarted,
            Some(index as u32),
            json!({ "goal": chain.steps[index].goal, "base": base_ref, "max_spend": step_cap }),
        )?;
        save_chain(paths, &chain)?;
        let run_id = match run_chain_step(&chain, index, &base_ref, step_cap, options.quiet).await {
            Ok(run_id) => run_id,
            Err(err) => {
                completed = handle_chain_step_failure(paths, &mut chain, index, err.to_string())?;
                if !completed {
                    break;
                }
                continue;
            }
        };
        chain.steps[index].run_id = Some(run_id.clone());
        let state = load_run(paths, &run_id)?;
        chain.steps[index].spend_usd = state.total_spend_usd;
        chain.total_spend_usd += state.total_spend_usd;
        chain.total_wall_seconds += state.total_wall_seconds;
        write_chain_step_marker(
            &state.working_dir,
            &ChainStepMarker::new(
                &chain,
                &chain.steps[index],
                latest_applied_sha_before(&chain, index),
            ),
        )?;
        append_chain_event(
            paths,
            &chain.chain_id,
            ChainEventKind::ChainRunCompleted,
            Some(index as u32),
            json!({ "run_id": run_id, "status": state.status.to_string() }),
        )?;
        if state.status != RunStatus::Completed {
            completed = handle_chain_step_failure(
                paths,
                &mut chain,
                index,
                format!("inner run {} ended {}", state.run_id, state.status),
            )?;
            if !completed {
                break;
            }
            continue;
        }
        match invoke_chain_hook(
            paths,
            &chain,
            "post-step",
            Some(index as u32),
            json!({
                "chain_id": chain.chain_id,
                "step_index": index,
                "run_id": state.run_id,
                "status": state.status.to_string(),
                "library_dir": state.promoted_library_dir
            }),
        )? {
            1 => {
                pause_chain_at_step(
                    paths,
                    &mut chain,
                    index,
                    "paused_by_post_step_hook".to_string(),
                )?;
                completed = false;
                break;
            }
            2 => {
                completed = handle_chain_step_failure(
                    paths,
                    &mut chain,
                    index,
                    "refused_by_post_step_hook".to_string(),
                )?;
                if !completed {
                    break;
                }
                continue;
            }
            _ => {}
        }
        chain.steps[index].status = ChainStepStatus::Completed;
        save_chain(paths, &chain)?;
        if chain.apply_mode == ApplyMode::Auto {
            if let Err(err) = auto_apply_chain_step(paths, &mut chain, index, &state.run_id) {
                pause_chain_at_step(
                    paths,
                    &mut chain,
                    index,
                    format!("apply_refused_{}", compact_reason(&err.to_string())),
                )?;
                completed = false;
                break;
            }
        } else {
            let reason = format!("apply_mode_{}", apply_mode_label(chain.apply_mode));
            pause_chain_at_step(paths, &mut chain, index, reason)?;
            completed = false;
            break;
        }
        if chain_spend_cap_hit(&chain) {
            pause_chain_at_step(paths, &mut chain, index, "cap".to_string())?;
            completed = false;
            break;
        }
    }
    if completed {
        let hook_status = invoke_chain_hook(
            paths,
            &chain,
            "on-chain-end",
            None,
            json!({
                "chain_id": chain.chain_id,
                "status": "completed",
                "steps_completed": chain.steps.iter().filter(|step| step.status == ChainStepStatus::Applied).count(),
                "total_spend_usd": chain.total_spend_usd
            }),
        )
        .unwrap_or_default();
        chain.status = ChainStatus::Completed;
        chain.completed_at = Some(Utc::now());
        chain.conductor_pid = None;
        append_chain_event(
            paths,
            &chain.chain_id,
            ChainEventKind::ChainCompleted,
            None,
            json!({ "steps_completed": chain.steps.iter().filter(|step| step.status == ChainStepStatus::Applied).count(), "total_spend_usd": chain.total_spend_usd, "on_chain_end_status": hook_status }),
        )?;
        save_chain(paths, &chain)?;
        if !options.quiet {
            println!(
                "chained: {} done {}/{}",
                chain_prefix(&chain.chain_id),
                chain.steps.len(),
                chain.steps.len()
            );
            println!(
                "show:    deadreckon chain show {}",
                chain_prefix(&chain.chain_id)
            );
            println!("list:    deadreckon chain list");
        }
    } else if !options.quiet {
        print_chain_paused_footer(&chain);
    }
    let _ = fs::remove_file(paths.conductor_json(&chain.chain_id));
    let _ = lock.release();
    Ok(())
}

async fn run_chain_step(
    chain: &Chain,
    index: usize,
    base_ref: &str,
    step_cap: Option<f64>,
    quiet: bool,
) -> Result<String> {
    let step = &chain.steps[index];
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .current_dir(&chain.cwd)
        .env("DEADRECKON_HOME", DeadreckonPaths::discover().home())
        .arg("run")
        .arg(&step.goal)
        .arg("--worktree")
        .arg("--base")
        .arg(base_ref)
        .arg("--yes")
        .arg("--no-confirm")
        .arg("--no-hints")
        .arg("--sandbox")
        .arg(&chain.sandbox);
    if let Some(provider) = chain.provider.as_deref() {
        command.arg("--provider").arg(provider);
    }
    if let Some(model) = chain.model.as_deref() {
        command.arg("--model").arg(model);
    }
    if let Some(max_wall) = chain.max_wall_seconds {
        command.arg("--max-wall-seconds").arg(max_wall.to_string());
    }
    if let Some(step_cap) = step_cap {
        command.arg("--max-spend").arg(format!("{step_cap:.6}"));
    }
    let output = command.output()?;
    if !quiet {
        io::stdout().write_all(&output.stdout)?;
        io::stderr().write_all(&output.stderr)?;
    }
    if !output.status.success() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "step {} run failed: {}{}",
            index + 1,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))));
    }
    parse_started_run_id(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "could not find inner run id in run output\ntry: deadreckon list".to_string(),
        ))
    })
}

fn auto_apply_chain_step(
    paths: &DeadreckonPaths,
    chain: &mut Chain,
    index: usize,
    run_id: &str,
) -> Result<()> {
    let state = load_run(paths, run_id)?;
    validate_acceptance_marker(&state)?;
    let record = read_codebase_record(&state.working_dir)?;
    let git_root = record.source_git_root.as_ref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing source_git_root".to_string(),
        ))
    })?;
    if !git_stdout(git_root, &["status", "--porcelain"])?
        .trim()
        .is_empty()
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("step '{}' refused auto-apply (dirty target)", index + 1),
            &format!(
                "git -C {} stash && deadreckon chain resume {}",
                git_root.display(),
                chain_prefix(&chain.chain_id)
            ),
        )));
    }
    if !chain.apply_allowlist.is_empty() {
        let branch = record.branch_name.as_deref().ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "missing branch_name".to_string(),
            ))
        })?;
        let files = git_stdout(
            git_root,
            &["diff", "--name-only", &format!("HEAD..{branch}")],
        )?;
        for file in files.lines().filter(|line| !line.trim().is_empty()) {
            if !chain
                .apply_allowlist
                .iter()
                .any(|pattern| allowlist_matches(pattern, file))
            {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!(
                        "step '{}' refused auto-apply ({file} outside allowlist)",
                        index + 1
                    ),
                    &format!(
                        "deadreckon chain resume {} --apply-mode preview",
                        chain_prefix(&chain.chain_id)
                    ),
                )));
            }
        }
    }
    let branch = record.branch_name.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing branch_name".to_string(),
        ))
    })?;
    let files_changed = git_stdout(
        git_root,
        &["diff", "--name-only", &format!("HEAD..{branch}")],
    )?
    .lines()
    .filter(|line| !line.trim().is_empty())
    .map(ToString::to_string)
    .collect::<Vec<_>>();
    let diff_stat =
        git_stdout(git_root, &["diff", "--stat", &format!("HEAD..{branch}")]).unwrap_or_default();
    match invoke_chain_hook(
        paths,
        chain,
        "on-promote",
        Some(index as u32),
        json!({
            "chain_id": chain.chain_id,
            "step_index": index,
            "run_id": run_id,
            "diff_stat": diff_stat,
            "files_changed": files_changed
        }),
    )? {
        1 => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("step '{}' paused by hook on-promote", index + 1),
                &format!("deadreckon chain resume {}", chain_prefix(&chain.chain_id)),
            )));
        }
        2 => {
            append_chain_event(
                paths,
                &chain.chain_id,
                ChainEventKind::ChainApplyRefused,
                Some(index as u32),
                json!({ "reason": "refused_by_hook_on_promote" }),
            )?;
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("step '{}' refused by hook on-promote", index + 1),
                "inspect ~/.deadreckon/hooks/chain/on-promote",
            )));
        }
        _ => {}
    }
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainApplyStarted,
        Some(index as u32),
        json!({ "run_id": run_id }),
    )?;
    apply_command(
        run_id.to_string(),
        apply_strategy_label(chain_apply_strategy(chain)).to_string(),
        None,
        true,
        true,
        false,
        None,
    )?;
    let applied_sha = git_stdout(git_root, &["rev-parse", "HEAD"])?;
    chain.steps[index].status = ChainStepStatus::Applied;
    chain.steps[index].applied_at = Some(Utc::now());
    chain.steps[index].applied_sha = Some(applied_sha.clone());
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainApplied,
        Some(index as u32),
        json!({ "run_id": run_id, "applied_sha": applied_sha }),
    )?;
    save_chain(paths, chain)?;
    Ok(())
}

fn handle_chain_step_failure(
    paths: &DeadreckonPaths,
    chain: &mut Chain,
    index: usize,
    reason: String,
) -> Result<bool> {
    chain.steps[index].status = ChainStepStatus::Failed;
    chain.steps[index].fail_reason = Some(reason.clone());
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainStepFailed,
        Some(index as u32),
        json!({ "reason": reason }),
    )?;
    match chain.on_fail {
        OnFail::Stop => {
            pause_chain_at_step(paths, chain, index, "step_failed".to_string())?;
            Ok(false)
        }
        OnFail::Skip => {
            chain.steps[index].status = ChainStepStatus::Skipped;
            chain.circuit_breaker_consecutive_failures += 1;
            if chain.circuit_breaker_consecutive_failures >= chain.circuit_breaker_threshold {
                pause_chain_at_step(paths, chain, index, "circuit_breaker_open".to_string())?;
                return Ok(false);
            }
            save_chain(paths, chain)?;
            Ok(true)
        }
        OnFail::Continue => {
            chain.steps[index].status = ChainStepStatus::Skipped;
            save_chain(paths, chain)?;
            Ok(true)
        }
    }
}

fn pause_chain_at_step(
    paths: &DeadreckonPaths,
    chain: &mut Chain,
    index: usize,
    reason: String,
) -> Result<()> {
    chain.status = ChainStatus::Paused;
    chain.paused_reason = Some(reason.clone());
    chain.conductor_pid = None;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainPaused,
        Some(index as u32),
        json!({ "reason": reason }),
    )?;
    save_chain(paths, chain)?;
    Ok(())
}

fn detach_chain_conductor(
    paths: &DeadreckonPaths,
    chain_id: &str,
    options: &ChainRunOptions,
) -> Result<()> {
    fs::create_dir_all(paths.chain_dir(chain_id))?;
    let stdout = fs::File::create(paths.chain_dir(chain_id).join("conductor.out"))?;
    let stderr = fs::File::create(paths.chain_dir(chain_id).join("conductor.err"))?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .arg("chain")
        .arg("run")
        .arg(chain_id)
        .arg("--quiet")
        .env("DEADRECKON_HOME", paths.home())
        .stdout(stdout)
        .stderr(stderr)
        .stdin(std::process::Stdio::null());
    if options.plain {
        command.arg("--plain");
    }
    let child = command.spawn()?;
    println!(
        "chain {} detached (pid {})",
        chain_prefix(chain_id),
        child.id()
    );
    println!("attach: deadreckon chain attach {}", chain_prefix(chain_id));
    Ok(())
}

fn invoke_chain_hook(
    paths: &DeadreckonPaths,
    chain: &Chain,
    hook: &str,
    step_index: Option<u32>,
    payload: Value,
) -> Result<i32> {
    let Some(path) = resolve_chain_hook(paths, &chain.cwd, hook) else {
        return Ok(0);
    };
    let mut child = std::process::Command::new(&path)
        .env("DEADRECKON_CHAIN_ID", &chain.chain_id)
        .env("DEADRECKON_HOME", paths.home())
        .env(
            "DEADRECKON_STEP_INDEX",
            step_index
                .map(|index| index.to_string())
                .unwrap_or_else(|| "-".to_string()),
        )
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    if let Some(stdin) = child.stdin.as_mut() {
        serde_json::to_writer(&mut *stdin, &payload)?;
        stdin.write_all(b"\n")?;
    }
    let output = child.wait_with_output()?;
    let stdout = truncate_text(&String::from_utf8_lossy(&output.stdout), 4096);
    let stderr = truncate_text(&String::from_utf8_lossy(&output.stderr), 4096);
    let code = output.status.code().unwrap_or(1);
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainHookInvoked,
        step_index,
        json!({
            "hook": hook,
            "path": path,
            "status": code,
            "stdout": stdout,
            "stderr": stderr
        }),
    )?;
    Ok(code)
}

fn resolve_chain_hook(paths: &DeadreckonPaths, cwd: &Path, hook: &str) -> Option<PathBuf> {
    [
        cwd.join(".deadreckon/hooks/chain").join(hook),
        paths.home().join("hooks/chain").join(hook),
        PathBuf::from("/Users/gdc/deadreckon/hooks/chain").join(hook),
    ]
    .into_iter()
    .find(|path| path.exists())
}

fn chain_status_command(id: Option<&str>, all: bool, full: bool, _plain: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    if let Some(id) = id {
        return chain_show_command(&paths, id, false);
    }
    let chains = list_chain_records(&paths, if all { None } else { Some(current_scope()?) })?;
    if chains.is_empty() {
        println!("no chains in scope");
        println!("try: deadreckon chain \"step one\" \"step two\"");
        return Ok(());
    }
    print_chain_table(&chains, full);
    Ok(())
}

fn chain_list_command(all: bool, full: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let chains = list_chain_records(&paths, if all { None } else { Some(current_scope()?) })?;
    if chains.is_empty() {
        println!("no chains");
        println!("try: deadreckon chain \"step one\" \"step two\"");
        return Ok(());
    }
    print_chain_table(&chains, full);
    Ok(())
}

fn chain_show_command(paths: &DeadreckonPaths, id: &str, why_failed: bool) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let chain = load_chain(paths, &id)?;
    println!("chain {}", ui_id(&chain.chain_id));
    println!("status {}", chain_status_label(&chain));
    println!(
        "policy {} apply={} on-fail={} base={}@{}",
        branch_policy_label(chain.branch_policy),
        apply_mode_label(chain.apply_mode),
        on_fail_label(chain.on_fail),
        chain.base_branch,
        short_sha(&chain.base_sha)
    );
    println!("cwd {}", chain.cwd.display());
    println!("path {}", paths.chain_json(&chain.chain_id).display());
    println!(
        "spend ${:.6} / {}",
        chain.total_spend_usd,
        chain
            .max_spend_usd
            .map(|value| format!("${value:.6}"))
            .unwrap_or_else(|| "uncapped".to_string())
    );
    for step in &chain.steps {
        println!(
            "{} step {} {:<9} {}{}",
            chain_step_dot(step.status),
            step.index + 1,
            chain_step_status_label(step.status),
            truncate_text(&step.goal, 72),
            step.run_id
                .as_deref()
                .map(|run_id| format!(" run={}", run_prefix(run_id)))
                .unwrap_or_default()
        );
        if why_failed && let Some(reason) = step.fail_reason.as_deref() {
            println!("  reason: {reason}");
        }
    }
    Ok(())
}

fn chain_attach_command(paths: &DeadreckonPaths, id: &str, _plain: bool) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let chain = load_chain(paths, &id)?;
    println!(
        "chain {} status: {} steps: {}/{} spend: ${:.2}/{}",
        chain_prefix(&chain.chain_id),
        chain_status_label(&chain),
        chain
            .steps
            .iter()
            .filter(|step| step.status == ChainStepStatus::Applied)
            .count(),
        chain.steps.len(),
        chain.total_spend_usd,
        chain
            .max_spend_usd
            .map(|value| format!("${value:.2}"))
            .unwrap_or_else(|| "uncapped".to_string())
    );
    println!(
        "policy: {} | apply={} | on-fail={} | base={}@{}",
        branch_policy_label(chain.branch_policy),
        apply_mode_label(chain.apply_mode),
        on_fail_label(chain.on_fail),
        chain.base_branch,
        short_sha(&chain.base_sha)
    );
    for step in &chain.steps {
        println!(
            "{} step {} {:<9} {}",
            chain_step_dot(step.status),
            step.index + 1,
            chain_step_status_label(step.status),
            truncate_text(&step.goal, 80)
        );
    }
    println!("[r] redo  [e] extend  [p] pause  [k] kill  [Ctrl-D] detach  [q] quit");
    Ok(())
}

fn chain_pause_command(paths: &DeadreckonPaths, id: &str, reason: Option<String>) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    if chain.status != ChainStatus::Running {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("cannot pause '{}' chain", chain_status_label(&chain)),
            &format!("deadreckon chain status {}", chain_prefix(&chain.chain_id)),
        )));
    }
    chain.status = ChainStatus::Paused;
    chain.paused_reason = Some(reason.unwrap_or_else(|| "user_paused".to_string()));
    save_chain(paths, &chain)?;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainPaused,
        None,
        json!({ "reason": chain.paused_reason }),
    )?;
    println!("paused {}", chain_prefix(&chain.chain_id));
    println!(
        "try: deadreckon chain resume {}",
        chain_prefix(&chain.chain_id)
    );
    Ok(())
}

fn chain_kill_command(paths: &DeadreckonPaths, id: &str, force: bool) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    if let Some(pid) = chain.conductor_pid {
        terminate_pid(pid, force)?;
    }
    chain.status = ChainStatus::Killed;
    chain.failure_reason = Some("killed by user".to_string());
    chain.conductor_pid = None;
    save_chain(paths, &chain)?;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainKilled,
        None,
        json!({ "force": force }),
    )?;
    println!("killed {}", chain_prefix(&chain.chain_id));
    Ok(())
}

fn chain_undo_command(
    paths: &DeadreckonPaths,
    id: &str,
    through_step: Option<u32>,
    no_confirm: bool,
) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    let mut applied = chain
        .steps
        .iter()
        .filter(|step| step.status == ChainStepStatus::Applied)
        .filter(|step| through_step.is_none_or(|limit| step.index <= limit))
        .filter_map(|step| {
            step.applied_sha
                .as_deref()
                .map(|sha| (step.index, sha.to_string()))
        })
        .collect::<Vec<_>>();
    if applied.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "nothing to undo",
            &format!("deadreckon chain show {}", chain_prefix(&chain.chain_id)),
        )));
    }
    if !no_confirm && io::stdin().is_terminal() {
        let answer = prompt("undo applied chain commits? [y/N]: ")?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("cancelled");
            return Ok(());
        }
    } else if !no_confirm {
        return Err(CliError::Core(deadreckon_core::user_error(
            "non-interactive chain undo requires --no-confirm",
            &format!(
                "deadreckon chain undo {} --no-confirm",
                chain_prefix(&chain.chain_id)
            ),
        )));
    }
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainUndoStarted,
        None,
        json!({ "count": applied.len() }),
    )?;
    applied.reverse();
    for (index, sha) in applied {
        git_status(&chain.cwd, &["revert", "--no-edit", &sha])?;
        if let Some(step) = chain.steps.iter_mut().find(|step| step.index == index) {
            step.status = ChainStepStatus::Undone;
        }
        append_chain_event(
            paths,
            &chain.chain_id,
            ChainEventKind::ChainUndoneStep,
            Some(index),
            json!({ "sha": sha }),
        )?;
    }
    chain.status = ChainStatus::Undone;
    save_chain(paths, &chain)?;
    println!("undone {}", chain_prefix(&chain.chain_id));
    Ok(())
}

fn chain_extend_command(
    paths: &DeadreckonPaths,
    id: &str,
    step_goal: String,
    insert_at: Option<u32>,
    max_spend_add: Option<f64>,
) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    if chain.status == ChainStatus::Completed && insert_at.is_none() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "cannot extend completed chain at end",
            &format!(
                "deadreckon chain extend {} \"...\" --insert-at <N>",
                chain_prefix(&chain.chain_id)
            ),
        )));
    }
    if let Some(add) = max_spend_add {
        chain.max_spend_usd = Some(chain.max_spend_usd.unwrap_or(0.0) + add);
    }
    let insert = insert_at
        .map(|value| value.saturating_sub(1) as usize)
        .unwrap_or(chain.steps.len())
        .min(chain.steps.len());
    chain.steps.insert(
        insert,
        deadreckon_core::ChainStep::new(insert as u32, step_goal),
    );
    for (index, step) in chain.steps.iter_mut().enumerate() {
        step.index = index as u32;
    }
    if chain.status == ChainStatus::Completed {
        chain.status = ChainStatus::Paused;
        chain.paused_reason = Some("extended".to_string());
    }
    save_chain(paths, &chain)?;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainStepExtended,
        Some(insert as u32),
        json!({ "insert_at": insert }),
    )?;
    println!("extended {}", chain_prefix(&chain.chain_id));
    println!(
        "next: deadreckon chain resume {}",
        chain_prefix(&chain.chain_id)
    );
    Ok(())
}

fn chain_redo_command(
    paths: &DeadreckonPaths,
    id: &str,
    step: Option<u32>,
    extend: Option<String>,
    reapply: bool,
) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let mut chain = load_chain(paths, &id)?;
    let index = step
        .map(|step| step.saturating_sub(1))
        .or_else(|| {
            chain
                .steps
                .iter()
                .find(|step| step.status == ChainStepStatus::Failed)
                .map(|step| step.index)
        })
        .or_else(|| {
            chain
                .steps
                .iter()
                .rev()
                .find(|step| step.status == ChainStepStatus::Applied)
                .map(|step| step.index)
        })
        .ok_or_else(|| {
            CliError::Core(deadreckon_core::user_error(
                "no failed or applied step to redo",
                &format!("deadreckon chain show {}", chain_prefix(&chain.chain_id)),
            ))
        })? as usize;
    let step = chain.steps.get_mut(index).ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            &format!("step {} does not exist", index + 1),
            &format!("deadreckon chain show {}", chain_prefix(&chain.chain_id)),
        ))
    })?;
    if step.status == ChainStepStatus::Applied && !reapply {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("step '{}' already applied; redo needs --reapply", index + 1),
            &format!(
                "deadreckon chain redo {} --step {} --reapply",
                chain_prefix(&chain.chain_id),
                index + 1
            ),
        )));
    }
    if reapply && let Some(sha) = step.applied_sha.as_deref() {
        git_status(&chain.cwd, &["revert", "--no-edit", sha])?;
    }
    let prior_goal = step.goal.clone();
    if let Some(extend) = extend {
        step.goal = extend;
    }
    step.status = ChainStepStatus::Pending;
    step.run_id = None;
    step.applied_at = None;
    step.applied_sha = None;
    save_chain(paths, &chain)?;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainStepRedone,
        Some(index as u32),
        json!({ "prior_goal": prior_goal, "new_goal": chain.steps[index].goal }),
    )?;
    println!("redo queued {}", chain_prefix(&chain.chain_id));
    println!(
        "next: deadreckon chain resume {}",
        chain_prefix(&chain.chain_id)
    );
    Ok(())
}

fn chain_hooks_list_command() -> Result<()> {
    let paths = DeadreckonPaths::discover();
    for hook in ["pre-step", "post-step", "on-promote", "on-chain-end"] {
        let project = std::env::current_dir()?
            .join(".deadreckon/hooks/chain")
            .join(hook);
        let user = paths.home().join("hooks/chain").join(hook);
        let repo = PathBuf::from("/Users/gdc/deadreckon/hooks/chain").join(hook);
        let (tier, path) = if project.exists() {
            ("project", project)
        } else if user.exists() {
            ("user", user)
        } else if repo.exists() {
            ("repo", repo)
        } else {
            ("missing", user)
        };
        println!("{hook}\t{tier}\t{}", path.display());
    }
    Ok(())
}

fn collect_chain_goals(
    args: &[String],
    from_file: Option<PathBuf>,
    from_stdin: bool,
) -> Result<Vec<String>> {
    let mut goals = Vec::new();
    goals.extend(args.iter().cloned());
    if let Some(path) = from_file {
        goals.extend(parse_goal_lines(&fs::read_to_string(&path).map_err(
            |source| {
                CliError::Core(DeadreckonError::Io {
                    path: path.clone(),
                    source,
                })
            },
        )?));
    }
    if from_stdin {
        if io::stdin().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "--from-stdin needs a pipe",
                "printf 'g1\\ng2\\n' | deadreckon chain --from-stdin --yes",
            )));
        }
        let mut raw = String::new();
        io::stdin().read_to_string(&mut raw)?;
        goals.extend(parse_goal_lines(&raw));
    }
    Ok(goals
        .into_iter()
        .map(|goal| goal.trim().to_string())
        .filter(|goal| !goal.is_empty())
        .collect())
}

fn parse_goal_lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(ToString::to_string)
        .collect()
}

fn chain_planner_prompt(goal: &str, n: u8) -> String {
    format!(
        "You are decomposing a coding goal into an ordered serial chain.\n\
Output a JSON array of <= {n} strings, each <= 160 chars, each a concrete next step. \
Each step builds on the previous step's result. No prose, no commentary. Goal: {goal:?}."
    )
}

fn parse_planner_goals(raw: &str, n: u8) -> Result<Vec<String>> {
    let raw = raw.trim();
    let json_text = if raw.starts_with("```") {
        raw.lines()
            .filter(|line| !line.trim_start().starts_with("```"))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        raw.to_string()
    };
    let value = serde_json::from_str::<Value>(json_text.trim()).map_err(|err| {
        CliError::Core(deadreckon_core::user_error(
            &format!("chain plan returned invalid JSON: {err}"),
            "deadreckon chain \"step one\" \"step two\"",
        ))
    })?;
    let array = value.as_array().ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            "chain plan must return a JSON array of strings",
            "deadreckon chain \"step one\" \"step two\"",
        ))
    })?;
    let mut seen = BTreeSet::new();
    let mut goals = Vec::new();
    for item in array.iter().take(usize::from(n)) {
        let Some(goal) = item.as_str().map(str::trim).filter(|goal| !goal.is_empty()) else {
            continue;
        };
        if goal.chars().count() > 160 {
            return Err(CliError::Core(deadreckon_core::user_error(
                "chain plan produced a step longer than 160 chars",
                "ask for fewer steps or use explicit `deadreckon chain \"...\" \"...\"`",
            )));
        }
        let key = goal
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();
        if !seen.insert(key) {
            return Err(CliError::Core(deadreckon_core::user_error(
                "chain plan produced duplicate steps",
                "rerun with --n 3 or provide explicit steps",
            )));
        }
        goals.push(goal.to_string());
    }
    deadreckon_core::validate_goal_count(goals.len()).map_err(|_| {
        CliError::Core(deadreckon_core::user_error(
            &format!("decomposition produced {} goals; need >= 2", goals.len()),
            "deadreckon chain plan \"goal\" --n 3",
        ))
    })?;
    Ok(goals)
}

fn append_chain_planner_spend(
    paths: &DeadreckonPaths,
    chain_id: &str,
    response: &deadreckon_providers::ProviderResponse,
) -> Result<()> {
    let path = paths.chain_dir(chain_id).join("spend.jsonl");
    let parent = path.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "path has no parent: {}",
            path.display()
        )))
    })?;
    fs::create_dir_all(parent)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    serde_json::to_writer(
        &mut file,
        &json!({
            "timestamp": Utc::now(),
            "kind": "chain.planner",
            "provider": response.provider,
            "model": response.model,
            "input_tokens": response.spend.input_tokens,
            "output_tokens": response.spend.output_tokens,
            "cost_usd": response.spend.cost_usd,
        }),
    )?;
    file.write_all(b"\n")?;
    Ok(())
}

fn chain_preview(chain: &Chain) -> String {
    let cap = chain
        .max_spend_usd
        .map(|value| format!("${value:.2}"))
        .unwrap_or_else(|| "uncapped".to_string());
    let mut lines = vec![
        format!("chain preview {}", chain_prefix(&chain.chain_id)),
        format!("scope {}", chain.scope),
        format!("cwd {}", chain.cwd.display()),
        format!("base {}@{}", chain.base_branch, short_sha(&chain.base_sha)),
        format!(
            "policy branch={} apply={} strategy={} on-fail={}",
            branch_policy_label(chain.branch_policy),
            apply_mode_label(chain.apply_mode),
            apply_strategy_label(chain.apply_strategy),
            on_fail_label(chain.on_fail)
        ),
        format!(
            "provider {} model {} sandbox {} max-spend {}",
            chain.provider.as_deref().unwrap_or("default"),
            chain.model.as_deref().unwrap_or("default"),
            chain.sandbox,
            cap
        ),
        "steps".to_string(),
    ];
    for step in &chain.steps {
        lines.push(format!("  {}. {}", step.index + 1, step.goal));
    }
    lines.join("\n")
}

fn print_chain_table(chains: &[Chain], full: bool) {
    println!("CHAIN     STATUS     STEPS  SPEND       UPDATED                  GOAL");
    for chain in chains {
        let id = if full {
            chain.chain_id.clone()
        } else {
            chain_prefix(&chain.chain_id)
        };
        let done = chain
            .steps
            .iter()
            .filter(|step| step.status == ChainStepStatus::Applied)
            .count();
        let updated = chain
            .completed_at
            .or(chain.started_at)
            .unwrap_or(chain.created_at);
        println!(
            "{:<9} {:<10} {:>2}/{:<2} ${:<9.6} {:<24} {}",
            id,
            chain_status_label(chain),
            done,
            chain.steps.len(),
            chain.total_spend_usd,
            updated,
            truncate_text(&chain.root_goal, 80)
        );
    }
}

fn list_chain_records(paths: &DeadreckonPaths, scope: Option<String>) -> Result<Vec<Chain>> {
    if !paths.chains_dir().exists() {
        return Ok(Vec::new());
    }
    let mut chains = Vec::new();
    for entry in fs::read_dir(paths.chains_dir())? {
        let entry = entry?;
        let path = entry.path().join("chain.json");
        if !path.exists() {
            continue;
        }
        let chain = serde_json::from_slice::<Chain>(&fs::read(&path)?)
            .map_err(|source| DeadreckonError::Json { path, source })?;
        if scope.as_deref().is_some_and(|scope| chain.scope != scope) {
            continue;
        }
        chains.push(chain);
    }
    chains.sort_by(|left, right| right.created_at.cmp(&left.created_at));
    Ok(chains)
}

fn resolve_chain_id(paths: &DeadreckonPaths, id: &str, all: bool) -> Result<String> {
    let scope = if all { None } else { Some(current_scope()?) };
    let chains = list_chain_records(paths, scope)?;
    if matches!(id, "latest" | "last") {
        return chains
            .first()
            .map(|chain| chain.chain_id.clone())
            .ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    "no chains in scope",
                    "deadreckon chain \"step one\" \"step two\"",
                ))
            });
    }
    let matches = chains
        .iter()
        .filter(|chain| chain.chain_id.starts_with(id))
        .map(|chain| chain.chain_id.clone())
        .collect::<Vec<_>>();
    match matches.len() {
        1 => Ok(matches[0].clone()),
        0 => Err(CliError::Core(deadreckon_core::user_error(
            &format!("no chain '{id}'"),
            "deadreckon chain list",
        ))),
        _ => Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "ambiguous chain id prefix {id}; matches {}",
                matches.join(", ")
            ),
            "deadreckon chain list --full",
        ))),
    }
}

fn parse_branch_policy(value: &str) -> Result<BranchPolicy> {
    match value {
        "stack" => Ok(BranchPolicy::Stack),
        "base" => Ok(BranchPolicy::Base),
        "merge" => Ok(BranchPolicy::Merge),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown branch policy {other}"),
            "use --branch-policy stack|base|merge",
        ))),
    }
}

fn parse_apply_mode(value: &str) -> Result<ApplyMode> {
    match value {
        "auto" => Ok(ApplyMode::Auto),
        "preview" => Ok(ApplyMode::Preview),
        "manual" => Ok(ApplyMode::Manual),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown apply mode {other}"),
            "use --apply-mode auto|preview|manual",
        ))),
    }
}

fn parse_apply_strategy(value: &str) -> Result<ApplyStrategy> {
    match value {
        "squash" => Ok(ApplyStrategy::Squash),
        "merge" => Ok(ApplyStrategy::Merge),
        "cherry-pick" => Ok(ApplyStrategy::CherryPick),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown apply strategy {other}"),
            "use --apply-strategy squash|merge|cherry-pick",
        ))),
    }
}

fn parse_on_fail(value: &str) -> Result<OnFail> {
    match value {
        "stop" => Ok(OnFail::Stop),
        "skip" => Ok(OnFail::Skip),
        "continue" => Ok(OnFail::Continue),
        other => Err(CliError::Core(deadreckon_core::user_error(
            &format!("unknown on-fail policy {other}"),
            "use --on-fail stop|skip|continue",
        ))),
    }
}

fn chain_step_base_ref(chain: &Chain) -> Result<String> {
    match chain.branch_policy {
        BranchPolicy::Base => Ok(chain.base_sha.clone()),
        BranchPolicy::Stack | BranchPolicy::Merge => git_stdout(&chain.cwd, &["rev-parse", "HEAD"]),
    }
}

fn per_step_spend_cap(chain: &Chain, index: usize) -> Option<f64> {
    let max = chain.max_spend_usd?;
    let remaining = (max - chain.total_spend_usd).max(0.0);
    let pending = chain
        .steps
        .iter()
        .skip(index)
        .filter(|step| {
            !matches!(
                step.status,
                ChainStepStatus::Applied | ChainStepStatus::Skipped
            )
        })
        .count()
        .max(1);
    Some(remaining / pending as f64)
}

fn chain_spend_cap_hit(chain: &Chain) -> bool {
    chain
        .max_spend_usd
        .is_some_and(|max| chain.total_spend_usd >= max)
}

fn latest_applied_sha_before(chain: &Chain, index: usize) -> Option<String> {
    chain
        .steps
        .iter()
        .take(index)
        .rev()
        .find_map(|step| step.applied_sha.clone())
}

fn parse_started_run_id(output: &str) -> Option<String> {
    for line in output.lines() {
        if let Some(start) = line.find('(')
            && let Some(end) = line[start + 1..].find(')')
        {
            let value = &line[start + 1..start + 1 + end];
            if value.len() >= 16 && value.chars().all(|ch| ch.is_ascii_hexdigit()) {
                return Some(value.to_string());
            }
        }
    }
    None
}

fn allowlist_matches(pattern: &str, file: &str) -> bool {
    pattern == "*"
        || pattern == file
        || file.starts_with(pattern.trim_end_matches('*'))
        || file.starts_with(pattern.trim_end_matches('/'))
}

fn looks_like_chain_id(value: &str) -> bool {
    value.len() >= 6 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn compact_reason(reason: &str) -> String {
    reason
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim_matches('_')
        .chars()
        .take(48)
        .collect()
}

fn chain_prefix(chain_id: &str) -> String {
    chain_id.chars().take(8).collect()
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(8).collect()
}

fn branch_policy_label(value: BranchPolicy) -> &'static str {
    match value {
        BranchPolicy::Stack => "stack",
        BranchPolicy::Base => "base",
        BranchPolicy::Merge => "merge",
    }
}

fn apply_mode_label(value: ApplyMode) -> &'static str {
    match value {
        ApplyMode::Auto => "auto",
        ApplyMode::Preview => "preview",
        ApplyMode::Manual => "manual",
    }
}

fn apply_strategy_label(value: ApplyStrategy) -> &'static str {
    match value {
        ApplyStrategy::Squash => "squash",
        ApplyStrategy::Merge => "merge",
        ApplyStrategy::CherryPick => "cherry-pick",
    }
}

fn chain_apply_strategy(chain: &Chain) -> ApplyStrategy {
    if chain.branch_policy == BranchPolicy::Merge {
        ApplyStrategy::Merge
    } else {
        chain.apply_strategy
    }
}

fn on_fail_label(value: OnFail) -> &'static str {
    match value {
        OnFail::Stop => "stop",
        OnFail::Skip => "skip",
        OnFail::Continue => "continue",
    }
}

fn chain_status_label(chain: &Chain) -> &'static str {
    match chain.status {
        ChainStatus::Pending => "pending",
        ChainStatus::Running => "running",
        ChainStatus::Paused => "paused",
        ChainStatus::Completed => "completed",
        ChainStatus::Failed => "failed",
        ChainStatus::Killed => "killed",
        ChainStatus::Undone => "undone",
    }
}

fn chain_step_status_label(status: ChainStepStatus) -> &'static str {
    match status {
        ChainStepStatus::Pending => "pending",
        ChainStepStatus::Running => "running",
        ChainStepStatus::Completed => "completed",
        ChainStepStatus::Failed => "failed",
        ChainStepStatus::Skipped => "skipped",
        ChainStepStatus::Applied => "applied",
        ChainStepStatus::Undone => "undone",
    }
}

fn chain_step_dot(status: ChainStepStatus) -> &'static str {
    match status {
        ChainStepStatus::Pending => "○",
        ChainStepStatus::Running => "●",
        ChainStepStatus::Completed => "◐",
        ChainStepStatus::Failed => "✗",
        ChainStepStatus::Skipped => "↷",
        ChainStepStatus::Applied => "●",
        ChainStepStatus::Undone => "↶",
    }
}

fn print_chain_paused_footer(chain: &Chain) {
    let reason = chain.paused_reason.as_deref().unwrap_or("paused");
    println!("chain {} paused ({reason})", chain_prefix(&chain.chain_id));
    println!(
        "  try: deadreckon chain show {} --why-failed",
        chain_prefix(&chain.chain_id)
    );
    println!(
        "  try: deadreckon chain resume {}",
        chain_prefix(&chain.chain_id)
    );
    println!(
        "  try: deadreckon chain resume {} --apply-mode preview",
        chain_prefix(&chain.chain_id)
    );
    println!(
        "  try: deadreckon chain undo {}",
        chain_prefix(&chain.chain_id)
    );
}

async fn run_command(args: RunCommandArgs) -> Result<()> {
    let RunCommandArgs {
        goal,
        fresh,
        worktree,
        from,
        in_place,
        base,
        branch,
        allow_dirty,
        init_git,
        yes,
        preview,
        brief,
        max_spend,
        max_wall_seconds,
        sandbox,
        provider,
        model,
        skill,
        smoke,
        i_know_its_a_lot,
        no_confirm,
        no_hints,
        no_docs,
        doc_skill,
    } = args;
    if smoke && provider.is_some() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "--smoke selects the local scripted provider; omit --provider".to_string(),
        )));
    }
    if smoke && model.is_some() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "--smoke selects the local scripted provider; omit --model".to_string(),
        )));
    }
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    let effective_provider = if smoke {
        Some("smoke".to_string())
    } else {
        provider.clone().or(defaults.provider)
    };
    let router = if smoke {
        ProviderRouter::smoke()
    } else {
        ProviderRouter::from_config_path_with_model(
            &paths.config_path(),
            effective_provider.as_deref(),
            model.as_deref(),
        )?
    };
    let selected_route = router.selected_route_info();
    let effective_provider = selected_route
        .as_ref()
        .map(|route| route.name.clone())
        .or(effective_provider);
    let effective_max_spend = max_spend.or(defaults.max_spend).or(Some(10.0));
    let effective_max_wall_seconds = max_wall_seconds
        .or(defaults.cli_max_wall_seconds)
        .or(Some(3600.0));
    let effective_doc_skill = doc_skill
        .or(defaults.doc_skill.clone())
        .unwrap_or_else(|| "run-narrator".to_string());
    if max_spend.is_none() {
        let cap = effective_max_spend.unwrap_or(10.0);
        println!(
            "using default --max-spend ${cap:.0} (override with --max-spend or in config defaults.max_spend)"
        );
    }
    confirm_spend_cap(effective_max_spend, i_know_its_a_lot, no_confirm)?;
    let cwd = std::env::current_dir()?;
    let sandbox = sandbox
        .or(defaults.sandbox)
        .unwrap_or_else(|| "auto".to_string());
    let backend: SandboxBackend = sandbox.parse()?;
    if init_git {
        init_git_repo(&cwd)?;
    }
    let run_id = Uuid::new_v4().simple().to_string();
    let mut mode_flags = ModeFlags {
        fresh,
        worktree,
        from,
        in_place,
        i_know_its_a_lot,
    };
    let explicit_mode =
        mode_flags.fresh || mode_flags.worktree || mode_flags.from.is_some() || mode_flags.in_place;
    if !explicit_mode
        && deadreckon_core::find_git_root(&cwd)?.is_none()
        && io::stdin().is_terminal()
    {
        match prompt_non_git_mode()? {
            NonGitChoice::Init => {
                init_git_repo(&cwd)?;
                mode_flags.worktree = true;
            }
            NonGitChoice::Copy => mode_flags.from = Some(cwd.clone()),
            NonGitChoice::Cancel => {
                println!("cancelled");
                return Ok(());
            }
        }
    }
    let resolved_mode = resolve_mode(&mode_flags, &cwd, io::stdin().is_terminal())?;
    let mut codebase = match &resolved_mode {
        ResolvedMode::Worktree { source_path, .. } => prepare_worktree_record(
            &paths,
            WorktreeOptions {
                run_id: run_id.clone(),
                task_key: deadreckon_core::paths::task_key(&goal),
                source_path: source_path.clone(),
                base_ref: base,
                branch_name: branch,
                allow_dirty,
            },
        )?,
        _ => record_for_resolved_mode(resolved_mode.clone()),
    };
    if codebase.mode == CodebaseMode::Fresh {
        codebase.source_path = None;
    }
    let preview_text = run_preview(RunPreview {
        goal: &goal,
        cwd: &cwd,
        codebase: &codebase,
        provider: effective_provider.as_deref(),
        route: selected_route.as_ref(),
        sandbox: &backend.to_string(),
        max_spend: effective_max_spend,
        max_wall_seconds: effective_max_wall_seconds,
        brief,
        run_id: &run_id,
    });
    if preview {
        eprintln!("{preview_text}");
        return Ok(());
    }
    if !yes {
        if !io::stdin().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "non-interactive without --yes",
                "--yes (skip confirm) or run interactively",
            )));
        }
        eprintln!("{preview_text}");
        let answer = prompt("continue? [Y/n]: ")?;
        if matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no") {
            println!("cancelled");
            return Ok(());
        }
    }
    if codebase.mode == CodebaseMode::Worktree {
        create_worktree(&codebase)?;
    }
    let mut state = create_run(
        &paths,
        RunOptions {
            goal,
            cwd,
            sandbox: backend.to_string(),
            provider: effective_provider.clone(),
            skill_name: skill,
            max_spend_usd: effective_max_spend,
            max_wall_seconds: effective_max_wall_seconds,
            run_id: Some(run_id),
            codebase: Some(codebase.clone()),
        },
    )?;
    if let Some(source_path) = codebase
        .source_path
        .as_ref()
        .filter(|_| codebase.mode == CodebaseMode::Copy)
    {
        copy_source_to_working(source_path, &state.working_dir)?;
        deadreckon_core::write_codebase_record(&state.working_dir, &codebase)?;
    }
    print_run_started(&state, selected_route.as_ref());
    let mut lock = acquire_lock(
        &paths,
        &state.task_key,
        &state.run_id,
        &state.scope,
        "run",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    )?;
    state.child_pids = vec![std::process::id()];
    save_state(&state)?;

    state.set_phase_status(PhaseId(20), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("provider")?;
    state.set_phase_status(PhaseId(30), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("turn-loop")?;
    let outcome = run_turn_loop(
        &mut state,
        &router,
        RunLoopConfig {
            provider: effective_provider.clone(),
            max_spend_usd: effective_max_spend,
            max_wall_seconds: effective_max_wall_seconds,
            sandbox_backend: backend,
            max_turns: 12,
            from_turn: None,
            event_sender: None,
            cancellation_token: None,
            docs: RunLoopDocsConfig {
                home: paths.home().to_path_buf(),
                config_path: Some(paths.config_path()),
                doc_provider: defaults.doc_provider.clone(),
                doc_skill: effective_doc_skill,
                no_docs,
            },
        },
    )
    .await?;
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;

    let completed = outcome == RunLoopOutcome::Done;
    match outcome {
        RunLoopOutcome::Done => println!("{} {}", ui_ok("completed run"), state.run_id),
        RunLoopOutcome::PausedAtCap => println!("{} {}", ui_warn("paused run"), state.run_id),
        RunLoopOutcome::Killed => println!("{} {}", ui_warn("killed run"), state.run_id),
        RunLoopOutcome::Failed => println!("{} {}", ui_warn("failed run"), state.run_id),
    }
    print_run_locations(&state);
    if completed && completion_hints_enabled(no_hints) {
        complete_run_actions(&state, !no_confirm).await?;
    }
    Ok(())
}

#[derive(Debug, Default)]
struct ConfigDefaults {
    provider: Option<String>,
    sandbox: Option<String>,
    max_spend: Option<f64>,
    cli_max_wall_seconds: Option<f64>,
    doc_provider: Option<String>,
    doc_skill: Option<String>,
}

fn config_defaults(paths: &DeadreckonPaths) -> Result<ConfigDefaults> {
    let root = load_config_value(paths)?;
    Ok(ConfigDefaults {
        provider: get_toml_path(&root, "defaults.provider")
            .or_else(|| get_toml_path(&root, "default_provider"))
            .and_then(toml::Value::as_str)
            .map(ToString::to_string),
        sandbox: get_toml_path(&root, "defaults.sandbox")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string),
        max_spend: get_toml_path(&root, "defaults.max_spend")
            .and_then(toml::Value::as_float)
            .or_else(|| {
                get_toml_path(&root, "defaults.max_spend")
                    .and_then(toml::Value::as_integer)
                    .map(|value| value as f64)
            }),
        cli_max_wall_seconds: get_toml_path(&root, "defaults.cli_max_wall_seconds")
            .and_then(toml::Value::as_float)
            .or_else(|| {
                get_toml_path(&root, "defaults.cli_max_wall_seconds")
                    .and_then(toml::Value::as_integer)
                    .map(|value| value as f64)
            }),
        doc_provider: get_toml_path(&root, "defaults.doc_provider")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string),
        doc_skill: get_toml_path(&root, "defaults.doc_skill")
            .and_then(toml::Value::as_str)
            .map(ToString::to_string),
    })
}

fn confirm_spend_cap(
    max_spend: Option<f64>,
    i_know_its_a_lot: bool,
    no_confirm: bool,
) -> Result<()> {
    let Some(max_spend) = max_spend else {
        return Ok(());
    };
    if max_spend <= 50.0 || i_know_its_a_lot || no_confirm {
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "max spend above $50 requires --i-know-its-a-lot or --no-confirm in scripts"
                .to_string(),
        )));
    }
    print!("--max-spend is ${max_spend:.2}. Continue? [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim(), "y" | "Y" | "yes" | "YES") {
        Ok(())
    } else {
        Err(CliError::Core(DeadreckonError::InvalidInput(
            "run cancelled by spend confirmation".to_string(),
        )))
    }
}

async fn doctor_command() -> Result<()> {
    let paths = DeadreckonPaths::discover();
    println!("{}", ui_heading("deadreckon doctor"));
    println!(
        "{} source /Users/gdc/deadreckon | {} cd /Users/gdc/deadreckon",
        ui_ok("✓"),
        ui_command("try:")
    );
    println!(
        "{} home {} | {} DEADRECKON_HOME={}",
        ui_ok("✓"),
        paths.home().display(),
        ui_command("try:"),
        paths.home().display()
    );
    for backend in deadreckon_sandbox::doctor() {
        if backend.available {
            let path = backend
                .path
                .as_ref()
                .map(|path| format!(" at {}", path.display()))
                .unwrap_or_default();
            let version = backend
                .path
                .as_ref()
                .and_then(|path| command_version(path))
                .map(|version| format!(" ({version})"))
                .unwrap_or_default();
            println!(
                "{} sandbox {} found{}{} | {} deadreckon run \"goal\" --sandbox {} --preview",
                ui_ok("✓"),
                backend.backend,
                path,
                version,
                ui_command("try:"),
                backend.backend
            );
        } else {
            println!("{} sandbox {} missing", ui_warn("✗"), backend.backend);
            println!("    {} {}", ui_command("fix:"), backend.note);
        }
    }
    if paths.config_path().exists() {
        match load_config_value(&paths) {
            Ok(root) => {
                println!(
                    "{} {} present and parseable | {} deadreckon config provider",
                    ui_ok("✓"),
                    paths.config_path().display(),
                    ui_command("try:")
                );
                doctor_providers(&paths, &root).await?;
            }
            Err(err) => {
                println!(
                    "{} {} is not parseable",
                    ui_warn("✗"),
                    paths.config_path().display()
                );
                println!(
                    "    {} check TOML syntax or rerun `deadreckon init` ({err})",
                    ui_command("fix:")
                );
            }
        }
    } else {
        println!("{} {} missing", ui_warn("✗"), paths.config_path().display());
        println!("    {} deadreckon init", ui_command("fix:"));
    }
    let defaults = config_defaults(&paths).unwrap_or_default();
    if defaults.provider.is_some() || paths.config_path().exists() {
        println!(
            "{} provider defaults configured | {} deadreckon config provider",
            ui_ok("✓"),
            ui_command("try:")
        );
    } else if command_exists("claude") || command_exists("codex") {
        println!(
            "{} cli subscription provider available | {} deadreckon init --no-confirm",
            ui_ok("✓"),
            ui_command("try:")
        );
    } else {
        println!("{} no provider configured", ui_warn("✗"));
        println!(
            "    {} deadreckon init or deadreckon config set providers.anthropic.api_key <KEY>",
            ui_command("fix:")
        );
    }
    doctor_disk_and_permissions(&paths);
    doctor_os();
    doctor_subscription_binary("claude");
    doctor_subscription_binary("codex");
    Ok(())
}

async fn doctor_providers(paths: &DeadreckonPaths, root: &toml::Value) -> Result<()> {
    let Some(providers) = root.get("providers").and_then(toml::Value::as_table) else {
        println!("{} providers table missing", ui_warn("✗"));
        println!("    {} deadreckon init", ui_command("fix:"));
        return Ok(());
    };
    for (name, entry) in providers {
        let kind = entry
            .get("kind")
            .and_then(toml::Value::as_str)
            .unwrap_or(name);
        if kind.contains("cli") || name.starts_with("cli:") {
            let binary = entry
                .get("binary")
                .and_then(toml::Value::as_str)
                .unwrap_or_else(|| {
                    if name.contains("claude") {
                        "claude"
                    } else {
                        "codex"
                    }
                });
            if command_exists(binary) || PathBuf::from(binary).exists() {
                println!(
                    "{} provider {name} CLI binary {binary} found | {} deadreckon run \"goal\" --provider {name} --preview",
                    ui_ok("✓"),
                    ui_command("try:")
                );
            } else {
                println!(
                    "{} provider {name} CLI binary {binary} missing",
                    ui_warn("✗")
                );
                println!(
                    "    {} install {binary} or set providers.\"{name}\".binary",
                    ui_command("fix:")
                );
            }
        } else if provider_has_key(entry) {
            if std::env::var_os("DEADRECKON_DOCTOR_PING").is_some() {
                doctor_provider_ping(paths, name).await?;
            } else {
                println!(
                    "{} provider {name} credential present; ping skipped | {} DEADRECKON_DOCTOR_PING=1 deadreckon doctor",
                    ui_ok("✓"),
                    ui_command("try:")
                );
            }
        } else {
            println!("{} provider {name} credential missing", ui_warn("✗"));
            println!(
                "    {} deadreckon config set providers.{name}.api_key <KEY>",
                ui_command("fix:")
            );
        }
    }
    Ok(())
}

async fn doctor_provider_ping(paths: &DeadreckonPaths, name: &str) -> Result<()> {
    let router = ProviderRouter::from_config_path(&paths.config_path(), Some(name))?;
    let request = ProviderRequest {
        prompt: "Reply with OK only.".to_string(),
        max_output_tokens: 8,
        cwd: None,
        output_path: None,
        sandbox_backend: None,
        pid_file: None,
        cancellation_token: None,
    };
    match tokio::time::timeout(
        std::time::Duration::from_secs(20),
        router.complete(&request),
    )
    .await
    {
        Ok(Ok(response)) => println!(
            "{} provider {name} ping ok model {} | {} deadreckon run \"goal\" --provider {name} --preview",
            ui_ok("✓"),
            response.model,
            ui_command("try:")
        ),
        Ok(Err(err)) => {
            println!("{} provider {name} ping failed", ui_warn("✗"));
            println!(
                "    {} check credentials or set a fallback provider ({err})",
                ui_command("fix:")
            );
        }
        Err(_) => {
            println!("{} provider {name} ping timed out", ui_warn("✗"));
            println!(
                "    {} check network/provider status or unset DEADRECKON_DOCTOR_PING",
                ui_command("fix:")
            );
        }
    }
    Ok(())
}

fn provider_has_key(entry: &toml::Value) -> bool {
    entry
        .get("api_key")
        .and_then(toml::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
        || entry
            .get("api_key_env")
            .and_then(toml::Value::as_str)
            .and_then(std::env::var_os)
            .is_some()
}

fn doctor_disk_and_permissions(paths: &DeadreckonPaths) {
    if let Err(err) = fs::create_dir_all(paths.runstate_dir()) {
        println!(
            "{} runstate dir {} not writable",
            ui_warn("✗"),
            paths.runstate_dir().display()
        );
        println!(
            "    {} mkdir -p {} && chmod u+w {}",
            ui_command("fix:"),
            paths.runstate_dir().display(),
            paths.runstate_dir().display()
        );
        println!("    detail: {err}");
        return;
    }
    let probe = paths.runstate_dir().join(".doctor-write-test");
    match fs::write(&probe, b"ok").and_then(|_| fs::remove_file(&probe)) {
        Ok(()) => println!(
            "{} runstate dir {} writable | {} deadreckon run \"goal\" --preview",
            ui_ok("✓"),
            paths.runstate_dir().display(),
            ui_command("try:")
        ),
        Err(err) => {
            println!(
                "{} runstate dir {} not writable",
                ui_warn("✗"),
                paths.runstate_dir().display()
            );
            println!(
                "    {} chmod u+w {}",
                ui_command("fix:"),
                paths.runstate_dir().display()
            );
            println!("    detail: {err}");
        }
    }
    match free_kb(paths.home()) {
        Some(kb) if kb < 1_048_576 => {
            println!(
                "{} disk space low: {} MB free in {}",
                ui_warn("✗"),
                kb / 1024,
                paths.home().display()
            );
            println!(
                "    {} free at least 1 GB or set DEADRECKON_HOME to a larger disk",
                ui_command("fix:")
            );
        }
        Some(kb) => println!(
            "{} disk space {} MB free in {} | {} deadreckon status",
            ui_ok("✓"),
            kb / 1024,
            paths.home().display(),
            ui_command("try:")
        ),
        None => {
            println!(
                "{} disk space check unavailable for {}",
                ui_warn("✗"),
                paths.home().display()
            );
            println!(
                "    {} run `df -Pk {}` manually",
                ui_command("fix:"),
                paths.home().display()
            );
        }
    }
}

fn doctor_os() {
    #[cfg(target_os = "macos")]
    {
        let version = std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "{} os macOS {version} | {} sw_vers -productVersion",
            ui_ok("✓"),
            ui_command("try:")
        );
    }
    #[cfg(target_os = "linux")]
    {
        let version = std::process::Command::new("uname")
            .arg("-r")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "{} os Linux kernel {version} | {} uname -r",
            ui_ok("✓"),
            ui_command("try:")
        );
    }
}

fn doctor_subscription_binary(binary: &str) {
    if command_exists(binary) {
        let provider = if binary == "claude" {
            "cli:claude-code"
        } else {
            "cli:codex"
        };
        println!(
            "{} subscription binary {binary} {} | {} deadreckon config provider {provider}",
            ui_ok("✓"),
            command_version(std::path::Path::new(binary))
                .unwrap_or_else(|| "version unknown".to_string()),
            ui_command("try:")
        );
    } else {
        println!("{} subscription binary {binary} missing", ui_warn("✗"));
        println!(
            "    {} install {binary} or choose another provider with `deadreckon config set defaults.provider <name>`",
            ui_command("fix:")
        );
    }
}

fn command_version(path: &std::path::Path) -> Option<String> {
    std::process::Command::new(path)
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            let text = if output.stdout.is_empty() {
                String::from_utf8_lossy(&output.stderr).to_string()
            } else {
                String::from_utf8_lossy(&output.stdout).to_string()
            };
            text.lines().next().unwrap_or_default().trim().to_string()
        })
        .filter(|line| !line.is_empty())
}

fn init_git_repo(cwd: &Path) -> Result<()> {
    if deadreckon_core::find_git_root(cwd)?.is_some() {
        return Ok(());
    }
    run_git(cwd, &["init", "-b", "main"])?;
    run_git(cwd, &["config", "user.email", "deadreckon@example.invalid"])?;
    run_git(cwd, &["config", "user.name", "deadreckon"])?;
    run_git(cwd, &["add", "-A"])?;
    run_git(
        cwd,
        &[
            "commit",
            "--allow-empty",
            "-m",
            "initial commit (deadreckon init)",
        ],
    )
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<()> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(CliError::Core(DeadreckonError::InvalidInput(
            if stderr.is_empty() {
                format!("git {:?} failed", args)
            } else {
                stderr
            },
        )))
    }
}

fn git_stdout(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "git {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))))
    }
}

fn git_status(cwd: &Path, args: &[&str]) -> Result<()> {
    git_stdout(cwd, args).map(|_| ())
}

fn path_to_str(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "path is not valid UTF-8: {}",
            path.display()
        )))
    })
}

struct RunPreview<'a> {
    goal: &'a str,
    cwd: &'a Path,
    codebase: &'a CodebaseRecord,
    provider: Option<&'a str>,
    route: Option<&'a ProviderRouteInfo>,
    sandbox: &'a str,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    brief: bool,
    run_id: &'a str,
}

fn run_preview(input: RunPreview<'_>) -> String {
    let RunPreview {
        goal,
        cwd,
        codebase,
        provider,
        route,
        sandbox,
        max_spend,
        max_wall_seconds,
        brief,
        run_id,
    } = input;
    let mode = codebase.mode.to_string();
    let agent = route
        .map(|route| route.name.as_str())
        .or(provider)
        .unwrap_or("-");
    let model = route.map(|route| route.model.as_str()).unwrap_or("unknown");
    let caps = format!(
        "spend {}, wall {}",
        max_spend
            .map(|cap| format!("<= ${cap:.0}"))
            .unwrap_or_else(|| "uncapped".to_string()),
        format_wall_cap(max_wall_seconds)
    );
    if brief {
        return format!(
            "mode={} branch={} base={} wt={} provider={} model={} cap={}/{}",
            mode,
            codebase.branch_name.as_deref().unwrap_or("-"),
            codebase.base_ref.as_deref().unwrap_or("-"),
            codebase
                .worktree_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            agent,
            model,
            max_spend
                .map(|cap| format!("${cap:.0}"))
                .unwrap_or_else(|| "uncapped".to_string()),
            format_wall_cap(max_wall_seconds)
        );
    }

    let git_label = match preview_git_state(cwd) {
        Ok(Some(git)) => format!("git: clean, branch={} @ {}", git.branch, git.head_sha),
        _ => "not git".to_string(),
    };
    let mut lines = vec![
        "deadreckon: ready to run".to_string(),
        String::new(),
        format!("  goal:     {goal}"),
        format!("  source:   {} ({git_label})", cwd.display()),
        format!("  mode:     {mode}"),
    ];
    if codebase.mode == CodebaseMode::Worktree {
        lines.extend([
            format!(
                "    branch:   {}",
                codebase.branch_name.as_deref().unwrap_or("-")
            ),
            format!(
                "    base:     {} ({})",
                codebase.base_ref.as_deref().unwrap_or("-"),
                codebase
                    .base_sha
                    .as_deref()
                    .map(|sha| sha.chars().take(8).collect::<String>())
                    .unwrap_or_else(|| "-".to_string())
            ),
            format!(
                "    worktree: {}",
                codebase
                    .worktree_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string())
            ),
        ]);
    } else if let Some(source_path) = codebase.source_path.as_ref() {
        lines.push(format!("    source-copy: {}", source_path.display()));
    }
    if codebase.mode == CodebaseMode::InPlace {
        lines.push("    warning: SOURCE-IS-USER-TREE; undo uses runstate snapshots".to_string());
    }
    lines.extend([
        format!("  provider: {agent}"),
        format!("  model:    {model}"),
        format!("  sandbox:  {sandbox}"),
        format!("  caps:     {caps}"),
    ]);
    match codebase.mode {
        CodebaseMode::Worktree => {
            lines.push(format!("  on success: deadreckon apply {run_id}"));
            lines.push(format!("  on fail:    deadreckon abandon {run_id}"));
        }
        CodebaseMode::Copy | CodebaseMode::Fresh => {
            lines.push(format!(
                "  on success: deadreckon materialize {run_id} --dest <path>"
            ));
            lines.push(format!("  inspect:    deadreckon show {run_id}"));
        }
        CodebaseMode::InPlace => {
            lines.push(format!("  rollback:   deadreckon undo --run {run_id}"));
            lines.push(format!("  inspect:    deadreckon show {run_id}"));
        }
    }
    lines.join("\n")
}

fn format_wall_cap(max_wall_seconds: Option<f64>) -> String {
    let Some(seconds) = max_wall_seconds else {
        return "uncapped".to_string();
    };
    if seconds >= 3600.0 && seconds % 3600.0 == 0.0 {
        format!("{:.0}h", seconds / 3600.0)
    } else if seconds >= 60.0 && seconds % 60.0 == 0.0 {
        format!("{:.0}m", seconds / 60.0)
    } else {
        format!("{seconds:.0}s")
    }
}

fn free_kb(path: &std::path::Path) -> Option<u64> {
    let output = std::process::Command::new("df")
        .arg("-Pk")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()
}

fn prompt_provider() -> Result<String> {
    let detected = if command_exists("claude") {
        "cli:claude-code"
    } else if command_exists("codex") {
        "cli:codex"
    } else {
        "anthropic"
    };
    let answer = prompt(&format!("provider [{detected}]: "))?;
    Ok(if answer.trim().is_empty() {
        detected.to_string()
    } else {
        answer.trim().to_string()
    })
}

enum NonGitChoice {
    Init,
    Copy,
    Cancel,
}

fn prompt_non_git_mode() -> Result<NonGitChoice> {
    eprintln!("deadreckon: this is not a git repo. options:");
    eprintln!("  [1] git init for me, then run with worktree mode (recommended)");
    eprintln!("  [2] copy mode - agent works on a copy in ~/.deadreckon/runstate/...");
    eprintln!("  [3] cancel");
    let answer = prompt("choose [1]: ")?;
    Ok(match answer.trim() {
        "" | "1" => NonGitChoice::Init,
        "2" => NonGitChoice::Copy,
        "3" => NonGitChoice::Cancel,
        _ => NonGitChoice::Cancel,
    })
}

fn prompt(message: &str) -> Result<String> {
    print!("{message}");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().to_string())
}

fn init_config_text(
    provider: &str,
    api_key: Option<&str>,
    base_url: Option<&str>,
    max_spend: f64,
    sandbox: &str,
) -> String {
    let fallback = match provider {
        "cli:claude-code" => "[\"cli:claude-code\", \"cli:codex\", \"anthropic\", \"openai\"]",
        "cli:codex" => "[\"cli:codex\", \"cli:claude-code\", \"anthropic\", \"openai\"]",
        "openai" => "[\"openai\", \"anthropic\", \"cli:codex\", \"cli:claude-code\"]",
        "openai-compatible" => "[\"openai-compatible\", \"openai\", \"anthropic\"]",
        _ => "[\"anthropic\", \"openai\", \"cli:claude-code\", \"cli:codex\"]",
    };
    let mut out = format!(
        "default_provider = \"{provider}\"\nfallback = {fallback}\n\n[defaults]\nprovider = \"{provider}\"\ndoc_provider = \"{provider}\"\ndoc_skill = \"run-narrator\"\nmax_spend = {max_spend}\ncli_max_wall_seconds = 3600\nsandbox = \"{sandbox}\"\n\n"
    );
    match provider {
        "cli:claude-code" => {
            out.push_str("[providers.\"cli:claude-code\"]\nkind = \"cli-claude-code\"\nbinary = \"claude\"\nextra_args = []\n");
        }
        "cli:codex" => {
            out.push_str("[providers.\"cli:codex\"]\nkind = \"cli-codex\"\nbinary = \"codex\"\nextra_args = []\n");
        }
        "openai" => {
            out.push_str("[providers.openai]\nkind = \"open-ai\"\n");
            if let Some(key) = api_key.filter(|key| !key.is_empty()) {
                out.push_str(&format!("api_key = \"{}\"\n", escape_toml_string(key)));
            } else {
                out.push_str("api_key_env = \"OPENAI_API_KEY\"\n");
            }
        }
        "openai-compatible" => {
            out.push_str("[providers.openai-compatible]\nkind = \"open-ai-compatible\"\n");
            if let Some(url) = base_url.filter(|url| !url.is_empty()) {
                out.push_str(&format!("base_url = \"{}\"\n", escape_toml_string(url)));
            }
            if let Some(key) = api_key.filter(|key| !key.is_empty()) {
                out.push_str(&format!("api_key = \"{}\"\n", escape_toml_string(key)));
            } else {
                out.push_str("api_key_env = \"OPENAI_COMPATIBLE_API_KEY\"\n");
            }
        }
        _ => {
            out.push_str("[providers.anthropic]\nkind = \"anthropic\"\n");
            if let Some(key) = api_key.filter(|key| !key.is_empty()) {
                out.push_str(&format!("api_key = \"{}\"\n", escape_toml_string(key)));
            } else {
                out.push_str("api_key_env = \"ANTHROPIC_API_KEY\"\n");
            }
        }
    }
    out
}

fn load_config_value(paths: &DeadreckonPaths) -> Result<toml::Value> {
    match fs::read_to_string(paths.config_path()) {
        Ok(raw) => Ok(toml::from_str(&raw)?),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            Ok(toml::Value::Table(Default::default()))
        }
        Err(source) => Err(source.into()),
    }
}

fn get_toml_path<'a>(root: &'a toml::Value, key: &str) -> Option<&'a toml::Value> {
    let mut cursor = root;
    for part in key.split('.') {
        cursor = cursor.get(part)?;
    }
    Some(cursor)
}

fn set_toml_path(root: &mut toml::Value, key: &str, value: toml::Value) {
    let parts = key.split('.').collect::<Vec<_>>();
    let mut cursor = root;
    for part in &parts[..parts.len().saturating_sub(1)] {
        if !cursor.is_table() {
            *cursor = toml::Value::Table(Default::default());
        }
        let table = cursor.as_table_mut().expect("table after initialization");
        cursor = table
            .entry((*part).to_string())
            .or_insert_with(|| toml::Value::Table(Default::default()));
    }
    if !cursor.is_table() {
        *cursor = toml::Value::Table(Default::default());
    }
    let table = cursor.as_table_mut().expect("table after initialization");
    if let Some(last) = parts.last() {
        table.insert((*last).to_string(), value);
    }
}

fn parse_config_value(value: &str) -> toml::Value {
    toml::from_str::<toml::Value>(&format!("value = {value}"))
        .ok()
        .and_then(|mut doc| doc.as_table_mut().and_then(|table| table.remove("value")))
        .unwrap_or_else(|| toml::Value::String(value.to_string()))
}

fn value_to_display(value: &toml::Value) -> String {
    value
        .as_str()
        .map(ToString::to_string)
        .unwrap_or_else(|| value.to_string())
}

#[derive(Debug, Serialize, Deserialize)]
struct ParentMarker {
    schema_version: u32,
    kind: String,
    parent_run_id: String,
    parent_scope: String,
    parent_goal: String,
    parent_completed_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    materialized_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extended_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    new_goal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_turns_included: Option<u32>,
    deadreckon_version: String,
}

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .any(|dir| {
            let path = dir.join(name);
            path.is_file()
        })
}

fn materialize_command(
    run_id: String,
    dest: Option<PathBuf>,
    force: bool,
    include_manifest: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = load_cli_run(&paths, &run_id)?;
    let materialized = materialize_completed_run(&paths, &state, dest, force, include_manifest)?;
    print_materialized(&materialized);
    Ok(())
}

#[derive(Debug)]
struct MaterializedRun {
    run_id: String,
    source: PathBuf,
    dest: PathBuf,
}

fn materialize_completed_run(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    dest: Option<PathBuf>,
    force: bool,
    include_manifest: bool,
) -> Result<MaterializedRun> {
    ensure_completed_run(state, "run")?;
    if let Ok(record) = read_codebase_record(&state.working_dir) {
        match record.mode {
            CodebaseMode::Worktree => {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "materialize is for copy/fresh runs; run was worktree",
                    &format!("deadreckon apply {}", state.run_id),
                )));
            }
            CodebaseMode::InPlace => {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "materialize is not needed; run edited the source in-place",
                    "deadreckon undo",
                )));
            }
            CodebaseMode::Copy | CodebaseMode::Fresh => {}
        }
    }
    let library_dir = paths.library_dir(&state.scope, &state.run_id);
    if !library_dir.is_dir() {
        return Err(CliError::Core(DeadreckonError::NotFound(format!(
            "library missing for run {}; was promotion successful?",
            state.run_id
        ))));
    }

    let dest = absolute_dest(dest.unwrap_or_else(|| {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(run_prefix(&state.run_id))
    }))?;
    refuse_dest_inside_home(paths, &dest, "materialize")?;
    prepare_empty_dest(&dest, force)?;

    copy_tree(&library_dir, &dest)?;
    if !include_manifest {
        remove_if_exists(&dest.join("manifest.json"))?;
    }
    remove_if_exists(&dest.join(".materialized-to"))?;
    write_parent_marker(
        &dest.join(".deadreckon").join("parent.json"),
        materialized_parent_marker(state),
    )?;
    normalize_permissions(&dest)?;
    append_materialized_marker(&library_dir, &dest)?;

    Ok(MaterializedRun {
        run_id: state.run_id.clone(),
        source: library_dir,
        dest,
    })
}

fn print_materialized(materialized: &MaterializedRun) {
    println!("{} {}", ui_ok("exported run"), ui_id(&materialized.run_id));
    println!("  source: {}", materialized.source.display());
    println!("  dest:   {}", materialized.dest.display());
}

fn apply_command(
    run_id: String,
    strategy: String,
    target_branch: Option<String>,
    no_confirm: bool,
    autostash: bool,
    cleanup: bool,
    message: Option<String>,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = load_cli_run(&paths, &run_id)?;
    if state.status != RunStatus::Completed {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("run {} is {}", state.run_id, state.status),
            &format!("deadreckon resume {}", state.run_id),
        )));
    }
    let record = read_codebase_record(&state.working_dir)?;
    if record.mode != CodebaseMode::Worktree {
        return Err(CliError::Core(apply_mode_error(&state.run_id, record.mode)));
    }
    let git_root = record.source_git_root.as_ref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing source_git_root".to_string(),
        ))
    })?;
    let branch = record.branch_name.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "missing branch_name".to_string(),
        ))
    })?;
    let target =
        target_branch.unwrap_or(git_stdout(git_root, &["symbolic-ref", "--short", "HEAD"])?);
    let diff_stat = git_stdout(
        git_root,
        &["diff", "--stat", &format!("{target}..{branch}")],
    )
    .unwrap_or_default();
    if diff_stat.trim().is_empty() {
        print_already_applied(&state, branch, &target);
        finish_apply_cleanup(&state, &record, cleanup, no_confirm)?;
        return Ok(());
    }
    eprintln!("{diff_stat}");

    if !no_confirm && io::stdin().is_terminal() {
        let answer = prompt("apply these changes? [Y/n]: ")?;
        if matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no") {
            println!("cancelled");
            return Ok(());
        }
    } else if !no_confirm && !io::stdin().is_terminal() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "non-interactive apply requires --no-confirm",
            &format!("deadreckon apply {} --no-confirm", state.run_id),
        )));
    }

    let autostash = prepare_apply_autostash(git_root, &state.run_id, autostash, no_confirm)?;

    let (commit_subject, commit_body) = match message {
        Some(message) => (message, None),
        None => (
            format!(
                "{} (deadreckon run {})",
                state.goal.lines().next().unwrap_or("deadreckon run"),
                state.run_id.chars().take(8).collect::<String>()
            ),
            Some(apply_commit_body(&state)),
        ),
    };
    let full_merge_message = commit_body
        .as_ref()
        .map(|body| format!("{commit_subject}\n\n{body}"))
        .unwrap_or_else(|| commit_subject.clone());
    match strategy.as_str() {
        "merge" => git_status(
            git_root,
            &["merge", "--no-ff", branch, "-m", &full_merge_message],
        )
        .map_err(|err| apply_merge_error(&state.run_id, &autostash, err))?,
        "squash" => {
            git_status(git_root, &["merge", "--squash", branch])
                .map_err(|err| apply_merge_error(&state.run_id, &autostash, err))?;
            let staged_stat = git_stdout(git_root, &["diff", "--cached", "--stat"])?;
            if staged_stat.trim().is_empty() {
                if let Some(stash) = autostash.as_ref() {
                    restore_apply_autostash(git_root, &state.run_id, stash)?;
                }
                print_already_applied(&state, branch, &target);
                finish_apply_cleanup(&state, &record, cleanup, no_confirm)?;
                return Ok(());
            }
            if let Some(body) = commit_body.as_deref() {
                git_status(git_root, &["commit", "-m", &commit_subject, "-m", body])?;
            } else {
                git_status(git_root, &["commit", "-m", &commit_subject])?;
            }
        }
        "cherry-pick" => {
            let base = record.base_sha.as_deref().unwrap_or("HEAD");
            git_status(git_root, &["cherry-pick", &format!("{base}..{branch}")])
                .map_err(|err| apply_merge_error(&state.run_id, &autostash, err))?;
        }
        other => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "unknown apply strategy {other}"
            ))));
        }
    }
    if let Some(stash) = autostash.as_ref() {
        restore_apply_autostash(git_root, &state.run_id, stash)?;
    }
    println!(
        "{} {} onto {}",
        ui_ok("applied"),
        ui_id(&state.run_id),
        target
    );
    println!("{}", git_stdout(git_root, &["log", "-1", "--stat"])?);
    finish_apply_cleanup(&state, &record, cleanup, no_confirm)
}

fn print_already_applied(state: &deadreckon_core::PipelineState, branch: &str, target: &str) {
    println!(
        "{} {} onto {}",
        ui_ok("already applied"),
        ui_id(&state.run_id),
        target
    );
    println!("  branch: {branch}");
    println!("  reason: no file changes remain between the branch and target");
}

fn finish_apply_cleanup(
    state: &deadreckon_core::PipelineState,
    record: &CodebaseRecord,
    cleanup: bool,
    no_confirm: bool,
) -> Result<()> {
    let cleanup_now = cleanup || should_prompt_cleanup(no_confirm)?;
    if cleanup_now {
        cleanup_worktree_run(state, record, false, false)?;
    } else {
        println!(
            "{} {}",
            ui_command("next:"),
            ui_command(format!("deadreckon discard {}", run_prefix(&state.run_id)))
        );
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ApplyAutoStash {
    refname: String,
}

fn prepare_apply_autostash(
    git_root: &Path,
    run_id: &str,
    requested: bool,
    no_confirm: bool,
) -> Result<Option<ApplyAutoStash>> {
    let dirty = git_stdout(git_root, &["status", "--porcelain"])?;
    if dirty.trim().is_empty() {
        return Ok(None);
    }

    eprintln!("working tree has uncommitted changes:");
    for line in dirty.lines().take(30) {
        eprintln!("  {line}");
    }
    if dirty.lines().count() > 30 {
        eprintln!("  ...");
    }

    let mut should_stash = requested;
    if !should_stash && !no_confirm && io::stdin().is_terminal() {
        let answer = prompt("stash these changes during apply and restore after? [Y/n]: ")?;
        should_stash = !matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no");
    }

    if !should_stash {
        return Err(CliError::Core(deadreckon_core::user_error(
            "your working tree has uncommitted changes",
            &apply_dirty_hint(run_id, no_confirm),
        )));
    }

    let marker = format!(
        "deadreckon apply {} autostash {}",
        run_prefix(run_id),
        Utc::now().timestamp_millis()
    );
    git_status(git_root, &["stash", "push", "-u", "-m", &marker])?;
    let refname = find_stash_by_marker(git_root, &marker)?.ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "git stash succeeded but the new stash could not be found".to_string(),
        ))
    })?;
    eprintln!("stashed local changes as {refname}");
    Ok(Some(ApplyAutoStash { refname }))
}

fn apply_dirty_hint(run_id: &str, no_confirm: bool) -> String {
    let mut hint = format!("deadreckon apply {run_id} --autostash");
    if no_confirm {
        hint.push_str(" --no-confirm");
    }
    hint
}

fn find_stash_by_marker(git_root: &Path, marker: &str) -> Result<Option<String>> {
    let output = git_stdout(git_root, &["stash", "list", "--format=%gd%x00%s"])?;
    for line in output.lines() {
        if let Some((refname, subject)) = line.split_once('\0')
            && subject.contains(marker)
        {
            return Ok(Some(refname.to_string()));
        }
    }
    Ok(None)
}

fn restore_apply_autostash(git_root: &Path, run_id: &str, stash: &ApplyAutoStash) -> Result<()> {
    git_status(git_root, &["stash", "pop", &stash.refname]).map_err(|err| {
        CliError::Core(deadreckon_core::user_error(
            &format!("applied {run_id}, but restoring autostash produced conflicts: {err}"),
            &format!(
                "resolve conflicts, then inspect `git stash list` before dropping {}",
                stash.refname
            ),
        ))
    })
}

fn apply_merge_error(run_id: &str, autostash: &Option<ApplyAutoStash>, err: CliError) -> CliError {
    CliError::Core(deadreckon_core::user_error(
        &format!("merge produced conflicts: {err}"),
        &apply_conflict_hint(run_id, autostash),
    ))
}

fn apply_conflict_hint(run_id: &str, autostash: &Option<ApplyAutoStash>) -> String {
    let mut hint = format!("resolve, then git commit && deadreckon abandon {run_id}");
    if let Some(stash) = autostash {
        hint.push_str(&format!(
            "; restore local changes with git stash pop {}",
            stash.refname
        ));
    }
    hint
}

fn should_prompt_cleanup(no_confirm: bool) -> Result<bool> {
    if no_confirm || !io::stdin().is_terminal() {
        return Ok(false);
    }
    let answer = prompt("remove deadreckon worktree and temporary branch now? [Y/n]: ")?;
    Ok(!matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "n" | "no"
    ))
}

fn abandon_command(run_id: String, keep_branch: bool, force: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = load_cli_run(&paths, &run_id)?;
    let record = match read_codebase_record(&state.working_dir) {
        Ok(record) => record,
        Err(_) => {
            println!("nothing to abandon for run {}", state.run_id);
            return Ok(());
        }
    };
    if record.mode == CodebaseMode::InPlace {
        return Err(CliError::Core(deadreckon_core::user_error(
            "cannot abandon in-place edits",
            "deadreckon undo",
        )));
    }
    if state.status == RunStatus::Executing {
        if !force {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("run {} is executing", state.run_id),
                &format!("deadreckon kill {} --force", state.run_id),
            )));
        }
        let _ = kill_loaded_run(&paths, &mut state, true);
    }
    cleanup_worktree_run(&state, &record, keep_branch, force)
}

fn cleanup_command(
    run_id: Option<String>,
    all: bool,
    completed: bool,
    stale: bool,
    no_confirm: bool,
    force: bool,
    keep_branch: bool,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    if let Some(run_id) = run_id {
        let mut state = load_cli_run(&paths, &run_id)?;
        if state.status == RunStatus::Executing {
            if !force {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!("run {} is executing", state.run_id),
                    &format!("deadreckon cleanup {} --force", state.run_id),
                )));
            }
            let _ = kill_loaded_run(&paths, &mut state, true);
        }
        let record = read_codebase_record(&state.working_dir)?;
        cleanup_worktree_run(&state, &record, keep_branch, force)?;
        return Ok(());
    }

    let candidates = cleanup_candidates(&paths, all, completed, stale)?;
    if candidates.is_empty() {
        println!("no cleanup candidates");
        if !completed {
            println!(
                "hint: use `deadreckon cleanup --completed` to discard completed worktree runs"
            );
        }
        if !all {
            println!("hint: use `deadreckon cleanup --all` to search every project");
        }
        return Ok(());
    }

    println!("cleanup candidates:");
    for candidate in &candidates {
        println!(
            "  {:<8} {:<10} {:<16} {}",
            run_prefix(&candidate.state.run_id),
            candidate.state.status,
            candidate.reason,
            one_line(&candidate.state.goal, 72)
        );
    }
    if !no_confirm {
        if !io::stdin().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "non-interactive cleanup requires --no-confirm",
                "deadreckon cleanup --no-confirm",
            )));
        }
        let answer = prompt("clean these runs? [y/N]: ")?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("cancelled");
            return Ok(());
        }
    }

    for mut candidate in candidates {
        if candidate.state.status == RunStatus::Executing {
            let _ = kill_loaded_run(&paths, &mut candidate.state, true);
        }
        cleanup_worktree_run(&candidate.state, &candidate.record, keep_branch, force)?;
    }
    Ok(())
}

#[derive(Debug)]
struct CleanupCandidate {
    state: deadreckon_core::PipelineState,
    record: CodebaseRecord,
    reason: String,
}

fn cleanup_candidates(
    paths: &DeadreckonPaths,
    all: bool,
    include_completed: bool,
    include_stale: bool,
) -> Result<Vec<CleanupCandidate>> {
    let scope = if all { None } else { Some(current_scope()?) };
    let mut candidates = Vec::new();
    for run in list_runs(paths, scope.as_deref())? {
        let Ok(state) = load_run(paths, &run.run_id) else {
            continue;
        };
        let Ok(record) = read_codebase_record(&state.working_dir) else {
            continue;
        };
        if record.mode != CodebaseMode::Worktree {
            continue;
        }
        let abandoned = state.run_root.join("abandoned.json").exists();
        let stale = include_stale && is_stale_executing(&state);
        let completed = include_completed && state.status == RunStatus::Completed && !abandoned;
        if abandoned || stale || completed {
            let reason = if abandoned {
                "cleaned".to_string()
            } else if stale {
                "stale".to_string()
            } else {
                "completed".to_string()
            };
            candidates.push(CleanupCandidate {
                state,
                record,
                reason,
            });
        }
    }
    Ok(candidates)
}

fn cleanup_worktree_run(
    state: &deadreckon_core::PipelineState,
    record: &CodebaseRecord,
    keep_branch: bool,
    force: bool,
) -> Result<()> {
    let mut removed = Vec::new();
    if record.mode == CodebaseMode::Worktree
        && let (Some(git_root), Some(worktree)) = (
            record.source_git_root.as_ref(),
            record.worktree_path.as_ref(),
        )
    {
        if worktree.exists() {
            let mut args = vec!["worktree", "remove"];
            if force {
                args.push("--force");
            }
            args.push(path_to_str(worktree)?);
            let _ = git_status(git_root, &args);
            removed.push(worktree.display().to_string());
        }
        if !keep_branch
            && let Some(branch) = record.branch_name.as_deref()
            && git_stdout(git_root, &["rev-parse", "--verify", branch]).is_ok()
        {
            let _ = git_status(git_root, &["branch", "-D", branch]);
            removed.push(format!("branch {branch}"));
        }
    }
    write_abandoned_marker(state)?;
    println!("{} {}", ui_ok("abandoned"), ui_id(&state.run_id));
    for item in removed {
        println!("  removed: {item}");
    }
    Ok(())
}

fn apply_mode_error(run_id: &str, mode: CodebaseMode) -> DeadreckonError {
    let hint = match mode {
        CodebaseMode::Copy | CodebaseMode::Fresh => {
            format!("deadreckon materialize {run_id} --dest <path>")
        }
        CodebaseMode::InPlace => "deadreckon undo to revert if needed".to_string(),
        CodebaseMode::Worktree => format!("deadreckon apply {run_id}"),
    };
    deadreckon_core::user_error(
        &format!("apply requires worktree mode; run was {mode}"),
        &hint,
    )
}

async fn extend_command(args: ExtendCommandArgs) -> Result<()> {
    let ExtendCommandArgs {
        parent_run_id,
        new_goal,
        dest,
        max_context_turns,
        no_context,
        max_spend,
        max_wall_seconds,
        provider,
        model,
        sandbox,
        no_docs,
        doc_skill,
        post_actions,
    } = args;
    let new_goal = new_goal.trim().to_string();
    if new_goal.is_empty() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "--goal must be non-empty".to_string(),
        )));
    }

    let paths = DeadreckonPaths::discover();
    let parent = load_cli_run(&paths, &parent_run_id)?;
    if parent.status != RunStatus::Completed {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "parent {} is {}; use 'deadreckon resume' for incomplete runs",
            parent.run_id, parent.status
        ))));
    }
    let parent_codebase = read_codebase_record(&parent.working_dir).ok();
    if parent_codebase
        .as_ref()
        .is_some_and(|record| record.mode == CodebaseMode::InPlace)
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            "extend is not available for in-place runs",
            &format!(
                "deadreckon run --in-place --i-know-its-a-lot {:?}",
                new_goal
            ),
        )));
    }
    let parent_library = paths.library_dir(&parent.scope, &parent.run_id);
    if !parent_library.is_dir() {
        return Err(CliError::Core(DeadreckonError::NotFound(
            "parent library missing; cannot extend".to_string(),
        )));
    }

    let defaults = config_defaults(&paths)?;
    let effective_provider = provider.or(defaults.provider);
    let router = ProviderRouter::from_config_path_with_model(
        &paths.config_path(),
        effective_provider.as_deref(),
        model.as_deref(),
    )?;
    let selected_route = router.selected_route_info();
    let effective_provider = selected_route
        .as_ref()
        .map(|route| route.name.clone())
        .or(effective_provider);
    let effective_max_spend = max_spend.or(defaults.max_spend).or(Some(10.0));
    let effective_max_wall_seconds = max_wall_seconds
        .or(defaults.cli_max_wall_seconds)
        .or(Some(3600.0));
    let effective_doc_skill = doc_skill
        .or(defaults.doc_skill.clone())
        .unwrap_or_else(|| "run-narrator".to_string());
    let sandbox = sandbox
        .or(defaults.sandbox)
        .unwrap_or_else(|| "auto".to_string());
    let backend: SandboxBackend = sandbox.parse()?;
    let cwd = if parent.cwd.exists() {
        parent.cwd.clone()
    } else {
        std::env::current_dir()?
    };
    let context_turns = context_turns(max_context_turns, no_context);
    if let Some(parent_record) = parent_codebase
        .as_ref()
        .filter(|record| record.mode == CodebaseMode::Worktree)
    {
        return extend_worktree_command(ExtendWorktreeArgs {
            paths,
            parent,
            parent_record: parent_record.clone(),
            new_goal,
            effective_provider,
            effective_max_spend,
            effective_max_wall_seconds,
            doc_provider: defaults.doc_provider.clone(),
            doc_skill: effective_doc_skill,
            no_docs,
            backend,
            router,
            post_actions,
            context_turns,
        })
        .await;
    }
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: new_goal.clone(),
            cwd,
            sandbox: backend.to_string(),
            provider: effective_provider.clone(),
            skill_name: parent.skill_name.clone(),
            max_spend_usd: effective_max_spend,
            max_wall_seconds: effective_max_wall_seconds,
            run_id: None,
            codebase: None,
        },
    )?;
    align_extended_run_with_parent(&paths, &mut state, &parent)?;

    let mut lock = match acquire_lock(
        &paths,
        &parent.task_key,
        &state.run_id,
        &parent.scope,
        "extend",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    ) {
        Ok(lock) => lock,
        Err(error) => {
            cleanup_new_run(&state);
            return Err(error.into());
        }
    };
    deadreckon_core::state::write_current_pointer(&paths, &state)?;

    if let Some(dest) = dest {
        let dest = absolute_dest(dest)?;
        refuse_dest_inside_home(&paths, &dest, "extend")?;
        prepare_empty_dest(&dest, false)?;
        remove_if_exists(&state.working_dir)?;
        state.working_dir = dest;
    }
    seed_working_from_library(&parent_library, &state.working_dir)?;
    write_parent_marker(
        &state.working_dir.join(".deadreckon").join("parent.json"),
        extended_parent_marker(&parent, &new_goal, context_turns),
    )?;
    write_parent_history(&state, &parent, context_turns)?;
    append_trace(
        &state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: 0,
            event: "extended_from_parent".to_string(),
            latency_ms: None,
            detail: json!({
                "parent_run_id": parent.run_id.clone(),
                "parent_scope": parent.scope.clone(),
                "parent_goal": parent.goal.clone(),
                "parent_completed_at": parent.updated_at,
                "context_turns_included": context_turns,
            }),
        },
    )?;
    state.child_pids = vec![std::process::id()];
    state.updated_at = Utc::now();
    save_state(&state)?;

    state.set_phase_status(PhaseId(20), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("provider")?;
    state.set_phase_status(PhaseId(30), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("turn-loop")?;
    let outcome = run_turn_loop(
        &mut state,
        &router,
        RunLoopConfig {
            provider: effective_provider,
            max_spend_usd: effective_max_spend,
            max_wall_seconds: effective_max_wall_seconds,
            sandbox_backend: backend,
            max_turns: 12,
            from_turn: None,
            event_sender: None,
            cancellation_token: None,
            docs: RunLoopDocsConfig {
                home: paths.home().to_path_buf(),
                config_path: Some(paths.config_path()),
                doc_provider: defaults.doc_provider.clone(),
                doc_skill: effective_doc_skill,
                no_docs,
            },
        },
    )
    .await?;
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;

    let completed = outcome == RunLoopOutcome::Done;
    match outcome {
        RunLoopOutcome::Done => println!("{} {}", ui_ok("completed extended run"), state.run_id),
        RunLoopOutcome::PausedAtCap => {
            println!("{} {}", ui_warn("paused extended run"), state.run_id)
        }
        RunLoopOutcome::Killed => println!("{} {}", ui_warn("killed extended run"), state.run_id),
        RunLoopOutcome::Failed => println!("{} {}", ui_warn("failed extended run"), state.run_id),
    }
    print_run_locations(&state);
    if completed {
        append_parent_narrative_update(&parent, &state)?;
    }
    if completed && post_actions {
        Box::pin(complete_run_actions(&state, true)).await?;
    }
    Ok(())
}

struct ExtendWorktreeArgs {
    paths: DeadreckonPaths,
    parent: deadreckon_core::PipelineState,
    parent_record: CodebaseRecord,
    new_goal: String,
    effective_provider: Option<String>,
    effective_max_spend: Option<f64>,
    effective_max_wall_seconds: Option<f64>,
    doc_provider: Option<String>,
    doc_skill: String,
    no_docs: bool,
    backend: SandboxBackend,
    router: ProviderRouter,
    post_actions: bool,
    context_turns: Option<u32>,
}

async fn extend_worktree_command(args: ExtendWorktreeArgs) -> Result<()> {
    let ExtendWorktreeArgs {
        paths,
        parent,
        parent_record,
        new_goal,
        effective_provider,
        effective_max_spend,
        effective_max_wall_seconds,
        doc_provider,
        doc_skill,
        no_docs,
        backend,
        router,
        post_actions,
        context_turns,
    } = args;
    let parent_branch = parent_record.branch_name.clone().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "parent worktree record missing branch_name".to_string(),
        ))
    })?;
    let source_git_root = parent_record.source_git_root.clone().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "parent worktree record missing source_git_root".to_string(),
        ))
    })?;
    let run_id = Uuid::new_v4().simple().to_string();
    let mut codebase = prepare_worktree_record(
        &paths,
        WorktreeOptions {
            run_id: run_id.clone(),
            task_key: deadreckon_core::paths::task_key(&new_goal),
            source_path: source_git_root.clone(),
            base_ref: Some(parent_branch.clone()),
            branch_name: None,
            allow_dirty: false,
        },
    )?;
    codebase.parent_branch = Some(parent_branch);
    create_worktree(&codebase)?;
    let mut state = create_run(
        &paths,
        RunOptions {
            goal: new_goal.clone(),
            cwd: source_git_root,
            sandbox: backend.to_string(),
            provider: effective_provider.clone(),
            skill_name: parent.skill_name.clone(),
            max_spend_usd: effective_max_spend,
            max_wall_seconds: effective_max_wall_seconds,
            run_id: Some(run_id),
            codebase: Some(codebase),
        },
    )?;

    let mut lock = acquire_lock(
        &paths,
        &parent.task_key,
        &state.run_id,
        &parent.scope,
        "extend",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    )?;
    write_parent_marker(
        &state.working_dir.join(".deadreckon").join("parent.json"),
        extended_parent_marker(&parent, &new_goal, context_turns),
    )?;
    write_parent_history(&state, &parent, context_turns)?;
    append_trace(
        &state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: 0,
            event: "extended_from_parent".to_string(),
            latency_ms: None,
            detail: json!({
                "parent_run_id": parent.run_id.clone(),
                "parent_scope": parent.scope.clone(),
                "parent_goal": parent.goal.clone(),
                "parent_completed_at": parent.updated_at,
                "context_turns_included": context_turns,
                "mode": "worktree",
            }),
        },
    )?;
    state.child_pids = vec![std::process::id()];
    state.updated_at = Utc::now();
    save_state(&state)?;

    state.set_phase_status(PhaseId(20), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("provider")?;
    state.set_phase_status(PhaseId(30), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("turn-loop")?;
    let outcome = run_turn_loop(
        &mut state,
        &router,
        RunLoopConfig {
            provider: effective_provider,
            max_spend_usd: effective_max_spend,
            max_wall_seconds: effective_max_wall_seconds,
            sandbox_backend: backend,
            max_turns: 12,
            from_turn: None,
            event_sender: None,
            cancellation_token: None,
            docs: RunLoopDocsConfig {
                home: paths.home().to_path_buf(),
                config_path: Some(paths.config_path()),
                doc_provider,
                doc_skill,
                no_docs,
            },
        },
    )
    .await?;
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;

    let completed = outcome == RunLoopOutcome::Done;
    match outcome {
        RunLoopOutcome::Done => println!("completed extended run {}", state.run_id),
        RunLoopOutcome::PausedAtCap => println!("paused extended run {}", state.run_id),
        RunLoopOutcome::Killed => println!("killed extended run {}", state.run_id),
        RunLoopOutcome::Failed => println!("failed extended run {}", state.run_id),
    }
    print_run_locations(&state);
    if completed {
        append_parent_narrative_update(&parent, &state)?;
    }
    if completed && post_actions {
        Box::pin(complete_run_actions(&state, true)).await?;
    }
    Ok(())
}

fn align_extended_run_with_parent(
    paths: &DeadreckonPaths,
    state: &mut deadreckon_core::PipelineState,
    parent: &deadreckon_core::PipelineState,
) -> Result<()> {
    let old_scope = state.scope.clone();
    let old_task_key = state.task_key.clone();
    let old_pointer = paths.current_pointer_path(&old_scope, &old_task_key);
    let desired_root = paths.run_root(&parent.scope, &state.run_id);
    if state.run_root != desired_root {
        if let Some(parent_dir) = desired_root.parent() {
            fs::create_dir_all(parent_dir)?;
        }
        fs::rename(&state.run_root, &desired_root)?;
        state.run_root = desired_root;
        state.working_dir = state.run_root.join("working");
        state.scope = parent.scope.clone();
    }
    state.task_key = parent.task_key.clone();
    state.cwd = parent.cwd.clone();
    state.updated_at = Utc::now();
    let new_pointer = paths.current_pointer_path(&state.scope, &state.task_key);
    if old_pointer != new_pointer {
        remove_if_exists(&old_pointer)?;
    }
    save_state(state)?;
    Ok(())
}

fn cleanup_new_run(state: &deadreckon_core::PipelineState) {
    let _ = remove_if_exists(&state.run_root);
}

fn seed_working_from_library(library_dir: &Path, working_dir: &Path) -> Result<()> {
    copy_tree(library_dir, working_dir)?;
    remove_if_exists(&working_dir.join("manifest.json"))?;
    remove_if_exists(&working_dir.join(".materialized-to"))?;
    Ok(())
}

fn context_turns(max_context_turns: Option<u32>, no_context: bool) -> Option<u32> {
    if no_context {
        return None;
    }
    let turns = max_context_turns.unwrap_or(5);
    if turns == 0 { None } else { Some(turns) }
}

fn extended_parent_marker(
    parent: &deadreckon_core::PipelineState,
    new_goal: &str,
    context_turns: Option<u32>,
) -> ParentMarker {
    ParentMarker {
        schema_version: 1,
        kind: "extended".to_string(),
        parent_run_id: parent.run_id.clone(),
        parent_scope: parent.scope.clone(),
        parent_goal: parent.goal.clone(),
        parent_completed_at: parent.updated_at,
        materialized_at: None,
        extended_at: Some(Utc::now()),
        new_goal: Some(new_goal.to_string()),
        context_turns_included: context_turns,
        deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn write_parent_history(
    state: &deadreckon_core::PipelineState,
    parent: &deadreckon_core::PipelineState,
    context_turns: Option<u32>,
) -> Result<()> {
    let history = vec![parent_summary(parent, context_turns)];
    fs::write(
        state.run_root.join("history.json"),
        serde_json::to_vec_pretty(&history)?,
    )?;
    Ok(())
}

fn parent_summary(parent: &deadreckon_core::PipelineState, context_turns: Option<u32>) -> String {
    let spend = read_jsonl::<SpendRecord>(&parent.run_root.join("spend.jsonl")).unwrap_or_default();
    let traces =
        read_jsonl::<TraceRecord>(&parent.run_root.join("traces.jsonl")).unwrap_or_default();
    let spend_label = if spend.iter().any(|record| record.subscription)
        || parent
            .provider
            .as_deref()
            .is_some_and(|provider| provider.starts_with("cli:"))
    {
        "subscription".to_string()
    } else {
        format!("${:.6}", parent.total_spend_usd)
    };
    let acceptance = if parent.run_root.join("proofs/turn-acceptance.json").exists() {
        "dr-gate accepted"
    } else {
        "not recorded"
    };
    let mut summary = format!(
        "# Previous run summary ({})\n\n**Original goal.** {}\n**Completed.** {}\n**Total turns.** {}\n**Total spend.** {}\n**Acceptance.** {}\n",
        parent.run_id,
        parent.goal,
        parent.updated_at.to_rfc3339(),
        parent.turn,
        spend_label,
        acceptance
    );
    if let Some(max_turns) = context_turns {
        let mut recent = traces
            .iter()
            .filter(|trace| trace.turn > 0)
            .rev()
            .take(max_turns as usize)
            .map(trace_one_liner)
            .collect::<Vec<_>>();
        recent.reverse();
        summary.push_str(&format!(
            "\n## Recent activity (last {} turns)\n\n",
            max_turns
        ));
        if recent.is_empty() {
            summary.push_str("- no trace activity recorded\n");
        } else {
            for line in recent {
                summary.push_str(&format!("- {line}\n"));
            }
        }
    }
    summary
}

fn trace_one_liner(trace: &TraceRecord) -> String {
    let detail = trace
        .detail
        .get("tool_call_id")
        .and_then(Value::as_str)
        .or_else(|| trace.detail.get("summary").and_then(Value::as_str))
        .unwrap_or("");
    if detail.is_empty() {
        format!("turn {}: {}", trace.turn, trace.event)
    } else {
        format!(
            "turn {}: {} {}",
            trace.turn,
            trace.event,
            one_line(detail, 90)
        )
    }
}

fn ensure_completed_run(state: &deadreckon_core::PipelineState, label: &str) -> Result<()> {
    if state.status != RunStatus::Completed {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "{label} {} is not completed (status={}); use 'deadreckon resume' first",
            state.run_id, state.status
        ))));
    }
    Ok(())
}

fn materialized_parent_marker(state: &deadreckon_core::PipelineState) -> ParentMarker {
    ParentMarker {
        schema_version: 1,
        kind: "materialized".to_string(),
        parent_run_id: state.run_id.clone(),
        parent_scope: state.scope.clone(),
        parent_goal: state.goal.clone(),
        parent_completed_at: state.updated_at,
        materialized_at: Some(Utc::now()),
        extended_at: None,
        new_goal: None,
        context_turns_included: None,
        deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

fn write_parent_marker(path: &Path, marker: ParentMarker) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(&marker)?)?;
    Ok(())
}

fn append_materialized_marker(library_dir: &Path, dest: &Path) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(library_dir.join(".materialized-to"))?;
    writeln!(file, "{}\t{}", Utc::now().to_rfc3339(), dest.display())?;
    Ok(())
}

fn write_abandoned_marker(state: &deadreckon_core::PipelineState) -> Result<()> {
    let path = state.run_root.join("abandoned.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({
            "schema_version": 1,
            "run_id": state.run_id,
            "abandoned_at": Utc::now(),
        }))?,
    )?;
    Ok(())
}

fn prepare_empty_dest(dest: &Path, force: bool) -> Result<()> {
    if dest.exists() {
        let non_empty = !path_is_empty_dir(dest)?;
        if non_empty && !force {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "dest {} is not empty (use --force to overwrite, or pass a fresh path)",
                dest.display()
            ))));
        }
        if force {
            remove_if_exists(dest)?;
        }
    }
    fs::create_dir_all(dest)?;
    Ok(())
}

fn path_is_empty_dir(path: &Path) -> Result<bool> {
    if path.is_dir() {
        Ok(fs::read_dir(path)?.next().is_none())
    } else {
        Ok(false)
    }
}

fn default_materialize_dest(state: &deadreckon_core::PipelineState) -> PathBuf {
    state
        .cwd
        .join(state.task_key.chars().take(24).collect::<String>())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path).map_err(CliError::from),
        Ok(_) => fs::remove_file(path).map_err(CliError::from),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn absolute_dest(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn refuse_dest_inside_home(paths: &DeadreckonPaths, dest: &Path, verb: &str) -> Result<()> {
    let home = paths.home();
    if dest.starts_with(home) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "refusing to {verb} back into runstate (pick a path outside ~/.deadreckon/)"
        ))));
    }
    Ok(())
}

fn run_prefix(run_id: &str) -> String {
    run_id.chars().take(8).collect()
}

fn normalize_permissions(root: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        normalize_permissions_inner(root)?;

        fn normalize_permissions_inner(path: &Path) -> Result<()> {
            let metadata = fs::symlink_metadata(path)?;
            let mut permissions = metadata.permissions();
            if metadata.is_dir() {
                permissions.set_mode(0o755);
                fs::set_permissions(path, permissions)?;
                for entry in fs::read_dir(path)? {
                    normalize_permissions_inner(&entry?.path())?;
                }
            } else if metadata.is_file() {
                permissions.set_mode(0o644);
                fs::set_permissions(path, permissions)?;
            }
            Ok(())
        }
    }
    #[cfg(not(unix))]
    {
        let _ = root;
    }
    Ok(())
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn list_command(scope: Option<String>, all: bool, full: bool) -> Result<()> {
    // REPORT.md: Workspace Inventory & Run Queue is a local scan over durable
    // runstate, not a live daemon query.
    let paths = DeadreckonPaths::discover();
    let effective_scope = if all {
        None
    } else {
        Some(scope.unwrap_or(current_scope()?))
    };
    let runs = list_runs(&paths, effective_scope.as_deref())?;
    if runs.is_empty() {
        match effective_scope.as_deref() {
            Some(scope) => {
                println!("no runs for current project ({scope})");
                println!("hint: use `deadreckon list --all` to see every project");
            }
            None => println!("no runs"),
        }
        return Ok(());
    }
    if full {
        println!(
            "{}",
            ui_heading("RUN\tSTATUS\tSCOPE\tUPDATED\tMODE\tDOCS\tMATERIALIZED\tNEXT\tGOAL")
        );
        for run in runs {
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                run.run_id,
                run.status,
                run.scope,
                run.updated_at,
                codebase_mode_status(&paths, &run),
                docs_status(&paths, &run),
                materialized_status(&paths, &run),
                next_action_label_for_entry(&paths, &run),
                run.goal
            );
        }
        return Ok(());
    }

    let header = format!(
        "{:<8}  {:<10}  {:<7}  {:<26}  {:<10}  {:<16}  GOAL",
        "RUN", "STATUS", "AGE", "SCOPE", "MODE", "NEXT"
    );
    println!("{}", ui_heading(header));
    for run in runs {
        println!(
            "{:<8}  {:<10}  {:<7}  {:<26}  {:<10}  {:<16}  {}",
            ui_id(run_prefix(&run.run_id)),
            truncate_text(&run.status.to_string(), 10),
            relative_age(run.updated_at),
            truncate_text(&run.scope, 26),
            truncate_text(&codebase_mode_status(&paths, &run), 10),
            truncate_text(&next_action_label_for_entry(&paths, &run), 16),
            truncate_text(&one_line(&run.goal, 80), 80)
        );
    }
    println!(
        "{} run ids accept prefixes; use `{}`, `{}`, or `{}`",
        ui_muted("hint:"),
        ui_command("deadreckon status latest"),
        ui_command("deadreckon list --all"),
        ui_command("deadreckon list --full")
    );
    Ok(())
}

#[derive(Debug, Clone)]
struct LibraryEntry {
    manifest: PromotionManifest,
    path: PathBuf,
    materialized_count: usize,
}

fn library_command(command: LibraryCommand) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    match command {
        LibraryCommand::List {
            scope,
            all,
            goal,
            since,
            until,
            full,
        } => {
            let filter = LibraryFilter::new(goal, since, until)?;
            let entries =
                filter_library_entries(library_entries(&paths, scope.clone(), all)?, &filter);
            if entries.is_empty() {
                print_empty_library_hint(scope.as_deref(), all);
                return Ok(());
            }
            print_library_table(&entries, full);
        }
        LibraryCommand::Search {
            query,
            scope,
            all,
            since,
            until,
        } => {
            let filter = LibraryFilter::new(None, since, until)?;
            let needle = query.to_lowercase();
            let entries =
                filter_library_entries(library_entries(&paths, scope.clone(), all)?, &filter)
                    .into_iter()
                    .filter(|entry| library_entry_matches_query(entry, &needle))
                    .collect::<Vec<_>>();
            if entries.is_empty() {
                println!("no library artifacts matched {query:?}");
                println!(
                    "{} try `{}`",
                    ui_muted("hint:"),
                    ui_command("deadreckon library list --all")
                );
                return Ok(());
            }
            print_library_table(&entries, false);
        }
        LibraryCommand::Show { run_id, scope } => {
            let entry = resolve_library_entry(&paths, &run_id, scope)?;
            print_library_entry(&entry);
        }
    }
    Ok(())
}

struct LibraryFilter {
    goal: Option<String>,
    since: Option<DateTime<Utc>>,
    until: Option<DateTime<Utc>>,
}

impl LibraryFilter {
    fn new(goal: Option<String>, since: Option<String>, until: Option<String>) -> Result<Self> {
        Ok(Self {
            goal: goal.map(|goal| goal.to_lowercase()),
            since: parse_library_date_filter("--since", since)?,
            until: parse_library_date_filter("--until", until)?,
        })
    }
}

fn filter_library_entries(entries: Vec<LibraryEntry>, filter: &LibraryFilter) -> Vec<LibraryEntry> {
    entries
        .into_iter()
        .filter(|entry| {
            let manifest = &entry.manifest;
            filter
                .goal
                .as_ref()
                .is_none_or(|goal| manifest.goal.to_lowercase().contains(goal))
                && filter
                    .since
                    .is_none_or(|since| manifest.promoted_at >= since)
                && filter
                    .until
                    .is_none_or(|until| manifest.promoted_at <= until)
        })
        .collect()
}

fn library_entry_matches_query(entry: &LibraryEntry, needle: &str) -> bool {
    let manifest = &entry.manifest;
    manifest.run_id.to_lowercase().contains(needle)
        || manifest.scope.to_lowercase().contains(needle)
        || manifest.goal.to_lowercase().contains(needle)
        || library_docs_contain(&entry.path, needle)
}

fn library_docs_contain(path: &Path, needle: &str) -> bool {
    inventory_files(path)
        .unwrap_or_default()
        .into_iter()
        .filter(|file| {
            matches!(
                file.extension().and_then(|ext| ext.to_str()),
                Some("md" | "txt" | "json" | "jsonl" | "toml")
            )
        })
        .any(|file| {
            fs::read_to_string(file)
                .ok()
                .is_some_and(|raw| raw.to_lowercase().contains(needle))
        })
}

fn parse_library_date_filter(label: &str, value: Option<String>) -> Result<Option<DateTime<Utc>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if let Ok(date_time) = DateTime::parse_from_rfc3339(&value) {
        return Ok(Some(date_time.with_timezone(&Utc)));
    }
    if let Ok(date) = chrono::NaiveDate::parse_from_str(&value, "%Y-%m-%d") {
        let date_time = if label == "--until" {
            date.and_hms_nano_opt(23, 59, 59, 999_999_999)
        } else {
            date.and_hms_opt(0, 0, 0)
        };
        let Some(date_time) = date_time else {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "invalid {label} date {value:?}\ntry: deadreckon library list {label} 2026-05-11"
            ))));
        };
        return Ok(Some(date_time.and_utc()));
    }
    Err(CliError::Core(DeadreckonError::InvalidInput(format!(
        "invalid {label} date {value:?}\ntry: deadreckon library list {label} 2026-05-11"
    ))))
}

fn library_entries(
    paths: &DeadreckonPaths,
    scope: Option<String>,
    all: bool,
) -> Result<Vec<LibraryEntry>> {
    let mut entries = Vec::new();
    let library_root = paths.home().join("library");
    if let Some(scope) = scope {
        scan_library_scope(paths, &scope, &mut entries)?;
    } else if all {
        if library_root.exists() {
            for scope_entry in fs::read_dir(&library_root)? {
                let scope_entry = scope_entry?;
                if !scope_entry.file_type()?.is_dir() {
                    continue;
                }
                let scope_name = scope_entry.file_name().to_string_lossy().to_string();
                if scope_name.starts_with('.') {
                    continue;
                }
                scan_library_scope(paths, &scope_name, &mut entries)?;
            }
        }
    } else {
        scan_library_scope(paths, &current_scope()?, &mut entries)?;
    }
    entries.sort_by(|left, right| {
        right
            .manifest
            .promoted_at
            .cmp(&left.manifest.promoted_at)
            .then_with(|| left.manifest.run_id.cmp(&right.manifest.run_id))
    });
    Ok(entries)
}

fn scan_library_scope(
    paths: &DeadreckonPaths,
    scope: &str,
    entries: &mut Vec<LibraryEntry>,
) -> Result<()> {
    let scope_dir = paths.home().join("library").join(scope);
    if !scope_dir.exists() {
        return Ok(());
    }
    for run_entry in fs::read_dir(scope_dir)? {
        let run_entry = run_entry?;
        if !run_entry.file_type()?.is_dir() {
            continue;
        }
        let path = run_entry.path();
        let manifest_path = path.join("manifest.json");
        if !manifest_path.exists() {
            continue;
        }
        let manifest: PromotionManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
        entries.push(LibraryEntry {
            materialized_count: materialized_marker_count(&path),
            manifest,
            path,
        });
    }
    Ok(())
}

fn materialized_marker_count(library_dir: &Path) -> usize {
    fs::read_to_string(library_dir.join(".materialized-to"))
        .ok()
        .map(|raw| raw.lines().filter(|line| !line.trim().is_empty()).count())
        .unwrap_or(0)
}

fn resolve_library_entry(
    paths: &DeadreckonPaths,
    run_id: &str,
    scope: Option<String>,
) -> Result<LibraryEntry> {
    let entries = library_entries(paths, scope, false)?;
    if matches!(run_id, "latest" | "last") {
        return entries.into_iter().next().ok_or_else(|| {
            CliError::Core(DeadreckonError::NotFound(
                "no library artifacts for this project".to_string(),
            ))
        });
    }
    let matches = entries
        .into_iter()
        .filter(|entry| entry.manifest.run_id.starts_with(run_id))
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(CliError::Core(DeadreckonError::NotFound(format!(
            "library artifact {run_id}"
        )))),
        [entry] => Ok(entry.clone()),
        many => Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "run id prefix {run_id:?} matched {} library artifacts; use more characters",
            many.len()
        )))),
    }
}

fn print_empty_library_hint(scope: Option<&str>, all: bool) {
    match scope {
        Some(scope) => println!("no library artifacts for scope {scope}"),
        None if all => println!("no library artifacts"),
        None => println!("no library artifacts for current project"),
    }
    println!(
        "{} completed fresh/copy runs are promoted automatically; use `{}` to inspect all scopes",
        ui_muted("hint:"),
        ui_command("deadreckon library list --all")
    );
}

fn print_library_table(entries: &[LibraryEntry], full: bool) {
    if full {
        println!(
            "{}",
            ui_heading("RUN\tSCOPE\tPROMOTED\tMATERIALIZED\tPATH\tGOAL")
        );
        for entry in entries {
            let manifest = &entry.manifest;
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                manifest.run_id,
                manifest.scope,
                manifest.promoted_at,
                entry.materialized_count,
                entry.path.display(),
                manifest.goal
            );
        }
        return;
    }

    println!(
        "{}",
        ui_heading(format!(
            "{:<8}  {:<7}  {:<26}  {:<12}  GOAL",
            "RUN", "AGE", "SCOPE", "EXPORTED"
        ))
    );
    for entry in entries {
        let manifest = &entry.manifest;
        println!(
            "{:<8}  {:<7}  {:<26}  {:<12}  {}",
            ui_id(run_prefix(&manifest.run_id)),
            relative_age(manifest.promoted_at),
            truncate_text(&manifest.scope, 26),
            materialized_count_label(entry.materialized_count),
            truncate_text(&one_line(&manifest.goal, 88), 88)
        );
    }
    println!(
        "{} use `{}` or `{}`",
        ui_muted("hint:"),
        ui_command("deadreckon library show <run-id>"),
        ui_command("deadreckon materialize <run-id> --dest <path>")
    );
}

fn materialized_count_label(count: usize) -> String {
    match count {
        0 => "no".to_string(),
        1 => "yes (1)".to_string(),
        count => format!("yes ({count})"),
    }
}

fn print_library_entry(entry: &LibraryEntry) {
    let manifest = &entry.manifest;
    println!("{}", ui_heading("library artifact"));
    println!("run:        {}", ui_id(&manifest.run_id));
    println!("scope:      {}", manifest.scope);
    println!("promoted:   {}", manifest.promoted_at);
    println!(
        "exported:   {}",
        materialized_count_label(entry.materialized_count)
    );
    println!("path:       {}", entry.path.display());
    println!("source:     {}", manifest.source_working_dir.display());
    println!("provenance: {}", manifest.provenance_hash);
    println!("goal:       {}", manifest.goal);
    println!();
    println!(
        "next:       {}",
        ui_command(format!(
            "deadreckon materialize {}",
            run_prefix(&manifest.run_id)
        ))
    );
}

fn current_scope() -> Result<String> {
    let cwd = std::env::current_dir()?;
    workspace_scope(&cwd).map_err(CliError::from)
}

fn load_cli_run(paths: &DeadreckonPaths, run_id: &str) -> Result<deadreckon_core::PipelineState> {
    load_cli_run_with_scope(paths, run_id, false)
}

fn load_cli_run_with_scope(
    paths: &DeadreckonPaths,
    run_id: &str,
    all: bool,
) -> Result<deadreckon_core::PipelineState> {
    if matches!(run_id, "latest" | "last") {
        latest_run(paths, all)
    } else {
        load_run(paths, run_id).map_err(CliError::from)
    }
}

fn next_action_label_for_entry(
    paths: &DeadreckonPaths,
    run: &deadreckon_core::RunListEntry,
) -> String {
    load_run(paths, &run.run_id)
        .map(|state| next_action_label(&state))
        .unwrap_or_else(|_| "-".to_string())
}

fn next_action_label(state: &deadreckon_core::PipelineState) -> String {
    if state.run_root.join("abandoned.json").exists() {
        return "cleaned".to_string();
    }
    if is_stale_executing(state) {
        return "cleanup --stale".to_string();
    }
    match state.status {
        RunStatus::Pending | RunStatus::Planned | RunStatus::Executing => "attach".to_string(),
        RunStatus::Failed | RunStatus::Killed => "resume".to_string(),
        RunStatus::Completed => match read_codebase_record(&state.working_dir)
            .map(|record| record.mode)
            .unwrap_or(CodebaseMode::Fresh)
        {
            CodebaseMode::Worktree => "apply".to_string(),
            CodebaseMode::Copy | CodebaseMode::Fresh => "export".to_string(),
            CodebaseMode::InPlace => "review".to_string(),
        },
    }
}

fn relative_age(updated_at: DateTime<Utc>) -> String {
    let seconds = (Utc::now() - updated_at).num_seconds().max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 60 * 60 {
        format!("{}m", seconds / 60)
    } else if seconds < 60 * 60 * 24 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return value.chars().take(max_chars).collect();
    }
    let prefix = max_chars.saturating_sub(3);
    format!("{}...", value.chars().take(prefix).collect::<String>())
}

fn docs_status(paths: &DeadreckonPaths, run: &deadreckon_core::RunListEntry) -> String {
    load_run(paths, &run.run_id)
        .map(|state| docs_status_for_state(&state).to_string())
        .unwrap_or_else(|_| "n/a".to_string())
}

fn codebase_mode_status(paths: &DeadreckonPaths, run: &deadreckon_core::RunListEntry) -> String {
    let state = match load_run(paths, &run.run_id) {
        Ok(state) => state,
        Err(_) => return "-".to_string(),
    };
    if state.run_root.join("abandoned.json").exists() {
        return "abandoned".to_string();
    }
    read_codebase_record(&state.working_dir)
        .map(|record| record.mode.to_string())
        .unwrap_or_else(|_| "-".to_string())
}

fn materialized_status(paths: &DeadreckonPaths, run: &deadreckon_core::RunListEntry) -> String {
    if run.status != RunStatus::Completed {
        return "n/a".to_string();
    }
    let marker = paths
        .library_dir(&run.scope, &run.run_id)
        .join(".materialized-to");
    let count = fs::read_to_string(marker)
        .ok()
        .map(|raw| raw.lines().filter(|line| !line.trim().is_empty()).count())
        .unwrap_or(0);
    match count {
        0 => "no".to_string(),
        1 => "yes (1 time)".to_string(),
        count => format!("yes ({count} times)"),
    }
}

async fn doc_command(
    run_id: String,
    kind: DocKind,
    export: Option<PathBuf>,
    polish: bool,
    no_confirm: bool,
    force: bool,
    doc_skill: Option<String>,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = load_cli_run(&paths, &run_id)?;
    if polish {
        if state.status != RunStatus::Completed {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "run {} is {}; docs are not yet polished",
                    state.run_id, state.status
                ),
                &format!("deadreckon resume {} or omit --polish", state.run_id),
            )));
        }
        let defaults = config_defaults(&paths)?;
        let doc_provider = defaults
            .doc_provider
            .clone()
            .or_else(|| state.provider.clone());
        let Some(provider) = doc_provider.clone() else {
            return Err(CliError::Core(deadreckon_core::user_error(
                "no doc provider configured",
                "deadreckon init or set defaults.doc_provider",
            )));
        };
        if !no_confirm && io::stdin().is_terminal() {
            let answer = prompt("doc polish may use one provider turn; continue? [y/N]: ")?;
            if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                println!("cancelled");
                return Ok(());
            }
        } else if !no_confirm && !io::stdin().is_terminal() {
            return Err(CliError::Core(deadreckon_core::user_error(
                "non-interactive doc polish requires --no-confirm",
                &format!("deadreckon doc {} --polish --no-confirm", state.run_id),
            )));
        }
        let router = ProviderRouter::from_config_path(&paths.config_path(), Some(&provider))?;
        polish_run_docs(
            &mut state,
            &router,
            &PolishConfig {
                home: paths.home().to_path_buf(),
                doc_skill: doc_skill
                    .or(defaults.doc_skill)
                    .unwrap_or_else(|| "run-narrator".to_string()),
                doc_provider: Some(provider),
                no_llm: false,
                force,
            },
        )
        .await?;
        save_state(&state)?;
    }
    let Some(path) = doc_path_for_kind(&state.working_dir, kind) else {
        if kind == DocKind::Delta {
            return Err(CliError::Core(deadreckon_core::user_error(
                "no delta produced; this run did not affect a project AS-BUILT",
                "deadreckon doc <run-id> --kind narrative",
            )));
        }
        return Err(CliError::Core(DeadreckonError::NotFound(format!(
            "{} for run {}",
            kind.file_name(),
            state.run_id
        ))));
    };
    if let Some(dest) = export {
        if dest.exists() && !force {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("dest {} exists", dest.display()),
                "--force or pick a fresh path",
            )));
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&path, &dest)?;
        println!("exported {} to {}", kind.file_name(), dest.display());
    } else {
        print!("{}", fs::read_to_string(&path)?);
    }
    Ok(())
}

async fn attach_command(run_id: String, no_hints: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = load_cli_run(&paths, &run_id)?;
    let run_id = state.run_id.clone();
    let show_hints = completion_hints_enabled(no_hints);
    if io::stdout().is_terminal() {
        attach_tui(&paths, &run_id, show_hints).await?;
        let state = load_run(&paths, &run_id)?;
        if state.status == RunStatus::Completed && show_hints {
            print_lifecycle_hints(&state);
        }
        return Ok(());
    }
    print_run_summary(&state);
    if state.status == RunStatus::Completed && show_hints {
        print_lifecycle_hints(&state);
    }
    Ok(())
}

fn kill_command(run_id: String, force: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = load_cli_run(&paths, &run_id)?;
    kill_loaded_run(&paths, &mut state, force)?;
    if force {
        println!("killed run {} forcefully", state.run_id);
    } else {
        println!("killed run {}", state.run_id);
    }
    Ok(())
}

fn kill_loaded_run(
    paths: &DeadreckonPaths,
    state: &mut deadreckon_core::PipelineState,
    force: bool,
) -> Result<()> {
    let pids = supervised_pids(state);
    write_cancel_marker(state, "killed by user")?;
    release_lock_file(paths, &state.scope, &state.task_key)?;
    state.status = RunStatus::Killed;
    state.failure_reason = Some("killed by user".to_string());
    state.killed_at = Some(Utc::now());
    state.updated_at = Utc::now();
    save_state(state)?;
    emit_event(
        state,
        None,
        deadreckon_core::RunEventKind::RunCompleted {
            status: "killed".to_string(),
        },
    )?;
    for pid in &pids {
        if *pid != std::process::id() {
            let _ = terminate_pid(*pid, force);
        }
    }
    if force {
        return Ok(());
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    loop {
        let any_alive = pids
            .iter()
            .any(|pid| *pid != std::process::id() && deadreckon_core::pid_is_alive(*pid));
        if !any_alive || std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    for pid in &pids {
        if *pid != std::process::id() && deadreckon_core::pid_is_alive(*pid) {
            let _ = terminate_pid(*pid, true);
        }
    }
    Ok(())
}

fn supervised_pids(state: &deadreckon_core::PipelineState) -> Vec<u32> {
    let mut pids = state.child_pids.iter().copied().collect::<BTreeSet<_>>();
    let pid_dir = state.run_root.join("child-pids");
    if let Ok(entries) = std::fs::read_dir(pid_dir) {
        for entry in entries.flatten() {
            if let Ok(raw) = std::fs::read_to_string(entry.path()) {
                for line in raw.lines() {
                    if let Ok(pid) = line.trim().parse::<u32>() {
                        pids.insert(pid);
                    }
                }
            }
        }
    }
    pids.into_iter().collect()
}

fn is_stale_executing(state: &deadreckon_core::PipelineState) -> bool {
    if state.status != RunStatus::Executing {
        return false;
    }
    let pids = supervised_pids(state);
    if pids.is_empty() {
        return Utc::now()
            .signed_duration_since(state.updated_at)
            .num_minutes()
            >= 5;
    }
    !pids
        .into_iter()
        .any(|pid| pid != std::process::id() && deadreckon_core::pid_is_alive(pid))
}

async fn resume_command(
    run_id: String,
    from_turn: Option<u32>,
    max_wall_seconds: Option<f64>,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = load_cli_run(&paths, &run_id)?;
    if state.status == RunStatus::Completed {
        println!("run {} is already completed", state.run_id);
        return Ok(());
    }
    let mut lock = acquire_lock(
        &paths,
        &state.task_key,
        &state.run_id,
        &state.scope,
        "resume",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    )?;
    state.failure_reason = None;
    state.pause_reason = None;
    state.killed_at = None;
    state.status = RunStatus::Planned;
    clear_cancel_marker(&state)?;
    if let Some(max_wall_seconds) = max_wall_seconds {
        state.max_wall_seconds = Some(max_wall_seconds);
    }
    state.child_pids = vec![std::process::id()];
    state.updated_at = Utc::now();
    save_state(&state)?;
    lock.heartbeat("resume-turn-loop")?;
    let provider = state.provider.clone();
    let backend: SandboxBackend = state.sandbox.parse()?;
    let router = ProviderRouter::from_config_path(&paths.config_path(), provider.as_deref())?;
    let defaults = config_defaults(&paths)?;
    let max_spend_usd = state.max_spend_usd;
    let max_wall_seconds = state.max_wall_seconds;
    let outcome = run_turn_loop(
        &mut state,
        &router,
        RunLoopConfig {
            provider,
            max_spend_usd,
            max_wall_seconds,
            sandbox_backend: backend,
            max_turns: 12,
            from_turn,
            event_sender: None,
            cancellation_token: None,
            docs: RunLoopDocsConfig {
                home: paths.home().to_path_buf(),
                config_path: Some(paths.config_path()),
                doc_provider: defaults.doc_provider,
                doc_skill: defaults
                    .doc_skill
                    .unwrap_or_else(|| "run-narrator".to_string()),
                no_docs: false,
            },
        },
    )
    .await?;
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;
    match outcome {
        RunLoopOutcome::Done => println!("resumed run {} to completion", state.run_id),
        RunLoopOutcome::PausedAtCap => println!("resumed run {} paused at cap", state.run_id),
        RunLoopOutcome::Killed => println!("resumed run {} was killed", state.run_id),
        RunLoopOutcome::Failed => println!("resumed run {} failed", state.run_id),
    }
    Ok(())
}

fn undo_command(run: Option<String>, turn: Option<u32>) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = match run {
        Some(run_id) => load_cli_run(&paths, &run_id)?,
        None => latest_run(&paths, false)?,
    };
    let target_turn = turn.unwrap_or_else(|| state.turn.saturating_sub(1));
    let restore_state = undo_restore_state(&state)?;
    restore_snapshot(&restore_state, target_turn)?;
    append_trace(
        &state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: target_turn,
            event: "undo.restore_snapshot".to_string(),
            latency_ms: None,
            detail: json!({ "snapshot": format!("turn-{target_turn}") }),
        },
    )?;
    println!("restored run {} to turn {}", state.run_id, target_turn);
    Ok(())
}

fn undo_restore_state(
    state: &deadreckon_core::PipelineState,
) -> Result<deadreckon_core::PipelineState> {
    let Ok(record) = read_codebase_record(&state.working_dir) else {
        return Ok(state.clone());
    };
    if record.mode != CodebaseMode::InPlace {
        return Ok(state.clone());
    }
    let source_path = record.source_path.as_ref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "in-place codebase record missing source_path".to_string(),
        ))
    })?;
    let mut restore_state = state.clone();
    restore_state.working_dir = source_path.clone();
    Ok(restore_state)
}

fn show_command(run_id: String, turn: Option<u32>) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = load_cli_run(&paths, &run_id)?;
    if let Some(line) = chain_context_line_for_working(&state.working_dir)? {
        println!("{line}");
    }
    if let Some(marker) = read_parent_marker(&state.working_dir)?
        && marker.kind == "extended"
    {
        println!("Extended from {}", marker.parent_run_id);
    }
    if let Ok(record) = read_codebase_record(&state.working_dir) {
        println!("Mode {}", record.mode);
        if let Some(branch) = record.branch_name.as_deref() {
            println!("Branch {branch}");
        }
        if let Some(worktree) = record.worktree_path.as_ref() {
            println!("Worktree {}", worktree.display());
        }
        if let Some(source) = record.source_path.as_ref() {
            println!("Source {}", source.display());
        }
    }
    println!("{}", serde_json::to_string_pretty(&state)?);
    let provenance_path = state.run_root.join("provenance.jsonl");
    if provenance_path.exists() {
        println!("provenance:");
        let raw = std::fs::read_to_string(provenance_path)?;
        for line in raw.lines().filter(|line| !line.trim().is_empty()) {
            let record: ProvenanceRecord = serde_json::from_str(line)?;
            if turn.is_some_and(|turn| record.prompt_id != format!("turn-{turn}")) {
                continue;
            }
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
    }
    let traces_path = state.run_root.join("traces.jsonl");
    if traces_path.exists() {
        println!("traces:");
        let raw = std::fs::read_to_string(traces_path)?;
        for line in raw.lines().filter(|line| !line.trim().is_empty()) {
            let record: TraceRecord = serde_json::from_str(line)?;
            if turn.is_some_and(|turn| record.turn != turn) {
                continue;
            }
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
    }
    Ok(())
}

fn read_parent_marker(root: &Path) -> Result<Option<ParentMarker>> {
    let path = root.join(".deadreckon/parent.json");
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn import_command(source: String) -> Result<()> {
    // REPORT.md: Cross-Tool State Sharing (read-only import) never writes into
    // Claude Code, Codex, or Cursor state directories.
    let root = match source.as_str() {
        "claude-code" => std::env::var_os("DEADRECKON_IMPORT_CLAUDE_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/Users/gdc/.claude/projects/")),
        "codex" => std::env::var_os("DEADRECKON_IMPORT_CODEX_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/Users/gdc/.codex/sessions/")),
        "cursor" => std::env::var_os("DEADRECKON_IMPORT_CURSOR_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/Users/gdc/.cursor/chats/")),
        other => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "unknown import source {other}; expected claude-code, codex, or cursor"
            ))));
        }
    };
    let paths = DeadreckonPaths::discover();
    let run_id = normalize_import(&paths, &source, &root)?;
    let files = inventory_files(&root)?;
    println!("source {source}");
    println!("root {}", root.display());
    println!("imported {run_id}");
    println!("files {}", files.len());
    for file in files.iter().rev().take(10) {
        println!("{}", file.display());
    }
    Ok(())
}

fn normalize_import(
    paths: &DeadreckonPaths,
    source: &str,
    root: &std::path::Path,
) -> Result<String> {
    let cwd = std::env::current_dir()?;
    let scope = workspace_scope(&cwd).map_err(CliError::from)?;
    let hash_root = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .display()
        .to_string();
    let imported_id = format!(
        "imported-{:016x}",
        stable_hash(&format!("{source}:{hash_root}"))
    );
    let existing_root = paths.run_root(&scope, &imported_id);
    if existing_root.exists() {
        fs::remove_dir_all(&existing_root)?;
    }
    let mut state = create_run(
        paths,
        RunOptions {
            goal: format!("imported {source} history"),
            cwd,
            sandbox: "none".to_string(),
            provider: Some(format!("import:{source}")),
            skill_name: "default-coding".to_string(),
            max_spend_usd: None,
            max_wall_seconds: None,
            run_id: Some(imported_id.clone()),
            codebase: None,
        },
    )?;

    let imported = match source {
        "cursor" => import_cursor_rows(root)?,
        _ => import_jsonl_rows(root)?,
    };
    for (idx, row) in imported.iter().enumerate() {
        let turn = (idx + 1) as u32;
        append_trace(
            &state,
            &TraceRecord {
                timestamp: Utc::now(),
                run_id: state.run_id.clone(),
                turn,
                event: format!("import.{source}"),
                latency_ms: None,
                detail: row.clone(),
            },
        )?;
        let imported_paths = import_provenance_paths(row);
        if !imported_paths.is_empty() {
            append_provenance(
                &state,
                &ProvenanceRecord {
                    timestamp: Utc::now(),
                    prompt_id: format!("turn-{turn}"),
                    model: format!("import:{source}"),
                    tool_call_id: import_tool_call_id(row),
                    session_id: state.run_id.clone(),
                    files: imported_paths,
                },
            )?;
        }
    }
    state.turn = imported.len() as u32;
    state.status = RunStatus::Completed;
    state.updated_at = Utc::now();
    save_state(&state)?;
    Ok(imported_id)
}

fn import_jsonl_rows(root: &std::path::Path) -> Result<Vec<serde_json::Value>> {
    let mut rows = Vec::new();
    for file in inventory_files(root)? {
        if file.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        for (line_idx, line) in fs::read_to_string(&file)?.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let value = serde_json::from_str::<serde_json::Value>(line).map_err(|err| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "malformed JSONL at {}:{}: {err}",
                    file.display(),
                    line_idx + 1
                )))
            })?;
            rows.push(import_row_with_metadata(value, &file, Some(line_idx + 1)));
        }
    }
    Ok(rows)
}

fn import_cursor_rows(root: &std::path::Path) -> Result<Vec<serde_json::Value>> {
    let mut rows = Vec::new();
    for file in inventory_files(root)? {
        let extension = file
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default();
        if !matches!(extension, "sqlite" | "sqlite3" | "db") {
            continue;
        }
        let output = std::process::Command::new("sqlite3")
            .arg("-json")
            .arg(&file)
            .arg("select rowid as source_rowid, * from messages order by rowid")
            .output();
        let output = output.map_err(|err| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "sqlite3 is required to import Cursor history from {}: {err}",
                file.display()
            )))
        })?;
        if !output.status.success() {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "failed to query Cursor database {}: {}",
                file.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ))));
        }
        let mut values: Vec<serde_json::Value> =
            serde_json::from_slice(&output.stdout).map_err(|err| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "sqlite3 returned invalid JSON for {}: {err}",
                    file.display()
                )))
            })?;
        for value in &mut values {
            if let Some(object) = value.as_object_mut() {
                object.insert("source_path".to_string(), json!(file));
            }
        }
        rows.extend(values);
    }
    Ok(rows)
}

fn import_row_with_metadata(
    mut value: serde_json::Value,
    file: &Path,
    line: Option<usize>,
) -> serde_json::Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("source_path".to_string(), json!(file));
        if let Some(line) = line {
            object.insert("source_line".to_string(), json!(line));
        }
        return value;
    }
    let mut object = serde_json::Map::new();
    object.insert("value".to_string(), value);
    object.insert("source_path".to_string(), json!(file));
    if let Some(line) = line {
        object.insert("source_line".to_string(), json!(line));
    }
    serde_json::Value::Object(object)
}

fn import_tool_call_id(row: &serde_json::Value) -> String {
    row.get("tool_call_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| row.get("id").and_then(serde_json::Value::as_str))
        .unwrap_or("imported-tool")
        .to_string()
}

fn import_provenance_paths(row: &serde_json::Value) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    for key in ["path", "file"] {
        if let Some(path) = row.get(key).and_then(serde_json::Value::as_str)
            && !path.trim().is_empty()
        {
            paths.insert(PathBuf::from(path));
        }
    }
    if let Some(files) = row.get("files").and_then(serde_json::Value::as_array) {
        for file in files {
            if let Some(path) = file.as_str()
                && !path.trim().is_empty()
            {
                paths.insert(PathBuf::from(path));
            }
        }
    }
    paths.into_iter().collect()
}

fn stable_hash(value: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn latest_run(paths: &DeadreckonPaths, all: bool) -> Result<deadreckon_core::PipelineState> {
    let scope = if all { None } else { Some(current_scope()?) };
    let latest = list_runs(paths, scope.as_deref())?
        .into_iter()
        .next()
        .ok_or_else(|| {
            DeadreckonError::NotFound(match scope {
                Some(scope) => format!("latest run for current project ({scope})"),
                None => "latest run".to_string(),
            })
        })?;
    load_run(paths, &latest.run_id).map_err(CliError::from)
}

fn status_command(run_id: Option<String>, all: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = match run_id {
        Some(run_id) => load_cli_run_with_scope(&paths, &run_id, all)?,
        None => latest_run(&paths, all)?,
    };
    print_status_card(&state);
    print_lifecycle_hints(&state);
    Ok(())
}

fn print_status_card(state: &deadreckon_core::PipelineState) {
    let paths = DeadreckonPaths::discover();
    let short = run_prefix(&state.run_id);
    let phase = state
        .active_phase()
        .map(|phase| format!("{} {}", phase.id.0, phase.name))
        .unwrap_or_else(|| "-".to_string());
    let next_action = next_action_label(state);
    let stale = is_stale_executing(state);
    let supervised = supervised_pids(state);
    println!("deadreckon status");
    println!("  run:      {} ({})", short, state.run_id);
    println!("  state:    {} -> {}", state.status, next_action);
    println!("  phase:    {phase}");
    println!("  scope:    {}", state.scope);
    println!("  updated:  {} ago", relative_age(state.updated_at));
    println!("  provider: {}", state.provider.as_deref().unwrap_or("-"));
    println!("  sandbox:  {}", state.sandbox);
    println!(
        "  spend:    ${:.6} / {}",
        state.total_spend_usd,
        state
            .max_spend_usd
            .map(|cap| format!("${cap:.6}"))
            .unwrap_or_else(|| "uncapped".to_string())
    );
    println!(
        "  wall:     {:.1}s / {}",
        state.total_wall_seconds,
        format_wall_cap(state.max_wall_seconds)
    );
    println!("  goal:     {}", one_line(&state.goal, 110));
    print_run_locations(state);
    if let Ok(record) = read_codebase_record(&state.working_dir) {
        println!("  mode:     {}", record.mode);
        if let Some(branch) = record.branch_name.as_deref() {
            println!("  branch:   {branch}");
        }
        if let Some(worktree) = record.worktree_path.as_ref() {
            println!("  worktree: {}", worktree.display());
        }
    }
    println!();
    println!("{}", ui_heading("run health"));
    println!("  next:     {next_action}");
    println!("  stale:    {}", if stale { "yes" } else { "no" });
    println!("  pids:     {}", supervised.len());
    if let Some(reason) = state.pause_reason.as_deref() {
        println!("  paused:   {}", one_line(reason, 100));
    }
    if let Some(reason) = state.failure_reason.as_deref() {
        println!("  failure:  {}", one_line(reason, 100));
    }
    println!("  docs:     {}", docs_status_for_state(state));

    println!();
    println!("{}", ui_heading("library"));
    let library_dir = paths.library_dir(&state.scope, &state.run_id);
    let manifest_present = library_dir.join("manifest.json").exists();
    let artifact_count = library_entries(&paths, Some(state.scope.clone()), false)
        .map(|entries| entries.len())
        .unwrap_or(0);
    println!("  scope artifacts: {artifact_count}");
    println!(
        "  current:  {}",
        if manifest_present {
            library_dir.display().to_string()
        } else {
            "not promoted".to_string()
        }
    );
    println!(
        "  exported: {}",
        materialized_count_label(materialized_marker_count(&library_dir))
    );

    println!();
    println!("{}", ui_heading("disk"));
    match free_kb(paths.home()) {
        Some(kb) => {
            let mb = kb / 1024;
            println!("  home:     {} MB free in {}", mb, paths.home().display());
            if mb > 10_240 {
                println!(
                    "  tip:      {}",
                    ui_command("deadreckon cleanup --completed")
                );
            } else {
                println!("  warning:  low disk; clean old worktrees and artifacts soon");
            }
        }
        None => println!("  home:     unavailable; run deadreckon doctor"),
    }
}

fn print_run_summary(state: &deadreckon_core::PipelineState) {
    if let Some(line) = chain_context_line_for_working(&state.working_dir)
        .ok()
        .flatten()
    {
        println!("{line}");
    }
    println!("{} {}", ui_heading("run"), ui_id(&state.run_id));
    println!("status {}", state.status);
    println!("goal {}", state.goal);
    print_run_locations(state);
    println!("spend {:.6}", state.total_spend_usd);
    if let Some(phase) = state.active_phase() {
        println!("phase {} {}", phase.id.0, phase.name);
    }
}

fn print_run_locations(state: &deadreckon_core::PipelineState) {
    println!("state {}", state.state_path().display());
    println!("launch-dir {}", state.cwd.display());
    if let Some(library_dir) = state.promoted_library_dir.as_ref() {
        println!("artifact {}", library_dir.display());
        println!("note launch-dir is unchanged; completed output lives in artifact");
    } else {
        println!("working {}", state.working_dir.display());
    }
}

fn chain_context_line_for_working(working_dir: &Path) -> Result<Option<String>> {
    let Some(marker) = read_chain_step_marker(working_dir)? else {
        return Ok(None);
    };
    let paths = DeadreckonPaths::discover();
    let chain = load_chain(&paths, &marker.chain_id).ok();
    let total_steps = chain.as_ref().map(|chain| chain.steps.len()).unwrap_or(0);
    let policy = chain
        .as_ref()
        .map(|chain| branch_policy_label(chain.branch_policy))
        .unwrap_or("unknown");
    let apply = chain
        .as_ref()
        .map(|chain| apply_mode_label(chain.apply_mode))
        .unwrap_or("unknown");
    let prior = marker
        .prior_applied_sha
        .as_deref()
        .map(short_sha)
        .unwrap_or_else(|| "none".to_string());
    Ok(Some(format!(
        "chain {} · step {}/{} · policy: {} | apply={} · prev: {}",
        chain_prefix(&marker.chain_id),
        marker.step_index + 1,
        total_steps,
        policy,
        apply,
        prior
    )))
}

fn print_run_started(state: &deadreckon_core::PipelineState, route: Option<&ProviderRouteInfo>) {
    println!(
        "{} {}",
        ui_ok("started run"),
        ui_id(format!("{} ({})", run_prefix(&state.run_id), state.run_id))
    );
    if let Some(route) = route {
        println!("provider {}", route.name);
        println!("model {}", route.model);
    } else if let Some(provider) = state.provider.as_deref() {
        println!("provider {provider}");
    }
    println!(
        "{} {}",
        ui_command("attach:"),
        ui_command(format!("deadreckon attach {}", run_prefix(&state.run_id)))
    );
    println!("state {}", state.state_path().display());
}

fn print_lifecycle_hints(state: &deadreckon_core::PipelineState) {
    if let Ok(record) = read_codebase_record(&state.working_dir)
        && record.mode == CodebaseMode::Worktree
    {
        println!("{}", ui_heading("next actions:"));
        if let Some(worktree) = record.worktree_path.as_ref() {
            println!("  inspect: cd {} && git status", worktree.display());
        }
        println!(
            "  apply:   {}",
            ui_command(format!("deadreckon apply {}", run_prefix(&state.run_id)))
        );
        println!(
            "  cleanup: {}",
            ui_command(format!(
                "deadreckon apply {} --autostash --cleanup",
                run_prefix(&state.run_id)
            ))
        );
        println!(
            "  discard: {}",
            ui_command(format!("deadreckon discard {}", run_prefix(&state.run_id)))
        );
        println!(
            "  docs:    {}",
            ui_command(format!("deadreckon doc {}", run_prefix(&state.run_id)))
        );
        return;
    }
    let task_prefix = state.task_key.chars().take(24).collect::<String>();
    println!("{}", ui_heading("next actions:"));
    println!(
        "  export: {}",
        ui_command(format!(
            "deadreckon export {} --dest ./{}",
            run_prefix(&state.run_id),
            task_prefix
        ))
    );
    println!(
        "  extend: {}",
        ui_command(format!(
            "deadreckon extend {} '<your follow-up goal>'",
            run_prefix(&state.run_id)
        ))
    );
    println!(
        "  show:   {}",
        ui_command(format!("deadreckon show {}", run_prefix(&state.run_id)))
    );
    println!(
        "  docs:   {}",
        ui_command(format!("deadreckon doc {}", run_prefix(&state.run_id)))
    );
}

async fn complete_run_actions(
    state: &deadreckon_core::PipelineState,
    allow_prompt: bool,
) -> Result<()> {
    print_lifecycle_hints(state);
    if allow_prompt && io::stdin().is_terminal() && io::stdout().is_terminal() {
        completion_action_loop(state).await?;
    }
    Ok(())
}

fn completion_hints_enabled(no_hints: bool) -> bool {
    if no_hints {
        return false;
    }
    !std::env::var("DEADRECKON_HINTS").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    })
}

async fn completion_action_loop(state: &deadreckon_core::PipelineState) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    loop {
        let prompt_text = if is_worktree_run(state) {
            "completed action [a apply, b abandon, d docs, s show, q quit]: "
        } else {
            "completed action [m materialize, e extend, d docs, s show, q quit]: "
        };
        let answer = prompt(prompt_text)?;
        match completion_action_from_input(&answer) {
            Some(CompletionAction::Materialize) => prompt_materialize_action(&paths, state)?,
            Some(CompletionAction::Extend) => prompt_extend_action(state).await?,
            Some(CompletionAction::Apply) => apply_command(
                state.run_id.clone(),
                "squash".to_string(),
                None,
                false,
                false,
                false,
                None,
            )?,
            Some(CompletionAction::Abandon) => abandon_command(state.run_id.clone(), false, false)?,
            Some(CompletionAction::Docs) => {
                Box::pin(doc_command(
                    state.run_id.clone(),
                    DocKind::Narrative,
                    None,
                    false,
                    true,
                    false,
                    None,
                ))
                .await?
            }
            Some(CompletionAction::Show) => show_command(state.run_id.clone(), None)?,
            Some(CompletionAction::Quit) | None => break,
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionAction {
    Materialize,
    Extend,
    Apply,
    Abandon,
    Docs,
    Show,
    Quit,
}

fn completion_action_from_input(input: &str) -> Option<CompletionAction> {
    match input.trim().to_ascii_lowercase().as_str() {
        "m" | "materialize" => Some(CompletionAction::Materialize),
        "e" | "extend" => Some(CompletionAction::Extend),
        "a" | "apply" => Some(CompletionAction::Apply),
        "b" | "abandon" => Some(CompletionAction::Abandon),
        "d" | "doc" | "docs" => Some(CompletionAction::Docs),
        "s" | "show" => Some(CompletionAction::Show),
        "q" | "quit" | "" => Some(CompletionAction::Quit),
        _ => None,
    }
}

fn prompt_materialize_action(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
) -> Result<()> {
    let default_dest = default_materialize_dest(state);
    let answer = prompt(&format!("materialize dest [{}]: ", default_dest.display()))?;
    let dest = if answer.trim().is_empty() {
        default_dest
    } else {
        PathBuf::from(answer.trim())
    };
    let dest = absolute_dest(dest)?;
    let force = if dest.exists() && !path_is_empty_dir(&dest)? {
        let overwrite = prompt("destination is not empty; overwrite? [y/N]: ")?;
        matches!(overwrite.trim().to_ascii_lowercase().as_str(), "y" | "yes")
    } else {
        false
    };
    let materialized = materialize_completed_run(paths, state, Some(dest), force, false)?;
    print_materialized(&materialized);
    Ok(())
}

async fn prompt_extend_action(state: &deadreckon_core::PipelineState) -> Result<()> {
    let goal = prompt("follow-up goal: ")?;
    if goal.trim().is_empty() {
        println!("extend skipped; follow-up goal was empty");
        return Ok(());
    }
    let dest = prompt("extension working dest [runstate working dir]: ")?;
    let dest = if dest.trim().is_empty() {
        None
    } else {
        Some(PathBuf::from(dest.trim()))
    };
    extend_command(ExtendCommandArgs {
        parent_run_id: state.run_id.clone(),
        new_goal: goal,
        dest,
        max_context_turns: None,
        no_context: false,
        max_spend: state.max_spend_usd,
        max_wall_seconds: state.max_wall_seconds,
        provider: state.provider.clone(),
        model: None,
        sandbox: Some(state.sandbox.clone()),
        no_docs: false,
        doc_skill: None,
        post_actions: false,
    })
    .await
}

fn is_worktree_run(state: &deadreckon_core::PipelineState) -> bool {
    read_codebase_record(&state.working_dir)
        .map(|record| record.mode == CodebaseMode::Worktree)
        .unwrap_or(false)
}

async fn attach_tui(
    paths: &DeadreckonPaths,
    run_id: &str,
    show_completion_actions: bool,
) -> Result<()> {
    let initial_state = load_run(paths, run_id)?;
    let mut event_feed =
        tui_events::TuiEventFeed::file_tail(initial_state.run_root.join(RUN_EVENTS_JSONL));
    let mut events = event_feed.refresh(std::time::Duration::ZERO).await?;
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut tui_state = AttachTuiState {
        show_completion_actions,
        ..AttachTuiState::default()
    };

    let result = loop {
        let state = load_run(paths, run_id)?;
        let spend = read_jsonl::<SpendRecord>(&state.run_root.join("spend.jsonl"))?;
        let traces = read_jsonl::<TraceRecord>(&state.run_root.join("traces.jsonl"))?;
        events.extend(event_feed.refresh(std::time::Duration::ZERO).await?);
        let live = collect_attach_live(&state);
        let terminal_size = terminal.size()?;
        let terminal_area =
            ratatui::layout::Rect::new(0, 0, terminal_size.width, terminal_size.height);
        let panel_layout = attach_panel_layout(terminal_area);
        let panel_counts = attach_panel_counts(&state, &spend, &traces, &events, &live, &tui_state);
        tui_state.clamp(panel_counts, panel_layout.rows);
        terminal.draw(|frame| {
            render_attach(frame, &state, &spend, &traces, &events, &live, &tui_state)
        })?;

        if event::poll(std::time::Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) if attach_should_quit(key) => break Ok(()),
                Event::Key(key)
                    if tui_state.show_completion_actions
                        && state.status == RunStatus::Completed =>
                {
                    if key.code == KeyCode::Char('d') && key.modifiers.is_empty() {
                        tui_state.toggle_docs();
                    } else if !handle_tui_completion_key(&mut terminal, paths, &state, key).await? {
                        tui_state.handle_key(key, panel_counts, panel_layout.rows);
                    }
                }
                Event::Key(key) => tui_state.handle_key(key, panel_counts, panel_layout.rows),
                Event::Mouse(mouse) => {
                    if let Some(panel) = panel_layout.panel_at(mouse.column, mouse.row) {
                        tui_state.focused_panel = panel;
                    }
                    match mouse.kind {
                        MouseEventKind::ScrollDown => {
                            tui_state.scroll_focused(3, panel_counts, panel_layout.rows)
                        }
                        MouseEventKind::ScrollUp => {
                            tui_state.scroll_focused(-3, panel_counts, panel_layout.rows)
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

async fn handle_tui_completion_key(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    key: KeyEvent,
) -> Result<bool> {
    let action = match key.code {
        KeyCode::Char('m') => CompletionAction::Materialize,
        KeyCode::Char('e') => CompletionAction::Extend,
        KeyCode::Char('a') => CompletionAction::Apply,
        KeyCode::Char('b') => CompletionAction::Abandon,
        KeyCode::Char('d') => CompletionAction::Docs,
        KeyCode::Char('s') => CompletionAction::Show,
        _ => return Ok(false),
    };

    suspend_tui(terminal)?;
    let action_result = match action {
        CompletionAction::Materialize => prompt_materialize_action(paths, state),
        CompletionAction::Extend => prompt_extend_action(state).await,
        CompletionAction::Apply => apply_command(
            state.run_id.clone(),
            "squash".to_string(),
            None,
            false,
            false,
            false,
            None,
        ),
        CompletionAction::Abandon => abandon_command(state.run_id.clone(), false, false),
        CompletionAction::Docs => {
            Box::pin(doc_command(
                state.run_id.clone(),
                DocKind::Narrative,
                None,
                false,
                true,
                false,
                None,
            ))
            .await
        }
        CompletionAction::Show => show_command(state.run_id.clone(), None),
        CompletionAction::Quit => Ok(()),
    };
    if let Err(err) = &action_result {
        eprintln!("error: {err}");
        if let Some(hint) = error_hint(err) {
            eprintln!("  hint: {hint}");
        }
    }
    let _ = prompt("press Enter to return to attach...");
    resume_tui(terminal)?;
    Ok(true)
}

fn suspend_tui(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn resume_tui(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    Ok(())
}

fn attach_should_quit(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        || (key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttachPanel {
    Activity,
    Files,
    Processes,
}

impl AttachPanel {
    fn next(self) -> Self {
        match self {
            Self::Activity => Self::Files,
            Self::Files => Self::Processes,
            Self::Processes => Self::Activity,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Activity => Self::Processes,
            Self::Files => Self::Activity,
            Self::Processes => Self::Files,
        }
    }
}

#[derive(Debug)]
struct AttachTuiState {
    focused_panel: AttachPanel,
    activity_scroll: usize,
    docs_scroll: usize,
    docs_open: bool,
    files_scroll: usize,
    processes_scroll: usize,
    show_completion_actions: bool,
}

impl Default for AttachTuiState {
    fn default() -> Self {
        Self {
            focused_panel: AttachPanel::Activity,
            activity_scroll: 0,
            docs_scroll: 0,
            docs_open: false,
            files_scroll: 0,
            processes_scroll: 0,
            show_completion_actions: true,
        }
    }
}

impl AttachTuiState {
    fn handle_key(&mut self, key: KeyEvent, counts: AttachPanelCounts, rows: AttachPanelRows) {
        match key.code {
            KeyCode::Tab => self.focused_panel = self.focused_panel.next(),
            KeyCode::BackTab => self.focused_panel = self.focused_panel.previous(),
            KeyCode::Up | KeyCode::Char('k') => self.scroll_focused(-1, counts, rows),
            KeyCode::Down | KeyCode::Char('j') => self.scroll_focused(1, counts, rows),
            KeyCode::PageUp => {
                self.scroll_focused(-page_delta(self.focused_panel, rows), counts, rows)
            }
            KeyCode::PageDown => {
                self.scroll_focused(page_delta(self.focused_panel, rows), counts, rows)
            }
            KeyCode::Home | KeyCode::Char('g') => self.set_focused_scroll(0),
            KeyCode::End | KeyCode::Char('G') => {
                self.set_focused_scroll(max_panel_scroll(self.focused_panel, counts, rows))
            }
            _ => {}
        }
        self.clamp(counts, rows);
    }

    fn toggle_docs(&mut self) {
        self.docs_open = !self.docs_open;
        self.focused_panel = AttachPanel::Activity;
    }

    fn scroll_focused(&mut self, delta: isize, counts: AttachPanelCounts, rows: AttachPanelRows) {
        let current = self.focused_scroll();
        let max = max_panel_scroll(self.focused_panel, counts, rows);
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            current.saturating_add(delta as usize)
        };
        self.set_focused_scroll(next.min(max));
    }

    fn clamp(&mut self, counts: AttachPanelCounts, rows: AttachPanelRows) {
        self.activity_scroll =
            self.activity_scroll
                .min(max_panel_scroll(AttachPanel::Activity, counts, rows));
        self.docs_scroll =
            self.docs_scroll
                .min(max_panel_scroll(AttachPanel::Activity, counts, rows));
        self.files_scroll =
            self.files_scroll
                .min(max_panel_scroll(AttachPanel::Files, counts, rows));
        self.processes_scroll =
            self.processes_scroll
                .min(max_panel_scroll(AttachPanel::Processes, counts, rows));
    }

    fn focused_scroll(&self) -> usize {
        match self.focused_panel {
            AttachPanel::Activity if self.docs_open => self.docs_scroll,
            AttachPanel::Activity => self.activity_scroll,
            AttachPanel::Files => self.files_scroll,
            AttachPanel::Processes => self.processes_scroll,
        }
    }

    fn set_focused_scroll(&mut self, offset: usize) {
        match self.focused_panel {
            AttachPanel::Activity if self.docs_open => self.docs_scroll = offset,
            AttachPanel::Activity => self.activity_scroll = offset,
            AttachPanel::Files => self.files_scroll = offset,
            AttachPanel::Processes => self.processes_scroll = offset,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct AttachPanelCounts {
    activity: usize,
    files: usize,
    processes: usize,
}

#[derive(Debug, Clone, Copy)]
struct AttachPanelRows {
    activity: usize,
    files: usize,
    processes: usize,
}

#[derive(Debug, Clone, Copy)]
struct AttachPanelLayout {
    header: ratatui::layout::Rect,
    activity: ratatui::layout::Rect,
    files: ratatui::layout::Rect,
    processes: ratatui::layout::Rect,
    footer: ratatui::layout::Rect,
    rows: AttachPanelRows,
}

impl AttachPanelLayout {
    fn panel_at(&self, column: u16, row: u16) -> Option<AttachPanel> {
        if rect_contains(self.activity, column, row) {
            Some(AttachPanel::Activity)
        } else if rect_contains(self.files, column, row) {
            Some(AttachPanel::Files)
        } else if rect_contains(self.processes, column, row) {
            Some(AttachPanel::Processes)
        } else {
            None
        }
    }
}

fn attach_panel_layout(area: ratatui::layout::Rect) -> AttachPanelLayout {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(10),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(area);
    let center = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(60), Constraint::Length(44)])
        .split(vertical[1]);
    AttachPanelLayout {
        header: vertical[0],
        activity: center[0],
        files: center[1],
        processes: vertical[2],
        footer: vertical[3],
        rows: AttachPanelRows {
            activity: panel_inner_rows(center[0]),
            files: panel_inner_rows(center[1]),
            processes: panel_inner_rows(vertical[2]),
        },
    }
}

fn attach_panel_counts(
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
    live: &AttachLive,
    tui_state: &AttachTuiState,
) -> AttachPanelCounts {
    AttachPanelCounts {
        activity: if tui_state.docs_open && state.status == RunStatus::Completed {
            render_markdown_doc_lines(state).len()
        } else {
            attach_activity_lines(state, spend, traces, events, live).len()
        },
        files: live_file_lines(live).len(),
        processes: process_lines(live).len(),
    }
}

fn panel_inner_rows(area: ratatui::layout::Rect) -> usize {
    area.height.saturating_sub(2) as usize
}

fn rect_contains(rect: ratatui::layout::Rect, column: u16, row: u16) -> bool {
    column >= rect.x
        && column < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

fn page_delta(panel: AttachPanel, rows: AttachPanelRows) -> isize {
    let rows = panel_rows(panel, rows).max(1);
    rows.saturating_sub(1).max(1) as isize
}

fn max_panel_scroll(panel: AttachPanel, counts: AttachPanelCounts, rows: AttachPanelRows) -> usize {
    panel_count(panel, counts).saturating_sub(panel_rows(panel, rows))
}

fn panel_count(panel: AttachPanel, counts: AttachPanelCounts) -> usize {
    match panel {
        AttachPanel::Activity => counts.activity,
        AttachPanel::Files => counts.files,
        AttachPanel::Processes => counts.processes,
    }
}

fn panel_rows(panel: AttachPanel, rows: AttachPanelRows) -> usize {
    match panel {
        AttachPanel::Activity => rows.activity,
        AttachPanel::Files => rows.files,
        AttachPanel::Processes => rows.processes,
    }
}

#[derive(Debug, Default)]
struct AttachLive {
    file_count: usize,
    total_bytes: u64,
    files: Vec<LiveFile>,
    pids: Vec<LivePid>,
    provider_activity: Vec<String>,
    provider_context_tokens: Option<u64>,
    provider_context_window: Option<u64>,
}

#[derive(Debug)]
struct LiveFile {
    path: String,
    bytes: u64,
    modified_at: Option<DateTime<Utc>>,
}

#[derive(Debug)]
struct LivePid {
    pid: u32,
    alive: bool,
    command: String,
}

fn collect_attach_live(state: &deadreckon_core::PipelineState) -> AttachLive {
    let mut files = inventory_files(&state.working_dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|path| {
            !path_has_component(path, "node_modules") && !path_has_component(path, ".git")
        })
        .filter_map(|path| live_file(&state.working_dir, path))
        .collect::<Vec<_>>();
    files.sort_by(|left, right| right.modified_at.cmp(&left.modified_at));
    let file_count = files.len();
    let total_bytes = files.iter().map(|file| file.bytes).sum();
    let pids = supervised_pids(state)
        .into_iter()
        .map(live_pid)
        .collect::<Vec<_>>();
    let provider_activity = collect_provider_activity(state);
    AttachLive {
        file_count,
        total_bytes,
        files,
        pids,
        provider_context_tokens: provider_activity.context_tokens,
        provider_context_window: provider_activity.context_window,
        provider_activity: provider_activity.lines,
    }
}

fn live_file(root: &Path, path: PathBuf) -> Option<LiveFile> {
    let metadata = fs::metadata(&path).ok()?;
    let relative = path.strip_prefix(root).unwrap_or(&path);
    Some(LiveFile {
        path: relative.display().to_string(),
        bytes: metadata.len(),
        modified_at: metadata.modified().ok().map(DateTime::<Utc>::from),
    })
}

fn path_has_component(path: &Path, component: &str) -> bool {
    path.components()
        .any(|part| part.as_os_str().to_string_lossy() == component)
}

fn live_pid(pid: u32) -> LivePid {
    let alive = deadreckon_core::pid_is_alive(pid);
    let command = if alive {
        std::process::Command::new("ps")
            .args(["-o", "stat=,etime=,command=", "-p", &pid.to_string()])
            .output()
            .ok()
            .and_then(|output| {
                if output.status.success() {
                    String::from_utf8(output.stdout).ok()
                } else {
                    None
                }
            })
            .map(|raw| one_line(raw.trim(), 110))
            .filter(|line| !line.is_empty())
            .unwrap_or_else(|| "alive".to_string())
    } else {
        "not running".to_string()
    };
    LivePid {
        pid,
        alive,
        command,
    }
}

#[derive(Debug, Default)]
struct ProviderActivity {
    lines: Vec<String>,
    context_tokens: Option<u64>,
    context_window: Option<u64>,
}

fn collect_provider_activity(state: &deadreckon_core::PipelineState) -> ProviderActivity {
    if state.provider.as_deref() != Some("cli:codex") {
        return ProviderActivity::default();
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return ProviderActivity::default();
    };
    let sessions_root = home.join(".codex/sessions");
    let since = state.started_at - ChronoDuration::minutes(2);
    let mut candidates = Vec::new();
    collect_recent_jsonl_files(&sessions_root, since, &mut candidates, 0);
    candidates.sort_by(|left, right| right.1.cmp(&left.1));
    let working_dirs = [
        state.working_dir.to_string_lossy().to_string(),
        state.run_root.join("working").to_string_lossy().to_string(),
    ];
    for (path, _) in candidates {
        if !codex_session_matches_run(&path, &working_dirs) {
            continue;
        }
        let Ok(raw) = fs::read_to_string(&path) else {
            continue;
        };
        let mut activity = ProviderActivity::default();
        for line in raw.lines() {
            if let Some(line) = codex_activity_line(line, &mut activity) {
                activity.lines.push(line);
            }
        }
        if activity.lines.is_empty() {
            continue;
        }
        activity.lines.push(format!(
            "{} provider log {}",
            format_age(
                fs::metadata(&path)
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .map(DateTime::<Utc>::from),
            ),
            path.display()
        ));
        activity.lines = activity
            .lines
            .into_iter()
            .rev()
            .take(240)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return activity;
    }
    ProviderActivity::default()
}

fn codex_session_matches_run(path: &Path, working_dirs: &[String]) -> bool {
    let Ok(file) = fs::File::open(path) else {
        return false;
    };
    let reader = io::BufReader::new(file);
    for line in reader.lines().map_while(std::result::Result::ok).take(8) {
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) != Some("session_meta") {
            continue;
        }
        let Some(cwd) = value
            .get("payload")
            .and_then(|payload| payload.get("cwd"))
            .and_then(Value::as_str)
        else {
            return false;
        };
        return working_dirs.iter().any(|working_dir| working_dir == cwd);
    }
    false
}

fn collect_recent_jsonl_files(
    root: &Path,
    since: DateTime<Utc>,
    files: &mut Vec<(PathBuf, DateTime<Utc>)>,
    depth: usize,
) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            collect_recent_jsonl_files(&path, since, files, depth + 1);
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(modified_at) = metadata.modified().ok().map(DateTime::<Utc>::from) else {
            continue;
        };
        if modified_at >= since {
            files.push((path, modified_at));
        }
    }
}

fn codex_activity_line(line: &str, activity: &mut ProviderActivity) -> Option<String> {
    let value: Value = serde_json::from_str(line).ok()?;
    let timestamp = short_timestamp(value.get("timestamp").and_then(Value::as_str));
    let payload = value.get("payload")?;
    match (
        value.get("type").and_then(Value::as_str),
        payload.get("type").and_then(Value::as_str),
    ) {
        (Some("event_msg"), Some("task_started")) => Some(format!("{timestamp} codex started")),
        (Some("event_msg"), Some("agent_message")) => payload
            .get("message")
            .and_then(Value::as_str)
            .map(|message| format!("{timestamp} agent {}", one_line(message, 140))),
        (Some("event_msg"), Some("token_count")) => {
            let usage = payload.get("info")?.get("total_token_usage")?;
            let total = usage.get("total_tokens").and_then(Value::as_u64)?;
            let window = payload
                .get("info")
                .and_then(|info| info.get("model_context_window"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            activity.context_tokens = Some(total);
            activity.context_window = Some(window);
            let rate = payload
                .get("rate_limits")
                .and_then(|limits| limits.get("primary"))
                .and_then(|primary| primary.get("used_percent"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            Some(format!(
                "{timestamp} tokens {total}/{window} rate {rate:.0}%"
            ))
        }
        (Some("response_item"), Some("function_call")) => {
            let name = payload
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let args = payload
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or("");
            Some(format!(
                "{timestamp} tool {name} {}",
                one_line(&tool_call_summary(name, args), 140)
            ))
        }
        (Some("response_item"), Some("function_call_output")) => payload
            .get("output")
            .and_then(Value::as_str)
            .map(|output| format!("{timestamp} result {}", one_line(output, 140))),
        _ => None,
    }
}

fn tool_call_summary(name: &str, args: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(args) else {
        return one_line(args, 140);
    };
    match name {
        "exec_command" => value
            .get("cmd")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or_else(|| value.to_string()),
        "update_plan" => value
            .get("plan")
            .and_then(Value::as_array)
            .map(|plan| {
                plan.iter()
                    .filter_map(|item| {
                        Some(format!(
                            "{}:{}",
                            item.get("status")?.as_str()?,
                            item.get("step")?.as_str()?
                        ))
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .filter(|summary| !summary.is_empty())
            .unwrap_or_else(|| value.to_string()),
        _ => value.to_string(),
    }
}

fn short_timestamp(timestamp: Option<&str>) -> String {
    timestamp
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc).format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "--:--:--".to_string())
}

fn render_attach(
    frame: &mut ratatui::Frame<'_>,
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
    live: &AttachLive,
    tui_state: &AttachTuiState,
) {
    let area = frame.area();
    let layout = attach_panel_layout(area);

    let metered_provider = provider_is_metered(state);
    let top_constraints = if metered_provider {
        vec![
            Constraint::Percentage(60),
            Constraint::Percentage(18),
            Constraint::Percentage(22),
        ]
    } else {
        vec![Constraint::Percentage(74), Constraint::Percentage(26)]
    };
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(top_constraints)
        .split(layout.header);
    let phase = state
        .active_phase()
        .map(|phase| format!("{} {}", phase.id.0, phase.name))
        .unwrap_or_else(|| "-".to_string());
    let path_label = if state.promoted_library_dir.is_some() {
        "artifact"
    } else {
        "working"
    };
    let header = Paragraph::new(format!(
        "run {}  status {}  phase {}  turn {}  provider {}  sandbox {}\ngoal {}\n{} {}",
        run_prefix(&state.run_id),
        state.status,
        phase,
        turn_timer(events, spend, traces, state),
        state.provider.as_deref().unwrap_or("-"),
        state.sandbox,
        one_line(&state.goal, top[0].width.saturating_sub(8) as usize),
        path_label,
        one_line(
            &state.working_dir.display().to_string(),
            top[0].width.saturating_sub(12) as usize
        )
    ))
    .block(Block::default().borders(Borders::ALL).title("deadreckon"));
    frame.render_widget(header, top[0]);
    if metered_provider {
        render_spend(frame, top[1], state);
        render_context(frame, top[2], spend, live);
    } else {
        render_context(frame, top[1], spend, live);
    }

    if tui_state.docs_open && state.status == RunStatus::Completed {
        render_run_docs(frame, layout.activity, state, tui_state);
    } else {
        let trace_lines = attach_activity_lines(state, spend, traces, events, live);
        let stream_rows = layout.activity.height.saturating_sub(2) as usize;
        let trace_items = visible_items(&trace_lines, tui_state.activity_scroll, stream_rows);
        frame.render_widget(
            List::new(trace_items).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(panel_border_style(
                        tui_state.focused_panel,
                        AttachPanel::Activity,
                    ))
                    .title(panel_title(
                        "tool calls / provider activity",
                        tui_state.focused_panel == AttachPanel::Activity,
                        tui_state.activity_scroll,
                        stream_rows,
                        trace_lines.len(),
                    )),
            ),
            layout.activity,
        );
    }
    render_live_files(frame, layout.files, live, tui_state);
    render_processes(frame, layout.processes, live, tui_state);
    frame.render_widget(
        Paragraph::new(footer_for_state(state, tui_state)),
        layout.footer,
    );
}

fn provider_is_metered(state: &deadreckon_core::PipelineState) -> bool {
    !state
        .provider
        .as_deref()
        .is_some_and(|provider| provider.starts_with("cli:") || provider.starts_with("import:"))
}

fn footer_for_state(state: &deadreckon_core::PipelineState, tui_state: &AttachTuiState) -> String {
    if tui_state.show_completion_actions && state.status == RunStatus::Completed {
        if is_worktree_run(state) {
            if tui_state.docs_open {
                "[d] Activity  [a] Apply  [b] Abandon  [s] Show  |  Tab focus  j/k scroll  q detach"
            } else {
                "[d] Docs  [a] Apply  [b] Abandon  [s] Show  |  Tab focus  j/k scroll  q detach"
            }
            .to_string()
        } else {
            if tui_state.docs_open {
                "[d] Activity  [m] Materialize  [e] Extend  [s] Show  |  Tab focus  j/k scroll  q detach"
            } else {
                "[d] Docs  [m] Materialize  [e] Extend  [s] Show  |  Tab focus  j/k scroll  q detach"
            }
            .to_string()
        }
    } else {
        "Detach: q Esc Ctrl-D  |  Focus: Tab  |  Scroll: j/k Up/Down PgUp/PgDn mouse".to_string()
    }
}

fn render_spend(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &deadreckon_core::PipelineState,
) {
    let cap = state.max_spend_usd.unwrap_or({
        if state.total_spend_usd <= 0.0 {
            1.0
        } else {
            state.total_spend_usd
        }
    });
    let spend_ratio = (state.total_spend_usd / cap).clamp(0.0, 1.0);
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("spend"))
            .gauge_style(Style::default().fg(meter_color(spend_ratio, state)))
            .ratio(spend_ratio)
            .label(format!("${:.6} / ${:.6}", state.total_spend_usd, cap)),
        area,
    );
}

fn render_context(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    spend: &[SpendRecord],
    live: &AttachLive,
) {
    let (token_total, context_window) = context_totals(spend, live);
    let context_ratio = if context_window == 0 {
        0.0
    } else {
        token_total as f64 / context_window as f64
    };
    let detail = if token_total == 0 {
        format!(
            "waiting for telemetry\n{} window",
            format_count(context_window)
        )
    } else if context_ratio >= 1.0 {
        format!(
            "{} used\n{} window\n{context_ratio:.1}x cumulative",
            format_count(token_total),
            format_count(context_window)
        )
    } else {
        format!(
            "{} used\n{} window\n{:.0}% of window",
            format_count(token_total),
            format_count(context_window),
            context_ratio * 100.0
        )
    };
    frame.render_widget(
        Paragraph::new(detail)
            .block(Block::default().borders(Borders::ALL).title("context"))
            .style(Style::default().fg(threshold_color(context_ratio.clamp(0.0, 1.0))))
            .alignment(Alignment::Center),
        area,
    );
}

fn context_totals(spend: &[SpendRecord], live: &AttachLive) -> (u64, u64) {
    let token_total = live.provider_context_tokens.unwrap_or_else(|| {
        spend
            .iter()
            .map(|record| record.input_tokens + record.output_tokens)
            .sum::<u64>()
    });
    let context_window = live.provider_context_window.unwrap_or(200_000).max(1);
    (token_total, context_window)
}

fn render_turn_summary(spend: &[SpendRecord], show_cost: bool) -> Vec<String> {
    if spend.is_empty() {
        vec!["provider turn in progress; results land when the provider exits".to_string()]
    } else {
        spend
            .iter()
            .rev()
            .take(3)
            .map(|record| {
                let tokens = record.input_tokens + record.output_tokens;
                if show_cost {
                    format!(
                        "turn {}  {}  {} tokens  ${:.6}",
                        record.turn, record.model, tokens, record.cost_usd
                    )
                } else if let Some(seconds) = record.wall_time_seconds {
                    format!(
                        "turn {}  {}  {} tokens  {:.0}s wall",
                        record.turn,
                        record.model,
                        tokens,
                        seconds.max(0.0)
                    )
                } else {
                    format!("turn {}  {}  {} tokens", record.turn, record.model, tokens)
                }
            })
            .collect()
    }
}

fn attach_activity_lines(
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
    live: &AttachLive,
) -> Vec<String> {
    let metered_provider = provider_is_metered(state);
    let mut lines = render_turn_summary(spend, metered_provider);
    if state.status == RunStatus::Executing && live.file_count > 0 {
        lines.push(format!(
            "live working tree: {} files, latest changes visible before provider exit",
            live.file_count
        ));
    }
    lines.extend(live.provider_activity.iter().rev().cloned());
    lines.extend(
        events
            .iter()
            .rev()
            .map(|event| event_line(event, metered_provider)),
    );
    lines.extend(traces.iter().rev().map(|record| {
        format!(
            "trace turn {}  {}  {:?}ms",
            record.turn, record.event, record.latency_ms
        )
    }));
    lines
}

fn render_run_docs(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &deadreckon_core::PipelineState,
    tui_state: &AttachTuiState,
) {
    let lines = render_markdown_doc_lines(state);
    let rows = area.height.saturating_sub(2) as usize;
    frame.render_widget(
        Paragraph::new(lines.clone())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(panel_border_style(
                        tui_state.focused_panel,
                        AttachPanel::Activity,
                    ))
                    .title(panel_title(
                        "run docs / narrative",
                        tui_state.focused_panel == AttachPanel::Activity,
                        tui_state.docs_scroll,
                        rows,
                        lines.len(),
                    )),
            )
            .wrap(Wrap { trim: false })
            .scroll((tui_state.docs_scroll as u16, 0)),
        area,
    );
}

fn render_markdown_doc_lines(state: &deadreckon_core::PipelineState) -> Vec<Line<'static>> {
    let Some(path) = doc_path_for_kind(&state.working_dir, DocKind::Narrative) else {
        return vec![Line::styled(
            "No narrative docs found for this run.",
            Style::default().fg(Color::Yellow),
        )];
    };
    match fs::read_to_string(&path) {
        Ok(raw) => markdown_to_tui_lines(&raw),
        Err(err) => vec![Line::styled(
            format!("Unable to read {}: {err}", path.display()),
            Style::default().fg(Color::Red),
        )],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkdownBlock {
    Paragraph,
    Heading(HeadingLevel),
    Item,
}

fn markdown_to_tui_lines(markdown: &str) -> Vec<Line<'static>> {
    let options = MarkdownOptions::ENABLE_TABLES
        | MarkdownOptions::ENABLE_STRIKETHROUGH
        | MarkdownOptions::ENABLE_TASKLISTS;
    let parser = MarkdownParser::new_ext(markdown, options);
    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut block: Option<MarkdownBlock> = None;
    let mut inline_style = Style::default();
    let mut code_block = false;

    for event in parser {
        match event {
            MarkdownEvent::Start(Tag::Heading { level, .. }) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                block = Some(MarkdownBlock::Heading(level));
            }
            MarkdownEvent::End(TagEnd::Heading(_)) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::raw(""));
            }
            MarkdownEvent::Start(Tag::Paragraph) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                block = Some(MarkdownBlock::Paragraph);
            }
            MarkdownEvent::End(TagEnd::Paragraph) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::raw(""));
            }
            MarkdownEvent::Start(Tag::Item) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                current.push(Span::styled("  - ", Style::default().fg(Color::Cyan)));
                block = Some(MarkdownBlock::Item);
            }
            MarkdownEvent::End(TagEnd::Item) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
            }
            MarkdownEvent::Start(Tag::CodeBlock(kind)) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                let language = match kind {
                    CodeBlockKind::Fenced(language) if !language.is_empty() => {
                        format!(" {}", language)
                    }
                    _ => String::new(),
                };
                lines.push(Line::styled(
                    format!("```{language}"),
                    Style::default().fg(Color::DarkGray),
                ));
                code_block = true;
            }
            MarkdownEvent::End(TagEnd::CodeBlock) => {
                code_block = false;
                lines.push(Line::styled("```", Style::default().fg(Color::DarkGray)));
                lines.push(Line::raw(""));
            }
            MarkdownEvent::Start(Tag::Strong) => {
                inline_style = inline_style.add_modifier(Modifier::BOLD);
            }
            MarkdownEvent::End(TagEnd::Strong) => {
                inline_style = inline_style.remove_modifier(Modifier::BOLD);
            }
            MarkdownEvent::Start(Tag::Emphasis) => {
                inline_style = inline_style.add_modifier(Modifier::ITALIC);
            }
            MarkdownEvent::End(TagEnd::Emphasis) => {
                inline_style = inline_style.remove_modifier(Modifier::ITALIC);
            }
            MarkdownEvent::Start(Tag::Link { dest_url, .. }) => {
                inline_style = inline_style
                    .fg(Color::Blue)
                    .add_modifier(Modifier::UNDERLINED);
                if !dest_url.is_empty() {
                    current.push(Span::styled(
                        "",
                        Style::default()
                            .fg(Color::Blue)
                            .add_modifier(Modifier::UNDERLINED),
                    ));
                }
            }
            MarkdownEvent::End(TagEnd::Link) => {
                inline_style = Style::default();
            }
            MarkdownEvent::Text(text) => {
                if code_block {
                    for line in text.lines() {
                        lines.push(Line::styled(
                            format!("  {line}"),
                            Style::default().fg(Color::LightGreen),
                        ));
                    }
                } else {
                    current.push(Span::styled(text.into_string(), inline_style));
                }
            }
            MarkdownEvent::Code(code) => current.push(Span::styled(
                code.into_string(),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            MarkdownEvent::SoftBreak => current.push(Span::raw(" ")),
            MarkdownEvent::HardBreak => {
                flush_markdown_line(&mut lines, &mut current, block);
                block = Some(MarkdownBlock::Paragraph);
            }
            MarkdownEvent::Rule => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::styled(
                    "────────────────────────────────────────",
                    Style::default().fg(Color::DarkGray),
                ));
            }
            MarkdownEvent::Html(html) | MarkdownEvent::InlineHtml(html) => current.push(
                Span::styled(html.into_string(), Style::default().fg(Color::DarkGray)),
            ),
            MarkdownEvent::InlineMath(math) => current.push(Span::styled(
                math.into_string(),
                Style::default().fg(Color::Magenta),
            )),
            MarkdownEvent::DisplayMath(math) => {
                flush_markdown_line(&mut lines, &mut current, block.take());
                lines.push(Line::styled(
                    math.into_string(),
                    Style::default().fg(Color::Magenta),
                ));
            }
            MarkdownEvent::Start(_)
            | MarkdownEvent::End(_)
            | MarkdownEvent::FootnoteReference(_) => {}
            MarkdownEvent::TaskListMarker(checked) => current.push(Span::styled(
                if checked { "[x] " } else { "[ ] " },
                Style::default().fg(Color::Cyan),
            )),
        }
    }
    flush_markdown_line(&mut lines, &mut current, block.take());
    if lines.is_empty() {
        lines.push(Line::styled(
            "Narrative docs are empty.",
            Style::default().fg(Color::Yellow),
        ));
    }
    lines
}

fn flush_markdown_line(
    lines: &mut Vec<Line<'static>>,
    current: &mut Vec<Span<'static>>,
    block: Option<MarkdownBlock>,
) {
    if current.is_empty() {
        return;
    }
    let style = match block.unwrap_or(MarkdownBlock::Paragraph) {
        MarkdownBlock::Heading(HeadingLevel::H1) => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        MarkdownBlock::Heading(HeadingLevel::H2) => Style::default()
            .fg(Color::LightCyan)
            .add_modifier(Modifier::BOLD),
        MarkdownBlock::Heading(_) => Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
        MarkdownBlock::Item => Style::default().fg(Color::White),
        MarkdownBlock::Paragraph => Style::default(),
    };
    let mut spans = Vec::new();
    if matches!(block, Some(MarkdownBlock::Heading(level)) if level != HeadingLevel::H1) {
        spans.push(Span::styled("▸ ", Style::default().fg(Color::Cyan)));
    }
    spans.extend(current.drain(..).map(|span| span.patch_style(style)));
    lines.push(Line::from(spans));
}

fn render_live_files(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    live: &AttachLive,
    tui_state: &AttachTuiState,
) {
    let lines = live_file_lines(live);
    let rows = area.height.saturating_sub(2) as usize;
    let items = visible_items(&lines, tui_state.files_scroll, rows);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(panel_border_style(
                    tui_state.focused_panel,
                    AttachPanel::Files,
                ))
                .title(panel_title(
                    &format!(
                        "live files  {} files  {}",
                        live.file_count,
                        format_bytes(live.total_bytes)
                    ),
                    tui_state.focused_panel == AttachPanel::Files,
                    tui_state.files_scroll,
                    rows,
                    lines.len(),
                )),
        ),
        area,
    );
}

fn live_file_lines(live: &AttachLive) -> Vec<String> {
    if live.files.is_empty() {
        return vec!["no files yet".to_string()];
    }
    let mut lines = Vec::new();
    lines.extend(live.files.iter().map(|file| {
        format!(
            "{:>7} {:>8}  {}",
            format_age(file.modified_at),
            format_bytes(file.bytes),
            file.path
        )
    }));
    lines
}

fn render_processes(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    live: &AttachLive,
    tui_state: &AttachTuiState,
) {
    let lines = process_lines(live);
    let rows = area.height.saturating_sub(2) as usize;
    let items = visible_items(&lines, tui_state.processes_scroll, rows);
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(panel_border_style(
                    tui_state.focused_panel,
                    AttachPanel::Processes,
                ))
                .title(panel_title(
                    "processes",
                    tui_state.focused_panel == AttachPanel::Processes,
                    tui_state.processes_scroll,
                    rows,
                    lines.len(),
                )),
        ),
        area,
    );
}

fn process_lines(live: &AttachLive) -> Vec<String> {
    if live.pids.is_empty() {
        vec!["no supervised pids".to_string()]
    } else {
        live.pids
            .iter()
            .map(|pid| {
                let status = if pid.alive { "alive" } else { "dead" };
                format!("{} {} {}", pid.pid, status, pid.command)
            })
            .collect()
    }
}

fn visible_items(lines: &[String], offset: usize, rows: usize) -> Vec<ListItem<'static>> {
    lines
        .iter()
        .skip(offset.min(lines.len()))
        .take(rows)
        .map(|line| ListItem::new(line.clone()))
        .collect()
}

fn panel_border_style(focused: AttachPanel, panel: AttachPanel) -> Style {
    if focused == panel {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    }
}

fn panel_title(title: &str, focused: bool, offset: usize, rows: usize, total: usize) -> String {
    let marker = if focused { "*" } else { " " };
    if total <= rows || total == 0 {
        format!("{marker}{title}")
    } else {
        let first = offset.saturating_add(1).min(total);
        let last = offset.saturating_add(rows).min(total);
        format!("{marker}{title} {first}-{last}/{total}")
    }
}

fn event_line(event: &RunEvent, show_cost: bool) -> String {
    match &event.event {
        deadreckon_core::RunEventKind::TurnStarted { turn } => {
            format!("turn {turn} started")
        }
        deadreckon_core::RunEventKind::ToolCallStarted {
            turn,
            tool_call_id,
            tool_name,
            ..
        } => format!("turn {turn} {tool_name} {tool_call_id} started"),
        deadreckon_core::RunEventKind::ToolCallResult {
            turn,
            tool_call_id,
            status,
            preview,
        } => format!("turn {turn} {tool_call_id} {status} {preview}"),
        deadreckon_core::RunEventKind::TokenUsageDelta {
            turn,
            input_tokens,
            output_tokens,
        } => format!("turn {turn} tokens +{}", input_tokens + output_tokens),
        deadreckon_core::RunEventKind::SpendDelta {
            turn,
            cost_usd,
            wall_time_seconds,
            ..
        } => {
            if show_cost {
                format!(
                    "turn {turn} spend +${cost_usd:.6} wall {}s",
                    wall_time_seconds.unwrap_or(0.0)
                )
            } else {
                format!("turn {turn} wall {}s", wall_time_seconds.unwrap_or(0.0))
            }
        }
        deadreckon_core::RunEventKind::RunCompleted { status } => {
            format!("run {status}")
        }
        deadreckon_core::RunEventKind::RunPromoted { library_dir } => {
            format!("promoted {}", library_dir.display())
        }
        deadreckon_core::RunEventKind::Error { turn, message } => {
            format!("turn {} error {message}", turn.unwrap_or_default())
        }
    }
}

fn turn_timer(
    events: &[RunEvent],
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    state: &deadreckon_core::PipelineState,
) -> String {
    let Some(started) = events.iter().rev().find(|event| {
        matches!(
            event.event,
            deadreckon_core::RunEventKind::TurnStarted { .. }
        )
    }) else {
        return "-".to_string();
    };
    if state.status == RunStatus::Executing {
        let elapsed = Utc::now()
            .signed_duration_since(started.timestamp)
            .num_seconds()
            .max(0);
        return format!("{elapsed}s running");
    }
    if let Some(done_at) = events.iter().rev().find_map(|event| match event.event {
        deadreckon_core::RunEventKind::RunCompleted { .. } => Some(event.timestamp),
        _ => None,
    }) {
        let elapsed = done_at
            .signed_duration_since(started.timestamp)
            .num_seconds()
            .max(0);
        return format!("{elapsed}s done");
    }
    if let Some(seconds) = spend
        .iter()
        .rev()
        .find_map(|record| record.wall_time_seconds)
    {
        return format!("{:.0}s done", seconds.max(0.0));
    }
    if let Some(ms) = traces.iter().rev().find_map(|record| record.latency_ms) {
        return format!("{:.0}s done", ms as f64 / 1000.0);
    }
    if let Some(done_at) = events
        .iter()
        .rev()
        .find(|event| {
            !matches!(
                event.event,
                deadreckon_core::RunEventKind::TurnStarted { .. }
            )
        })
        .map(|event| event.timestamp)
    {
        let elapsed = done_at
            .signed_duration_since(started.timestamp)
            .num_seconds()
            .max(0);
        return format!("{elapsed}s done");
    }
    "done".to_string()
}

fn meter_color(ratio: f64, state: &deadreckon_core::PipelineState) -> Color {
    if state.pause_reason.as_deref() == Some("spend cap reached") {
        Color::Magenta
    } else {
        threshold_color(ratio)
    }
}

fn threshold_color(ratio: f64) -> Color {
    if ratio >= 0.8 {
        Color::Red
    } else if ratio >= 0.5 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn format_age(modified_at: Option<DateTime<Utc>>) -> String {
    let Some(modified_at) = modified_at else {
        return "-".to_string();
    };
    let seconds = Utc::now()
        .signed_duration_since(modified_at)
        .num_seconds()
        .max(0);
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h", seconds / 3600)
    }
}

fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}M", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn format_count(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn one_line(value: &str, max_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = compact.chars();
    let shortened = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{shortened}...")
    } else {
        shortened
    }
}

fn read_jsonl<T: DeserializeOwned>(path: &std::path::Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(std::fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect::<Vec<T>>())
}

#[cfg(test)]
mod tui_tests {
    use super::{
        AttachPanel, AttachPanelCounts, AttachPanelRows, AttachTuiState, CompletionAction,
        completion_action_from_input, markdown_to_tui_lines, max_panel_scroll,
    };
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::style::{Color, Modifier};

    fn counts() -> AttachPanelCounts {
        AttachPanelCounts {
            activity: 20,
            files: 10,
            processes: 3,
        }
    }

    fn rows() -> AttachPanelRows {
        AttachPanelRows {
            activity: 5,
            files: 4,
            processes: 4,
        }
    }

    #[test]
    fn tui_scroll_offsets_clamp_to_panel_content() {
        let mut state = AttachTuiState::default();
        state.scroll_focused(100, counts(), rows());
        assert_eq!(state.activity_scroll, 15);

        state.scroll_focused(-100, counts(), rows());
        assert_eq!(state.activity_scroll, 0);

        state.focused_panel = AttachPanel::Processes;
        state.scroll_focused(10, counts(), rows());
        assert_eq!(state.processes_scroll, 0);
    }

    #[test]
    fn tui_focus_and_page_keys_move_active_panel_only() {
        let mut state = AttachTuiState::default();
        state.handle_key(
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
            counts(),
            rows(),
        );
        assert_eq!(state.focused_panel, AttachPanel::Files);

        state.handle_key(
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
            counts(),
            rows(),
        );
        assert_eq!(state.files_scroll, 3);
        assert_eq!(state.activity_scroll, 0);

        state.handle_key(
            KeyEvent::new(KeyCode::End, KeyModifiers::NONE),
            counts(),
            rows(),
        );
        assert_eq!(
            state.files_scroll,
            max_panel_scroll(AttachPanel::Files, counts(), rows())
        );

        state.handle_key(
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            counts(),
            rows(),
        );
        assert_eq!(state.focused_panel, AttachPanel::Activity);
    }

    #[test]
    fn completion_action_parser_accepts_short_and_long_forms() {
        assert_eq!(
            completion_action_from_input("m"),
            Some(CompletionAction::Materialize)
        );
        assert_eq!(
            completion_action_from_input("extend"),
            Some(CompletionAction::Extend)
        );
        assert_eq!(
            completion_action_from_input("S"),
            Some(CompletionAction::Show)
        );
        assert_eq!(
            completion_action_from_input(""),
            Some(CompletionAction::Quit)
        );
        assert_eq!(completion_action_from_input("wat"), None);
    }

    #[test]
    fn markdown_renderer_styles_headings_lists_and_code() {
        let lines = markdown_to_tui_lines(
            "# Summary\n\nImplemented `apply`.\n\n- safer checkout\n\n```rust\nfn main() {}\n```\n",
        );
        let joined = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("Summary"), "{joined}");
        assert!(joined.contains("Implemented apply."), "{joined}");
        assert!(joined.contains("- safer checkout"), "{joined}");
        assert!(joined.contains("fn main() {}"), "{joined}");
        assert!(
            lines
                .iter()
                .flat_map(|line| line.spans.iter())
                .any(|span| span.content.as_ref() == "apply"
                    && span.style.fg == Some(Color::Yellow)
                    && span.style.add_modifier.contains(Modifier::BOLD)),
            "inline code should keep its styling"
        );
    }

    #[test]
    fn docs_toggle_uses_activity_panel_scroll_slot() {
        let mut state = AttachTuiState::default();
        state.toggle_docs();
        state.scroll_focused(4, counts(), rows());
        assert!(state.docs_open);
        assert_eq!(state.docs_scroll, 4);
        assert_eq!(state.activity_scroll, 0);
    }
}
