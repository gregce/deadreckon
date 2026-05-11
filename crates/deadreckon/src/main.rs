use std::collections::BTreeSet;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use chrono::Utc;
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use deadreckon_core::{
    DeadreckonError, DeadreckonPaths, PhaseId, PhaseStatus, ProvenanceRecord, RunLoopConfig,
    RunLoopOutcome, RunOptions, RunStatus, SpendRecord, TraceRecord, acquire_lock, append_trace,
    create_run, inventory_files, list_runs, load_run, release_lock_file, restore_snapshot,
    run_turn_loop, save_state, terminate_pid,
};
use deadreckon_providers::ProviderRouter;
use deadreckon_sandbox::SandboxBackend;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph};
use serde::de::DeserializeOwned;
use serde_json::json;
use tracing_subscriber::EnvFilter;

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
        CliError::Sandbox(_) => Some("run `deadreckon doctor` to inspect sandbox availability"),
        CliError::TomlDe(_) | CliError::TomlSer(_) => {
            Some("check /Users/gdc/.deadreckon/config.toml or rerun `deadreckon init`")
        }
        CliError::Io(_) => Some("check that the referenced path exists and is writable"),
        CliError::Json(_) => Some("inspect the referenced JSON file for invalid syntax"),
        CliError::Core(_) | CliError::Provider(_) => None,
    }
}

#[derive(Parser)]
#[command(
    name = "deadreckon",
    version,
    about = "Unattended agentic coding harness"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Init {
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long, default_value_t = 10.0)]
        max_spend: f64,
        #[arg(long, default_value = "auto")]
        sandbox: String,
        #[arg(long)]
        no_confirm: bool,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Run {
        goal: String,
        #[arg(long)]
        max_spend: Option<f64>,
        #[arg(long)]
        sandbox: Option<String>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long, default_value = "default-coding")]
        skill: String,
        #[arg(long)]
        smoke: bool,
        #[arg(long)]
        i_know_its_a_lot: bool,
        #[arg(long)]
        no_confirm: bool,
    },
    Doctor,
    List {
        #[arg(long)]
        scope: Option<String>,
    },
    Attach {
        run_id: String,
    },
    Kill {
        run_id: String,
    },
    Resume {
        run_id: String,
    },
    Undo {
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        turn: Option<u32>,
    },
    Show {
        run_id: String,
        #[arg(long)]
        turn: Option<u32>,
    },
    Import {
        source: String,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    Get { key: String },
    Set { key: String, value: String },
}

#[tokio::main]
async fn main() {
    if let Err(err) = main_inner().await {
        eprintln!("error: {err}");
        if let Some(hint) = error_hint(&err) {
            eprintln!("  hint: {hint}");
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
    match cli.command {
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
            max_spend,
            sandbox,
            provider,
            skill,
            smoke,
            i_know_its_a_lot,
            no_confirm,
        } => {
            run_command(
                goal,
                max_spend,
                sandbox,
                provider,
                skill,
                smoke,
                i_know_its_a_lot,
                no_confirm,
            )
            .await
        }
        Commands::Doctor => {
            doctor_command();
            Ok(())
        }
        Commands::List { scope } => list_command(scope),
        Commands::Attach { run_id } => attach_command(run_id),
        Commands::Kill { run_id } => kill_command(run_id),
        Commands::Resume { run_id } => resume_command(run_id).await,
        Commands::Undo { run, turn } => undo_command(run, turn),
        Commands::Show { run_id, turn } => show_command(run_id, turn),
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
    println!("wrote {}", paths.config_path().display());
    println!("next: deadreckon doctor");
    doctor_command();
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
            println!("set {key}");
        }
    }
    Ok(())
}

async fn run_command(
    goal: String,
    max_spend: Option<f64>,
    sandbox: Option<String>,
    provider: Option<String>,
    skill: String,
    smoke: bool,
    i_know_its_a_lot: bool,
    no_confirm: bool,
) -> Result<()> {
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
    let mut state = create_run(
        &paths,
        RunOptions {
            goal,
            cwd,
            sandbox: backend.to_string(),
            provider: effective_provider.clone(),
            skill_name: skill,
            max_spend_usd: effective_max_spend,
        },
    )?;
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
            sandbox_backend: backend,
            max_turns: 12,
        },
    )
    .await?;
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;

    match outcome {
        RunLoopOutcome::Done => println!("completed run {}", state.run_id),
        RunLoopOutcome::PausedAtCap => println!("paused run {}", state.run_id),
        RunLoopOutcome::Failed => println!("failed run {}", state.run_id),
    }
    println!("state {}", state.state_path().display());
    println!("working {}", state.working_dir.display());
    Ok(())
}

#[derive(Debug, Default)]
struct ConfigDefaults {
    provider: Option<String>,
    sandbox: Option<String>,
    max_spend: Option<f64>,
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
    println!("✓ source /Users/gdc/deadreckon");
    println!("✓ home {}", paths.home().display());
    for backend in deadreckon_sandbox::doctor() {
        if backend.available {
            let path = backend
                .path
                .as_ref()
                .map(|path| format!(" at {}", path.display()))
                .unwrap_or_default();
            println!("✓ sandbox {} found{}", backend.backend, path);
        } else {
            println!("✗ sandbox {} missing", backend.backend);
            println!("    fix: {}", backend.note);
        }
    }
    if paths.config_path().exists() {
        println!("✓ {} present", paths.config_path().display());
    } else {
        println!("✗ {} missing", paths.config_path().display());
        println!("    fix: deadreckon init");
    }
    let defaults = config_defaults(&paths).unwrap_or_default();
    if defaults.provider.is_some() || paths.config_path().exists() {
        println!("✓ provider defaults configured");
    } else if command_exists("claude") || command_exists("codex") {
        println!("✓ cli subscription provider available");
    } else {
        println!("✗ no provider configured");
        println!(
            "    fix: deadreckon init or deadreckon config set providers.anthropic.api_key <KEY>"
        );
    }
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
        "default_provider = \"{provider}\"\nfallback = {fallback}\n\n[defaults]\nprovider = \"{provider}\"\nmax_spend = {max_spend}\nsandbox = \"{sandbox}\"\n\n"
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

fn command_exists(name: &str) -> bool {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .any(|dir| {
            let path = dir.join(name);
            path.is_file()
        })
}

fn escape_toml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn list_command(scope: Option<String>) -> Result<()> {
    // REPORT.md: Workspace Inventory & Run Queue is a local scan over durable
    // runstate, not a live daemon query.
    let paths = DeadreckonPaths::discover();
    let runs = list_runs(&paths, scope.as_deref())?;
    if runs.is_empty() {
        println!("no runs");
        return Ok(());
    }
    for run in runs {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            run.run_id, run.status, run.scope, run.updated_at, run.goal
        );
    }
    Ok(())
}

fn attach_command(run_id: String) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    if io::stdout().is_terminal() {
        attach_tui(&paths, &run_id)?;
        return Ok(());
    }
    let state = load_run(&paths, &run_id)?;
    print_run_summary(&state);
    Ok(())
}

fn kill_command(run_id: String) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = load_run(&paths, &run_id)?;
    let pids = supervised_pids(&state);
    release_lock_file(&paths, &state.task_key)?;
    state.status = RunStatus::Killed;
    state.failure_reason = Some("killed by user".to_string());
    state.killed_at = Some(Utc::now());
    state.updated_at = Utc::now();
    save_state(&state)?;
    for pid in &pids {
        if *pid != std::process::id() {
            let _ = terminate_pid(*pid, false);
        }
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
    println!("killed run {}", state.run_id);
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

async fn resume_command(run_id: String) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = load_run(&paths, &run_id)?;
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
    state.child_pids = vec![std::process::id()];
    state.updated_at = Utc::now();
    save_state(&state)?;
    lock.heartbeat("resume-turn-loop")?;
    let provider = state.provider.clone();
    let backend: SandboxBackend = state.sandbox.parse()?;
    let router = ProviderRouter::from_config_path(&paths.config_path(), provider.as_deref())?;
    let max_spend_usd = state.max_spend_usd;
    let outcome = run_turn_loop(
        &mut state,
        &router,
        RunLoopConfig {
            provider,
            max_spend_usd,
            sandbox_backend: backend,
            max_turns: 12,
        },
    )
    .await?;
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;
    match outcome {
        RunLoopOutcome::Done => println!("resumed run {} to completion", state.run_id),
        RunLoopOutcome::PausedAtCap => println!("resumed run {} paused at cap", state.run_id),
        RunLoopOutcome::Failed => println!("resumed run {} failed", state.run_id),
    }
    Ok(())
}

fn undo_command(run: Option<String>, turn: Option<u32>) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = match run {
        Some(run_id) => load_run(&paths, &run_id)?,
        None => latest_run(&paths)?,
    };
    let target_turn = turn.unwrap_or_else(|| state.turn.saturating_sub(1));
    restore_snapshot(&state, target_turn)?;
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

fn show_command(run_id: String, turn: Option<u32>) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let state = load_run(&paths, &run_id)?;
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
    Ok(())
}

fn import_command(source: String) -> Result<()> {
    // REPORT.md: Cross-Tool State Sharing (read-only import) never writes into
    // Claude Code, Codex, or Cursor state directories.
    let root = match source.as_str() {
        "claude-code" => PathBuf::from("/Users/gdc/.claude/projects/"),
        "codex" => PathBuf::from("/Users/gdc/.codex/sessions/"),
        "cursor" => PathBuf::from("/Users/gdc/.cursor/chats/"),
        other => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "unknown import source {other}; expected claude-code, codex, or cursor"
            ))));
        }
    };
    let files = inventory_files(&root)?;
    println!("source {source}");
    println!("root {}", root.display());
    println!("files {}", files.len());
    for file in files.iter().rev().take(10) {
        println!("{}", file.display());
    }
    Ok(())
}

fn latest_run(paths: &DeadreckonPaths) -> Result<deadreckon_core::PipelineState> {
    let latest = list_runs(paths, None)?
        .into_iter()
        .next()
        .ok_or_else(|| DeadreckonError::NotFound("latest run".to_string()))?;
    load_run(paths, &latest.run_id).map_err(CliError::from)
}

fn print_run_summary(state: &deadreckon_core::PipelineState) {
    println!("run {}", state.run_id);
    println!("status {}", state.status);
    println!("goal {}", state.goal);
    println!("state {}", state.state_path().display());
    println!("working {}", state.working_dir.display());
    println!("spend {:.6}", state.total_spend_usd);
    if let Some(phase) = state.active_phase() {
        println!("phase {} {}", phase.id.0, phase.name);
    }
}

fn attach_tui(paths: &DeadreckonPaths, run_id: &str) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = loop {
        let state = load_run(paths, run_id)?;
        let spend = read_jsonl::<SpendRecord>(&state.run_root.join("spend.jsonl"))?;
        let traces = read_jsonl::<TraceRecord>(&state.run_root.join("traces.jsonl"))?;
        terminal.draw(|frame| render_attach(frame, &state, &spend, &traces))?;

        if event::poll(std::time::Duration::from_millis(500))?
            && let Event::Key(key) = event::read()?
            && (matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                || (key.code == KeyCode::Char('d')
                    && key.modifiers.contains(KeyModifiers::CONTROL)))
        {
            break Ok(());
        }
    };

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn render_attach(
    frame: &mut ratatui::Frame<'_>,
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Min(8),
            Constraint::Length(6),
            Constraint::Length(1),
        ])
        .split(area);

    let phase = state
        .active_phase()
        .map(|phase| format!("{} {}", phase.id.0, phase.name))
        .unwrap_or_else(|| "-".to_string());
    let header = Paragraph::new(format!(
        "run {}\nstatus {}  phase {}\ngoal {}",
        state.run_id, state.status, phase, state.goal
    ))
    .block(Block::default().borders(Borders::ALL).title("deadreckon"));
    frame.render_widget(header, vertical[0]);

    let meters = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(vertical[1]);
    let cap = state.max_spend_usd.unwrap_or({
        if state.total_spend_usd <= 0.0 {
            1.0
        } else {
            state.total_spend_usd
        }
    });
    let spend_ratio = (state.total_spend_usd / cap).clamp(0.0, 1.0);
    let token_total = spend
        .iter()
        .map(|record| record.input_tokens + record.output_tokens)
        .sum::<u64>();
    let context_ratio = (token_total as f64 / 200_000.0).clamp(0.0, 1.0);
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("spend"))
            .gauge_style(Style::default().fg(meter_color(spend_ratio, state)))
            .ratio(spend_ratio)
            .label(format!("${:.6} / ${:.6}", state.total_spend_usd, cap)),
        meters[0],
    );
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("context"))
            .gauge_style(Style::default().fg(threshold_color(context_ratio)))
            .ratio(context_ratio)
            .label(format!("{token_total} / 200000 tokens")),
        meters[1],
    );

    let spend_items = spend
        .iter()
        .rev()
        .take(8)
        .map(|record| {
            ListItem::new(format!(
                "turn {}  {}  {} tokens  ${:.6}",
                record.turn,
                record.model,
                record.input_tokens + record.output_tokens,
                record.cost_usd
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(spend_items).block(Block::default().borders(Borders::ALL).title("recent turns")),
        vertical[2],
    );

    let trace_items = traces
        .iter()
        .rev()
        .take(5)
        .map(|record| {
            ListItem::new(format!(
                "turn {}  {}  {:?}ms",
                record.turn, record.event, record.latency_ms
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(trace_items).block(Block::default().borders(Borders::ALL).title("tool calls")),
        vertical[3],
    );
    frame.render_widget(
        Paragraph::new("Ctrl-D detach  q quit  Esc quit"),
        vertical[4],
    );
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

fn read_jsonl<T: DeserializeOwned>(path: &std::path::Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    std::fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<std::result::Result<Vec<T>, serde_json::Error>>()
        .map_err(CliError::from)
}
