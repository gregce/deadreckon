use std::collections::BTreeMap;
use std::ffi::OsString;
use std::time::Instant;

use chrono::Utc;
use clap::{Parser, Subcommand};
use deadreckon_core::{
    DeadreckonError, DeadreckonPaths, PhaseId, PhaseStatus, ProvenanceRecord, RunOptions,
    RunStatus, SpendRecord, TraceRecord, acquire_lock, append_provenance, append_spend,
    append_trace, create_run, inventory_files, list_runs, load_run, release_lock_file,
    restore_snapshot, save_state, snapshot_working,
};
use deadreckon_providers::{ProviderRouter, ProviderUsage};
use deadreckon_sandbox::{SandboxBackend, SandboxSpec, run as run_sandbox};
use serde_json::json;
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
        } => run_command(goal, max_spend, sandbox, provider, skill).await,
        Commands::Doctor => {
            doctor_command();
            Ok(())
        }
        Commands::List { scope } => list_command(scope),
        Commands::Attach { run_id } => attach_command(run_id),
        Commands::Kill { run_id } => kill_command(run_id),
        Commands::Resume { run_id } => resume_command(run_id),
        Commands::Undo { run, turn } => undo_command(run, turn),
        Commands::Show { run_id, turn } => show_command(run_id, turn),
    }
}

async fn run_command(
    goal: String,
    max_spend: Option<f64>,
    sandbox: String,
    provider: Option<String>,
    skill: String,
) -> Result<()> {
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

    state.set_phase_status(PhaseId(20), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("provider")?;
    let router = ProviderRouter::from_config_path(&paths.config_path(), provider.as_deref())?;
    let usage = estimate_usage(&state.goal);
    let spend = router.estimate_for_route(provider.as_deref(), usage)?;
    let next_total = state.total_spend_usd + spend.cost_usd;
    if max_spend.is_some_and(|cap| next_total > cap) {
        state.pause_reason = Some(format!(
            "estimated turn spend ${:.6} would exceed cap ${:.6}",
            spend.cost_usd,
            max_spend.unwrap_or_default()
        ));
        save_state(&state)?;
        println!(
            "paused run {}: {}",
            state.run_id,
            state.pause_reason.as_deref().unwrap_or("spend cap reached")
        );
        lock.release()?;
        return Ok(());
    }

    append_trace(
        &state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: 1,
            event: "provider.route.estimated".to_string(),
            latency_ms: None,
            detail: json!({
                "provider": spend.provider,
                "model": spend.model,
                "input_tokens": spend.input_tokens,
                "output_tokens": spend.output_tokens,
                "cost_usd": spend.cost_usd,
            }),
        },
    )?;

    snapshot_working(&state, 0)?;
    state.set_phase_status(PhaseId(30), PhaseStatus::Executing)?;
    save_state(&state)?;
    lock.heartbeat("sandbox")?;

    let started = Instant::now();
    let sandbox_output = run_sandbox(SandboxSpec {
        backend,
        cwd: state.working_dir.clone(),
        program: OsString::from("sh"),
        args: vec![OsString::from("-lc"), OsString::from(coding_turn_script())],
        env: BTreeMap::from([("DEADRECKON_GOAL".to_string(), state.goal.clone())]),
        allow_network: false,
    })
    .await?;
    append_trace(
        &state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: 1,
            event: "tool.sandbox.run".to_string(),
            latency_ms: Some(started.elapsed().as_millis()),
            detail: json!({
                "backend": sandbox_output.backend.to_string(),
                "status_code": sandbox_output.status_code,
                "stdout": sandbox_output.stdout,
                "stderr": sandbox_output.stderr,
                "warning": sandbox_output.warning,
            }),
        },
    )?;

    if sandbox_output.status_code != Some(0) {
        state.failure_reason = Some("sandbox coding turn failed".to_string());
        state.set_phase_status(PhaseId(40), PhaseStatus::Failed)?;
        save_state(&state)?;
        lock.release()?;
        return Ok(());
    }

    state.set_phase_status(PhaseId(40), PhaseStatus::Completed)?;
    snapshot_working(&state, 1)?;
    let files = inventory_files(&state.working_dir)?;
    append_provenance(
        &state,
        &ProvenanceRecord {
            timestamp: Utc::now(),
            prompt_id: "turn-1".to_string(),
            model: spend.model.clone(),
            tool_call_id: Uuid::new_v4().to_string(),
            session_id: state.run_id.clone(),
            files,
        },
    )?;

    state.turn = 1;
    state.total_spend_usd = next_total;
    append_spend(
        &state,
        &SpendRecord {
            timestamp: Utc::now(),
            turn: 1,
            provider: spend.provider,
            model: spend.model,
            input_tokens: spend.input_tokens,
            output_tokens: spend.output_tokens,
            cost_usd: spend.cost_usd,
            total_cost_usd: state.total_spend_usd,
            cap_usd: max_spend,
        },
    )?;
    state.set_phase_status(PhaseId(50), PhaseStatus::Completed)?;
    state.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
    save_state(&state)?;
    lock.release()?;

    println!("completed run {}", state.run_id);
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
    let state = load_run(&paths, &run_id)?;
    print_run_summary(&state);
    Ok(())
}

fn kill_command(run_id: String) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = load_run(&paths, &run_id)?;
    release_lock_file(&paths, &state.task_key)?;
    state.status = RunStatus::Failed;
    state.failure_reason = Some("killed by user".to_string());
    state.updated_at = Utc::now();
    save_state(&state)?;
    println!("killed run {}", state.run_id);
    Ok(())
}

fn resume_command(run_id: String) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let mut state = load_run(&paths, &run_id)?;
    let lock = acquire_lock(
        &paths,
        &state.task_key,
        &state.run_id,
        &state.scope,
        "resume",
        deadreckon_core::lock::DEFAULT_STALE_AFTER,
    )?;
    if state.status != RunStatus::Completed {
        state.failure_reason = None;
        state.pause_reason = None;
        state.status = RunStatus::Planned;
        state.updated_at = Utc::now();
        save_state(&state)?;
    }
    lock.release()?;
    println!("resumed run {}", state.run_id);
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

fn estimate_usage(goal: &str) -> ProviderUsage {
    let words = goal.split_whitespace().count() as u64;
    ProviderUsage {
        input_tokens: 128 + words * 4,
        output_tokens: 256,
    }
}

fn coding_turn_script() -> &'static str {
    r#"set -eu
mkdir -p src
cat > Cargo.toml <<'TOML'
[package]
name = "deadreckon-output"
version = "0.1.0"
edition = "2024"

[dependencies]
TOML
cat > src/main.rs <<'RS'
fn main() {
    println!("hello from deadreckon");
}
RS
printf '%s\n' "$DEADRECKON_GOAL" > GOAL.txt
printf 'completed\n' > RESULT.txt
"#
}
