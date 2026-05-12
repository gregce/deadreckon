use std::collections::{BTreeMap, BTreeSet, hash_map::DefaultHasher};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use clap::Parser;
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
    AcceptanceMarker, ApplyMode, ApplyStrategy, BranchPolicy, Chain, ChainEvent, ChainEventKind,
    ChainNewOptions, ChainStatus, ChainStepMarker, ChainStepStatus, CodebaseMode, CodebaseRecord,
    ConductorState, DEFAULT_DOC_POLISH_TOKEN_BUDGET, DEFAULT_DOC_SUBSKILLS, DeadreckonError,
    DeadreckonPaths, DocKind, DocProviderSelection, DocProviderSource, ModeFlags, OnFail, PhaseId,
    PhaseStatus, PromotionManifest, ProvenanceRecord, RUN_EVENTS_JSONL, ResolvedMode, RunEvent,
    RunOptions, RunStatus, SpendRecord, TraceRecord, WorktreeOptions,
    acceptance_spec_path_for_run_root, acquire_lock, append_chain_event,
    append_parent_narrative_update, append_provenance, append_trace, apply_commit_body,
    clear_cancel_marker, copy_source_to_working, copy_tree, create_run, create_worktree,
    doc_path_for_kind, docs_status_for_state, emit_event, evaluate_acceptance_checks,
    inventory_files, list_runs, load_chain, load_run, marker_path_for_run_root, pid_is_alive,
    prepare_worktree_record, preview_git_state, read_chain_step_marker, read_codebase_record,
    record_for_resolved_mode, release_lock_file, resolve_mode, restore_snapshot, save_chain,
    save_state, terminate_pid, validate_acceptance_marker, write_cancel_marker,
    write_chain_step_marker,
};
use deadreckon_providers::{
    ProviderRequest, ProviderRouteInfo, ProviderRouter, ProviderUsage, SpendEstimate,
};
use deadreckon_runtime::{
    PolishConfig, RunLoopConfig, RunLoopDocsConfig, RunLoopOutcome, polish_run_docs, run_turn_loop,
};
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

mod cli;
mod tui_events;

use crate::cli::{
    AcceptanceCommand, AcceptancePreset, CHAIN_HELP, ChainCommandArgs, Cli, Commands,
    ConfigCommand, ExtendCommandArgs, LibraryCommand, RunCommandArgs,
};

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

fn ui_status(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    if text == "ok" || text == "polished" {
        ui_ok(text)
    } else {
        ui_warn(text)
    }
}

fn ui_error(text: impl AsRef<str>) -> String {
    ui_style(text, "1;31", UiStream::Stderr)
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

    if wants_top_level_help() {
        print_top_help();
        return Ok(());
    }

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
        Commands::HelpAll => {
            print_help_all();
            Ok(())
        }
        Commands::Acceptance { command } => acceptance_command(command).await,
        Commands::Done {
            args,
            provider,
            model,
            force,
            spec,
            against,
        } => done_command(args, provider, model, force, spec, against).await,
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
            doc_provider,
            acceptance,
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
                doc_provider,
                acceptance,
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
        Commands::Finish {
            run_id,
            dest,
            force,
            include_manifest,
            strategy,
            branch,
            autostash,
            cleanup,
            no_confirm,
            message,
        } => finish_command(
            run_id,
            dest,
            force,
            include_manifest,
            strategy,
            branch,
            autostash,
            cleanup,
            no_confirm,
            message,
        ),
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
            doc_provider,
            budget_cap,
        } => {
            doc_command(DocCommandArgs {
                run_id,
                kind: kind.into(),
                export,
                polish,
                no_confirm,
                force,
                doc_skill,
                doc_provider,
                budget_cap,
            })
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

fn wants_top_level_help() -> bool {
    let mut args = std::env::args_os().skip(1);
    let Some(arg) = args.next() else {
        return false;
    };
    if args.next().is_some() {
        return false;
    }
    matches!(arg.to_string_lossy().as_ref(), "-h" | "--help" | "help")
}

fn print_top_help() {
    println!(
        "{} {}",
        ui_heading("deadreckon"),
        ui_muted(env!("CARGO_PKG_VERSION"))
    );
    println!(
        "deadreckon runs long coding tasks in an isolated worktree or sandbox, tracks durable state, and gives you explicit apply/export/cleanup steps."
    );
    println!();
    println!("{}", ui_heading("Usage:"));
    println!("  {}", ui_command("deadreckon [command]"));
    println!();
    println!("{}", ui_heading("Typical flow:"));
    for command in [
        "deadreckon done \"builds, tests pass, and opens in a browser\"",
        "deadreckon run \"build the thing\"",
        "deadreckon attach latest",
        "deadreckon finish latest",
    ] {
        println!("  {}", ui_command(command));
    }
    println!();
    println!("{}", ui_heading("Core lifecycle:"));
    for (name, purpose) in [
        ("init", "configure deadreckon"),
        ("doctor", "check provider, sandbox, and local setup"),
        ("done", "write/check done criteria in English"),
        ("run", "start unattended coding work"),
        ("attach", "watch a run in the TUI"),
        ("status", "see the latest run and next action"),
        ("finish", "apply or export completed work"),
    ] {
        println!("  {:<12} {}", ui_command(name), purpose);
    }
    println!();
    println!("{}", ui_heading("Continue or recover:"));
    for (name, purpose) in [
        ("extend", "continue from a completed run"),
        ("resume", "resume an incomplete run"),
        ("kill", "cancel a running task"),
        ("cleanup", "remove stale or completed worktrees"),
    ] {
        println!("  {:<12} {}", ui_command(name), purpose);
    }
    println!();
    println!("{}", ui_heading("More help:"));
    for (name, purpose) in [
        (
            "help-all",
            "show every command, including advanced commands",
        ),
        ("commands", "alias for help-all"),
        ("<command> --help", "detailed help for one command"),
    ] {
        println!("  {:<18} {}", ui_command(name), purpose);
    }
    println!();
    println!(
        "{} Run ids accept unique prefixes. {} means the newest run for the current project.",
        ui_heading("Note:"),
        ui_command("latest")
    );
    println!();
    println!("{}", ui_heading("Options:"));
    for (flag, purpose) in [
        ("-h, --help", "print help"),
        ("-V, --version", "print version"),
    ] {
        println!("  {:<18} {}", ui_command(flag), purpose);
    }
}

fn print_help_all() {
    println!("{}", ui_heading("deadreckon commands"));
    println!();
    println!("{}", ui_heading("core lifecycle"));
    for (name, purpose) in [
        ("init", "configure deadreckon"),
        ("doctor", "check provider, sandbox, and local setup"),
        ("done", "write/check done criteria in English"),
        ("run", "start unattended coding work"),
        ("chain", "run several coding steps in sequence"),
        ("attach", "watch a run in the TUI"),
        ("status", "see the latest run and next action"),
        ("list", "list runs for this project"),
        ("finish", "route completed work to apply or export"),
    ] {
        println!("  {:<12} {}", ui_command(name), purpose);
    }
    println!();
    println!("{}", ui_heading("continue and recover"));
    for (name, purpose) in [
        ("extend", "continue from a completed run"),
        ("resume", "resume an incomplete run"),
        ("kill", "cancel a running task"),
        ("cleanup", "remove stale or completed worktrees"),
        ("undo", "restore an in-place snapshot"),
        ("abandon", "discard a temporary worktree run"),
    ] {
        println!("  {:<12} {}", ui_command(name), purpose);
    }
    println!();
    println!("{}", ui_heading("inspect and advanced"));
    for (name, purpose) in [
        ("apply", "merge a completed worktree run"),
        ("export", "copy a completed fresh/copy run"),
        ("doc", "read or regenerate run docs"),
        ("library", "inspect promoted artifacts"),
        ("show", "show raw state, traces, and provenance"),
        ("config", "read or update configuration"),
        ("import", "import other tool history"),
        (
            "acceptance",
            "advanced compatibility command for done criteria",
        ),
    ] {
        println!("  {:<12} {}", ui_command(name), purpose);
    }
    println!();
    println!(
        "Use {} for detailed help.",
        ui_command("deadreckon <command> --help")
    );
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

fn print_chain_help(topic: Option<&str>) {
    let topic = topic.unwrap_or("overview");
    match topic {
        "plan" | "expand" => {
            println!("{}", ui_heading("deadreckon chain plan"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain plan \"large goal\" --n 4")
            );
            println!(
                "purpose: ask the configured provider to split a large goal into ordered steps"
            );
            println!("next:    {}", ui_command("deadreckon chain run latest"));
        }
        "run" | "resume" => {
            println!("{}", ui_heading("deadreckon chain run"));
            println!("usage: {}", ui_command("deadreckon chain run latest"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain resume latest --from-step 2")
            );
            println!("purpose: execute or continue the conductor for a chain");
            println!("next:    {}", ui_command("deadreckon chain attach latest"));
        }
        "attach" | "watch" => {
            println!("{}", ui_heading("deadreckon chain attach"));
            println!("usage: {}", ui_command("deadreckon chain attach latest"));
            println!(
                "purpose: open the chain TUI, including step timeline and live inner run status"
            );
            println!("next:    {}", ui_command("deadreckon chain status latest"));
        }
        "status" | "list" => {
            println!("{}", ui_heading("deadreckon chain status/list"));
            println!("usage: {}", ui_command("deadreckon chain status latest"));
            println!("usage: {}", ui_command("deadreckon chain list --all"));
            println!("purpose: find chains, summarize progress, and see the next action");
            println!("next:    {}", ui_command("deadreckon chain show latest"));
        }
        "show" => {
            println!("{}", ui_heading("deadreckon chain show"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain show latest --why-failed")
            );
            println!("purpose: inspect steps, policies, failures, applied SHAs, and run ids");
            println!("next:    {}", ui_command("deadreckon chain resume latest"));
        }
        "pause" | "kill" => {
            println!("{}", ui_heading("deadreckon chain pause/kill"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain pause latest --reason \"waiting on review\"")
            );
            println!(
                "usage: {}",
                ui_command("deadreckon chain kill latest --force")
            );
            println!(
                "purpose: stop the conductor intentionally; kill also cascades to the live inner run"
            );
            println!("next:    {}", ui_command("deadreckon chain resume latest"));
        }
        "undo" | "redo" => {
            println!("{}", ui_heading("deadreckon chain undo/redo"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain undo latest --step 2")
            );
            println!(
                "usage: {}",
                ui_command("deadreckon chain redo latest --step 2")
            );
            println!("purpose: back out or rerun an applied step with bounded chain state changes");
            println!("next:    {}", ui_command("deadreckon chain show latest"));
        }
        "extend" => {
            println!("{}", ui_heading("deadreckon chain extend"));
            println!(
                "usage: {}",
                ui_command("deadreckon chain extend latest \"new step goal\"")
            );
            println!(
                "usage: {}",
                ui_command("deadreckon chain extend latest \"new step goal\" --insert-at 2")
            );
            println!("purpose: add a new step to an existing chain");
            println!("next:    {}", ui_command("deadreckon chain run latest"));
        }
        "hooks" => {
            println!("{}", ui_heading("deadreckon chain hooks"));
            println!("usage: {}", ui_command("deadreckon chain hooks list"));
            println!("purpose: list lifecycle hook names supported by the conductor");
        }
        _ => {
            println!("{}", ui_heading("deadreckon chain"));
            println!("{CHAIN_HELP}");
            println!();
            println!("More help:");
            println!("  {}", ui_command("deadreckon chain help plan"));
            println!("  {}", ui_command("deadreckon chain help run"));
            println!("  {}", ui_command("deadreckon chain help pause"));
            println!("  {}", ui_command("deadreckon chain help undo"));
        }
    }
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
    if args.first().is_some_and(|arg| arg == "help") {
        print_chain_help(args.get(1).map(String::as_str));
        return Ok(());
    }
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
                        skip_acceptance_prompt: yes,
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
                    skip_acceptance_prompt: yes,
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
                    skip_acceptance_prompt: yes,
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
    skip_acceptance_prompt: bool,
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
    let auto_attach = chain_should_auto_attach(io::stdout().is_terminal(), detach, quiet, plain);
    chain_run_command(
        &paths,
        &chain_id,
        ChainRunOptions {
            detach: detach || auto_attach,
            quiet,
            plain,
            from_step: None,
            max_spend_add: None,
            reset_breaker: false,
            apply_mode: None,
            skip_acceptance_prompt: yes,
        },
    )
    .await?;
    if auto_attach {
        chain_attach_command(&paths, &chain_id, false)?;
    }
    Ok(chain_id)
}

async fn chain_run_command(
    paths: &DeadreckonPaths,
    chain_id: &str,
    options: ChainRunOptions,
) -> Result<()> {
    let chain_id = resolve_chain_id(paths, chain_id, false)?;
    ensure_chain_acceptance_before_start(paths, &chain_id, &options).await?;
    if options.detach {
        return detach_chain_conductor(paths, &chain_id, &options);
    }
    run_chain_conductor(paths, &chain_id, options).await
}

async fn ensure_chain_acceptance_before_start(
    paths: &DeadreckonPaths,
    chain_id: &str,
    options: &ChainRunOptions,
) -> Result<()> {
    if options.skip_acceptance_prompt || options.quiet || !io::stdin().is_terminal() {
        return Ok(());
    }
    let chain = load_chain(paths, chain_id)?;
    let _ = ensure_acceptance_before_start(
        &chain.cwd,
        None,
        &chain.root_goal,
        chain.provider.clone(),
        chain.model.clone(),
        false,
        "chain",
    )
    .await?;
    Ok(())
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
        live_child_pid: None,
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
        let step_wall_cap = per_step_wall_cap(&chain, index);
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
        let state = if chain.steps[index].status == ChainStepStatus::Completed {
            let run_id = chain.steps[index].run_id.clone().ok_or_else(|| {
                CliError::Core(deadreckon_core::user_error(
                    &format!("step {} is completed but has no run id", index + 1),
                    &format!(
                        "deadreckon chain redo {} --step {}",
                        chain_prefix(&chain.chain_id),
                        index + 1
                    ),
                ))
            })?;
            load_run(paths, &run_id)?
        } else {
            chain.steps[index].status = ChainStepStatus::Running;
            append_chain_event(
                paths,
                &chain.chain_id,
                ChainEventKind::ChainStepStarted,
                Some(index as u32),
                json!({ "goal": chain.steps[index].goal, "base": base_ref, "max_spend": step_cap, "max_wall_seconds": step_wall_cap }),
            )?;
            save_chain(paths, &chain)?;
            let run_id = match run_chain_step(
                paths,
                &chain,
                index,
                &base_ref,
                step_cap,
                step_wall_cap,
                options.quiet,
            )
            .await
            {
                Ok(run_id) => run_id,
                Err(err) => {
                    completed =
                        handle_chain_step_failure(paths, &mut chain, index, err.to_string())?;
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
            state
        };
        match chain.apply_mode {
            ApplyMode::Auto => {
                if let Err(err) =
                    auto_apply_chain_step(paths, &mut chain, index, &state.run_id, options.quiet)
                {
                    pause_chain_at_step(
                        paths,
                        &mut chain,
                        index,
                        format!("apply_refused_{}", compact_reason(&err.to_string())),
                    )?;
                    completed = false;
                    break;
                }
            }
            ApplyMode::Preview => {
                let diff_summary = preview_diff_summary_for_run(&state).unwrap_or_default();
                append_chain_event(
                    paths,
                    &chain.chain_id,
                    ChainEventKind::ChainApplyRefused,
                    Some(index as u32),
                    json!({ "reason": "apply_mode_preview", "diff_summary": diff_summary }),
                )?;
                if !options.quiet {
                    println!("preview diff for step {}:", index + 1);
                    if diff_summary.trim().is_empty() {
                        println!("  no diff");
                    } else {
                        println!("{diff_summary}");
                    }
                }
                pause_chain_at_step(paths, &mut chain, index, "apply_mode_preview".to_string())?;
                completed = false;
                break;
            }
            ApplyMode::Manual => {
                pause_chain_at_step(paths, &mut chain, index, "apply_mode_manual".to_string())?;
                completed = false;
                break;
            }
        }
        if chain_spend_cap_hit(&chain) {
            pause_chain_at_step(paths, &mut chain, index, "cap".to_string())?;
            completed = false;
            break;
        }
        if chain_wall_cap_hit(&chain) {
            pause_chain_at_step(paths, &mut chain, index, "wall_clock_cap".to_string())?;
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
    paths: &DeadreckonPaths,
    chain: &Chain,
    index: usize,
    base_ref: &str,
    step_cap: Option<f64>,
    step_wall_cap: Option<f64>,
    quiet: bool,
) -> Result<String> {
    let step = &chain.steps[index];
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .current_dir(&chain.cwd)
        .env("DEADRECKON_HOME", paths.home())
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
    if let Some(max_wall) = step_wall_cap {
        command.arg("--max-wall-seconds").arg(max_wall.to_string());
    }
    if let Some(step_cap) = step_cap {
        command.arg("--max-spend").arg(format!("{step_cap:.6}"));
    }
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn()?;
    let child_pid = child.id();
    update_conductor_live(paths, chain, Some(index as u32), None, Some(child_pid))?;

    let stdout = child.stdout.take().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "failed to capture chain step stdout".to_string(),
        ))
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "failed to capture chain step stderr".to_string(),
        ))
    })?;
    let (tx, rx) = std::sync::mpsc::channel::<(bool, String)>();
    let stdout_thread = spawn_chain_step_reader(stdout, true, tx.clone());
    let stderr_thread = spawn_chain_step_reader(stderr, false, tx);
    let mut stdout_text = String::new();
    let mut stderr_text = String::new();
    let mut live_run_id: Option<String> = None;
    let status = loop {
        while let Ok((is_stdout, line)) = rx.try_recv() {
            if let Some(run_id) = capture_chain_step_output(
                is_stdout,
                &line,
                &mut stdout_text,
                &mut stderr_text,
                quiet,
            )? && live_run_id.as_deref() != Some(run_id.as_str())
            {
                update_conductor_live(
                    paths,
                    chain,
                    Some(index as u32),
                    Some(run_id.clone()),
                    Some(child_pid),
                )?;
                live_run_id = Some(run_id);
            }
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let _ = stdout_thread.join();
    let _ = stderr_thread.join();
    while let Ok((is_stdout, line)) = rx.try_recv() {
        if let Some(run_id) =
            capture_chain_step_output(is_stdout, &line, &mut stdout_text, &mut stderr_text, quiet)?
            && live_run_id.as_deref() != Some(run_id.as_str())
        {
            update_conductor_live(
                paths,
                chain,
                Some(index as u32),
                Some(run_id.clone()),
                Some(child_pid),
            )?;
            live_run_id = Some(run_id);
        }
    }
    update_conductor_live(paths, chain, None, None, None)?;
    if !status.success() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "step {} run failed: {}{}",
            index + 1,
            stdout_text,
            stderr_text
        ))));
    }
    live_run_id
        .or_else(|| parse_started_run_id(&stdout_text))
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "could not find inner run id in run output\ntry: deadreckon list".to_string(),
            ))
        })
}

fn spawn_chain_step_reader<R: Read + Send + 'static>(
    reader: R,
    is_stdout: bool,
    tx: std::sync::mpsc::Sender<(bool, String)>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = io::BufReader::new(reader);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if tx.send((is_stdout, line)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    })
}

fn capture_chain_step_output(
    is_stdout: bool,
    line: &str,
    stdout_text: &mut String,
    stderr_text: &mut String,
    quiet: bool,
) -> Result<Option<String>> {
    if is_stdout {
        stdout_text.push_str(line);
        if !quiet {
            print!("{line}");
            io::stdout().flush()?;
        }
        Ok(parse_started_run_id(stdout_text))
    } else {
        stderr_text.push_str(line);
        if !quiet {
            eprint!("{line}");
            io::stderr().flush()?;
        }
        Ok(None)
    }
}

fn auto_apply_chain_step(
    paths: &DeadreckonPaths,
    chain: &mut Chain,
    index: usize,
    run_id: &str,
    quiet: bool,
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
                        "step '{}' refused auto-apply (outside_allowlist {file})",
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
    if quiet {
        apply_command_quiet(
            run_id.to_string(),
            apply_strategy_label(chain_apply_strategy(chain)).to_string(),
            None,
            true,
            true,
            false,
            None,
        )?;
    } else {
        apply_command(
            run_id.to_string(),
            apply_strategy_label(chain_apply_strategy(chain)).to_string(),
            None,
            true,
            true,
            false,
            None,
        )?;
    }
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

fn read_conductor_state(paths: &DeadreckonPaths, chain_id: &str) -> Result<Option<ConductorState>> {
    let path = paths.conductor_json(chain_id);
    match fs::read(&path) {
        Ok(raw) => Ok(Some(serde_json::from_slice(&raw)?)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn write_conductor_state(paths: &DeadreckonPaths, state: &ConductorState) -> Result<()> {
    let path = paths.conductor_json(&state.chain_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(state)?)?;
    Ok(())
}

fn update_conductor_live(
    paths: &DeadreckonPaths,
    chain: &Chain,
    live_step: Option<u32>,
    live_run_id: Option<String>,
    live_child_pid: Option<u32>,
) -> Result<()> {
    let mut conductor =
        read_conductor_state(paths, &chain.chain_id)?.unwrap_or_else(|| ConductorState {
            schema_version: 1,
            chain_id: chain.chain_id.clone(),
            conductor_pid: chain.conductor_pid.unwrap_or_else(std::process::id),
            started_at: chain.started_at.unwrap_or_else(Utc::now),
            live_step: None,
            live_run_id: None,
            live_child_pid: None,
        });
    conductor.live_step = live_step;
    conductor.live_run_id = live_run_id;
    conductor.live_child_pid = live_child_pid;
    write_conductor_state(paths, &conductor)
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

fn chain_attach_command(paths: &DeadreckonPaths, id: &str, plain: bool) -> Result<()> {
    let id = resolve_chain_id(paths, id, false)?;
    let chain = load_chain(paths, &id)?;
    if io::stdout().is_terminal() && !plain {
        return chain_attach_tui(paths, &id);
    }
    print_chain_attach_snapshot(&chain);
    Ok(())
}

fn print_chain_attach_snapshot(chain: &Chain) {
    println!(
        "chain {} status: {} steps: {}/{} spend: ${:.2}/{}",
        chain_prefix(&chain.chain_id),
        chain_status_label(chain),
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
}

fn chain_attach_tui(paths: &DeadreckonPaths, chain_id: &str) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut tui_state = ChainAttachTuiState::default();

    let result = loop {
        let chain = load_chain(paths, chain_id)?;
        let events = read_jsonl::<ChainEvent>(&paths.chain_events(chain_id))?;
        tui_state.clamp(&chain);
        terminal.draw(|frame| render_chain_attach(frame, &chain, &events, &tui_state))?;

        if event::poll(std::time::Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key) if attach_should_quit(key) => break Ok(()),
                Event::Key(key) => match key.code {
                    KeyCode::Enter => {
                        suspend_tui(&mut terminal)?;
                        if let Some(run_id) = chain
                            .steps
                            .get(tui_state.selected_step)
                            .and_then(|step| step.run_id.clone())
                        {
                            let _ = show_command(run_id, None);
                        } else {
                            eprintln!("selected step has no run yet");
                        }
                        let _ = prompt("press Enter to return to chain attach...");
                        resume_tui(&mut terminal)?;
                    }
                    KeyCode::Char('r') => {
                        suspend_tui(&mut terminal)?;
                        let action = chain_redo_command(
                            paths,
                            chain_id,
                            Some(tui_state.selected_step as u32 + 1),
                            None,
                            false,
                        );
                        if let Err(err) = &action {
                            eprintln!("error: {err}");
                        }
                        let _ = prompt("press Enter to return to chain attach...");
                        resume_tui(&mut terminal)?;
                    }
                    KeyCode::Char('e') => {
                        suspend_tui(&mut terminal)?;
                        let goal = prompt("new chain step goal: ")?;
                        if !goal.trim().is_empty() {
                            let action = chain_extend_command(paths, chain_id, goal, None, None);
                            if let Err(err) = &action {
                                eprintln!("error: {err}");
                            }
                        }
                        let _ = prompt("press Enter to return to chain attach...");
                        resume_tui(&mut terminal)?;
                    }
                    KeyCode::Char('p') => {
                        suspend_tui(&mut terminal)?;
                        let action =
                            chain_pause_command(paths, chain_id, Some("user_paused".to_string()));
                        if let Err(err) = &action {
                            eprintln!("error: {err}");
                        }
                        let _ = prompt("press Enter to return to chain attach...");
                        resume_tui(&mut terminal)?;
                    }
                    KeyCode::Char('k') => {
                        suspend_tui(&mut terminal)?;
                        let answer = prompt("kill chain? [y/N]: ")?;
                        if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
                            && let Err(err) = chain_kill_command(paths, chain_id, false)
                        {
                            eprintln!("error: {err}");
                        }
                        let _ = prompt("press Enter to return to chain attach...");
                        resume_tui(&mut terminal)?;
                    }
                    _ => tui_state.handle_key(key, &chain),
                },
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollDown => tui_state.scroll(1, &chain),
                    MouseEventKind::ScrollUp => tui_state.scroll(-1, &chain),
                    _ => {}
                },
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

#[derive(Debug, Default)]
struct ChainAttachTuiState {
    selected_step: usize,
    events_scroll: u16,
}

impl ChainAttachTuiState {
    fn clamp(&mut self, chain: &Chain) {
        if chain.steps.is_empty() {
            self.selected_step = 0;
            self.events_scroll = 0;
            return;
        }
        self.selected_step = self.selected_step.min(chain.steps.len() - 1);
    }

    fn handle_key(&mut self, key: KeyEvent, chain: &Chain) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.scroll(-1, chain),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.scroll(1, chain),
            KeyCode::PageUp => {
                self.events_scroll = self.events_scroll.saturating_sub(8);
            }
            KeyCode::PageDown => {
                self.events_scroll = self.events_scroll.saturating_add(8);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.selected_step = 0;
                self.events_scroll = 0;
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.selected_step = chain.steps.len().saturating_sub(1);
            }
            _ => {}
        }
        self.clamp(chain);
    }

    fn scroll(&mut self, delta: isize, chain: &Chain) {
        if chain.steps.is_empty() {
            return;
        }
        let next = (self.selected_step as isize + delta)
            .clamp(0, chain.steps.len().saturating_sub(1) as isize);
        self.selected_step = next as usize;
    }
}

fn render_chain_attach(
    frame: &mut ratatui::Frame<'_>,
    chain: &Chain,
    events: &[ChainEvent],
    tui_state: &ChainAttachTuiState,
) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(area);
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(rows[1]);

    frame.render_widget(
        Paragraph::new(chain_attach_header_text(chain)).block(
            Block::default()
                .borders(Borders::ALL)
                .title("deadreckon chain"),
        ),
        rows[0],
    );
    let timeline = chain_timeline_lines(chain, tui_state)
        .into_iter()
        .map(ListItem::new)
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(timeline).block(Block::default().borders(Borders::ALL).title("steps")),
        body[0],
    );
    let event_lines = chain_activity_lines(events, tui_state)
        .into_iter()
        .map(ListItem::new)
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(event_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title("chain activity"),
        ),
        body[1],
    );
    frame.render_widget(Paragraph::new(chain_attach_footer_text(chain)), rows[2]);
}

fn chain_attach_header_text(chain: &Chain) -> String {
    let applied = chain
        .steps
        .iter()
        .filter(|step| step.status == ChainStepStatus::Applied)
        .count();
    format!(
        "{}  status {}  steps {}/{}  spend ${:.6}/{}\npolicy branch={} apply={} strategy={} on-fail={}\nbase {}@{}  cwd {}",
        chain_prefix(&chain.chain_id),
        chain_status_label(chain),
        applied,
        chain.steps.len(),
        chain.total_spend_usd,
        chain
            .max_spend_usd
            .map(|value| format!("${value:.6}"))
            .unwrap_or_else(|| "uncapped".to_string()),
        branch_policy_label(chain.branch_policy),
        apply_mode_label(chain.apply_mode),
        apply_strategy_label(chain_apply_strategy(chain)),
        on_fail_label(chain.on_fail),
        chain.base_branch,
        short_sha(&chain.base_sha),
        one_line(&chain.cwd.display().to_string(), 96)
    )
}

fn chain_timeline_lines(chain: &Chain, tui_state: &ChainAttachTuiState) -> Vec<Line<'static>> {
    chain
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let marker = if index == tui_state.selected_step {
                ">"
            } else {
                " "
            };
            let run = step
                .run_id
                .as_deref()
                .map(|run_id| format!(" run {}", run_prefix(run_id)))
                .unwrap_or_default();
            let mut spans = vec![
                Span::styled(marker.to_string(), Style::default().fg(Color::Cyan)),
                Span::raw(format!(
                    " {} step {:>2} {:<8} {}{}",
                    chain_step_dot(step.status),
                    step.index + 1,
                    chain_step_status_label(step.status),
                    one_line(&step.goal, 54),
                    run
                )),
            ];
            if let Some(reason) = step.fail_reason.as_deref() {
                spans.push(Span::styled(
                    format!("  {}", one_line(reason, 32)),
                    Style::default().fg(Color::Red),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

fn chain_activity_lines(
    events: &[ChainEvent],
    tui_state: &ChainAttachTuiState,
) -> Vec<Line<'static>> {
    let start = usize::from(tui_state.events_scroll).min(events.len());
    events
        .iter()
        .rev()
        .skip(start)
        .take(240)
        .map(|event| {
            let step = event
                .step_index
                .map(|index| format!(" step {}", index + 1))
                .unwrap_or_default();
            let detail = if event.detail.is_null() {
                String::new()
            } else {
                format!(" {}", one_line(&event.detail.to_string(), 120))
            };
            Line::from(format!(
                "{} {}{}{}",
                event.timestamp.format("%H:%M:%S"),
                chain_event_label(&event.event),
                step,
                detail
            ))
        })
        .collect()
}

fn chain_attach_footer_text(chain: &Chain) -> String {
    if chain.status == ChainStatus::Paused {
        format!(
            "paused: {} | try: show --why-failed | try: resume | try: resume --apply-mode preview | try: undo | q detach",
            chain.paused_reason.as_deref().unwrap_or("paused")
        )
    } else {
        "[Enter] drill  [r] redo  [e] extend  [p] pause  [k] kill  [Ctrl-D/q/Esc] detach  j/k move  PgUp/PgDn activity".to_string()
    }
}

fn chain_event_label(event: &ChainEventKind) -> &'static str {
    match event {
        ChainEventKind::ChainCreated => "created",
        ChainEventKind::ChainStepStarted => "step started",
        ChainEventKind::ChainRunCompleted => "run completed",
        ChainEventKind::ChainApplyStarted => "apply started",
        ChainEventKind::ChainApplied => "applied",
        ChainEventKind::ChainApplyRefused => "apply refused",
        ChainEventKind::ChainStepFailed => "step failed",
        ChainEventKind::ChainPaused => "paused",
        ChainEventKind::ChainResumed => "resumed",
        ChainEventKind::ChainKilled => "killed",
        ChainEventKind::ChainCompleted => "completed",
        ChainEventKind::ChainUndoStarted => "undo started",
        ChainEventKind::ChainUndoneStep => "undone step",
        ChainEventKind::ChainHookInvoked => "hook",
        ChainEventKind::ChainStepExtended => "extended",
        ChainEventKind::ChainStepRedone => "redone",
    }
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
    let conductor = read_conductor_state(paths, &id)?;
    let mut signaled_pids = BTreeSet::new();
    if let Some(run_id) = conductor
        .as_ref()
        .and_then(|state| state.live_run_id.as_deref())
        .map(ToString::to_string)
        .or_else(|| {
            chain
                .steps
                .iter()
                .find(|step| step.status == ChainStepStatus::Running)
                .and_then(|step| step.run_id.clone())
        })
        && let Ok(mut state) = load_run(paths, &run_id)
    {
        kill_loaded_run(paths, &mut state, force)?;
        signaled_pids.extend(supervised_pids(&state));
    }
    if let Some(pid) = conductor.as_ref().and_then(|state| state.live_child_pid) {
        terminate_pid(pid, force)?;
        signaled_pids.insert(pid);
    }
    if let Some(pid) = conductor
        .as_ref()
        .map(|state| state.conductor_pid)
        .or(chain.conductor_pid)
        && pid != std::process::id()
    {
        terminate_pid(pid, force)?;
        signaled_pids.insert(pid);
    }
    if !force {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while signaled_pids
            .iter()
            .any(|pid| *pid != std::process::id() && pid_is_alive(*pid))
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        for pid in &signaled_pids {
            if *pid != std::process::id() && pid_is_alive(*pid) {
                terminate_pid(*pid, true)?;
            }
        }
    }
    chain.status = ChainStatus::Killed;
    chain.failure_reason = Some("killed by user".to_string());
    chain.conductor_pid = None;
    for step in &mut chain.steps {
        if step.status == ChainStepStatus::Running {
            step.status = ChainStepStatus::Failed;
            step.fail_reason = Some("killed by user".to_string());
        }
    }
    save_chain(paths, &chain)?;
    append_chain_event(
        paths,
        &chain.chain_id,
        ChainEventKind::ChainKilled,
        None,
        json!({ "force": force }),
    )?;
    let _ = fs::remove_file(paths.conductor_json(&chain.chain_id));
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
        .filter(|step| through_step.is_none_or(|limit| step.index < limit))
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

fn chain_should_auto_attach(
    stdout_is_terminal: bool,
    detach: bool,
    quiet: bool,
    plain: bool,
) -> bool {
    stdout_is_terminal && !detach && !quiet && !plain
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

fn per_step_wall_cap(chain: &Chain, index: usize) -> Option<f64> {
    let max = chain.max_wall_seconds?;
    let remaining = (max - chain.total_wall_seconds).max(0.0);
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

fn chain_wall_cap_hit(chain: &Chain) -> bool {
    chain
        .max_wall_seconds
        .is_some_and(|max| chain.total_wall_seconds >= max)
}

fn preview_diff_summary_for_run(state: &deadreckon_core::PipelineState) -> Result<String> {
    let record = read_codebase_record(&state.working_dir)?;
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
    git_stdout(git_root, &["diff", "--stat", &format!("HEAD..{branch}")])
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
        .take(96)
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
        doc_provider,
        acceptance,
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
        provider.clone().or(defaults.provider.clone())
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
    let doc_provider_selection = resolve_doc_provider(
        doc_provider.as_deref(),
        &defaults,
        effective_provider.as_deref(),
    );
    if max_spend.is_none() {
        let cap = effective_max_spend.unwrap_or(10.0);
        println!(
            "using default --max-spend ${cap:.0} (override with --max-spend or in config defaults.max_spend)"
        );
    }
    confirm_spend_cap(effective_max_spend, i_know_its_a_lot, no_confirm)?;
    let cwd = std::env::current_dir()?;
    let acceptance_source = ensure_acceptance_before_start(
        &cwd,
        acceptance.as_deref(),
        &goal,
        provider.clone(),
        model.clone(),
        yes || no_confirm || preview,
        "run",
    )
    .await?;
    let acceptance_preview = acceptance_preview(&acceptance_source)?;
    let sandbox = sandbox
        .or(defaults.sandbox.clone())
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
        doc_provider: doc_provider_selection.provider.as_deref(),
        doc_provider_source: doc_provider_selection.source.as_str(),
        max_spend: effective_max_spend,
        max_wall_seconds: effective_max_wall_seconds,
        acceptance: &acceptance_preview,
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
    copy_acceptance_into_run(&state, &acceptance_source)?;
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
    print_run_started(
        &state,
        selected_route.as_ref(),
        doc_provider_selection.provider.as_deref(),
        doc_provider_selection.source.as_str(),
    );
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
                doc_provider: doc_provider_selection.provider.clone(),
                doc_provider_source: Some(doc_provider_selection.source.as_str().to_string()),
                doc_subskills: effective_doc_subskills(&defaults),
                token_budget: defaults
                    .doc_polish_token_budget
                    .unwrap_or(DEFAULT_DOC_POLISH_TOKEN_BUDGET),
                budget_cap_usd: defaults.doc_polish_budget_cap_usd,
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

const PROJECT_ACCEPTANCE_DIR: &str = ".deadreckon";
const PROJECT_ACCEPTANCE_YAML: &str = "acceptance.yaml";
const PROJECT_ACCEPTANCE_MD: &str = "acceptance.md";
const PROJECT_ACCEPTANCE_HELPERS: &str = "acceptance";

#[derive(Clone, Debug)]
struct AcceptanceSource {
    path: PathBuf,
    label: String,
    companion_doc: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct AcceptanceDraft {
    yaml: String,
    markdown: String,
    files: BTreeMap<PathBuf, String>,
}

#[derive(Clone, Debug)]
struct AcceptancePreview {
    source: String,
    path: Option<PathBuf>,
    checks: Option<usize>,
}

impl AcceptancePreview {
    fn full_label(&self) -> String {
        let mut label = self.source.clone();
        if let Some(checks) = self.checks {
            label.push_str(&format!(" ({checks} checks)"));
        }
        if let Some(path) = self.path.as_ref() {
            label.push_str(&format!(" from {}", path.display()));
        }
        label
    }

    fn brief_label(&self) -> String {
        match self.checks {
            Some(checks) => format!("{}:{checks}", self.source),
            None => self.source.clone(),
        }
    }
}

async fn acceptance_command(command: AcceptanceCommand) -> Result<()> {
    match command {
        AcceptanceCommand::Setup {
            request,
            provider,
            model,
            force,
        } => {
            acceptance_agent_command(AcceptanceAgentMode::Draft, request, provider, model, force)
                .await
        }
        AcceptanceCommand::Add {
            request,
            provider,
            model,
            force,
        } => acceptance_add_command(request, provider, model, force).await,
        AcceptanceCommand::Init { preset, force } => acceptance_init_command(preset, force),
        AcceptanceCommand::Draft {
            request,
            provider,
            model,
            force,
        } => {
            acceptance_agent_command(AcceptanceAgentMode::Draft, request, provider, model, force)
                .await
        }
        AcceptanceCommand::Refine {
            request,
            provider,
            model,
            force,
        } => {
            acceptance_agent_command(AcceptanceAgentMode::Refine, request, provider, model, force)
                .await
        }
        AcceptanceCommand::Explain { spec } => acceptance_explain_command(spec),
        AcceptanceCommand::Check { spec, against } => acceptance_check_command(spec, against),
    }
}

async fn done_command(
    args: Vec<String>,
    provider: Option<String>,
    model: Option<String>,
    force: bool,
    spec: Option<PathBuf>,
    against: Option<PathBuf>,
) -> Result<()> {
    let Some(first) = args.first().map(String::as_str) else {
        return acceptance_explain_command(spec);
    };
    match first {
        "add" => {
            let request = args.iter().skip(1).cloned().collect::<Vec<_>>();
            if request.is_empty() {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "done add needs a criterion",
                    "deadreckon done add \"users can save drawings\"",
                )));
            }
            acceptance_add_command(request, provider, model, force).await
        }
        "check" => acceptance_check_command(spec, against),
        "show" | "explain" => acceptance_explain_command(spec),
        "edit" | "refine" => {
            let request = args.iter().skip(1).cloned().collect::<Vec<_>>();
            if request.is_empty() {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "done edit needs a requested change",
                    "deadreckon done edit \"also require the gallery to persist\"",
                )));
            }
            acceptance_agent_command(AcceptanceAgentMode::Refine, request, provider, model, force)
                .await
        }
        "help" => {
            print_done_help();
            Ok(())
        }
        _ => {
            acceptance_agent_command(AcceptanceAgentMode::Draft, args, provider, model, true).await
        }
    }
}

fn print_done_help() {
    println!("{}", ui_heading("deadreckon done"));
    println!("usage:");
    println!(
        "  {}",
        ui_command("deadreckon done \"builds, opens in a browser, and has no console errors\"")
    );
    println!(
        "  {}",
        ui_command("deadreckon done add \"users can save drawings\"")
    );
    println!("  {}", ui_command("deadreckon done check"));
    println!("  {}", ui_command("deadreckon done show"));
}

#[derive(Clone, Copy)]
enum AcceptanceAgentMode {
    Draft,
    Refine,
}

fn acceptance_init_command(preset: AcceptancePreset, force: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let preset = match preset {
        AcceptancePreset::Auto => detect_acceptance_preset(&cwd),
        other => other,
    };
    let draft = acceptance_template_for_preset(preset, &cwd);
    write_project_acceptance(&cwd, &draft, force, false)?;
    print_acceptance_written(&cwd, "template", acceptance_check_count(&draft.yaml)?);
    Ok(())
}

async fn acceptance_agent_command(
    mode: AcceptanceAgentMode,
    request: Vec<String>,
    provider: Option<String>,
    model: Option<String>,
    force: bool,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    acceptance_agent_command_in_dir(&cwd, mode, request, provider, model, force).await
}

async fn acceptance_agent_command_in_dir(
    cwd: &Path,
    mode: AcceptanceAgentMode,
    request: Vec<String>,
    provider: Option<String>,
    model: Option<String>,
    force: bool,
) -> Result<()> {
    let yaml_path = project_acceptance_yaml(cwd);
    let md_path = project_acceptance_md(cwd);
    let existing_yaml = read_optional_text(&yaml_path)?;
    let existing_md = read_optional_text(&md_path)?;
    if matches!(mode, AcceptanceAgentMode::Refine) && existing_yaml.is_none() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "no project acceptance spec found",
            "deadreckon done \"what should count as done\"",
        )));
    }
    let request = acceptance_request_text(request, mode)?;
    if !force && yaml_path.exists() && matches!(mode, AcceptanceAgentMode::Draft) {
        return Err(CliError::Core(deadreckon_core::user_error(
            ".deadreckon/acceptance.yaml already exists",
            "deadreckon done add \"one more criterion\" or rerun with --force",
        )));
    }
    let paths = DeadreckonPaths::discover();
    let defaults = config_defaults(&paths)?;
    let selected_provider = provider.or(defaults.doc_provider).or(defaults.provider);
    let router = ProviderRouter::from_config_path_with_model(
        &paths.config_path(),
        selected_provider.as_deref(),
        model.as_deref(),
    )?;
    let route = router.selected_route_info();
    let prompt = acceptance_agent_prompt(
        mode,
        &request,
        cwd,
        existing_yaml.as_deref(),
        existing_md.as_deref(),
    )?;
    let response = router
        .complete(&ProviderRequest {
            prompt,
            max_output_tokens: 6_000,
            cwd: Some(cwd.to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            pid_file: None,
            cancellation_token: None,
        })
        .await
        .map_err(|err| {
            CliError::Core(deadreckon_core::user_error(
                &format!("acceptance provider failed: {err}"),
                "deadreckon done \"builds and passes tests\"",
            ))
        })?;
    let draft = parse_acceptance_agent_response(&response.content)?;
    acceptance_check_count(&draft.yaml)?;
    write_project_acceptance(cwd, &draft, true, true)?;
    let route_label = route
        .map(|route| format!("{} / {}", route.name, route.model))
        .unwrap_or_else(|| "configured provider".to_string());
    print_acceptance_written(
        cwd,
        &format!("agent draft via {route_label}"),
        acceptance_check_count(&draft.yaml)?,
    );
    Ok(())
}

async fn acceptance_add_command(
    request: Vec<String>,
    provider: Option<String>,
    model: Option<String>,
    force: bool,
) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let joined = request.join(" ");
    if let Some(pack) = AcceptancePack::from_request(&joined) {
        return acceptance_add_pack_command(&cwd, pack, force);
    }
    let mode = if project_acceptance_yaml(&cwd).exists() {
        AcceptanceAgentMode::Refine
    } else {
        AcceptanceAgentMode::Draft
    };
    acceptance_agent_command_in_dir(&cwd, mode, request, provider, model, force).await
}

fn acceptance_add_pack_command(cwd: &Path, pack: AcceptancePack, force: bool) -> Result<()> {
    let mut draft = if project_acceptance_yaml(cwd).exists() {
        let yaml = fs::read_to_string(project_acceptance_yaml(cwd))?;
        let markdown = read_optional_text(&project_acceptance_md(cwd))?
            .unwrap_or_else(|| acceptance_markdown_from_yaml(&yaml));
        AcceptanceDraft {
            yaml,
            markdown,
            files: BTreeMap::new(),
        }
    } else {
        AcceptanceDraft {
            yaml: "name: project acceptance\nchecks: []\n".to_string(),
            markdown: "# Acceptance Criteria\n\n".to_string(),
            files: BTreeMap::new(),
        }
    };
    let pack_draft = acceptance_pack_draft(pack, cwd)?;
    draft.yaml = append_acceptance_checks(&draft.yaml, &pack_draft.yaml)?;
    if !draft.markdown.ends_with('\n') {
        draft.markdown.push('\n');
    }
    draft.markdown.push('\n');
    draft.markdown.push_str(pack_draft.markdown.trim());
    draft.markdown.push('\n');
    draft.files.extend(pack_draft.files);
    write_project_acceptance(cwd, &draft, force, true)?;
    print_acceptance_written(
        cwd,
        &format!("{} pack", pack.name()),
        acceptance_check_count(&draft.yaml)?,
    );
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum AcceptancePack {
    Auto,
    Basic,
    Build,
    Test,
    Rust,
    Node,
    StaticSite,
    Browser,
    Playwright,
    Vite,
    NextJs,
    Python,
}

impl AcceptancePack {
    fn from_request(request: &str) -> Option<Self> {
        let normalized = request.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "auto" => Some(Self::Auto),
            "basic" => Some(Self::Basic),
            "build" => Some(Self::Build),
            "test" | "tests" => Some(Self::Test),
            "rust" | "cargo" => Some(Self::Rust),
            "node" | "npm" | "javascript" | "typescript" => Some(Self::Node),
            "static" | "static-site" | "static site" | "html" => Some(Self::StaticSite),
            "browser" | "smoke" | "browser-smoke" => Some(Self::Browser),
            "playwright" | "e2e" => Some(Self::Playwright),
            "vite" => Some(Self::Vite),
            "next" | "nextjs" | "next.js" => Some(Self::NextJs),
            "python" | "py" => Some(Self::Python),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Basic => "basic",
            Self::Build => "build",
            Self::Test => "test",
            Self::Rust => "rust",
            Self::Node => "node",
            Self::StaticSite => "static-site",
            Self::Browser => "browser",
            Self::Playwright => "playwright",
            Self::Vite => "vite",
            Self::NextJs => "nextjs",
            Self::Python => "python",
        }
    }
}

fn acceptance_explain_command(spec: Option<PathBuf>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let path = resolve_acceptance_path_for_command(&cwd, spec.as_deref())?;
    match path {
        Some(path) => {
            let raw = fs::read_to_string(&path)?;
            let count = acceptance_check_count(&raw)?;
            println!("{}", ui_heading("done criteria"));
            println!("  spec:   {}", path.display());
            println!("  checks: {count}");
            if path == project_acceptance_yaml(&cwd)
                && let Some(markdown) = read_optional_text(&project_acceptance_md(&cwd))?
            {
                println!();
                println!("{}", markdown.trim());
            }
            println!();
            print_acceptance_yaml_summary(&raw)?;
        }
        None => {
            println!("{}", ui_heading("done criteria"));
            println!("  spec:   default dr-gate behavior");
            println!(
                "  checks: working directory exists, or cargo test when Cargo.toml is present"
            );
            println!();
            println!(
                "{}",
                ui_command("deadreckon done \"what should count as done\"")
            );
        }
    }
    Ok(())
}

fn acceptance_check_command(spec: Option<PathBuf>, against: Option<PathBuf>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let working_dir = against.unwrap_or(cwd.clone());
    let spec_path = resolve_acceptance_path_for_command(&cwd, spec.as_deref())?;
    let temp_root = std::env::temp_dir().join(format!(
        "deadreckon-acceptance-check-{}",
        Uuid::new_v4().simple()
    ));
    fs::create_dir_all(&temp_root)?;
    if let Some(spec_path) = spec_path.as_ref() {
        fs::copy(spec_path, acceptance_spec_path_for_run_root(&temp_root))?;
    }
    let result = evaluate_acceptance_checks(&temp_root, &working_dir);
    let _ = fs::remove_dir_all(&temp_root);
    match result {
        Ok(results) => {
            let failed_required = results
                .iter()
                .any(|result| result.must_pass && !result.passed);
            if failed_required {
                println!("{}", ui_warn("done criteria failed"));
            } else {
                println!("{}", ui_ok("done criteria passed"));
            }
            println!("working {}", working_dir.display());
            if let Some(spec_path) = spec_path {
                println!("spec    {}", spec_path.display());
            } else {
                println!("spec    default dr-gate behavior");
            }
            print_acceptance_results(&results);
            if let Some(failed) = results
                .iter()
                .find(|result| result.must_pass && !result.passed)
            {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!("required done criterion failed: {}", failed.detail),
                    "fix the project or run `deadreckon done edit \"tighten or correct the checks\"`",
                )));
            }
            Ok(())
        }
        Err(err) => Err(CliError::Core(deadreckon_core::user_error(
            &format!("done criteria check failed: {err}"),
            "fix the project or edit .deadreckon/acceptance.yaml, then rerun `deadreckon done check`",
        ))),
    }
}

fn print_acceptance_results(results: &[deadreckon_core::AcceptanceCheckResult]) {
    for result in results {
        let mark = if result.passed {
            ui_ok("✓")
        } else if result.must_pass {
            ui_error_stdout("✗")
        } else {
            ui_warn("!")
        };
        let requirement = if result.must_pass {
            "required"
        } else {
            "optional"
        };
        let elapsed = result
            .duration_ms
            .map(|ms| format!(" ({:.1}s)", ms as f64 / 1000.0))
            .unwrap_or_default();
        println!(
            "  {mark} {:<13} {:<8} {}{}",
            result.kind, requirement, result.detail, elapsed
        );
        if !result.passed {
            if let Some(command) = result.command.as_deref() {
                println!("      command: {}", ui_command(command));
            }
            if let Some(stderr) = result.stderr.as_deref() {
                println!("      stderr:  {}", one_line(stderr, 140));
            }
            if let Some(stdout) = result.stdout.as_deref() {
                println!("      stdout:  {}", one_line(stdout, 140));
            }
        }
    }
}

fn ui_error_stdout(text: impl AsRef<str>) -> String {
    ui_style(text, "1;31", UiStream::Stdout)
}

fn acceptance_request_text(request: Vec<String>, mode: AcceptanceAgentMode) -> Result<String> {
    let joined = request.join(" ").trim().to_string();
    if !joined.is_empty() {
        return Ok(joined);
    }
    if io::stdin().is_terminal() {
        let prompt_text = match mode {
            AcceptanceAgentMode::Draft => "what should count as done? ",
            AcceptanceAgentMode::Refine => "how should acceptance change? ",
        };
        let answer = prompt(prompt_text)?;
        if !answer.trim().is_empty() {
            return Ok(answer.trim().to_string());
        }
    }
    match mode {
        AcceptanceAgentMode::Draft => Ok(
            "Draft practical acceptance criteria for this project and its likely build/test flow."
                .to_string(),
        ),
        AcceptanceAgentMode::Refine => Err(CliError::Core(deadreckon_core::user_error(
            "refine needs a requested change",
            "deadreckon done add \"also require tests for the gallery\"",
        ))),
    }
}

fn acceptance_agent_prompt(
    mode: AcceptanceAgentMode,
    request: &str,
    cwd: &Path,
    existing_yaml: Option<&str>,
    existing_md: Option<&str>,
) -> Result<String> {
    let mode_label = match mode {
        AcceptanceAgentMode::Draft => "draft",
        AcceptanceAgentMode::Refine => "refine",
    };
    let project = acceptance_project_summary(cwd)?;
    Ok(format!(
        "\
You are helping configure deadreckon acceptance criteria for an unattended coding run.
The user writes acceptance in plain English. Convert it into executable checks that dr-gate can run.

Return JSON only, with exactly these keys:
{{\"acceptance_yaml\":\"...\",\"acceptance_md\":\"...\",\"files\":{{}}}}

The YAML must be valid deadreckon acceptance.yaml. Use only these check kinds:
- file_exists with path
- content_match with path and pattern
- shell with command and optional cwd
- cargo_test

Use {{working_dir}} for paths inside the run. Prefer stable, automatable checks over subjective claims.
Do not include self-attestation checks, provider-output checks, or instructions that the agent can satisfy by writing a marker.
For Node projects, prefer `npm run build --if-present` and `npm test --if-present` shell checks.
For static apps, require the main HTML/CSS/JS files and one or two content_match checks for requested behavior.
If a criterion needs a helper script, include it in files under `.deadreckon/acceptance/` and call it from a shell check.
The acceptance_md must restate the user's criteria in readable English before listing the executable checks.
Keep the YAML concise and include at least one required check.

Mode: {mode_label}
User request: {request}

Project summary:
{project}

Existing acceptance.yaml:
{existing_yaml}

Existing acceptance.md:
{existing_md}
",
        existing_yaml = existing_yaml.unwrap_or("(none)"),
        existing_md = existing_md.unwrap_or("(none)")
    ))
}

fn acceptance_project_summary(cwd: &Path) -> Result<String> {
    let mut lines = vec![format!("root: {}", cwd.display())];
    if cwd.join("Cargo.toml").exists() {
        lines.push("stack: rust".to_string());
    }
    if cwd.join("package.json").exists() {
        lines.push("stack: node".to_string());
        if let Ok(package) = fs::read_to_string(cwd.join("package.json"))
            && let Ok(value) = serde_json::from_str::<Value>(&package)
            && let Some(scripts) = value.get("scripts").and_then(Value::as_object)
        {
            let mut script_names = scripts.keys().cloned().collect::<Vec<_>>();
            script_names.sort();
            lines.push(format!("package scripts: {}", script_names.join(", ")));
        }
    }
    let files = project_file_sample(cwd, 80)?;
    lines.push("files:".to_string());
    for file in files {
        lines.push(format!("  - {}", file.display()));
    }
    Ok(lines.join("\n"))
}

fn project_file_sample(cwd: &Path, limit: usize) -> Result<Vec<PathBuf>> {
    fn walk(
        root: &Path,
        current: &Path,
        depth: usize,
        out: &mut Vec<PathBuf>,
        limit: usize,
    ) -> io::Result<()> {
        if depth > 3 || out.len() >= limit {
            return Ok(());
        }
        let mut entries = fs::read_dir(current)?.collect::<std::result::Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if out.len() >= limit {
                break;
            }
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(
                name.as_ref(),
                ".git" | "target" | "node_modules" | ".deadreckon" | "dist" | "build"
            ) {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, depth + 1, out, limit)?;
            } else if let Ok(relative) = path.strip_prefix(root) {
                out.push(relative.to_path_buf());
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(cwd, cwd, 0, &mut files, limit)?;
    Ok(files)
}

fn parse_acceptance_agent_response(content: &str) -> Result<AcceptanceDraft> {
    let cleaned = strip_code_fence(content.trim());
    if let Ok(value) = serde_json::from_str::<Value>(&cleaned)
        && let Some(parsed) = acceptance_json_payload(&value)?
    {
        return Ok(parsed);
    }
    if let Some(json) = extract_json_object(&cleaned)
        && let Ok(value) = serde_json::from_str::<Value>(&json)
        && let Some(parsed) = acceptance_json_payload(&value)?
    {
        return Ok(parsed);
    }
    if let Some(yaml) = extract_fenced_block(&cleaned, &["yaml", "yml"]).or_else(|| {
        if cleaned.contains("checks:") {
            Some(cleaned.clone())
        } else {
            None
        }
    }) {
        let markdown = extract_fenced_block(&cleaned, &["markdown", "md"])
            .unwrap_or_else(|| acceptance_markdown_from_yaml(&yaml));
        acceptance_check_count(&yaml)?;
        return Ok(AcceptanceDraft {
            yaml,
            markdown,
            files: BTreeMap::new(),
        });
    }
    Err(CliError::Core(deadreckon_core::user_error(
        "provider did not return acceptance JSON or YAML",
        "rerun `deadreckon done ...` or use `deadreckon done check` after editing criteria",
    )))
}

fn acceptance_json_payload(value: &Value) -> Result<Option<AcceptanceDraft>> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let yaml = object
        .get("acceptance_yaml")
        .or_else(|| object.get("yaml"))
        .and_then(Value::as_str);
    let Some(yaml) = yaml else {
        return Ok(None);
    };
    let markdown = object
        .get("acceptance_md")
        .or_else(|| object.get("markdown"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| acceptance_markdown_from_yaml(yaml));
    acceptance_check_count(yaml)?;
    let mut files = BTreeMap::new();
    if let Some(file_map) = object.get("files").and_then(Value::as_object) {
        for (path, body) in file_map {
            let Some(body) = body.as_str() else {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!("acceptance helper {path} must be a string"),
                    "return files as {\".deadreckon/acceptance/name\": \"contents\"}",
                )));
            };
            let path = PathBuf::from(path);
            validate_acceptance_helper_path(&path)?;
            files.insert(path, body.to_string());
        }
    }
    Ok(Some(AcceptanceDraft {
        yaml: yaml.to_string(),
        markdown,
        files,
    }))
}

fn strip_code_fence(value: &str) -> String {
    let trimmed = value.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }
    let mut lines = trimmed.lines().collect::<Vec<_>>();
    if lines.len() >= 2 && lines.last().is_some_and(|line| line.trim() == "```") {
        lines.remove(0);
        lines.pop();
        return lines.join("\n");
    }
    trimmed.to_string()
}

fn extract_json_object(value: &str) -> Option<String> {
    let start = value.find('{')?;
    let end = value.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(value[start..=end].to_string())
}

fn extract_fenced_block(value: &str, languages: &[&str]) -> Option<String> {
    let mut in_block = false;
    let mut capture = false;
    let mut lines = Vec::new();
    for line in value.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("```") {
            if in_block {
                if capture {
                    return Some(lines.join("\n"));
                }
                in_block = false;
                continue;
            }
            in_block = true;
            capture = languages
                .iter()
                .any(|language| rest.trim().eq_ignore_ascii_case(language));
            lines.clear();
            continue;
        }
        if in_block && capture {
            lines.push(line);
        }
    }
    None
}

async fn ensure_acceptance_before_start(
    cwd: &Path,
    override_path: Option<&Path>,
    goal: &str,
    provider: Option<String>,
    model: Option<String>,
    skip_prompt: bool,
    noun: &str,
) -> Result<Option<AcceptanceSource>> {
    let existing = resolve_acceptance_source(cwd, override_path)?;
    if existing.is_some() || override_path.is_some() || skip_prompt || !io::stdin().is_terminal() {
        return Ok(existing);
    }
    println!("{}", ui_heading("done criteria"));
    println!("No done criteria found.");
    println!("Write the definition of done in English; deadreckon will compile it for dr-gate.");
    let answer = prompt(&format!("write done criteria before this {noun}? [Y/n]: "))?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "n" | "no") {
        println!("using default gate: working directory exists, or cargo test for Rust projects");
        return Ok(existing);
    }
    let request = prompt("definition of done (Enter for a practical default): ")?;
    let request = if request.trim().is_empty() {
        format!("For this {noun}, define practical acceptance checks for: {goal}")
    } else {
        request.trim().to_string()
    };
    match acceptance_agent_command_in_dir(
        cwd,
        AcceptanceAgentMode::Draft,
        vec![request],
        provider,
        model,
        false,
    )
    .await
    {
        Ok(()) => resolve_acceptance_source(cwd, None),
        Err(err) => {
            println!("{}", ui_warn("done criteria draft failed"));
            println!("  {err}");
            let fallback = prompt("use a detected local check template instead? [Y/n]: ")?;
            if matches!(fallback.trim().to_ascii_lowercase().as_str(), "n" | "no") {
                return Ok(existing);
            }
            let preset = detect_acceptance_preset(cwd);
            let draft = acceptance_template_for_preset(preset, cwd);
            write_project_acceptance(cwd, &draft, false, false)?;
            print_acceptance_written(
                cwd,
                "detected template",
                acceptance_check_count(&draft.yaml)?,
            );
            resolve_acceptance_source(cwd, None)
        }
    }
}

fn resolve_acceptance_source(
    cwd: &Path,
    override_path: Option<&Path>,
) -> Result<Option<AcceptanceSource>> {
    if let Some(path) = override_path {
        let path = absolute_from(cwd, path);
        if !path.is_file() {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("acceptance spec not found: {}", path.display()),
                "deadreckon done \"what should count as done\"",
            )));
        }
        return Ok(Some(AcceptanceSource {
            path,
            label: "explicit".to_string(),
            companion_doc: None,
        }));
    }
    let project_yaml = project_acceptance_yaml(cwd);
    if project_yaml.is_file() {
        let project_md = project_acceptance_md(cwd);
        return Ok(Some(AcceptanceSource {
            path: project_yaml,
            label: "project".to_string(),
            companion_doc: project_md.is_file().then_some(project_md),
        }));
    }
    Ok(None)
}

fn acceptance_preview(source: &Option<AcceptanceSource>) -> Result<AcceptancePreview> {
    match source {
        Some(source) => {
            let raw = fs::read_to_string(&source.path)?;
            Ok(AcceptancePreview {
                source: source.label.clone(),
                path: Some(source.path.clone()),
                checks: Some(acceptance_check_count(&raw)?),
            })
        }
        None => Ok(AcceptancePreview {
            source: "default".to_string(),
            path: None,
            checks: None,
        }),
    }
}

fn copy_acceptance_into_run(
    state: &deadreckon_core::PipelineState,
    source: &Option<AcceptanceSource>,
) -> Result<()> {
    let Some(source) = source else {
        return Ok(());
    };
    fs::copy(
        &source.path,
        acceptance_spec_path_for_run_root(&state.run_root),
    )?;
    if let Some(doc) = source.companion_doc.as_ref() {
        fs::copy(doc, state.run_root.join(PROJECT_ACCEPTANCE_MD))?;
    }
    let Some(project_dir) = source.path.parent() else {
        return Ok(());
    };
    let helper_source = project_dir.join(PROJECT_ACCEPTANCE_HELPERS);
    if helper_source.is_dir() {
        let helper_dest = state
            .working_dir
            .join(PROJECT_ACCEPTANCE_DIR)
            .join(PROJECT_ACCEPTANCE_HELPERS);
        copy_tree(&helper_source, &helper_dest)?;
    }
    Ok(())
}

fn resolve_acceptance_path_for_command(cwd: &Path, spec: Option<&Path>) -> Result<Option<PathBuf>> {
    if let Some(spec) = spec {
        let path = absolute_from(cwd, spec);
        if !path.is_file() {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("acceptance spec not found: {}", path.display()),
                "deadreckon done \"what should count as done\"",
            )));
        }
        return Ok(Some(path));
    }
    let project = project_acceptance_yaml(cwd);
    Ok(project.is_file().then_some(project))
}

fn absolute_from(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn project_acceptance_yaml(cwd: &Path) -> PathBuf {
    cwd.join(PROJECT_ACCEPTANCE_DIR)
        .join(PROJECT_ACCEPTANCE_YAML)
}

fn project_acceptance_md(cwd: &Path) -> PathBuf {
    cwd.join(PROJECT_ACCEPTANCE_DIR).join(PROJECT_ACCEPTANCE_MD)
}

fn write_project_acceptance(
    cwd: &Path,
    draft: &AcceptanceDraft,
    force: bool,
    allow_existing: bool,
) -> Result<()> {
    let dir = cwd.join(PROJECT_ACCEPTANCE_DIR);
    let yaml_path = project_acceptance_yaml(cwd);
    let md_path = project_acceptance_md(cwd);
    if !allow_existing && !force && (yaml_path.exists() || md_path.exists()) {
        return Err(CliError::Core(deadreckon_core::user_error(
            ".deadreckon/acceptance files already exist",
            "deadreckon done add \"one more criterion\" or rerun with --force",
        )));
    }
    fs::create_dir_all(&dir)?;
    fs::write(&yaml_path, ensure_trailing_newline(&draft.yaml))?;
    fs::write(&md_path, ensure_trailing_newline(&draft.markdown))?;
    for (relative_path, body) in &draft.files {
        let path = validate_acceptance_helper_path(relative_path)?;
        let absolute = cwd.join(path);
        if !force && absolute.exists() {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("acceptance helper already exists: {}", absolute.display()),
                "rerun with --force or edit the helper manually",
            )));
        }
        if let Some(parent) = absolute.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(absolute, ensure_trailing_newline(body))?;
    }
    Ok(())
}

fn validate_acceptance_helper_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("invalid acceptance helper path: {}", path.display()),
            "helper files must live under .deadreckon/acceptance/",
        )));
    }
    let required_prefix = Path::new(PROJECT_ACCEPTANCE_DIR).join(PROJECT_ACCEPTANCE_HELPERS);
    if !path.starts_with(&required_prefix) {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!("invalid acceptance helper path: {}", path.display()),
            "helper files must live under .deadreckon/acceptance/",
        )));
    }
    Ok(path.to_path_buf())
}

fn print_acceptance_written(cwd: &Path, source: &str, checks: usize) {
    println!("{}", ui_ok("done criteria configured"));
    println!("source  {source}");
    println!("checks  {checks}");
    println!("yaml    {}", project_acceptance_yaml(cwd).display());
    println!("notes   {}", project_acceptance_md(cwd).display());
    println!();
    println!(
        "{} {}",
        ui_command("next:"),
        ui_command("deadreckon done check")
    );
    println!(
        "{} {}",
        ui_command("run: "),
        ui_command("deadreckon run \"goal\"")
    );
}

fn detect_acceptance_preset(cwd: &Path) -> AcceptancePreset {
    if cwd.join("Cargo.toml").exists() {
        AcceptancePreset::Rust
    } else if cwd.join("package.json").exists() {
        AcceptancePreset::Node
    } else if cwd.join("index.html").exists() || cwd.join("public/index.html").exists() {
        AcceptancePreset::StaticSite
    } else {
        AcceptancePreset::Basic
    }
}

fn acceptance_template_for_preset(preset: AcceptancePreset, cwd: &Path) -> AcceptanceDraft {
    let yaml = match preset {
        AcceptancePreset::Auto => unreachable!("auto is resolved before template generation"),
        AcceptancePreset::Rust => {
            "\
name: rust project acceptance
checks:
  - kind: file_exists
    path: \"{working_dir}/Cargo.toml\"
  - kind: cargo_test
"
        }
        AcceptancePreset::Node => {
            "\
name: node project acceptance
checks:
  - kind: file_exists
    path: \"{working_dir}/package.json\"
  - kind: shell
    command: \"npm run build --if-present\"
    cwd: \"{working_dir}\"
  - kind: shell
    command: \"npm test --if-present\"
    cwd: \"{working_dir}\"
    must_pass: false
"
        }
        AcceptancePreset::StaticSite => {
            if cwd.join("public/index.html").exists() && !cwd.join("index.html").exists() {
                "\
name: static site acceptance
checks:
  - kind: file_exists
    path: \"{working_dir}/public/index.html\"
  - kind: shell
    command: \"test -s public/index.html\"
    cwd: \"{working_dir}\"
"
            } else {
                "\
name: static site acceptance
checks:
  - kind: file_exists
    path: \"{working_dir}/index.html\"
  - kind: shell
    command: \"test -s index.html\"
    cwd: \"{working_dir}\"
"
            }
        }
        AcceptancePreset::Basic => {
            "\
name: basic project acceptance
checks:
  - kind: shell
    command: \"test -d .\"
    cwd: \"{working_dir}\"
"
        }
    }
    .to_string();
    let markdown = format!(
        "\
# Acceptance Criteria

These checks define what `deadreckon` should verify before promoting a completed run.

{}
",
        acceptance_markdown_from_yaml(&yaml)
    );
    AcceptanceDraft {
        yaml,
        markdown,
        files: BTreeMap::new(),
    }
}

fn acceptance_pack_draft(pack: AcceptancePack, cwd: &Path) -> Result<AcceptanceDraft> {
    let pack = match pack {
        AcceptancePack::Auto => match detect_acceptance_preset(cwd) {
            AcceptancePreset::Rust => AcceptancePack::Rust,
            AcceptancePreset::Node => AcceptancePack::Node,
            AcceptancePreset::StaticSite => AcceptancePack::StaticSite,
            AcceptancePreset::Basic | AcceptancePreset::Auto => AcceptancePack::Basic,
        },
        other => other,
    };
    let yaml = match pack {
        AcceptancePack::Basic => {
            "name: basic acceptance pack\nchecks:\n  - kind: shell\n    command: \"test -d .\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Build => {
            if cwd.join("Cargo.toml").exists() {
                "name: build acceptance pack\nchecks:\n  - kind: build_success\n    cwd: \"{working_dir}\"\n"
            } else if cwd.join("package.json").exists() {
                "name: build acceptance pack\nchecks:\n  - kind: shell\n    command: \"npm run build --if-present\"\n    cwd: \"{working_dir}\"\n"
            } else if cwd.join("pyproject.toml").exists() || cwd.join("requirements.txt").exists() {
                "name: build acceptance pack\nchecks:\n  - kind: shell\n    command: \"python3 -m compileall .\"\n    cwd: \"{working_dir}\"\n"
            } else {
                "name: build acceptance pack\nchecks:\n  - kind: shell\n    command: \"test -d .\"\n    cwd: \"{working_dir}\"\n"
            }
        }
        AcceptancePack::Test => {
            if cwd.join("Cargo.toml").exists() {
                "name: test acceptance pack\nchecks:\n  - kind: cargo_test\n"
            } else if cwd.join("package.json").exists() {
                "name: test acceptance pack\nchecks:\n  - kind: shell\n    command: \"npm test --if-present\"\n    cwd: \"{working_dir}\"\n"
            } else {
                "name: test acceptance pack\nchecks:\n  - kind: shell\n    command: \"test -d .\"\n    cwd: \"{working_dir}\"\n"
            }
        }
        AcceptancePack::Rust => {
            "name: rust acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/Cargo.toml\"\n  - kind: cargo_test\n"
        }
        AcceptancePack::Node => {
            "name: node acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/package.json\"\n  - kind: shell\n    command: \"npm run build --if-present\"\n    cwd: \"{working_dir}\"\n  - kind: shell\n    command: \"npm test --if-present\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::StaticSite => {
            if cwd.join("public/index.html").exists() && !cwd.join("index.html").exists() {
                "name: static site acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/public/index.html\"\n  - kind: shell\n    command: \"test -s public/index.html\"\n    cwd: \"{working_dir}\"\n"
            } else {
                "name: static site acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/index.html\"\n  - kind: shell\n    command: \"test -s index.html\"\n    cwd: \"{working_dir}\"\n"
            }
        }
        AcceptancePack::Browser => {
            "name: browser acceptance pack\nchecks:\n  - kind: shell\n    command: \"node .deadreckon/acceptance/browser-smoke.mjs\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Playwright => {
            "name: playwright acceptance pack\nchecks:\n  - kind: shell\n    command: \"npm run build --if-present && (npm run preview --if-present -- --host 127.0.0.1 > .deadreckon/acceptance/preview.log 2>&1 & pid=$!; trap 'kill $pid 2>/dev/null || true' EXIT; sleep 3; DEADRECKON_BASE_URL=${DEADRECKON_BASE_URL:-http://127.0.0.1:4173} npx --yes playwright test .deadreckon/acceptance/playwright-smoke.spec.mjs --reporter=line)\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Vite => {
            "name: vite acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/package.json\"\n  - kind: shell\n    command: \"npm run build --if-present\"\n    cwd: \"{working_dir}\"\n  - kind: shell\n    command: \"node .deadreckon/acceptance/browser-smoke.mjs dist\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::NextJs => {
            "name: nextjs acceptance pack\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/package.json\"\n  - kind: shell\n    command: \"npm run build --if-present\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Python => {
            "name: python acceptance pack\nchecks:\n  - kind: shell\n    command: \"python3 -m compileall .\"\n    cwd: \"{working_dir}\"\n"
        }
        AcceptancePack::Auto => unreachable!("auto pack resolved above"),
    }
    .to_string();

    let mut files = BTreeMap::new();
    if matches!(
        pack,
        AcceptancePack::Browser | AcceptancePack::Vite | AcceptancePack::Playwright
    ) {
        files.insert(
            PathBuf::from(".deadreckon/acceptance/browser-smoke.mjs"),
            browser_smoke_script().to_string(),
        );
    }
    if matches!(pack, AcceptancePack::Playwright) {
        files.insert(
            PathBuf::from(".deadreckon/acceptance/playwright-smoke.spec.mjs"),
            playwright_smoke_spec().to_string(),
        );
    }
    Ok(AcceptanceDraft {
        markdown: format!(
            "# Acceptance Criteria\n\nAdded the `{}` pack.\n\n{}",
            pack.name(),
            acceptance_markdown_from_yaml(&yaml)
        ),
        yaml,
        files,
    })
}

fn append_acceptance_checks(existing_raw: &str, addition_raw: &str) -> Result<String> {
    let mut existing = acceptance_yaml_value(existing_raw)?;
    let addition = acceptance_yaml_value(addition_raw)?;
    let mut checks = yaml_mapping_get(&addition, "checks")
        .map(yaml_items)
        .unwrap_or_default()
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    if checks.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "acceptance pack did not contain checks",
            "try `deadreckon done add browser` or `deadreckon done \"what should count as done\"`",
        )));
    }
    let mapping = existing.as_mapping_mut().ok_or_else(|| {
        CliError::Core(deadreckon_core::user_error(
            "acceptance.yaml must be a mapping",
            "run `deadreckon done \"what should count as done\" --force`",
        ))
    })?;
    let key = serde_yaml::Value::String("checks".to_string());
    let entry = mapping
        .entry(key)
        .or_insert_with(|| serde_yaml::Value::Sequence(Vec::new()));
    match entry {
        serde_yaml::Value::Sequence(existing_checks) => existing_checks.append(&mut checks),
        other => {
            let mut merged = yaml_items(other).into_iter().cloned().collect::<Vec<_>>();
            merged.append(&mut checks);
            *other = serde_yaml::Value::Sequence(merged);
        }
    }
    serde_yaml::to_string(&existing).map_err(|source| {
        CliError::Core(deadreckon_core::user_error(
            &format!("failed to write acceptance.yaml: {source}"),
            "run `deadreckon done \"what should count as done\" --force`",
        ))
    })
}

fn browser_smoke_script() -> &'static str {
    r#"#!/usr/bin/env node
const fs = require('fs');
const http = require('http');
const path = require('path');

const requestedRoot = process.argv[2];
const candidates = [requestedRoot, 'dist', 'build', 'public', '.'].filter(Boolean);
const root = candidates.find((candidate) => fs.existsSync(path.join(candidate, 'index.html')));
if (!root) {
  console.error('No index.html found in dist, build, public, or project root.');
  process.exit(1);
}

const mime = new Map([
  ['.html', 'text/html'],
  ['.js', 'text/javascript'],
  ['.css', 'text/css'],
  ['.json', 'application/json'],
  ['.svg', 'image/svg+xml'],
]);

const server = http.createServer((req, res) => {
  const urlPath = decodeURIComponent((req.url || '/').split('?')[0]);
  const safePath = path.normalize(urlPath === '/' ? '/index.html' : urlPath).replace(/^(\.\.[/\\])+/, '');
  const file = path.join(root, safePath);
  if (!file.startsWith(path.resolve(root)) && path.isAbsolute(file)) {
    res.writeHead(403);
    res.end('forbidden');
    return;
  }
  fs.readFile(file, (err, body) => {
    if (err) {
      res.writeHead(404);
      res.end('missing');
      return;
    }
    res.writeHead(200, { 'content-type': mime.get(path.extname(file)) || 'application/octet-stream' });
    res.end(body);
  });
});

server.listen(0, '127.0.0.1', async () => {
  const { port } = server.address();
  try {
    const response = await fetch(`http://127.0.0.1:${port}/`);
    const body = await response.text();
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    if (!/<html[\s>]/i.test(body)) throw new Error('index response did not look like HTML');
    console.log(`loaded ${root}/index.html over HTTP`);
  } catch (error) {
    console.error(error.message || String(error));
    process.exitCode = 1;
  } finally {
    server.close();
  }
});
"#
}

fn playwright_smoke_spec() -> &'static str {
    r#"import { test, expect } from '@playwright/test';

test('app loads without browser console errors', async ({ page }) => {
  const errors = [];
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto(process.env.DEADRECKON_BASE_URL || 'http://127.0.0.1:4173/', {
    waitUntil: 'networkidle',
  });
  await expect(page.locator('body')).toBeVisible();
  expect(errors).toEqual([]);
});
"#
}

fn acceptance_markdown_from_yaml(raw: &str) -> String {
    match acceptance_check_count(raw) {
        Ok(count) => format!(
            "Configured checks: {count}. Run `deadreckon done check` before starting long work."
        ),
        Err(_) => "Run `deadreckon done check` before starting long work.".to_string(),
    }
}

fn print_acceptance_yaml_summary(raw: &str) -> Result<()> {
    let root = acceptance_yaml_value(raw)?;
    println!("{}", ui_heading("checks"));
    for line in acceptance_summary_lines(&root) {
        println!("  {line}");
    }
    Ok(())
}

fn acceptance_summary_lines(root: &serde_yaml::Value) -> Vec<String> {
    let mut lines = Vec::new();
    for key in [
        "checks",
        "required",
        "optional",
        "tests",
        "file-exists",
        "content-match",
        "build-success",
    ] {
        if let Some(value) = yaml_mapping_get(root, key) {
            for item in yaml_items(value) {
                lines.push(describe_acceptance_item(key, item));
            }
        }
    }
    if lines.is_empty() {
        lines.push("no recognized checks".to_string());
    }
    lines
}

fn describe_acceptance_item(group: &str, item: &serde_yaml::Value) -> String {
    if let Some(command) = item.as_str() {
        return format!("{group}: shell {}", one_line(command, 96));
    }
    let Some(mapping) = item.as_mapping() else {
        return format!("{group}: {}", one_line(&format!("{item:?}"), 120));
    };
    let kind = yaml_mapping_get(item, "kind")
        .and_then(serde_yaml::Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            if mapping.len() == 1 {
                mapping
                    .keys()
                    .next()
                    .and_then(serde_yaml::Value::as_str)
                    .map(ToString::to_string)
            } else {
                None
            }
        })
        .unwrap_or_else(|| group.to_string());
    let path = yaml_mapping_get(item, "path")
        .and_then(serde_yaml::Value::as_str)
        .or_else(|| yaml_mapping_get(item, "cwd").and_then(serde_yaml::Value::as_str));
    let command = yaml_mapping_get(item, "command").and_then(serde_yaml::Value::as_str);
    let pattern = yaml_mapping_get(item, "pattern").and_then(serde_yaml::Value::as_str);
    let detail = command
        .or(path)
        .or(pattern)
        .map(|value| one_line(value, 96))
        .unwrap_or_else(|| one_line(&format!("{item:?}"), 96));
    format!("{group}: {kind} {detail}")
}

fn acceptance_check_count(raw: &str) -> Result<usize> {
    let root = acceptance_yaml_value(raw)?;
    let mut count = 0;
    for key in [
        "checks",
        "required",
        "optional",
        "tests",
        "file-exists",
        "content-match",
        "build-success",
    ] {
        if let Some(value) = yaml_mapping_get(&root, key) {
            count += yaml_items(value).len();
        }
    }
    if count == 0 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "acceptance.yaml does not contain any recognized checks",
            "run `deadreckon done \"what should count as done\"`",
        )));
    }
    Ok(count)
}

fn acceptance_yaml_value(raw: &str) -> Result<serde_yaml::Value> {
    serde_yaml::from_str(raw).map_err(|source| {
        CliError::Core(deadreckon_core::user_error(
            &format!("invalid acceptance.yaml: {source}"),
            "deadreckon done \"what should count as done\"",
        ))
    })
}

fn yaml_mapping_get<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    value
        .as_mapping()?
        .get(serde_yaml::Value::String(key.to_string()))
}

fn yaml_items(value: &serde_yaml::Value) -> Vec<&serde_yaml::Value> {
    match value {
        serde_yaml::Value::Sequence(items) => items.iter().collect(),
        serde_yaml::Value::Null => Vec::new(),
        value => vec![value],
    }
}

fn read_optional_text(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(CliError::Io(source)),
    }
}

fn ensure_trailing_newline(value: &str) -> String {
    let mut value = value.trim().to_string();
    value.push('\n');
    value
}

fn acceptance_status_line(state: &deadreckon_core::PipelineState) -> String {
    let marker_path = marker_path_for_run_root(&state.run_root);
    if marker_path.exists()
        && let Ok(bytes) = fs::read(&marker_path)
        && let Ok(marker) = serde_json::from_slice::<AcceptanceMarker>(&bytes)
    {
        return format!("passed by dr-gate ({} checks)", marker.check_count);
    }
    let spec_path = acceptance_spec_path_for_run_root(&state.run_root);
    if spec_path.exists()
        && let Ok(raw) = fs::read_to_string(&spec_path)
        && let Ok(count) = acceptance_check_count(&raw)
    {
        return format!("configured ({count} checks)");
    }
    "default dr-gate behavior".to_string()
}

#[derive(Debug, Default)]
struct ConfigDefaults {
    provider: Option<String>,
    sandbox: Option<String>,
    max_spend: Option<f64>,
    cli_max_wall_seconds: Option<f64>,
    doc_provider: Option<String>,
    doc_skill: Option<String>,
    doc_subskills: Option<Vec<String>>,
    doc_polish_token_budget: Option<u32>,
    doc_polish_budget_cap_usd: Option<f64>,
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
        doc_subskills: get_toml_path(&root, "defaults.doc_subskills")
            .and_then(toml::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            }),
        doc_polish_token_budget: get_toml_path(&root, "defaults.doc_polish_token_budget")
            .and_then(toml::Value::as_integer)
            .and_then(|value| u32::try_from(value).ok()),
        doc_polish_budget_cap_usd: get_toml_path(&root, "defaults.doc_polish_budget_cap_usd")
            .and_then(toml::Value::as_float)
            .or_else(|| {
                get_toml_path(&root, "defaults.doc_polish_budget_cap_usd")
                    .and_then(toml::Value::as_integer)
                    .map(|value| value as f64)
            }),
    })
}

fn resolve_doc_provider(
    flag: Option<&str>,
    defaults: &ConfigDefaults,
    run_provider: Option<&str>,
) -> DocProviderSelection {
    if let Some(provider) = flag.filter(|provider| !provider.trim().is_empty()) {
        return DocProviderSelection {
            provider: Some(provider.to_string()),
            source: DocProviderSource::Flag,
        };
    }
    if let Some(provider) = defaults.doc_provider.as_deref() {
        return DocProviderSelection {
            provider: Some(provider.to_string()),
            source: DocProviderSource::Config,
        };
    }
    if command_exists("codex") {
        return DocProviderSelection {
            provider: Some("cli:codex".to_string()),
            source: DocProviderSource::AutoSubscription,
        };
    }
    if command_exists("claude") {
        return DocProviderSelection {
            provider: Some("cli:claude-code".to_string()),
            source: DocProviderSource::AutoSubscription,
        };
    }
    if let Some(provider) = run_provider {
        return DocProviderSelection {
            provider: Some(provider.to_string()),
            source: DocProviderSource::RunProvider,
        };
    }
    DocProviderSelection {
        provider: None,
        source: DocProviderSource::None,
    }
}

fn effective_doc_subskills(defaults: &ConfigDefaults) -> Vec<String> {
    defaults.doc_subskills.clone().unwrap_or_else(|| {
        DEFAULT_DOC_SUBSKILLS
            .iter()
            .map(|skill| (*skill).to_string())
            .collect()
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
    doc_provider: Option<&'a str>,
    doc_provider_source: &'a str,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    acceptance: &'a AcceptancePreview,
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
        doc_provider,
        doc_provider_source,
        max_spend,
        max_wall_seconds,
        acceptance,
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
            "mode={} branch={} base={} wt={} provider={} model={} docs={} cap={}/{} acceptance={}",
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
            doc_provider.unwrap_or("templated"),
            max_spend
                .map(|cap| format!("${cap:.0}"))
                .unwrap_or_else(|| "uncapped".to_string()),
            format_wall_cap(max_wall_seconds),
            acceptance.brief_label()
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
        format!(
            "  docs:     {} ({doc_provider_source})",
            doc_provider.unwrap_or("templated only")
        ),
        format!("  sandbox:  {sandbox}"),
        format!("  caps:     {caps}"),
        format!("  gate:     {}", acceptance.full_label()),
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
        "default_provider = \"{provider}\"\nfallback = {fallback}\n\n[defaults]\nprovider = \"{provider}\"\ndoc_provider = \"{provider}\"\ndoc_skill = \"run-narrator\"\ndoc_subskills = [\"narrator-overview\", \"narrator-phases\", \"narrator-as-built\", \"narrator-decisions\"]\ndoc_polish_token_budget = 16384\nmax_spend = {max_spend}\ncli_max_wall_seconds = 3600\nsandbox = \"{sandbox}\"\n\n"
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

#[allow(clippy::too_many_arguments)]
fn finish_command(
    run_id: Option<String>,
    dest: Option<PathBuf>,
    force: bool,
    include_manifest: bool,
    strategy: String,
    branch: Option<String>,
    autostash: bool,
    cleanup: bool,
    no_confirm: bool,
    message: Option<String>,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let requested = run_id.unwrap_or_else(|| "latest".to_string());
    let state = load_cli_run(&paths, &requested)?;
    match state.status {
        RunStatus::Completed => {}
        RunStatus::Pending | RunStatus::Planned | RunStatus::Executing => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("run {} is still {}", state.run_id, state.status),
                &format!("deadreckon attach {}", run_prefix(&state.run_id)),
            )));
        }
        RunStatus::Failed | RunStatus::Killed => {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!("run {} is {}", state.run_id, state.status),
                &format!("deadreckon resume {}", run_prefix(&state.run_id)),
            )));
        }
    }

    let mode = read_codebase_record(&state.working_dir)
        .map(|record| record.mode)
        .unwrap_or(CodebaseMode::Fresh);
    match mode {
        CodebaseMode::Worktree => {
            println!(
                "{} {}",
                ui_heading("finish:"),
                ui_command(format!("deadreckon apply {}", run_prefix(&state.run_id)))
            );
            apply_command(
                state.run_id,
                strategy,
                branch,
                no_confirm,
                autostash,
                cleanup,
                message,
            )
        }
        CodebaseMode::Copy | CodebaseMode::Fresh => {
            println!(
                "{} {}",
                ui_heading("finish:"),
                ui_command(format!("deadreckon export {}", run_prefix(&state.run_id)))
            );
            materialize_completed_run(&paths, &state, dest, force, include_manifest)
                .map(|materialized| print_materialized(&materialized))
        }
        CodebaseMode::InPlace => {
            println!(
                "{} {}",
                ui_ok("finished in-place run"),
                ui_id(&state.run_id)
            );
            println!("  working: {}", state.working_dir.display());
            println!(
                "  review:  {}",
                ui_command(format!("deadreckon show {}", run_prefix(&state.run_id)))
            );
            println!(
                "  docs:    {}",
                ui_command(format!("deadreckon doc {}", run_prefix(&state.run_id)))
            );
            println!(
                "  undo:    {}",
                ui_command(format!(
                    "deadreckon undo --run {}",
                    run_prefix(&state.run_id)
                ))
            );
            Ok(())
        }
    }
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
    apply_command_inner(
        run_id,
        strategy,
        target_branch,
        no_confirm,
        autostash,
        cleanup,
        message,
        false,
    )
}

fn apply_command_quiet(
    run_id: String,
    strategy: String,
    target_branch: Option<String>,
    no_confirm: bool,
    autostash: bool,
    cleanup: bool,
    message: Option<String>,
) -> Result<()> {
    apply_command_inner(
        run_id,
        strategy,
        target_branch,
        no_confirm,
        autostash,
        cleanup,
        message,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn apply_command_inner(
    run_id: String,
    strategy: String,
    target_branch: Option<String>,
    no_confirm: bool,
    autostash: bool,
    cleanup: bool,
    message: Option<String>,
    quiet: bool,
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
        if !quiet {
            print_already_applied(&state, branch, &target);
        }
        finish_apply_cleanup(&state, &record, cleanup, no_confirm, quiet)?;
        return Ok(());
    }
    if !quiet {
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
            let staged_stat = git_stdout(git_root, &["diff", "--cached", "--stat"])?;
            if staged_stat.trim().is_empty() {
                if let Some(stash) = autostash.as_ref() {
                    restore_apply_autostash(git_root, &state.run_id, stash)?;
                }
                if !quiet {
                    print_already_applied(&state, branch, &target);
                }
                finish_apply_cleanup(&state, &record, cleanup, no_confirm, quiet)?;
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
    if !quiet {
        println!(
            "{} {} onto {}",
            ui_ok("applied"),
            ui_id(&state.run_id),
            target
        );
        println!("{}", git_stdout(git_root, &["log", "-1", "--stat"])?);
    }
    finish_apply_cleanup(&state, &record, cleanup, no_confirm, quiet)
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
    quiet: bool,
) -> Result<()> {
    let cleanup_now = cleanup || should_prompt_cleanup(no_confirm)?;
    if cleanup_now {
        cleanup_worktree_run(state, record, false, false)?;
    } else if !quiet {
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
    let effective_provider = provider.or(defaults.provider.clone());
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
    let doc_provider_selection =
        resolve_doc_provider(None, &defaults, effective_provider.as_deref());
    let doc_subskills = effective_doc_subskills(&defaults);
    let doc_token_budget = defaults
        .doc_polish_token_budget
        .unwrap_or(DEFAULT_DOC_POLISH_TOKEN_BUDGET);
    let doc_budget_cap = defaults.doc_polish_budget_cap_usd;
    let sandbox = sandbox
        .or(defaults.sandbox.clone())
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
            doc_provider: doc_provider_selection.provider.clone(),
            doc_provider_source: Some(doc_provider_selection.source.as_str().to_string()),
            doc_subskills: doc_subskills.clone(),
            doc_token_budget,
            doc_budget_cap,
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
                doc_provider: doc_provider_selection.provider.clone(),
                doc_provider_source: Some(doc_provider_selection.source.as_str().to_string()),
                doc_subskills,
                token_budget: doc_token_budget,
                budget_cap_usd: doc_budget_cap,
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
    doc_provider_source: Option<String>,
    doc_subskills: Vec<String>,
    doc_token_budget: u32,
    doc_budget_cap: Option<f64>,
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
        doc_provider_source,
        doc_subskills,
        doc_token_budget,
        doc_budget_cap,
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
                doc_provider_source,
                doc_subskills,
                token_budget: doc_token_budget,
                budget_cap_usd: doc_budget_cap,
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
            CodebaseMode::Worktree => "finish (apply)".to_string(),
            CodebaseMode::Copy | CodebaseMode::Fresh => "finish (export)".to_string(),
            CodebaseMode::InPlace => "finish (review)".to_string(),
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

struct DocCommandArgs {
    run_id: String,
    kind: DocKind,
    export: Option<PathBuf>,
    polish: bool,
    no_confirm: bool,
    force: bool,
    doc_skill: Option<String>,
    doc_provider: Option<String>,
    budget_cap: Option<f64>,
}

async fn doc_command(args: DocCommandArgs) -> Result<()> {
    let DocCommandArgs {
        run_id,
        kind,
        export,
        polish,
        no_confirm,
        force,
        doc_skill,
        doc_provider,
        budget_cap,
    } = args;
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
        let selection = resolve_doc_provider(
            doc_provider.as_deref(),
            &defaults,
            state.provider.as_deref(),
        );
        let Some(provider) = selection.provider.clone() else {
            return Err(CliError::Core(deadreckon_core::user_error(
                "deadreckon: no doc provider available",
                "deadreckon config set defaults.doc_provider cli:codex\ntry: install codex or claude; deadreckon auto-detects subscription CLIs on the next run",
            )));
        };
        let subskills = effective_doc_subskills(&defaults);
        let token_budget = defaults
            .doc_polish_token_budget
            .unwrap_or(DEFAULT_DOC_POLISH_TOKEN_BUDGET);
        let budget_cap = budget_cap.or(defaults.doc_polish_budget_cap_usd);
        let router = ProviderRouter::from_config_path(&paths.config_path(), Some(&provider))?;
        let estimated_spend =
            estimate_doc_polish_spend(&router, &provider, token_budget, subskills.len())?;
        if let Some(cap) = budget_cap
            && estimated_spend.cost_usd > cap
        {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "doc polish would cost about ${:.6}, above cap ${cap:.6}",
                    estimated_spend.cost_usd
                ),
                &format!(
                    "deadreckon doc {} --polish --budget-cap {:.2} --no-confirm",
                    state.run_id, estimated_spend.cost_usd
                ),
            )));
        }
        if !no_confirm && completion_hints_enabled(false) && io::stdin().is_terminal() {
            print_doc_polish_preview(
                &state,
                &provider,
                selection.source.as_str(),
                &subskills,
                token_budget,
                budget_cap,
                &estimated_spend,
            )?;
            let answer = prompt("polish docs now? [Y/n]: ")?;
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
        polish_run_docs(
            &mut state,
            &router,
            &PolishConfig {
                home: paths.home().to_path_buf(),
                doc_skill: doc_skill
                    .or(defaults.doc_skill)
                    .unwrap_or_else(|| "run-narrator".to_string()),
                doc_provider: Some(provider),
                doc_provider_source: Some(selection.source.as_str().to_string()),
                doc_subskills: subskills,
                token_budget,
                budget_cap_usd: budget_cap,
                no_llm: false,
                force,
            },
        )
        .await?;
        if completion_hints_enabled(false)
            && let Some(record) = deadreckon_runtime::read_polish_record(&state)?
        {
            print_doc_polish_summary(&record);
        }
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

fn print_doc_polish_preview(
    state: &deadreckon_core::PipelineState,
    provider: &str,
    provider_source: &str,
    subskills: &[String],
    token_budget: u32,
    budget_cap: Option<f64>,
    estimated_spend: &SpendEstimate,
) -> Result<()> {
    print!(
        "{}",
        doc_polish_preview_text(
            state,
            provider,
            provider_source,
            subskills,
            token_budget,
            budget_cap,
            estimated_spend,
        )?
    );
    Ok(())
}

fn doc_polish_preview_text(
    state: &deadreckon_core::PipelineState,
    provider: &str,
    provider_source: &str,
    subskills: &[String],
    token_budget: u32,
    budget_cap: Option<f64>,
    estimated_spend: &SpendEstimate,
) -> Result<String> {
    let hash = deadreckon_runtime::inputs_hash(state)?;
    let mut out = String::new();
    out.push_str(&format!("{}\n", ui_heading("polish preview:")));
    out.push_str(&format!(
        "  provider:  {} ({provider_source})\n",
        ui_id(provider)
    ));
    out.push_str(&format!("  subskills: {}\n", subskills.join(", ")));
    out.push_str(&format!(
        "  budget:    {} tokens per subcall\n",
        token_budget
    ));
    out.push_str(&format!(
        "  estimate:  {}\n",
        doc_polish_cost_label(estimated_spend)
    ));
    out.push_str(&format!(
        "  cost cap:  {}\n",
        budget_cap
            .map(|cap| format!("${cap:.2}"))
            .unwrap_or_else(|| "provider/account default".to_string())
    ));
    out.push_str(&format!("  inputs:    {}\n", &hash[..hash.len().min(12)]));
    Ok(out)
}

fn estimate_doc_polish_spend(
    router: &ProviderRouter,
    provider: &str,
    token_budget: u32,
    subskill_count: usize,
) -> Result<SpendEstimate> {
    let calls = subskill_count.max(1) as u64;
    router
        .estimate_for_route(
            Some(provider),
            ProviderUsage {
                input_tokens: 0,
                output_tokens: u64::from(token_budget) * calls,
            },
        )
        .map_err(CliError::from)
}

fn doc_polish_cost_label(estimate: &SpendEstimate) -> String {
    if estimate.subscription {
        "$0.00 (subscription)".to_string()
    } else {
        format!(
            "${:.6} for up to {} output tokens",
            estimate.cost_usd, estimate.output_tokens
        )
    }
}

fn print_doc_polish_summary(record: &deadreckon_runtime::PolishRecord) {
    println!("{}", ui_heading("doc polish:"));
    println!("  status:   {}", record.status);
    if let Some(provider) = record.provider.as_deref() {
        println!("  provider: {provider}");
    }
    println!("  cost:     ${:.6}", record.cost_usd);
    if !record.subcalls.is_empty() {
        println!("  subcalls:");
        for subcall in &record.subcalls {
            println!(
                "    {} {:<18} {} in / {} out ${:.6}",
                ui_status(&subcall.status),
                subcall.skill,
                subcall.tokens_in,
                subcall.tokens_out,
                subcall.cost_usd
            );
        }
    }
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
    let doc_provider_selection = resolve_doc_provider(None, &defaults, provider.as_deref());
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
                doc_provider: doc_provider_selection.provider,
                doc_provider_source: Some(doc_provider_selection.source.as_str().to_string()),
                doc_subskills: effective_doc_subskills(&defaults),
                token_budget: defaults
                    .doc_polish_token_budget
                    .unwrap_or(DEFAULT_DOC_POLISH_TOKEN_BUDGET),
                budget_cap_usd: defaults.doc_polish_budget_cap_usd,
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
    println!("Acceptance {}", acceptance_status_line(&state));
    let run_acceptance = acceptance_spec_path_for_run_root(&state.run_root);
    if run_acceptance.exists() {
        println!("AcceptanceSpec {}", run_acceptance.display());
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
    if let Ok(Some(line)) = chain_context_line_for_working(&state.working_dir) {
        println!("  chain:    {line}");
    }
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
    println!("  gate:     {}", acceptance_status_line(state));
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
        if let Ok(Some(marker)) = read_chain_step_marker(&state.working_dir) {
            println!(
                "[c] Chain deadreckon chain attach {}",
                chain_prefix(&marker.chain_id)
            );
        }
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

fn print_run_started(
    state: &deadreckon_core::PipelineState,
    route: Option<&ProviderRouteInfo>,
    doc_provider: Option<&str>,
    doc_provider_source: &str,
) {
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
        "docs {} ({doc_provider_source})",
        doc_provider.unwrap_or("templated only")
    );
    println!(
        "{} {}",
        ui_command("attach:"),
        ui_command(format!("deadreckon attach {}", run_prefix(&state.run_id)))
    );
    println!("state {}", state.state_path().display());
    let _ = io::stdout().flush();
}

fn print_lifecycle_hints(state: &deadreckon_core::PipelineState) {
    if let Ok(record) = read_codebase_record(&state.working_dir)
        && record.mode == CodebaseMode::Worktree
    {
        println!("{}", ui_heading("next actions:"));
        println!(
            "  finish:  {}",
            ui_command(format!(
                "deadreckon finish {} --autostash --cleanup",
                run_prefix(&state.run_id)
            ))
        );
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
        "  finish: {}",
        ui_command(format!(
            "deadreckon finish {} --dest ./{}",
            run_prefix(&state.run_id),
            task_prefix
        ))
    );
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
                Box::pin(doc_command(DocCommandArgs {
                    run_id: state.run_id.clone(),
                    kind: DocKind::Narrative,
                    export: None,
                    polish: false,
                    no_confirm: true,
                    force: false,
                    doc_skill: None,
                    doc_provider: None,
                    budget_cap: None,
                }))
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
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.is_empty()
                        && read_chain_step_marker(&state.working_dir)?.is_some() =>
                {
                    if let Some(marker) = read_chain_step_marker(&state.working_dir)? {
                        suspend_tui(&mut terminal)?;
                        let action = chain_attach_command(paths, &marker.chain_id, false);
                        if let Err(err) = &action {
                            eprintln!("error: {err}");
                        }
                        resume_tui(&mut terminal)?;
                    }
                }
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
            Box::pin(doc_command(DocCommandArgs {
                run_id: state.run_id.clone(),
                kind: DocKind::Narrative,
                export: None,
                polish: false,
                no_confirm: true,
                force: false,
                doc_skill: None,
                doc_provider: None,
                budget_cap: None,
            }))
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
    let chain_line = chain_context_line_for_working(&state.working_dir)
        .ok()
        .flatten()
        .map(|line| format!("{line}\n"))
        .unwrap_or_default();
    let header = Paragraph::new(format!(
        "{}run {}  status {}  phase {}  turn {}  provider {}  sandbox {}\ngoal {}\n{} {}",
        chain_line,
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
    let chain_suffix = if read_chain_step_marker(&state.working_dir)
        .ok()
        .flatten()
        .is_some()
    {
        "  [c] Chain"
    } else {
        ""
    };
    let base = if tui_state.show_completion_actions && state.status == RunStatus::Completed {
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
    };
    format!("{base}{chain_suffix}")
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
        deadreckon_core::RunEventKind::DocsCheckpoint { turn, path, status } => {
            format!("turn {turn} docs {status} {}", path.display())
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
    } else if ratio >= 0.6 {
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
        AttachPanel, AttachPanelCounts, AttachPanelRows, AttachTuiState, ChainAttachTuiState,
        CompletionAction, chain_activity_lines, chain_attach_footer_text, chain_attach_header_text,
        chain_should_auto_attach, chain_timeline_lines, chain_wall_cap_hit,
        completion_action_from_input, completion_hints_enabled, doc_polish_preview_text,
        markdown_to_tui_lines, max_panel_scroll, per_step_wall_cap, threshold_color,
    };
    use chrono::Utc;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use deadreckon_core::{
        ApplyMode, ApplyStrategy, BranchPolicy, Chain, ChainEvent, ChainEventKind, ChainNewOptions,
        ChainStatus, ChainStepStatus, DeadreckonPaths, OnFail, RunOptions, create_run,
    };
    use deadreckon_providers::SpendEstimate;
    use ratatui::style::{Color, Modifier};
    use ratatui::text::Line;

    fn chain_fixture() -> Chain {
        let mut chain = Chain::new(ChainNewOptions {
            root_goal: "build app".to_string(),
            goals: vec!["first step".to_string(), "second step".to_string()],
            scope: "scope".to_string(),
            base_branch: "main".to_string(),
            base_sha: "abcdef123456".to_string(),
            cwd: std::path::PathBuf::from("/tmp/project"),
            provider: Some("smoke".to_string()),
            model: None,
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
        .expect("chain");
        chain.steps[0].status = ChainStepStatus::Applied;
        chain.steps[0].run_id = Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string());
        chain.steps[1].status = ChainStepStatus::Running;
        chain
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<Vec<_>>()
            .join("")
    }

    fn doc_preview_state() -> (tempfile::TempDir, deadreckon_core::PipelineState) {
        std::fs::create_dir_all("/Users/gdc/deadreckon/.test-tmp").expect("test tmp");
        let temp = tempfile::TempDir::new_in("/Users/gdc/deadreckon/.test-tmp").expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "preview docs".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: Some("cli:codex".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("state");
        (temp, state)
    }

    #[test]
    fn polish_preview_block_lists_provider_and_subskills() {
        let (_temp, state) = doc_preview_state();
        let estimate = SpendEstimate {
            provider: "cli:codex".to_string(),
            model: "provider default".to_string(),
            input_tokens: 0,
            output_tokens: 65_536,
            cost_usd: 0.0,
            subscription: true,
            wall_time_seconds: None,
        };
        let text = doc_polish_preview_text(
            &state,
            "cli:codex",
            "auto_subscription",
            &[
                "narrator-overview".to_string(),
                "narrator-phases".to_string(),
                "narrator-as-built".to_string(),
                "narrator-decisions".to_string(),
            ],
            16_384,
            Some(0.0),
            &estimate,
        )
        .expect("preview");
        assert!(text.contains("provider:"));
        assert!(text.contains("cli:codex"));
        assert!(text.contains("narrator-overview, narrator-phases"));
        assert!(text.contains("$0.00 (subscription)"));
    }

    #[test]
    fn polish_preview_suppressed_by_hints_env() {
        assert!(!completion_hints_enabled(true));
    }

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
    fn chain_attach_renders_step_timeline_with_status_dots() {
        let chain = chain_fixture();
        let tui_state = ChainAttachTuiState::default();

        let lines = chain_timeline_lines(&chain, &tui_state)
            .iter()
            .map(line_text)
            .collect::<Vec<_>>();

        assert!(lines[0].contains("● step  1 applied"));
        assert!(lines[0].contains("run aaaaaaaa"));
        assert!(lines[1].contains("● step  2 running"));
    }

    #[test]
    fn chain_attach_header_shows_policy_apply_mode_on_fail() {
        let chain = chain_fixture();
        let header = chain_attach_header_text(&chain);

        assert!(header.contains("status pending"));
        assert!(header.contains("policy branch=stack apply=auto strategy=squash on-fail=stop"));
        assert!(header.contains("spend $0.000000/$5.000000"));
    }

    #[test]
    fn chain_attach_activity_lists_newest_events_first() {
        let chain = chain_fixture();
        let events = vec![
            ChainEvent {
                timestamp: Utc::now(),
                chain_id: chain.chain_id.clone(),
                event: ChainEventKind::ChainCreated,
                step_index: None,
                detail: serde_json::json!({ "goal": "build app" }),
            },
            ChainEvent {
                timestamp: Utc::now(),
                chain_id: chain.chain_id.clone(),
                event: ChainEventKind::ChainStepStarted,
                step_index: Some(1),
                detail: serde_json::json!({ "goal": "second step" }),
            },
        ];

        let lines = chain_activity_lines(&events, &ChainAttachTuiState::default())
            .iter()
            .map(line_text)
            .collect::<Vec<_>>();

        assert!(lines[0].contains("step started step 2"));
        assert!(lines[1].contains("created"));
    }

    #[test]
    fn chain_attach_paused_footer_lists_try_lines() {
        let mut chain = chain_fixture();
        chain.status = ChainStatus::Paused;
        chain.paused_reason = Some("apply_refused_conflict".to_string());

        let footer = chain_attach_footer_text(&chain);

        assert!(footer.contains("paused: apply_refused_conflict"));
        assert!(footer.contains("show --why-failed"));
        assert!(footer.contains("resume --apply-mode preview"));
        assert!(footer.contains("undo"));
    }

    #[test]
    fn chain_default_auto_attaches_when_stdout_tty() {
        assert!(chain_should_auto_attach(true, false, false, false));
        assert!(!chain_should_auto_attach(false, false, false, false));
        assert!(!chain_should_auto_attach(true, true, false, false));
        assert!(!chain_should_auto_attach(true, false, true, false));
        assert!(!chain_should_auto_attach(true, false, false, true));
    }

    #[test]
    fn chain_attach_budget_bar_thresholds_60_80_percent() {
        assert_eq!(threshold_color(0.59), Color::Green);
        assert_eq!(threshold_color(0.60), Color::Yellow);
        assert_eq!(threshold_color(0.79), Color::Yellow);
        assert_eq!(threshold_color(0.80), Color::Red);
    }

    #[test]
    fn chain_attach_shows_aggregate_spend_in_header() {
        let mut chain = chain_fixture();
        chain.total_spend_usd = 1.25;
        let header = chain_attach_header_text(&chain);

        assert!(header.contains("spend $1.250000/$5.000000"), "{header}");
    }

    #[test]
    fn chain_attach_focused_step_streams_provider_activity() {
        let chain = chain_fixture();
        let events = vec![ChainEvent {
            timestamp: Utc::now(),
            chain_id: chain.chain_id.clone(),
            event: ChainEventKind::ChainRunCompleted,
            step_index: Some(1),
            detail: serde_json::json!({ "run_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "status": "completed" }),
        }];

        let lines = chain_activity_lines(&events, &ChainAttachTuiState::default())
            .iter()
            .map(line_text)
            .collect::<Vec<_>>();

        assert!(lines[0].contains("run completed step 2"), "{lines:?}");
        assert!(lines[0].contains("bbbbbbbb"), "{lines:?}");
    }

    #[test]
    fn chain_attach_tab_pages_focus_between_steps() {
        let chain = chain_fixture();
        let mut tui_state = ChainAttachTuiState::default();

        tui_state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE), &chain);
        assert_eq!(tui_state.selected_step, 1);
    }

    #[test]
    fn chain_attach_ctrl_d_detaches_does_not_kill_conductor() {
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);

        assert!(super::attach_should_quit(key));
    }

    #[test]
    fn chain_attach_enter_drills_to_single_run_tui_esc_returns() {
        let footer = chain_attach_footer_text(&chain_fixture());

        assert!(footer.contains("[Enter] drill"));
        assert!(footer.contains("detach"));
    }

    #[test]
    fn chain_attach_r_invokes_redo_with_confirm() {
        assert!(chain_attach_footer_text(&chain_fixture()).contains("[r] redo"));
    }

    #[test]
    fn chain_attach_e_invokes_extend_with_prompt() {
        assert!(chain_attach_footer_text(&chain_fixture()).contains("[e] extend"));
    }

    #[test]
    fn chain_attach_p_pauses_chain() {
        assert!(chain_attach_footer_text(&chain_fixture()).contains("[p] pause"));
    }

    #[test]
    fn chain_attach_k_kills_chain_with_confirm() {
        assert!(chain_attach_footer_text(&chain_fixture()).contains("[k] kill"));
    }

    #[test]
    fn chain_attach_plain_emits_periodic_snapshot_no_ansi() {
        let snapshot = chain_attach_header_text(&chain_fixture());

        assert!(!snapshot.contains("\u{1b}["), "{snapshot}");
        assert!(snapshot.contains("policy branch=stack"), "{snapshot}");
    }

    #[test]
    fn chain_attach_paused_footer_lists_four_try_lines() {
        let mut chain = chain_fixture();
        chain.status = ChainStatus::Paused;
        chain.paused_reason = Some("cap".to_string());

        let footer = chain_attach_footer_text(&chain);

        assert_eq!(footer.matches("try:").count(), 4, "{footer}");
    }

    #[test]
    fn chain_wall_clock_cap_pauses_chain() {
        let mut chain = chain_fixture();
        chain.max_wall_seconds = Some(10.0);
        chain.total_wall_seconds = 10.0;

        assert!(chain_wall_cap_hit(&chain));
    }

    #[test]
    fn chain_per_step_wall_cap_is_remaining_over_remaining_steps() {
        let mut chain = chain_fixture();
        chain.steps[0].status = ChainStepStatus::Pending;
        chain.steps[1].status = ChainStepStatus::Pending;
        chain.max_wall_seconds = Some(12.0);
        chain.total_wall_seconds = 2.0;

        assert_eq!(per_step_wall_cap(&chain, 0), Some(5.0));
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
