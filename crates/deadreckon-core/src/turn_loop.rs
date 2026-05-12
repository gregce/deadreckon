use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use chrono::Utc;
use deadreckon_providers::{ProviderRequest, ProviderResponse, ProviderRouter};
use deadreckon_sandbox::{SandboxBackend, SandboxSpec, run as run_sandbox};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::artifacts::{
    ProvenanceRecord, SpendRecord, TraceRecord, append_provenance, append_spend, append_trace,
    inventory_files, snapshot_working,
};
use crate::cancel::{cancel_marker_path_for_run_root, cancel_marker_present};
use crate::codebase::{CodebaseMode, read_codebase_record};
use crate::docs::{TurnDocInput, append_turn_doc};
use crate::error::{DeadreckonError, IoContext, Result};
use crate::events::{RunEvent, RunEventKind, emit_event, event_preview, tool_args_json};
use crate::gate::validate_acceptance_marker;
use crate::paths::DeadreckonPaths;
use crate::polish::{PolishConfig, polish_run_docs};
use crate::promotion::promote_completed_run;
use crate::state::{PhaseId, PhaseStatus, PipelineState, RunStatus, save_state};

#[derive(Debug, Clone)]
pub struct RunLoopConfig {
    pub provider: Option<String>,
    pub max_spend_usd: Option<f64>,
    pub max_wall_seconds: Option<f64>,
    pub sandbox_backend: SandboxBackend,
    pub max_turns: u32,
    pub from_turn: Option<u32>,
    pub event_sender: Option<broadcast::Sender<RunEvent>>,
    pub cancellation_token: Option<CancellationToken>,
    pub docs: RunLoopDocsConfig,
}

#[derive(Debug, Clone)]
pub struct RunLoopDocsConfig {
    pub home: PathBuf,
    pub config_path: Option<PathBuf>,
    pub doc_provider: Option<String>,
    pub doc_skill: String,
    pub no_docs: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunLoopOutcome {
    Done,
    PausedAtCap,
    Killed,
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
    let mut history = load_or_reconstruct_history(state, config.from_turn)?;
    state.set_phase_status(PhaseId(40), PhaseStatus::Executing)?;
    save_state(state)?;
    let run_token = config.cancellation_token.clone().unwrap_or_default();
    let _cancel_marker_guard = CancelMarkerGuard::spawn(state.run_root.clone(), run_token.clone());
    if let Some(from_turn) = config.from_turn {
        state.turn = from_turn;
        save_state(state)?;
    }

    for _ in 0..config.max_turns {
        if should_cancel_run(state, &run_token) {
            state.status = crate::state::RunStatus::Killed;
            state.failure_reason = Some("run cancelled".to_string());
            save_state(state)?;
            emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Killed)?;
            return Ok(RunLoopOutcome::Killed);
        }
        let turn_token = run_token.child_token();
        let turn = state.turn + 1;
        emit_event(
            state,
            config.event_sender.as_ref(),
            RunEventKind::TurnStarted { turn },
        )?;
        snapshot_working(state, turn.saturating_sub(1))?;
        let prompt = if config.provider.as_deref().is_some_and(is_cli_provider_name) {
            build_cli_subagent_prompt(state, &history)
        } else {
            build_prompt(state, &history)
        };
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
            sandbox_backend: Some(config.sandbox_backend),
            pid_file: Some(
                state
                    .run_root
                    .join("child-pids")
                    .join(format!("provider-turn-{turn}.pid")),
            ),
            cancellation_token: Some(turn_token.clone()),
        };

        let started = Instant::now();
        let response = match router.complete(&request).await {
            Ok(response) => response,
            Err(err) if should_cancel_run(state, &run_token) => {
                state.status = crate::state::RunStatus::Killed;
                state.failure_reason = Some(format!("run cancelled during provider call: {err}"));
                save_state(state)?;
                emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Killed)?;
                return Ok(RunLoopOutcome::Killed);
            }
            Err(err) => return Err(err.into()),
        };
        if should_cancel_run(state, &run_token) {
            state.status = crate::state::RunStatus::Killed;
            state.failure_reason = Some("run cancelled after provider call".to_string());
            save_state(state)?;
            emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Killed)?;
            return Ok(RunLoopOutcome::Killed);
        }
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
        state.total_wall_seconds += response.spend.wall_time_seconds.unwrap_or(0.0);
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
                subscription: response.spend.subscription,
                wall_time_seconds: response.spend.wall_time_seconds,
                wall_time_cap_seconds: config.max_wall_seconds,
            },
        )?;
        emit_event(
            state,
            config.event_sender.as_ref(),
            RunEventKind::TokenUsageDelta {
                turn,
                input_tokens: response.spend.input_tokens,
                output_tokens: response.spend.output_tokens,
            },
        )?;
        emit_event(
            state,
            config.event_sender.as_ref(),
            RunEventKind::SpendDelta {
                turn,
                cost_usd: response.spend.cost_usd,
                total_cost_usd: state.total_spend_usd,
                wall_time_seconds: response.spend.wall_time_seconds,
            },
        )?;
        if config
            .max_spend_usd
            .is_some_and(|cap| state.total_spend_usd > cap)
        {
            state.pause_reason = Some("spend cap reached".to_string());
            save_state(state)?;
            emit_run_completed(
                state,
                config.event_sender.as_ref(),
                RunLoopOutcome::PausedAtCap,
            )?;
            return Ok(RunLoopOutcome::PausedAtCap);
        }
        if response.spend.subscription
            && config
                .max_wall_seconds
                .is_some_and(|cap| state.total_wall_seconds > cap)
        {
            state.pause_reason = Some("wall-clock cap reached".to_string());
            save_state(state)?;
            emit_run_completed(
                state,
                config.event_sender.as_ref(),
                RunLoopOutcome::PausedAtCap,
            )?;
            return Ok(RunLoopOutcome::PausedAtCap);
        }

        if is_cli_subagent(&response) {
            let tool_call_id = format!("cli-subagent-turn-{turn}");
            emit_event(
                state,
                config.event_sender.as_ref(),
                RunEventKind::ToolCallStarted {
                    turn,
                    tool_call_id: tool_call_id.clone(),
                    tool_name: "cli_subagent".to_string(),
                    args: response.trace.clone(),
                },
            )?;
            let changed = changed_files_since_snapshot(state, turn.saturating_sub(1))?;
            if changed.is_empty() {
                state.failure_reason =
                    Some("cli subagent completed without file changes".to_string());
                state.set_phase_status(PhaseId(40), PhaseStatus::Failed)?;
                save_state(state)?;
                emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Failed)?;
                return Ok(RunLoopOutcome::Failed);
            }
            snapshot_working(state, turn)?;
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
            append_provenance_for_files(
                state,
                turn,
                &tool_call_id,
                &response.model,
                changed.clone(),
            )?;
            commit_worktree_turn(state, turn, "cli_subagent")?;
            append_turn_doc(
                state,
                TurnDocInput {
                    turn,
                    tool_kind: "cli_subagent".to_string(),
                    latency_ms: response
                        .trace
                        .get("duration_ms")
                        .and_then(Value::as_u64)
                        .map(u128::from),
                    files: changed,
                    outcome: event_preview(&response.content),
                    response_text: response.content.clone(),
                },
            )?;
            emit_event(
                state,
                config.event_sender.as_ref(),
                RunEventKind::ToolCallResult {
                    turn,
                    tool_call_id,
                    status: "ok".to_string(),
                    preview: event_preview(&response.content),
                },
            )?;
            state.turn = turn;
            complete_run_docs(state, router, &config).await?;
            run_acceptance_gate(state)?;
            validate_acceptance_marker(state)?;
            promote_if_ready(state)?;
            state.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
            save_state(state)?;
            emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Done)?;
            return Ok(RunLoopOutcome::Done);
        }

        let action = parse_action(&response)?;
        match action {
            Action::Bash {
                tool_call_id,
                command,
            } => {
                let tool_token = turn_token.child_token();
                emit_event(
                    state,
                    config.event_sender.as_ref(),
                    RunEventKind::ToolCallStarted {
                        turn,
                        tool_call_id: tool_call_id.clone(),
                        tool_name: "bash".to_string(),
                        args: tool_args_json(&command),
                    },
                )?;
                let started = Instant::now();
                let output = match run_sandbox(SandboxSpec {
                    backend: config.sandbox_backend,
                    cwd: state.working_dir.clone(),
                    program: OsString::from("sh"),
                    args: vec![OsString::from("-lc"), OsString::from(command.clone())],
                    env: BTreeMap::new(),
                    allow_network: false,
                    pid_file: Some(
                        state
                            .run_root
                            .join("child-pids")
                            .join(format!("tool-{tool_call_id}.pid")),
                    ),
                    cancellation_token: Some(tool_token),
                    profile_dir: Some(state.run_root.join("sandbox")),
                    read_allowlist: vec![state.working_dir.clone()],
                    write_allowlist: Vec::new(),
                    network_allowlist: Vec::new(),
                })
                .await
                {
                    Ok(output) => output,
                    Err(deadreckon_sandbox::SandboxError::Cancelled)
                        if should_cancel_run(state, &run_token) =>
                    {
                        state.status = crate::state::RunStatus::Killed;
                        state.failure_reason = Some("run cancelled during tool call".to_string());
                        save_state(state)?;
                        emit_run_completed(
                            state,
                            config.event_sender.as_ref(),
                            RunLoopOutcome::Killed,
                        )?;
                        return Ok(RunLoopOutcome::Killed);
                    }
                    Err(err) => return Err(err.into()),
                };
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
                append_provenance_for_files(
                    state,
                    turn,
                    &tool_call_id,
                    &response.model,
                    changed.clone(),
                )?;
                commit_worktree_turn(state, turn, &format!("bash {tool_call_id}"))?;
                append_turn_doc(
                    state,
                    TurnDocInput {
                        turn,
                        tool_kind: "bash".to_string(),
                        latency_ms: Some(started.elapsed().as_millis()),
                        files: changed,
                        outcome: format!("status={:?}", output.status_code),
                        response_text: response.content.clone(),
                    },
                )?;
                emit_event(
                    state,
                    config.event_sender.as_ref(),
                    RunEventKind::ToolCallResult {
                        turn,
                        tool_call_id: tool_call_id.clone(),
                        status: format!("{:?}", output.status_code),
                        preview: event_preview(format!("{}{}", output.stdout, output.stderr)),
                    },
                )?;
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
                emit_event(
                    state,
                    config.event_sender.as_ref(),
                    RunEventKind::ToolCallStarted {
                        turn,
                        tool_call_id: tool_call_id.clone(),
                        tool_name: "write_file".to_string(),
                        args: tool_args_json(path.display().to_string()),
                    },
                )?;
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
                let changed = vec![target.clone()];
                append_provenance_for_files(
                    state,
                    turn,
                    &tool_call_id,
                    &response.model,
                    changed.clone(),
                )?;
                commit_worktree_turn(state, turn, &format!("write_file {tool_call_id}"))?;
                append_turn_doc(
                    state,
                    TurnDocInput {
                        turn,
                        tool_kind: "write_file".to_string(),
                        latency_ms: None,
                        files: changed,
                        outcome: "ok".to_string(),
                        response_text: response.content.clone(),
                    },
                )?;
                emit_event(
                    state,
                    config.event_sender.as_ref(),
                    RunEventKind::ToolCallResult {
                        turn,
                        tool_call_id: tool_call_id.clone(),
                        status: "ok".to_string(),
                        preview: "wrote file".to_string(),
                    },
                )?;
                history.push(format!("tool {tool_call_id} result: wrote file"));
            }
            Action::Done { summary } => {
                state.turn = turn;
                append_turn_doc(
                    state,
                    TurnDocInput {
                        turn,
                        tool_kind: "done".to_string(),
                        latency_ms: None,
                        files: Vec::new(),
                        outcome: summary.clone().unwrap_or_else(|| "done".to_string()),
                        response_text: response.content.clone(),
                    },
                )?;
                complete_run_docs(state, router, &config).await?;
                run_acceptance_gate(state)?;
                validate_acceptance_marker(state)?;
                promote_if_ready(state)?;
                state.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
                save_state(state)?;
                history.push(format!("done: {}", summary.unwrap_or_default()));
                save_history(state, &history)?;
                emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Done)?;
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
    emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Failed)?;
    Ok(RunLoopOutcome::Failed)
}

fn should_cancel_run(state: &PipelineState, token: &CancellationToken) -> bool {
    state.status == RunStatus::Killed || token.is_cancelled() || cancel_marker_present(state)
}

struct CancelMarkerGuard {
    shutdown: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

impl CancelMarkerGuard {
    fn spawn(run_root: PathBuf, run_token: CancellationToken) -> Self {
        let shutdown = CancellationToken::new();
        let shutdown_for_task = shutdown.clone();
        let marker = cancel_marker_path_for_run_root(&run_root);
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_for_task.cancelled() => break,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(50)) => {
                        if marker.exists() {
                            run_token.cancel();
                            break;
                        }
                    }
                }
            }
        });
        Self { shutdown, handle }
    }
}

impl Drop for CancelMarkerGuard {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.handle.abort();
    }
}

fn emit_run_completed(
    state: &PipelineState,
    sender: Option<&broadcast::Sender<RunEvent>>,
    outcome: RunLoopOutcome,
) -> Result<()> {
    let status = match outcome {
        RunLoopOutcome::Done => "completed",
        RunLoopOutcome::PausedAtCap => "paused",
        RunLoopOutcome::Killed => "killed",
        RunLoopOutcome::Failed => "failed",
    };
    emit_event(
        state,
        sender,
        RunEventKind::RunCompleted {
            status: status.to_string(),
        },
    )
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

fn build_cli_subagent_prompt(state: &PipelineState, history: &[String]) -> String {
    let history_text = if history.is_empty() {
        "none".to_string()
    } else {
        history.join("\n")
    };
    format!(
        "You are a deadreckon CLI sub-agent running unattended coding work.\nGoal: {}\nWorking directory: {}\nModify files directly in the working directory. Do not write outside it. Do not ask questions. When finished, print a concise summary of changed files.\nHistory:\n{}",
        state.goal,
        state.working_dir.display(),
        history_text
    )
}

fn is_cli_provider_name(provider: &str) -> bool {
    provider.starts_with("cli:") || provider.starts_with("cli-")
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

fn load_or_reconstruct_history(
    state: &mut PipelineState,
    from_turn: Option<u32>,
) -> Result<Vec<String>> {
    let trace_reconstruction = reconstruct_history_from_traces(state)?;
    let history_exists = history_path(state).exists();
    let mut history = if history_exists {
        load_history(state)?
    } else {
        trace_reconstruction.history
    };
    if let Some(from_turn) = from_turn {
        history.truncate(from_turn as usize);
        state.turn = from_turn;
        truncate_run_artifacts_after_turn(state, from_turn)?;
        save_history(state, &history)?;
        save_state(state)?;
    } else if !history_exists && trace_reconstruction.last_complete_turn > state.turn {
        state.turn = trace_reconstruction.last_complete_turn;
        save_history(state, &history)?;
        save_state(state)?;
    }
    Ok(history)
}

struct ReconstructedHistory {
    history: Vec<String>,
    last_complete_turn: u32,
}

fn reconstruct_history_from_traces(state: &PipelineState) -> Result<ReconstructedHistory> {
    let path = state.run_root.join("traces.jsonl");
    if !path.exists() {
        return Ok(ReconstructedHistory {
            history: Vec::new(),
            last_complete_turn: 0,
        });
    }
    let raw = std::fs::read_to_string(&path).with_path(&path)?;
    let mut complete_lines = Vec::new();
    let mut history = Vec::new();
    let mut last_complete_turn = 0;
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            break;
        };
        complete_lines.push(line.to_string());
        let event = value
            .get("event")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let turn = value
            .get("turn")
            .and_then(Value::as_u64)
            .unwrap_or_default() as u32;
        if is_completed_tool_trace(event)
            && let Some(tool_call_id) = value
                .pointer("/detail/tool_call_id")
                .and_then(Value::as_str)
        {
            history.push(format!(
                "tool {tool_call_id} result: reconstructed from trace"
            ));
            last_complete_turn = last_complete_turn.max(turn);
        }
    }
    let sanitized = complete_lines.join("\n");
    if sanitized.len() != raw.trim_end_matches('\n').len() {
        std::fs::write(
            &path,
            if sanitized.is_empty() {
                String::new()
            } else {
                format!("{sanitized}\n")
            },
        )
        .with_path(&path)?;
    }
    Ok(ReconstructedHistory {
        history,
        last_complete_turn,
    })
}

fn is_completed_tool_trace(event: &str) -> bool {
    matches!(event, "tool.bash" | "tool.write_file" | "tool.cli_subagent")
}

fn truncate_run_artifacts_after_turn(state: &PipelineState, from_turn: u32) -> Result<()> {
    for name in ["traces.jsonl", "spend.jsonl"] {
        truncate_jsonl_after_turn(&state.run_root.join(name), from_turn)?;
    }
    let snapshots = state.run_root.join("snapshots");
    if let Ok(entries) = std::fs::read_dir(&snapshots) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(turn) = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("turn-"))
                .and_then(|turn| turn.parse::<u32>().ok())
            else {
                continue;
            };
            if turn > from_turn {
                std::fs::remove_dir_all(&path).with_path(&path)?;
            }
        }
    }
    Ok(())
}

fn truncate_jsonl_after_turn(path: &Path, from_turn: u32) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(path).with_path(path)?;
    let mut kept = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            break;
        };
        if value
            .get("turn")
            .and_then(Value::as_u64)
            .is_none_or(|turn| turn <= u64::from(from_turn))
        {
            kept.push(line.to_string());
        }
    }
    std::fs::write(
        path,
        if kept.is_empty() {
            String::new()
        } else {
            format!("{}\n", kept.join("\n"))
        },
    )
    .with_path(path)
}

fn promote_if_ready(state: &mut PipelineState) -> Result<()> {
    let paths = paths_for_state(state)?;
    promote_completed_run(&paths, state).map(|_| ())
}

async fn complete_run_docs(
    state: &mut PipelineState,
    router: &ProviderRouter,
    config: &RunLoopConfig,
) -> Result<()> {
    let owned_router = if let (Some(config_path), Some(doc_provider)) = (
        config.docs.config_path.as_ref(),
        config.docs.doc_provider.as_deref(),
    ) {
        ProviderRouter::from_config_path(config_path, Some(doc_provider)).ok()
    } else {
        None
    };
    let router = owned_router.as_ref().unwrap_or(router);
    let previous_status = state.status;
    state.status = RunStatus::Completed;
    let result = polish_run_docs(
        state,
        router,
        &PolishConfig {
            home: config.docs.home.clone(),
            doc_skill: config.docs.doc_skill.clone(),
            doc_provider: config.docs.doc_provider.clone(),
            no_llm: config.docs.no_docs,
            force: false,
        },
    )
    .await;
    state.status = previous_status;
    result
}

fn paths_for_state(state: &PipelineState) -> Result<DeadreckonPaths> {
    let home = state
        .run_root
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| {
            DeadreckonError::InvalidInput(format!(
                "cannot infer home from run root {}",
                state.run_root.display()
            ))
        })?;
    Ok(DeadreckonPaths::from_home(home))
}

fn provider_output_name(provider: &str) -> &'static str {
    match provider {
        "cli:claude-code" => "claude.out",
        "cli:codex" => "codex.out",
        _ => "provider.out",
    }
}

fn run_acceptance_gate(state: &PipelineState) -> Result<()> {
    let gate = gate_binary_path()?;
    let output = std::process::Command::new(&gate)
        .arg("--run")
        .arg(&state.run_id)
        .arg("--run-root")
        .arg(&state.run_root)
        .arg("--working-dir")
        .arg(&state.working_dir)
        .output()
        .map_err(|source| DeadreckonError::Io {
            path: gate.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(DeadreckonError::InvalidInput(format!(
            "dr-gate failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(())
}

fn commit_worktree_turn(state: &PipelineState, turn: u32, label: &str) -> Result<()> {
    let record = match read_codebase_record(&state.working_dir) {
        Ok(record) => record,
        Err(_) => return Ok(()),
    };
    if record.mode != CodebaseMode::Worktree {
        return Ok(());
    }
    git_status(
        &state.working_dir,
        &["config", "user.email", "deadreckon@example.invalid"],
    )?;
    git_status(&state.working_dir, &["config", "user.name", "deadreckon"])?;
    git_status(&state.working_dir, &["add", "-A"])?;
    if git_quiet(&state.working_dir, &["diff", "--cached", "--quiet"])? {
        return Ok(());
    }
    git_status(
        &state.working_dir,
        &["commit", "-m", &format!("turn {turn}: {label}")],
    )
}

fn git_quiet(cwd: &Path, args: &[&str]) -> Result<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|source| DeadreckonError::Io {
            path: PathBuf::from("git"),
            source,
        })?;
    Ok(output.status.success())
}

fn git_status(cwd: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|source| DeadreckonError::Io {
            path: PathBuf::from("git"),
            source,
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(DeadreckonError::InvalidInput(format!(
            "git {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

fn gate_binary_path() -> Result<PathBuf> {
    let current = std::env::current_exe().map_err(|source| DeadreckonError::Io {
        path: PathBuf::from("/Users/gdc/deadreckon/target"),
        source,
    })?;
    let gate = current.with_file_name("dr-gate");
    if gate.exists() {
        return Ok(gate);
    }
    Err(DeadreckonError::NotFound(format!(
        "dr-gate binary next to {}",
        current.display()
    )))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use deadreckon_providers::ProviderRouter;
    use deadreckon_sandbox::SandboxBackend;
    use tempfile::TempDir;

    use crate::events::{RunEventBus, RunEventKind};
    use crate::paths::DeadreckonPaths;
    use crate::state::{RunOptions, RunStatus, create_run};

    use super::{RunLoopConfig, RunLoopDocsConfig, load_or_reconstruct_history, run_turn_loop};

    #[tokio::test]
    async fn tui_streams_tool_call_within_250ms() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "stream".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        let bus = RunEventBus::new(32);
        let mut receiver = bus.subscribe();
        let router = ProviderRouter::smoke();
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_for_loop = cancel.clone();
        let handle = tokio::spawn(async move {
            let _ = run_turn_loop(
                &mut state,
                &router,
                RunLoopConfig {
                    provider: Some("smoke".to_string()),
                    max_spend_usd: Some(1.0),
                    max_wall_seconds: None,
                    sandbox_backend: SandboxBackend::None,
                    max_turns: 1,
                    from_turn: None,
                    event_sender: Some(bus.sender()),
                    cancellation_token: Some(cancel_for_loop),
                    docs: RunLoopDocsConfig {
                        home: paths.home().to_path_buf(),
                        config_path: None,
                        doc_provider: None,
                        doc_skill: "run-narrator".to_string(),
                        no_docs: true,
                    },
                },
            )
            .await;
        });
        let deadline = tokio::time::timeout(Duration::from_millis(250), async move {
            loop {
                let event = receiver.recv().await.expect("event");
                if matches!(event.event, RunEventKind::ToolCallStarted { .. }) {
                    return;
                }
            }
        })
        .await;
        assert!(
            deadline.is_ok(),
            "tool-call event missed 250ms streaming SLA"
        );
        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn tui_detach_does_not_kill_run() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "detach".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        let bus = RunEventBus::new(32);
        let mut receiver = bus.subscribe();
        let router = ProviderRouter::smoke();
        let handle = tokio::spawn(async move {
            run_turn_loop(
                &mut state,
                &router,
                RunLoopConfig {
                    provider: Some("smoke".to_string()),
                    max_spend_usd: Some(1.0),
                    max_wall_seconds: None,
                    sandbox_backend: SandboxBackend::None,
                    max_turns: 1,
                    from_turn: None,
                    event_sender: Some(bus.sender()),
                    cancellation_token: None,
                    docs: RunLoopDocsConfig {
                        home: paths.home().to_path_buf(),
                        config_path: None,
                        doc_provider: None,
                        doc_skill: "run-narrator".to_string(),
                        no_docs: true,
                    },
                },
            )
            .await
            .map(|_| state)
        });
        let _ = receiver.recv().await.expect("first event");
        drop(receiver);
        let outcome = handle.await.expect("join").expect("loop");
        assert_ne!(outcome.status, RunStatus::Killed);
        assert!(outcome.run_root.join("events.jsonl").exists());
    }

    #[test]
    fn resume_partial_trace_replays_history() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "partial".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "none".to_string(),
                provider: Some("mock".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        std::fs::write(
            state.run_root.join("traces.jsonl"),
            r#"{"timestamp":"2026-05-11T00:00:00Z","run_id":"r","turn":1,"event":"tool.bash","latency_ms":1,"detail":{"tool_call_id":"tool-1"}}
{"timestamp":"#,
        )
        .expect("trace");

        let history = load_or_reconstruct_history(&mut state, None).expect("history");
        assert_eq!(history.len(), 1);
        assert!(history[0].contains("tool-1"));
        let trace = std::fs::read_to_string(state.run_root.join("traces.jsonl")).expect("trace");
        assert_eq!(trace.lines().count(), 1);
        assert_eq!(state.turn, 1);
    }

    #[test]
    fn resume_partial_trace_ignores_mid_tool_call_trace() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "mid-tool".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "none".to_string(),
                provider: Some("mock".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        std::fs::write(
            state.run_root.join("traces.jsonl"),
            r#"{"timestamp":"2026-05-11T00:00:00Z","run_id":"r","turn":1,"event":"tool.bash.started","latency_ms":null,"detail":{"tool_call_id":"tool-open"}}
{"timestamp":"2026-05-11T00:00:01Z","run_id":"r","turn":1,"event":"tool.bash","latency_ms":1,"detail":{"tool_call_id":"tool-done"}}
"#,
        )
        .expect("trace");

        let history = load_or_reconstruct_history(&mut state, None).expect("history");

        assert_eq!(history.len(), 1);
        assert!(history[0].contains("tool-done"));
        assert!(!history[0].contains("tool-open"));
        assert_eq!(state.turn, 1);
    }

    #[test]
    fn resume_from_turn_override() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "from-turn".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "none".to_string(),
                provider: Some("mock".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        let history = vec!["one", "two", "three", "four", "five"];
        std::fs::write(
            state.run_root.join("history.json"),
            serde_json::to_vec_pretty(&history).expect("json"),
        )
        .expect("history");
        state.turn = 5;

        let history = load_or_reconstruct_history(&mut state, Some(2)).expect("history");
        assert_eq!(history, vec!["one".to_string(), "two".to_string()]);
        assert_eq!(state.turn, 2);
    }

    #[test]
    fn from_turn_override_truncates_trace_tail_and_future_snapshots() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "from-turn-artifacts".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "none".to_string(),
                provider: Some("mock".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        std::fs::write(
            state.run_root.join("history.json"),
            serde_json::to_vec_pretty(&vec!["one", "two", "three"]).expect("json"),
        )
        .expect("history");
        std::fs::write(
            state.run_root.join("traces.jsonl"),
            r#"{"timestamp":"2026-05-11T00:00:00Z","run_id":"r","turn":1,"event":"tool.bash","latency_ms":1,"detail":{"tool_call_id":"tool-1"}}
{"timestamp":"2026-05-11T00:00:01Z","run_id":"r","turn":3,"event":"tool.bash","latency_ms":1,"detail":{"tool_call_id":"tool-3"}}
"#,
        )
        .expect("trace");
        let future_snapshot = state.run_root.join("snapshots/turn-3");
        std::fs::create_dir_all(&future_snapshot).expect("snapshot");
        std::fs::write(future_snapshot.join("future.txt"), "future").expect("future");
        state.turn = 3;

        let history = load_or_reconstruct_history(&mut state, Some(1)).expect("history");
        let trace = std::fs::read_to_string(state.run_root.join("traces.jsonl")).expect("trace");

        assert_eq!(history, vec!["one".to_string()]);
        assert!(trace.contains("tool-1"));
        assert!(!trace.contains("tool-3"));
        assert!(!future_snapshot.exists());
        assert_eq!(state.turn, 1);
    }
}
