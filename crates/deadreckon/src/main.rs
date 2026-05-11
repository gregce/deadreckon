use std::io::{self, IsTerminal};
use std::path::PathBuf;

use chrono::Utc;
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyCode};
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
}

type Result<T> = std::result::Result<T, CliError>;

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
    Run {
        goal: String,
        #[arg(long)]
        max_spend: Option<f64>,
        #[arg(long, default_value = "auto")]
        sandbox: String,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long, default_value = "default-coding")]
        skill: String,
        #[arg(long)]
        smoke: bool,
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Run {
            goal,
            max_spend,
            sandbox,
            provider,
            skill,
            smoke,
        } => run_command(goal, max_spend, sandbox, provider, skill, smoke).await,
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

async fn run_command(
    goal: String,
    max_spend: Option<f64>,
    sandbox: String,
    provider: Option<String>,
    skill: String,
    smoke: bool,
) -> Result<()> {
    if smoke {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "--smoke is reserved for an explicit labeled fallback; default run is provider-driven"
                .to_string(),
        )));
    }
    let paths = DeadreckonPaths::discover();
    let cwd = std::env::current_dir()?;
    let backend: SandboxBackend = sandbox.parse()?;
    let mut state = create_run(
        &paths,
        RunOptions {
            goal,
            cwd,
            sandbox: backend.to_string(),
            provider: provider.clone(),
            skill_name: skill,
            max_spend_usd: max_spend,
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
    let router = ProviderRouter::from_config_path(&paths.config_path(), provider.as_deref())?;
    state.set_phase_status(PhaseId(30), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("turn-loop")?;
    let outcome = run_turn_loop(
        &mut state,
        &router,
        RunLoopConfig {
            provider: provider.clone(),
            max_spend_usd: max_spend,
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

fn doctor_command() {
    println!("deadreckon source /Users/gdc/deadreckon");
    println!(
        "deadreckon home {}",
        DeadreckonPaths::discover().home().display()
    );
    for backend in deadreckon_sandbox::doctor() {
        let status = if backend.available { "ok" } else { "missing" };
        let path = backend
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "sandbox {:<12} {:<8} {:<32} {}",
            backend.backend, status, path, backend.note
        );
    }
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
    let pids = state.child_pids.clone();
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
            && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
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
            Constraint::Length(7),
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
            .gauge_style(Style::default().fg(Color::Green))
            .ratio(spend_ratio)
            .label(format!("${:.6} / ${:.6}", state.total_spend_usd, cap)),
        meters[0],
    );
    frame.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("context"))
            .gauge_style(Style::default().fg(Color::Cyan))
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
