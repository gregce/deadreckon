use std::collections::{BTreeSet, hash_map::DefaultHasher};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use deadreckon_core::{
    DeadreckonError, DeadreckonPaths, PhaseId, PhaseStatus, ProvenanceRecord, RUN_EVENTS_JSONL,
    RunEvent, RunLoopConfig, RunLoopOutcome, RunOptions, RunStatus, SpendRecord, TraceRecord,
    acquire_lock, append_provenance, append_trace, create_run, inventory_files, list_runs,
    load_run, release_lock_file, restore_snapshot, run_turn_loop, save_state, terminate_pid,
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
        max_wall_seconds: Option<f64>,
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
        #[arg(long)]
        force: bool,
    },
    Resume {
        run_id: String,
        #[arg(long)]
        from_turn: Option<u32>,
        #[arg(long)]
        max_wall_seconds: Option<f64>,
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
            max_wall_seconds,
            sandbox,
            provider,
            skill,
            smoke,
            i_know_its_a_lot,
            no_confirm,
        } => {
            run_command(RunCommandArgs {
                goal,
                max_spend,
                max_wall_seconds,
                sandbox,
                provider,
                skill,
                smoke,
                i_know_its_a_lot,
                no_confirm,
            })
            .await
        }
        Commands::Doctor => {
            doctor_command();
            Ok(())
        }
        Commands::List { scope } => list_command(scope),
        Commands::Attach { run_id } => attach_command(run_id),
        Commands::Kill { run_id, force } => kill_command(run_id, force),
        Commands::Resume {
            run_id,
            from_turn,
            max_wall_seconds,
        } => resume_command(run_id, from_turn, max_wall_seconds).await,
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
    doctor_command();
    println!("next: deadreckon run \"describe the coding task\"");
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

struct RunCommandArgs {
    goal: String,
    max_spend: Option<f64>,
    max_wall_seconds: Option<f64>,
    sandbox: Option<String>,
    provider: Option<String>,
    skill: String,
    smoke: bool,
    i_know_its_a_lot: bool,
    no_confirm: bool,
}

async fn run_command(args: RunCommandArgs) -> Result<()> {
    let RunCommandArgs {
        goal,
        max_spend,
        max_wall_seconds,
        sandbox,
        provider,
        skill,
        smoke,
        i_know_its_a_lot,
        no_confirm,
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
            max_wall_seconds: effective_max_wall_seconds,
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
            max_wall_seconds: effective_max_wall_seconds,
            sandbox_backend: backend,
            max_turns: 12,
            from_turn: None,
            event_sender: None,
            cancellation_token: None,
        },
    )
    .await?;
    state.child_pids.clear();
    save_state(&state)?;
    lock.release()?;

    match outcome {
        RunLoopOutcome::Done => println!("completed run {}", state.run_id),
        RunLoopOutcome::PausedAtCap => println!("paused run {}", state.run_id),
        RunLoopOutcome::Killed => println!("killed run {}", state.run_id),
        RunLoopOutcome::Failed => println!("failed run {}", state.run_id),
    }
    print_run_locations(&state);
    Ok(())
}

#[derive(Debug, Default)]
struct ConfigDefaults {
    provider: Option<String>,
    sandbox: Option<String>,
    max_spend: Option<f64>,
    cli_max_wall_seconds: Option<f64>,
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
            let version = backend
                .path
                .as_ref()
                .and_then(|path| command_version(path))
                .map(|version| format!(" ({version})"))
                .unwrap_or_default();
            println!("✓ sandbox {} found{}{}", backend.backend, path, version);
        } else {
            println!("✗ sandbox {} missing", backend.backend);
            println!("    fix: {}", backend.note);
        }
    }
    if paths.config_path().exists() {
        match load_config_value(&paths) {
            Ok(root) => {
                println!("✓ {} present and parseable", paths.config_path().display());
                doctor_providers(&root);
            }
            Err(err) => {
                println!("✗ {} is not parseable", paths.config_path().display());
                println!("    fix: check TOML syntax or rerun `deadreckon init` ({err})");
            }
        }
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
    doctor_disk_and_permissions(&paths);
    doctor_os();
    doctor_subscription_binary("claude");
    doctor_subscription_binary("codex");
}

fn doctor_providers(root: &toml::Value) {
    let Some(providers) = root.get("providers").and_then(toml::Value::as_table) else {
        println!("✗ providers table missing");
        println!("    fix: deadreckon init");
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
                println!("✓ provider {name} CLI binary {binary} found");
            } else {
                println!("✗ provider {name} CLI binary {binary} missing");
                println!("    fix: install {binary} or set providers.\"{name}\".binary");
            }
        } else if provider_has_key(entry) {
            if std::env::var_os("DEADRECKON_DOCTOR_PING").is_some() {
                println!("✓ provider {name} credential present; ping requested");
            } else {
                println!("✓ provider {name} credential present");
            }
        } else {
            println!("✗ provider {name} credential missing");
            println!("    fix: deadreckon config set providers.{name}.api_key <KEY>");
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
            "✗ runstate dir {} not writable",
            paths.runstate_dir().display()
        );
        println!(
            "    fix: mkdir -p {} && chmod u+w {}",
            paths.runstate_dir().display(),
            paths.runstate_dir().display()
        );
        println!("    detail: {err}");
        return;
    }
    let probe = paths.runstate_dir().join(".doctor-write-test");
    match fs::write(&probe, b"ok").and_then(|_| fs::remove_file(&probe)) {
        Ok(()) => println!("✓ runstate dir {} writable", paths.runstate_dir().display()),
        Err(err) => {
            println!(
                "✗ runstate dir {} not writable",
                paths.runstate_dir().display()
            );
            println!("    fix: chmod u+w {}", paths.runstate_dir().display());
            println!("    detail: {err}");
        }
    }
    match free_kb(paths.home()) {
        Some(kb) if kb < 1_048_576 => {
            println!(
                "✗ disk space low: {} MB free in {}",
                kb / 1024,
                paths.home().display()
            );
            println!("    fix: free at least 1 GB or set DEADRECKON_HOME to a larger disk");
        }
        Some(kb) => println!(
            "✓ disk space {} MB free in {}",
            kb / 1024,
            paths.home().display()
        ),
        None => {
            println!(
                "✗ disk space check unavailable for {}",
                paths.home().display()
            );
            println!("    fix: run `df -Pk {}` manually", paths.home().display());
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
        println!("✓ os macOS {version}");
    }
    #[cfg(target_os = "linux")]
    {
        let version = std::process::Command::new("uname")
            .arg("-r")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!("✓ os Linux kernel {version}");
    }
}

fn doctor_subscription_binary(binary: &str) {
    if command_exists(binary) {
        println!(
            "✓ subscription binary {binary} {}",
            command_version(std::path::Path::new(binary))
                .unwrap_or_else(|| "version unknown".to_string())
        );
    } else {
        println!("✗ subscription binary {binary} missing");
        println!(
            "    fix: install {binary} or choose another provider with `deadreckon config set defaults.provider <name>`"
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
        "default_provider = \"{provider}\"\nfallback = {fallback}\n\n[defaults]\nprovider = \"{provider}\"\nmax_spend = {max_spend}\ncli_max_wall_seconds = 3600\nsandbox = \"{sandbox}\"\n\n"
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

fn kill_command(run_id: String, force: bool) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = load_run(&paths, &run_id)?;
    let pids = supervised_pids(&state);
    release_lock_file(&paths, &state.scope, &state.task_key)?;
    state.status = RunStatus::Killed;
    state.failure_reason = Some("killed by user".to_string());
    state.killed_at = Some(Utc::now());
    state.updated_at = Utc::now();
    save_state(&state)?;
    for pid in &pids {
        if *pid != std::process::id() {
            let _ = terminate_pid(*pid, force);
        }
    }
    if force {
        println!("killed run {} forcefully", state.run_id);
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

async fn resume_command(
    run_id: String,
    from_turn: Option<u32>,
    max_wall_seconds: Option<f64>,
) -> Result<()> {
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
        let events = read_jsonl::<RunEvent>(&state.run_root.join(RUN_EVENTS_JSONL))?;
        let live = collect_attach_live(&state);
        terminal.draw(|frame| render_attach(frame, &state, &spend, &traces, &events, &live))?;

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

#[derive(Debug, Default)]
struct AttachLive {
    file_count: usize,
    total_bytes: u64,
    files: Vec<LiveFile>,
    pids: Vec<LivePid>,
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
    AttachLive {
        file_count,
        total_bytes,
        files,
        pids,
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

fn render_attach(
    frame: &mut ratatui::Frame<'_>,
    state: &deadreckon_core::PipelineState,
    spend: &[SpendRecord],
    traces: &[TraceRecord],
    events: &[RunEvent],
    live: &AttachLive,
) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Length(5),
            Constraint::Min(8),
            Constraint::Length(8),
            Constraint::Length(1),
        ])
        .split(area);

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
        "run {}\nstatus {}  phase {}  provider {}  sandbox {}  turn timer {}\n{} {}\ngoal {}",
        state.run_id,
        state.status,
        phase,
        state.provider.as_deref().unwrap_or("-"),
        state.sandbox,
        turn_timer(events),
        path_label,
        state.working_dir.display(),
        state.goal
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

    let live_area = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(30),
            Constraint::Percentage(50),
            Constraint::Percentage(20),
        ])
        .split(vertical[2]);
    render_recent_turns(frame, live_area[0], spend);
    render_live_files(frame, live_area[1], live);
    render_processes(frame, live_area[2], live);

    let mut trace_items = events
        .iter()
        .rev()
        .take(8)
        .map(render_event_item)
        .chain(traces.iter().rev().take(2).map(|record| {
            ListItem::new(format!(
                "trace turn {}  {}  {:?}ms",
                record.turn, record.event, record.latency_ms
            ))
        }))
        .collect::<Vec<_>>();
    if state.status == RunStatus::Executing && live.file_count > 0 {
        trace_items.push(ListItem::new(format!(
            "live working tree: {} files, latest changes visible before provider exit",
            live.file_count
        )));
    }
    frame.render_widget(
        List::new(trace_items).block(Block::default().borders(Borders::ALL).title("tool calls")),
        vertical[3],
    );
    frame.render_widget(
        Paragraph::new("Ctrl-D detach  q quit  Esc quit"),
        vertical[4],
    );
}

fn render_recent_turns(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    spend: &[SpendRecord],
) {
    let items = if spend.is_empty() {
        vec![
            ListItem::new("provider turn in progress"),
            ListItem::new("spend and token rows appear after provider exits"),
        ]
    } else {
        spend
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
            .collect()
    };
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("recent turns")),
        area,
    );
}

fn render_live_files(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    live: &AttachLive,
) {
    let mut items = vec![ListItem::new(format!(
        "{} files  {}",
        live.file_count,
        format_bytes(live.total_bytes)
    ))];
    items.extend(live.files.iter().take(12).map(|file| {
        ListItem::new(format!(
            "{:>7} {:>8}  {}",
            format_age(file.modified_at),
            format_bytes(file.bytes),
            file.path
        ))
    }));
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("live files")),
        area,
    );
}

fn render_processes(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    live: &AttachLive,
) {
    let items = if live.pids.is_empty() {
        vec![ListItem::new("no supervised pids")]
    } else {
        live.pids
            .iter()
            .map(|pid| {
                let status = if pid.alive { "alive" } else { "dead" };
                ListItem::new(format!("{} {} {}", pid.pid, status, pid.command))
            })
            .collect()
    };
    frame.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title("processes")),
        area,
    );
}

fn render_event_item(event: &RunEvent) -> ListItem<'static> {
    let label = match &event.event {
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
        } => format!(
            "turn {turn} spend +${cost_usd:.6} wall {}s",
            wall_time_seconds.unwrap_or(0.0)
        ),
        deadreckon_core::RunEventKind::Error { turn, message } => {
            format!("turn {} error {message}", turn.unwrap_or_default())
        }
    };
    ListItem::new(label)
}

fn turn_timer(events: &[RunEvent]) -> String {
    let Some(event) = events.iter().rev().find(|event| {
        matches!(
            event.event,
            deadreckon_core::RunEventKind::TurnStarted { .. }
        )
    }) else {
        return "-".to_string();
    };
    let elapsed = Utc::now()
        .signed_duration_since(event.timestamp)
        .num_seconds()
        .max(0);
    format!("{elapsed}s")
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
