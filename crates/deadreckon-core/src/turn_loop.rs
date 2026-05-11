use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;
use deadreckon_providers::{ProviderRequest, ProviderResponse, ProviderRouter};
use deadreckon_sandbox::{SandboxBackend, SandboxSpec, run as run_sandbox};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::artifacts::{
    ProvenanceRecord, SpendRecord, TraceRecord, append_provenance, append_spend, append_trace,
    inventory_files, snapshot_working,
};
use crate::error::{DeadreckonError, IoContext, Result};
use crate::state::{PhaseId, PhaseStatus, PipelineState, save_state};

#[derive(Debug, Clone)]
pub struct RunLoopConfig {
    pub provider: Option<String>,
    pub max_spend_usd: Option<f64>,
    pub sandbox_backend: SandboxBackend,
    pub max_turns: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunLoopOutcome {
    Done,
    PausedAtCap,
    Failed,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Action {
    Bash {
        tool_call_id: String,
        command: String,
    },
    WriteFile {
        tool_call_id: String,
        path: PathBuf,
        content: String,
    },
    Done {
        summary: Option<String>,
    },
}

pub async fn run_turn_loop(
    state: &mut PipelineState,
    router: &ProviderRouter,
    config: RunLoopConfig,
) -> Result<RunLoopOutcome> {
    // AS-BUILT §9: the harness, not the model, owns the bounded mutation loop
    // and writes state after every turn boundary.
    let mut history = load_history(state)?;
    state.set_phase_status(PhaseId(40), PhaseStatus::Executing)?;
    save_state(state)?;

    for _ in 0..config.max_turns {
        let turn = state.turn + 1;
        snapshot_working(state, turn.saturating_sub(1))?;
        let prompt = build_prompt(state, &history);
        let turn_dir = state.run_root.join("turns").join(format!("turn-{turn}"));
        let stdout_name = config
            .provider
            .as_deref()
            .map(provider_output_name)
            .unwrap_or("provider.out");
        let request = ProviderRequest {
            prompt,
            max_output_tokens: 2048,
            cwd: Some(state.working_dir.clone()),
            output_path: Some(turn_dir.join(stdout_name)),
        };

        let started = Instant::now();
        let response = router.complete(&request).await?;
        let provider_trace_id = format!("llm-turn-{turn}");
        append_trace(
            state,
            &TraceRecord {
                timestamp: Utc::now(),
                run_id: state.run_id.clone(),
                turn,
                event: "llm.complete".to_string(),
                latency_ms: Some(started.elapsed().as_millis()),
                detail: json!({
                    "tool_call_id": provider_trace_id,
                    "provider": response.provider,
                    "model": response.model,
                    "trace": response.trace,
                }),
            },
        )?;
        state.total_spend_usd += response.spend.cost_usd;
        append_spend(
            state,
            &SpendRecord {
                timestamp: Utc::now(),
                turn,
                provider: response.spend.provider.clone(),
                model: response.spend.model.clone(),
                input_tokens: response.spend.input_tokens,
                output_tokens: response.spend.output_tokens,
                cost_usd: response.spend.cost_usd,
                total_cost_usd: state.total_spend_usd,
                cap_usd: config.max_spend_usd,
            },
        )?;
        if config
            .max_spend_usd
            .is_some_and(|cap| state.total_spend_usd > cap)
        {
            state.pause_reason = Some("spend cap reached".to_string());
            save_state(state)?;
            return Ok(RunLoopOutcome::PausedAtCap);
        }

        if is_cli_subagent(&response) {
            let changed = changed_files_since_snapshot(state, turn.saturating_sub(1))?;
            snapshot_working(state, turn)?;
            let tool_call_id = format!("cli-subagent-turn-{turn}");
            append_trace(
                state,
                &TraceRecord {
                    timestamp: Utc::now(),
                    run_id: state.run_id.clone(),
                    turn,
                    event: "tool.cli_subagent".to_string(),
                    latency_ms: response
                        .trace
                        .get("duration_ms")
                        .and_then(Value::as_u64)
                        .map(u128::from),
                    detail: json!({
                        "tool_call_id": tool_call_id,
                        "provider": response.provider,
                        "trace": response.trace,
                    }),
                },
            )?;
            append_provenance_for_files(state, turn, &tool_call_id, &response.model, changed)?;
            state.turn = turn;
            state.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
            save_state(state)?;
            return Ok(RunLoopOutcome::Done);
        }

        let action = parse_action(&response)?;
        match action {
            Action::Bash {
                tool_call_id,
                command,
            } => {
                let started = Instant::now();
                let output = run_sandbox(SandboxSpec {
                    backend: config.sandbox_backend,
                    cwd: state.working_dir.clone(),
                    program: OsString::from("sh"),
                    args: vec![OsString::from("-lc"), OsString::from(command.clone())],
                    env: BTreeMap::new(),
                    allow_network: false,
                })
                .await?;
                append_trace(
                    state,
                    &TraceRecord {
                        timestamp: Utc::now(),
                        run_id: state.run_id.clone(),
                        turn,
                        event: "tool.bash".to_string(),
                        latency_ms: Some(started.elapsed().as_millis()),
                        detail: json!({
                            "tool_call_id": tool_call_id,
                            "command": command,
                            "status_code": output.status_code,
                            "stdout": output.stdout,
                            "stderr": output.stderr,
                            "warning": output.warning,
                        }),
                    },
                )?;
                let changed = changed_files_since_snapshot(state, turn.saturating_sub(1))?;
                snapshot_working(state, turn)?;
                append_provenance_for_files(state, turn, &tool_call_id, &response.model, changed)?;
                history.push(format!(
                    "tool {tool_call_id} result: status={:?}",
                    output.status_code
                ));
            }
            Action::WriteFile {
                tool_call_id,
                path,
                content,
            } => {
                let target = safe_working_path(&state.working_dir, &path)?;
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).with_path(parent)?;
                }
                std::fs::write(&target, content).with_path(&target)?;
                append_trace(
                    state,
                    &TraceRecord {
                        timestamp: Utc::now(),
                        run_id: state.run_id.clone(),
                        turn,
                        event: "tool.write_file".to_string(),
                        latency_ms: None,
                        detail: json!({
                            "tool_call_id": tool_call_id,
                            "path": target,
                        }),
                    },
                )?;
                snapshot_working(state, turn)?;
                append_provenance_for_files(
                    state,
                    turn,
                    &tool_call_id,
                    &response.model,
                    vec![target],
                )?;
                history.push(format!("tool {tool_call_id} result: wrote file"));
            }
            Action::Done { summary } => {
                state.turn = turn;
                state.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
                save_state(state)?;
                history.push(format!("done: {}", summary.unwrap_or_default()));
                save_history(state, &history)?;
                return Ok(RunLoopOutcome::Done);
            }
        }
        state.turn = turn;
        save_history(state, &history)?;
        save_state(state)?;
    }

    state.failure_reason = Some("max turn budget exhausted".to_string());
    state.set_phase_status(PhaseId(40), PhaseStatus::Failed)?;
    save_state(state)?;
    Ok(RunLoopOutcome::Failed)
}

fn build_prompt(state: &PipelineState, history: &[String]) -> String {
    let history_text = if history.is_empty() {
        "none".to_string()
    } else {
        history.join("\n")
    };
    format!(
        "You are deadreckon running unattended coding work.\nGoal: {}\nWorking directory: {}\nReturn exactly one JSON object with action bash, write_file, or done.\nHistory:\n{}",
        state.goal,
        state.working_dir.display(),
        history_text
    )
}

fn parse_action(response: &ProviderResponse) -> Result<Action> {
    serde_json::from_str(&response.content).map_err(|err| {
        DeadreckonError::InvalidInput(format!(
            "provider returned non-action JSON: {err}; content={}",
            response.content
        ))
    })
}

fn is_cli_subagent(response: &ProviderResponse) -> bool {
    response.trace.get("kind").and_then(Value::as_str) == Some("cli_subagent")
}

fn safe_working_path(root: &Path, relative: &Path) -> Result<PathBuf> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(DeadreckonError::InvalidInput(format!(
            "unsafe write path {}",
            relative.display()
        )));
    }
    Ok(root.join(relative))
}

fn changed_files_since_snapshot(state: &PipelineState, snapshot_turn: u32) -> Result<Vec<PathBuf>> {
    let snapshot_dir = state
        .run_root
        .join("snapshots")
        .join(format!("turn-{snapshot_turn}"));
    let before = file_set(&snapshot_dir)?;
    let after_files = inventory_files(&state.working_dir)?;
    let mut changed = Vec::new();
    for file in after_files {
        let relative = file.strip_prefix(&state.working_dir).map_err(|err| {
            DeadreckonError::InvalidInput(format!("working path prefix error: {err}"))
        })?;
        let before_path = snapshot_dir.join(relative);
        if !before.contains(relative) || file_bytes(&file)? != file_bytes(&before_path)? {
            changed.push(file);
        }
    }
    Ok(changed)
}

fn file_set(root: &Path) -> Result<BTreeSet<PathBuf>> {
    Ok(inventory_files(root)?
        .into_iter()
        .filter_map(|path| path.strip_prefix(root).ok().map(Path::to_path_buf))
        .collect())
}

fn file_bytes(path: &Path) -> Result<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn append_provenance_for_files(
    state: &PipelineState,
    turn: u32,
    tool_call_id: &str,
    model: &str,
    files: Vec<PathBuf>,
) -> Result<()> {
    if files.is_empty() {
        return Ok(());
    }
    append_provenance(
        state,
        &ProvenanceRecord {
            timestamp: Utc::now(),
            prompt_id: format!("turn-{turn}"),
            model: model.to_string(),
            tool_call_id: tool_call_id.to_string(),
            session_id: state.run_id.clone(),
            files,
        },
    )
}

fn load_history(state: &PipelineState) -> Result<Vec<String>> {
    let path = history_path(state);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path).with_path(&path)?;
    serde_json::from_str(&raw).map_err(|source| DeadreckonError::Json { path, source })
}

fn save_history(state: &PipelineState, history: &[String]) -> Result<()> {
    let path = history_path(state);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_path(parent)?;
    }
    let data = serde_json::to_vec_pretty(history).map_err(|source| DeadreckonError::Json {
        path: path.clone(),
        source,
    })?;
    std::fs::write(&path, data).with_path(&path)
}

fn history_path(state: &PipelineState) -> PathBuf {
    state.run_root.join("history.json")
}

fn provider_output_name(provider: &str) -> &'static str {
    match provider {
        "cli:claude-code" => "claude.out",
        "cli:codex" => "codex.out",
        _ => "provider.out",
    }
}
