use std::collections::BTreeMap;
use std::ffi::OsString;
use std::time::Instant;

use chrono::Utc;
use clap::{Parser, Subcommand};
use deadreckon_core::{
    DeadreckonError, DeadreckonPaths, PhaseId, PhaseStatus, ProvenanceRecord, RunOptions,
    SpendRecord, TraceRecord, acquire_lock, append_provenance, append_spend, append_trace,
    create_run, inventory_files, save_state, snapshot_working,
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
