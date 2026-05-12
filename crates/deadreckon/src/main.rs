use std::collections::{BTreeSet, hash_map::DefaultHasher};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, IsTerminal, Write};
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
    CodebaseMode, CodebaseRecord, DeadreckonError, DeadreckonPaths, DocKind, ModeFlags, PhaseId,
    PhaseStatus, PolishConfig, ProvenanceRecord, RUN_EVENTS_JSONL, ResolvedMode, RunEvent,
    RunLoopConfig, RunLoopDocsConfig, RunLoopOutcome, RunOptions, RunStatus, SpendRecord,
    TraceRecord, WorktreeOptions, acquire_lock, append_parent_narrative_update, append_provenance,
    append_trace, apply_commit_body, copy_source_to_working, copy_tree, create_run,
    create_worktree, doc_path_for_kind, docs_status_for_state, inventory_files, list_runs,
    load_run, polish_run_docs, prepare_worktree_record, preview_git_state, read_codebase_record,
    record_for_resolved_mode, release_lock_file, resolve_mode, restore_snapshot, run_turn_loop,
    save_state, terminate_pid,
};
use deadreckon_providers::ProviderRouter;
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
    after_help = "Lifecycle:\n  deadreckon run \"build the thing\"\n  deadreckon attach latest\n  deadreckon status\n  deadreckon apply latest --autostash --cleanup\n  deadreckon cleanup --completed\n\nRun ids accept unique prefixes. `latest` means the newest run for the current project."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Create ~/.deadreckon/config.toml and check the local setup")]
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
    #[command(about = "Read or update ~/.deadreckon/config.toml")]
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    #[command(about = "Start an unattended coding run")]
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
    #[command(about = "Check providers, sandboxing, disk, and local prerequisites")]
    Doctor,
    #[command(about = "Show runs for the current project by default")]
    List {
        #[arg(long, help = "Filter to a specific scope key")]
        scope: Option<String>,
        #[arg(long, help = "Show runs from all projects")]
        all: bool,
        #[arg(long, help = "Print full TSV-style values for scripts")]
        full: bool,
    },
    #[command(
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
    #[command(about = "Merge a completed worktree run back into the source checkout")]
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
    #[command(about = "Continue from a completed run with a follow-up goal")]
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
        #[arg(long, help = "Sandbox backend override")]
        sandbox: Option<String>,
        #[arg(long, help = "Skip generated run documentation")]
        no_docs: bool,
        #[arg(long, help = "Documentation skill name")]
        doc_skill: Option<String>,
    },
    #[command(about = "Print or regenerate generated run documentation")]
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
    #[command(about = "Attach the live terminal UI to a run")]
    Attach {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Suppress post-completion action hints")]
        no_hints: bool,
    },
    #[command(about = "Cancel a running task")]
    Kill {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Escalate subprocess termination")]
        force: bool,
    },
    #[command(about = "Resume an incomplete run")]
    Resume {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Resume from this turn number")]
        from_turn: Option<u32>,
        #[arg(long, help = "Override wall-clock cap")]
        max_wall_seconds: Option<f64>,
    },
    #[command(about = "Restore an in-place run snapshot")]
    Undo {
        #[arg(
            long,
            help = "Run id, unique prefix, or latest; defaults to current project's latest"
        )]
        run: Option<String>,
        #[arg(long, help = "Snapshot turn to restore")]
        turn: Option<u32>,
    },
    #[command(about = "Show full state, provenance, and trace details for a run")]
    Show {
        #[arg(help = "Run id, unique prefix, or latest")]
        run_id: String,
        #[arg(long, help = "Only show trace/provenance records for this turn")]
        turn: Option<u32>,
    },
    #[command(
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
    #[command(about = "Import read-only history from another coding tool")]
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
        } => init_command(provider, api_key, base_url, max_spend, sandbox, no_confirm),
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
        Commands::Doctor => {
            doctor_command();
            Ok(())
        }
        Commands::List { scope, all, full } => list_command(scope, all, full),
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
                sandbox,
                no_docs,
                doc_skill,
                post_actions: true,
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

fn init_command(
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
    doctor_command();
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
    }
    Ok(())
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
    skill: String,
    smoke: bool,
    i_know_its_a_lot: bool,
    no_confirm: bool,
    no_hints: bool,
    no_docs: bool,
    doc_skill: Option<String>,
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
    sandbox: Option<String>,
    no_docs: bool,
    doc_skill: Option<String>,
    post_actions: bool,
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
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    let effective_provider = if smoke {
        Some("smoke".to_string())
    } else {
        provider.clone().or(defaults.provider)
    };
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
    print_run_started(&state);
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
    let router = if smoke {
        ProviderRouter::smoke()
    } else {
        ProviderRouter::from_config_path(&paths.config_path(), effective_provider.as_deref())?
    };
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
    if completed && !no_hints {
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

fn doctor_command() {
    let paths = DeadreckonPaths::discover();
    println!("{}", ui_heading("deadreckon doctor"));
    println!("{} source /Users/gdc/deadreckon", ui_ok("✓"));
    println!("{} home {}", ui_ok("✓"), paths.home().display());
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
                "{} sandbox {} found{}{}",
                ui_ok("✓"),
                backend.backend,
                path,
                version
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
                    "{} {} present and parseable",
                    ui_ok("✓"),
                    paths.config_path().display()
                );
                doctor_providers(&root);
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
        println!("{} provider defaults configured", ui_ok("✓"));
    } else if command_exists("claude") || command_exists("codex") {
        println!("{} cli subscription provider available", ui_ok("✓"));
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
}

fn doctor_providers(root: &toml::Value) {
    let Some(providers) = root.get("providers").and_then(toml::Value::as_table) else {
        println!("{} providers table missing", ui_warn("✗"));
        println!("    {} deadreckon init", ui_command("fix:"));
        return;
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
                println!("{} provider {name} CLI binary {binary} found", ui_ok("✓"));
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
                println!(
                    "{} provider {name} credential present; ping requested",
                    ui_ok("✓")
                );
            } else {
                println!("{} provider {name} credential present", ui_ok("✓"));
            }
        } else {
            println!("{} provider {name} credential missing", ui_warn("✗"));
            println!(
                "    {} deadreckon config set providers.{name}.api_key <KEY>",
                ui_command("fix:")
            );
        }
    }
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
            "{} runstate dir {} writable",
            ui_ok("✓"),
            paths.runstate_dir().display()
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
            "{} disk space {} MB free in {}",
            ui_ok("✓"),
            kb / 1024,
            paths.home().display()
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
        println!("{} os macOS {version}", ui_ok("✓"));
    }
    #[cfg(target_os = "linux")]
    {
        let version = std::process::Command::new("uname")
            .arg("-r")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!("{} os Linux kernel {version}", ui_ok("✓"));
    }
}

fn doctor_subscription_binary(binary: &str) {
    if command_exists(binary) {
        println!(
            "{} subscription binary {binary} {}",
            ui_ok("✓"),
            command_version(std::path::Path::new(binary))
                .unwrap_or_else(|| "version unknown".to_string())
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
        sandbox,
        max_spend,
        max_wall_seconds,
        brief,
        run_id,
    } = input;
    let mode = codebase.mode.to_string();
    let agent = provider.unwrap_or("-");
    let caps = format!(
        "spend {}, wall {}",
        max_spend
            .map(|cap| format!("<= ${cap:.0}"))
            .unwrap_or_else(|| "uncapped".to_string()),
        format_wall_cap(max_wall_seconds)
    );
    if brief {
        return format!(
            "mode={} branch={} base={} wt={} agent={} cap={}/{}",
            mode,
            codebase.branch_name.as_deref().unwrap_or("-"),
            codebase.base_ref.as_deref().unwrap_or("-"),
            codebase
                .worktree_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string()),
            agent,
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
        format!("  agent:    {agent}"),
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
    if !diff_stat.trim().is_empty() {
        eprintln!("{diff_stat}");
    }
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
    let cleanup_now = cleanup || should_prompt_cleanup(no_confirm)?;
    if cleanup_now {
        cleanup_worktree_run(&state, &record, false, false)?;
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
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), effective_provider.as_deref())?;
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
    let router =
        ProviderRouter::from_config_path(&paths.config_path(), effective_provider.as_deref())?;
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
    if io::stdout().is_terminal() {
        attach_tui(&paths, &run_id, !no_hints).await?;
        let state = load_run(&paths, &run_id)?;
        if state.status == RunStatus::Completed && !no_hints {
            print_lifecycle_hints(&state);
        }
        return Ok(());
    }
    print_run_summary(&state);
    if state.status == RunStatus::Completed && !no_hints {
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
    release_lock_file(paths, &state.scope, &state.task_key)?;
    state.status = RunStatus::Killed;
    state.failure_reason = Some("killed by user".to_string());
    state.killed_at = Some(Utc::now());
    state.updated_at = Utc::now();
    save_state(state)?;
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
    let imported_id = format!(
        "imported-{:016x}",
        stable_hash(&format!("{source}:{}", root.display()))
    );
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
            run_id: None,
            codebase: None,
        },
    )?;
    let old_root = state.run_root.clone();
    let new_root = paths.run_root(&state.scope, &imported_id);
    if new_root.exists() {
        fs::remove_dir_all(&new_root)?;
    }
    if let Some(parent) = new_root.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&old_root, &new_root)?;
    state.run_id = imported_id.clone();
    state.run_root = new_root;
    state.working_dir = state.run_root.join("working");
    fs::create_dir_all(&state.working_dir)?;

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
        if let Some(path) = row
            .get("path")
            .or_else(|| row.get("file"))
            .and_then(serde_json::Value::as_str)
        {
            append_provenance(
                &state,
                &ProvenanceRecord {
                    timestamp: Utc::now(),
                    prompt_id: format!("turn-{turn}"),
                    model: format!("import:{source}"),
                    tool_call_id: row
                        .get("tool_call_id")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("imported-tool")
                        .to_string(),
                    session_id: state.run_id.clone(),
                    files: vec![PathBuf::from(path)],
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
        for line in fs::read_to_string(&file)?.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(object) = value.as_object_mut() {
                    object.insert("source_path".to_string(), json!(file));
                }
                rows.push(value);
            }
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
            .arg("select * from messages")
            .output();
        let Ok(output) = output else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let mut values: Vec<serde_json::Value> =
            serde_json::from_slice(&output.stdout).unwrap_or_default();
        for value in &mut values {
            if let Some(object) = value.as_object_mut() {
                object.insert("source_path".to_string(), json!(file));
            }
        }
        rows.extend(values);
    }
    Ok(rows)
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
    let short = run_prefix(&state.run_id);
    let phase = state
        .active_phase()
        .map(|phase| format!("{} {}", phase.id.0, phase.name))
        .unwrap_or_else(|| "-".to_string());
    println!("deadreckon status");
    println!("  run:      {} ({})", short, state.run_id);
    println!(
        "  state:    {} -> {}",
        state.status,
        next_action_label(state)
    );
    println!("  phase:    {phase}");
    println!("  scope:    {}", state.scope);
    println!("  updated:  {} ago", relative_age(state.updated_at));
    println!("  provider: {}", state.provider.as_deref().unwrap_or("-"));
    println!("  sandbox:  {}", state.sandbox);
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
}

fn print_run_summary(state: &deadreckon_core::PipelineState) {
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

fn print_run_started(state: &deadreckon_core::PipelineState) {
    println!(
        "{} {}",
        ui_ok("started run"),
        ui_id(format!("{} ({})", run_prefix(&state.run_id), state.run_id))
    );
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
        let events = read_jsonl::<RunEvent>(&state.run_root.join(RUN_EVENTS_JSONL))?;
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

        if event::poll(std::time::Duration::from_millis(500))? {
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
