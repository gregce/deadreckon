use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;
use deadreckon_providers::{ProviderKind, ProviderRequest, ProviderResponse, ProviderRouter};
use deadreckon_sandbox::{SandboxBackend, SandboxSpec, ToolSandboxPolicy, run as run_sandbox};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::compaction::{append_compaction_record, compact_history, read_compaction_config};
use crate::error::IoContext;
use crate::flight::{ProviderFlightRecorder, ProviderFlightRecorderHandle};
use crate::polish::{PolishConfig, polish_run_docs};
use deadreckon_core::artifacts::{
    ProvenanceRecord, SpendRecord, TraceRecord, append_provenance, append_spend, append_trace,
    inventory_files, snapshot_working,
};
use deadreckon_core::cancel::{cancel_marker_path_for_run_root, cancel_marker_present};
use deadreckon_core::codebase::{CodebaseMode, read_codebase_record};
use deadreckon_core::docs::{
    IMPLEMENTATION_NOTES_HTML, ImplementationNotesStatus, TurnDocInput, append_turn_doc,
    check_implementation_notes_current, incremental_path, rewrite_templated_docs,
};
use deadreckon_core::error::{DeadreckonError, Result};
use deadreckon_core::events::{RunEvent, RunEventKind, emit_event, event_preview, tool_args_json};
use deadreckon_core::flight::FlightSessionStatus;
use deadreckon_core::gate::{acceptance_spec_path_for_run_root, validate_acceptance_marker};
use deadreckon_core::git::run_git;
use deadreckon_core::paths::DeadreckonPaths;
use deadreckon_core::promotion::promote_completed_run;
use deadreckon_core::state::{
    PhaseId, PhaseStatus, PipelineState, RunStatus, append_json_line, save_state,
};

use crate::seam::{
    SeamKind, SeamOutcome, SeamRunCtx, SeamsConfig, dispatch_seam, read_seams_config,
    write_seams_audit,
};

#[derive(Debug, Clone)]
pub struct RunLoopConfig {
    pub provider: Option<String>,
    pub max_spend_usd: Option<f64>,
    pub max_wall_seconds: Option<f64>,
    pub sandbox_backend: SandboxBackend,
    pub no_seams: bool,
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
    pub doc_provider_source: Option<String>,
    pub doc_subskills: Vec<String>,
    pub token_budget: u32,
    pub budget_cap_usd: Option<f64>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SandboxToml {
    version: u32,
    tools: BTreeMap<String, SandboxTomlTool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SandboxTomlTool {
    #[serde(default)]
    read: Vec<PathBuf>,
    #[serde(default)]
    write: Vec<PathBuf>,
    #[serde(default)]
    network: Vec<String>,
}

pub async fn run_turn_loop(
    state: &mut PipelineState,
    router: &ProviderRouter,
    config: RunLoopConfig,
) -> Result<RunLoopOutcome> {
    let mut config = config;
    // AS-BUILT §9: the harness, not the model, owns the bounded mutation loop
    // and writes state after every turn boundary.
    let mut history = load_or_reconstruct_history(state, config.from_turn)?;
    ensure_sandbox_toml(state)?;
    let seam_config_path = config
        .docs
        .config_path
        .clone()
        .unwrap_or(paths_for_state(state)?.config_path());
    let seams = read_seams_config(&seam_config_path, config.no_seams)?;
    let compaction = read_compaction_config(&seam_config_path)?;
    write_seams_audit(&state.run_root, &state.run_id, &seams)?;
    let seam_ctx = SeamRunCtx {
        run_root: state.run_root.clone(),
        working_dir: state.working_dir.clone(),
        sandbox_backend: config.sandbox_backend,
    };
    let _event_sink_forwarder = if seams.command_for(SeamKind::EventSink).is_some() {
        if config.event_sender.is_none() {
            let (sender, _) = broadcast::channel(256);
            config.event_sender = Some(sender);
        }
        config
            .event_sender
            .as_ref()
            .map(|sender| spawn_event_sink_forwarder(seams.clone(), seam_ctx.clone(), sender))
    } else {
        None
    };
    let run_token = config.cancellation_token.clone().unwrap_or_default();
    let _cancel_marker_guard = CancelMarkerGuard::spawn(&state.run_root, run_token.clone());
    if should_cancel_run(state, &run_token) {
        state.status = deadreckon_core::state::RunStatus::Killed;
        state.failure_reason = Some("run cancelled before turn loop".to_string());
        save_state(state)?;
        emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Killed)?;
        return Ok(RunLoopOutcome::Killed);
    }
    state.set_phase_status(PhaseId(40), PhaseStatus::Executing)?;
    save_state(state)?;
    if let Some(from_turn) = config.from_turn {
        state.turn = from_turn;
        save_state(state)?;
    }

    for _ in 0..config.max_turns {
        if should_cancel_run(state, &run_token) {
            state.status = deadreckon_core::state::RunStatus::Killed;
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
        let selected_route = router.selected_route_info();
        let selected_provider = config
            .provider
            .clone()
            .or_else(|| selected_route.as_ref().map(|route| route.name.clone()));
        let mut prompt_history = history.clone();
        if selected_route
            .as_ref()
            .is_some_and(|route| is_direct_api_provider_kind(&route.kind))
        {
            let (context_window, source) = router
                .context_window_for_route_with_source(selected_provider.as_deref())
                .map(|(window, source)| (window, source.as_str().to_string()))
                .unwrap_or((compaction.fallback_context_window, "fallback".to_string()));
            let (compacted, record) =
                compact_history(&history, context_window, compaction, turn, &source);
            if let Some(record) = record {
                append_compaction_record(&state.run_root, &record)?;
            }
            prompt_history = compacted;
        }
        let prompt = if selected_provider
            .as_deref()
            .is_some_and(is_cli_provider_name)
        {
            build_cli_subagent_prompt(state, &history)
        } else {
            build_prompt(state, &prompt_history)
        };
        let turn_dir = state.run_root.join("turns").join(format!("turn-{turn}"));
        let stdout_name = selected_provider
            .as_deref()
            .map(provider_output_name)
            .unwrap_or_else(|| "provider.out".to_string());
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

        let mut flight_recorder: Option<ProviderFlightRecorderHandle> =
            match selected_provider.as_deref() {
                Some(provider) if is_cli_provider_name(provider) => {
                    ProviderFlightRecorder::start(state, provider, &config.docs.home, turn)?
                        .map(|recorder| recorder.spawn(state.clone()))
                }
                _ => None,
            };
        let started = Instant::now();
        let response = match router.complete(&request).await {
            Ok(response) => response,
            Err(err) if should_cancel_run(state, &run_token) => {
                if let Some(recorder) = flight_recorder.take() {
                    recorder.finish(state, FlightSessionStatus::Killed).await?;
                }
                state.status = deadreckon_core::state::RunStatus::Killed;
                state.failure_reason = Some(format!("run cancelled during provider call: {err}"));
                save_state(state)?;
                emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Killed)?;
                return Ok(RunLoopOutcome::Killed);
            }
            Err(err) => {
                if let Some(recorder) = flight_recorder.take() {
                    recorder.finish(state, FlightSessionStatus::Failed).await?;
                }
                return Err(provider_error(&err));
            }
        };
        if should_cancel_run(state, &run_token) {
            if let Some(recorder) = flight_recorder.take() {
                recorder.finish(state, FlightSessionStatus::Killed).await?;
            }
            state.status = deadreckon_core::state::RunStatus::Killed;
            state.failure_reason = Some("run cancelled after provider call".to_string());
            save_state(state)?;
            emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Killed)?;
            return Ok(RunLoopOutcome::Killed);
        }
        if let Some(recorder) = flight_recorder.take() {
            recorder
                .finish(state, FlightSessionStatus::Completed)
                .await?;
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
                estimated: false,
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
            emit_tool_event_with_hook(
                state,
                config.event_sender.as_ref(),
                &seams,
                &seam_ctx,
                RunEventKind::ToolCallStarted {
                    turn,
                    tool_call_id: tool_call_id.clone(),
                    tool_name: "cli_subagent".to_string(),
                    args: response.trace.clone(),
                },
            )
            .await?;
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
            append_turn_doc_checkpoint(
                state,
                config.event_sender.as_ref(),
                TurnDocInput {
                    turn,
                    tool_kind: "cli_subagent".to_string(),
                    latency_ms: response
                        .trace
                        .get("duration_ms")
                        .and_then(Value::as_u64)
                        .map(u128::from),
                    files: changed,
                    outcome: response.content.clone(),
                    response_text: response.content.clone(),
                    tool_stdout: response
                        .trace
                        .get("stdout")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    tool_stderr: response
                        .trace
                        .get("stderr")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                },
            )?;
            emit_tool_event_with_hook(
                state,
                config.event_sender.as_ref(),
                &seams,
                &seam_ctx,
                RunEventKind::ToolCallResult {
                    turn,
                    tool_call_id,
                    status: "ok".to_string(),
                    preview: event_preview(&response.content),
                },
            )
            .await?;
            if !implementation_notes_ready_or_request_followup(
                state,
                config.event_sender.as_ref(),
                turn,
                &mut history,
            )? {
                state.turn = turn;
                save_history(state, &history)?;
                save_state(state)?;
                continue;
            }
            state.turn = turn;
            save_history(state, &history)?;
            save_state(state)?;
            complete_run_docs(state, router, &config).await?;
            if !acceptance_gate_passed_or_record_failure(
                state,
                config.event_sender.as_ref(),
                turn,
                &mut history,
            )? {
                continue;
            }
            promote_if_ready(state)?;
            state.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
            state.failure_reason = None;
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
                emit_tool_event_with_hook(
                    state,
                    config.event_sender.as_ref(),
                    &seams,
                    &seam_ctx,
                    RunEventKind::ToolCallStarted {
                        turn,
                        tool_call_id: tool_call_id.clone(),
                        tool_name: "bash".to_string(),
                        args: tool_args_json(&command),
                    },
                )
                .await?;
                let started = Instant::now();
                let policy = load_tool_policy_from_sandbox_toml(state, "bash")?;
                if let Some(reason) = bash_policy_refusal(state, &command, &policy) {
                    append_tool_refusal(
                        state,
                        turn,
                        &tool_call_id,
                        "bash",
                        &response.model,
                        &reason,
                        config.event_sender.as_ref(),
                    )?;
                    history.push(format!("tool {tool_call_id} refused: {reason}"));
                    state.turn = turn;
                    save_history(state, &history)?;
                    save_state(state)?;
                    continue;
                }
                if let Some(reason) = policy_seam_refusal(
                    &seams,
                    &seam_ctx,
                    &state.run_id,
                    "bash",
                    &command,
                    &state.working_dir,
                )
                .await
                {
                    append_tool_refusal(
                        state,
                        turn,
                        &tool_call_id,
                        "bash",
                        &response.model,
                        &reason,
                        config.event_sender.as_ref(),
                    )?;
                    history.push(format!("tool {tool_call_id} refused: {reason}"));
                    state.turn = turn;
                    save_history(state, &history)?;
                    save_state(state)?;
                    continue;
                }
                let output = match run_sandbox(SandboxSpec {
                    backend: config.sandbox_backend,
                    cwd: state.working_dir.clone(),
                    program: OsString::from("sh"),
                    args: vec![OsString::from("-lc"), OsString::from(command.clone())],
                    stdin: None,
                    env: BTreeMap::new(),
                    allow_network: policy.allow_network,
                    pid_file: Some(
                        state
                            .run_root
                            .join("child-pids")
                            .join(format!("tool-{tool_call_id}.pid")),
                    ),
                    cancellation_token: Some(tool_token),
                    profile_dir: Some(state.run_root.join("sandbox")),
                    read_allowlist: policy.read_allowlist,
                    write_allowlist: policy.write_allowlist,
                    read_denylist: Vec::new(),
                    write_denylist: Vec::new(),
                    network_allowlist: policy.network_allowlist,
                })
                .await
                {
                    Ok(output) => output,
                    Err(deadreckon_sandbox::SandboxError::Cancelled)
                        if should_cancel_run(state, &run_token) =>
                    {
                        state.status = deadreckon_core::state::RunStatus::Killed;
                        state.failure_reason = Some("run cancelled during tool call".to_string());
                        save_state(state)?;
                        emit_run_completed(
                            state,
                            config.event_sender.as_ref(),
                            RunLoopOutcome::Killed,
                        )?;
                        return Ok(RunLoopOutcome::Killed);
                    }
                    Err(err) => return Err(sandbox_error(&err)),
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
                append_turn_doc_checkpoint(
                    state,
                    config.event_sender.as_ref(),
                    TurnDocInput {
                        turn,
                        tool_kind: "bash".to_string(),
                        latency_ms: Some(started.elapsed().as_millis()),
                        files: changed,
                        outcome: format!("status={:?}", output.status_code),
                        response_text: response.content.clone(),
                        tool_stdout: Some(output.stdout.clone()),
                        tool_stderr: Some(output.stderr.clone()),
                    },
                )?;
                emit_tool_event_with_hook(
                    state,
                    config.event_sender.as_ref(),
                    &seams,
                    &seam_ctx,
                    RunEventKind::ToolCallResult {
                        turn,
                        tool_call_id: tool_call_id.clone(),
                        status: format!("{:?}", output.status_code),
                        preview: event_preview(format!("{}{}", output.stdout, output.stderr)),
                    },
                )
                .await?;
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
                emit_tool_event_with_hook(
                    state,
                    config.event_sender.as_ref(),
                    &seams,
                    &seam_ctx,
                    RunEventKind::ToolCallStarted {
                        turn,
                        tool_call_id: tool_call_id.clone(),
                        tool_name: "write_file".to_string(),
                        args: tool_args_json(path.display().to_string()),
                    },
                )
                .await?;
                let write_policy = load_tool_policy_from_sandbox_toml(state, "write_file")?;
                let target =
                    match safe_working_path_with_policy(&state.working_dir, &path, &write_policy) {
                        Ok(target) => target,
                        Err(err) => {
                            let reason = err.to_string();
                            append_tool_refusal(
                                state,
                                turn,
                                &tool_call_id,
                                "write_file",
                                &response.model,
                                &reason,
                                config.event_sender.as_ref(),
                            )?;
                            history.push(format!("tool {tool_call_id} refused: {reason}"));
                            state.turn = turn;
                            save_history(state, &history)?;
                            save_state(state)?;
                            continue;
                        }
                    };
                let target_label = target.display().to_string();
                if let Some(reason) = policy_seam_refusal(
                    &seams,
                    &seam_ctx,
                    &state.run_id,
                    "write_file",
                    &target_label,
                    &state.working_dir,
                )
                .await
                {
                    append_tool_refusal(
                        state,
                        turn,
                        &tool_call_id,
                        "write_file",
                        &response.model,
                        &reason,
                        config.event_sender.as_ref(),
                    )?;
                    history.push(format!("tool {tool_call_id} refused: {reason}"));
                    state.turn = turn;
                    save_history(state, &history)?;
                    save_state(state)?;
                    continue;
                }
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
                append_turn_doc_checkpoint(
                    state,
                    config.event_sender.as_ref(),
                    TurnDocInput {
                        turn,
                        tool_kind: "write_file".to_string(),
                        latency_ms: None,
                        files: changed,
                        outcome: "ok".to_string(),
                        response_text: response.content.clone(),
                        tool_stdout: None,
                        tool_stderr: None,
                    },
                )?;
                emit_tool_event_with_hook(
                    state,
                    config.event_sender.as_ref(),
                    &seams,
                    &seam_ctx,
                    RunEventKind::ToolCallResult {
                        turn,
                        tool_call_id: tool_call_id.clone(),
                        status: "ok".to_string(),
                        preview: "wrote file".to_string(),
                    },
                )
                .await?;
                history.push(format!("tool {tool_call_id} result: wrote file"));
            }
            Action::Done { summary } => {
                state.turn = turn;
                history.push(format!("done: {}", summary.clone().unwrap_or_default()));
                save_history(state, &history)?;
                save_state(state)?;
                append_turn_doc_checkpoint(
                    state,
                    config.event_sender.as_ref(),
                    TurnDocInput {
                        turn,
                        tool_kind: "done".to_string(),
                        latency_ms: None,
                        files: Vec::new(),
                        outcome: summary.clone().unwrap_or_else(|| "done".to_string()),
                        response_text: response.content.clone(),
                        tool_stdout: None,
                        tool_stderr: None,
                    },
                )?;
                if !implementation_notes_ready_or_request_followup(
                    state,
                    config.event_sender.as_ref(),
                    turn,
                    &mut history,
                )? {
                    state.turn = turn;
                    save_history(state, &history)?;
                    save_state(state)?;
                    continue;
                }
                complete_run_docs(state, router, &config).await?;
                if !acceptance_gate_passed_or_record_failure(
                    state,
                    config.event_sender.as_ref(),
                    turn,
                    &mut history,
                )? {
                    continue;
                }
                promote_if_ready(state)?;
                state.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
                state.failure_reason = None;
                save_state(state)?;
                emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Done)?;
                return Ok(RunLoopOutcome::Done);
            }
        }
        state.turn = turn;
        save_history(state, &history)?;
        save_state(state)?;
    }

    state.failure_reason = Some(match state.failure_reason.take() {
        Some(reason) => format!("{reason}; max turn budget exhausted"),
        None => "max turn budget exhausted".to_string(),
    });
    state.set_phase_status(PhaseId(40), PhaseStatus::Failed)?;
    save_state(state)?;
    emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Failed)?;
    Ok(RunLoopOutcome::Failed)
}

fn should_cancel_run(state: &PipelineState, token: &CancellationToken) -> bool {
    state.status == RunStatus::Killed || token.is_cancelled() || cancel_marker_present(state)
}

fn provider_error(err: &deadreckon_providers::ProviderError) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!("provider error: {err}"))
}

fn sandbox_error(err: &deadreckon_sandbox::SandboxError) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!("sandbox error: {err}"))
}

struct CancelMarkerGuard {
    shutdown: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

impl CancelMarkerGuard {
    fn spawn(run_root: &Path, run_token: CancellationToken) -> Self {
        let shutdown = CancellationToken::new();
        let shutdown_for_task = shutdown.clone();
        let marker = cancel_marker_path_for_run_root(run_root);
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

// SAFETY: `RunLoopOutcome` is the stable event vocabulary used at call sites.
#[allow(clippy::needless_pass_by_value)]
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
    let spec_text = spec_prompt_text(state);
    let skill_text = run_skill_text(state);
    format!(
        "You are deadreckon running unattended coding work.\nWorking directory: {}\nSPEC:\n{}\n\nSkill and implementation-notes contract:\n{}\n\nHistory:\n{}\n\nReturn exactly one JSON object with action bash, write_file, or done.",
        state.working_dir.display(),
        spec_text,
        skill_text,
        history_text
    )
}

fn build_cli_subagent_prompt(state: &PipelineState, history: &[String]) -> String {
    let history_text = if history.is_empty() {
        "none".to_string()
    } else {
        history.join("\n")
    };
    let spec_text = spec_prompt_text(state);
    let skill_text = run_skill_text(state);
    format!(
        "You are a deadreckon CLI sub-agent running unattended coding work.\nWorking directory: {}\nSPEC:\n{}\n\nSkill and implementation-notes contract:\n{}\n\nModify files directly in the working directory. Do not write outside it. Do not ask questions. When finished, print a concise summary of changed files.\nHistory:\n{}",
        state.working_dir.display(),
        spec_text,
        skill_text,
        history_text
    )
}

fn spec_prompt_text(state: &PipelineState) -> String {
    format!(
        "Goal:\n{}\n\nAcceptance criteria:\n{}",
        state.goal,
        acceptance_prompt_text(state)
    )
}

fn run_skill_text(state: &PipelineState) -> String {
    std::fs::read_to_string(&state.skill_path).unwrap_or_else(|_| implementation_notes_contract())
}

fn implementation_notes_contract() -> String {
    format!(
        "Implement the SPEC in the working directory.\n\nAs you work, maintain {IMPLEMENTATION_NOTES_HTML} at the working-directory root. Keep it current with anything the owner should know about how the implementation interprets or diverges from the spec:\n- Design decisions: choices made where the spec was ambiguous.\n- Deviations: intentional departures from the spec, with reasons.\n- Tradeoffs: alternatives considered and why the chosen path won.\n- Open questions: anything the owner should confirm or revise.\n\nBefore reporting done, update {IMPLEMENTATION_NOTES_HTML} after the latest documentable code/config/test/doc change. The run docs render the same content into RUN-DECISIONS.md, which is the published Markdown ledger."
    )
}

fn acceptance_prompt_text(state: &PipelineState) -> String {
    let yaml_path = acceptance_spec_path_for_run_root(&state.run_root);
    let yaml = std::fs::read_to_string(&yaml_path).ok();
    let markdown = std::fs::read_to_string(state.run_root.join("acceptance.md")).ok();
    match (yaml, markdown) {
        (Some(yaml), Some(markdown)) => format!(
            "{}\n\nacceptance.yaml:\n{}",
            markdown.trim(),
            yaml.trim()
        ),
        (Some(yaml), None) => format!("acceptance.yaml:\n{}", yaml.trim()),
        (None, _) => {
            "default dr-gate behavior: working directory exists, or cargo test when Cargo.toml exists"
                .to_string()
        }
    }
}

fn is_cli_provider_name(provider: &str) -> bool {
    provider.starts_with("cli:") || provider.starts_with("cli-")
}

fn is_direct_api_provider_kind(kind: &ProviderKind) -> bool {
    matches!(
        kind,
        ProviderKind::Anthropic | ProviderKind::OpenAi | ProviderKind::OpenAiCompatible
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
            "unsafe write path {}\ntry: write inside the run working directory or edit sandbox.toml [tools.write_file].write",
            relative.display()
        )));
    }
    Ok(root.join(relative))
}

fn safe_working_path_with_policy(
    root: &Path,
    relative: &Path,
    policy: &ToolSandboxPolicy,
) -> Result<PathBuf> {
    let target = safe_working_path(root, relative)?;
    if policy
        .write_allowlist
        .iter()
        .any(|allowed| target.starts_with(allowed))
    {
        return Ok(target);
    }
    Err(DeadreckonError::InvalidInput(format!(
        "write_file denied by sandbox.toml for {}\ntry: edit sandbox.toml [tools.write_file].write or choose an allowed project-local path",
        relative.display()
    )))
}

fn sandbox_toml_path(state: &PipelineState) -> PathBuf {
    state.run_root.join("sandbox.toml")
}

fn ensure_sandbox_toml(state: &PipelineState) -> Result<()> {
    let path = sandbox_toml_path(state);
    if path.exists() {
        return Ok(());
    }
    let mut tools = BTreeMap::new();
    tools.insert(
        "bash".to_string(),
        SandboxTomlTool {
            read: vec![state.working_dir.clone()],
            write: vec![state.working_dir.clone()],
            network: Vec::new(),
        },
    );
    tools.insert(
        "write_file".to_string(),
        SandboxTomlTool {
            read: vec![state.working_dir.clone()],
            write: vec![state.working_dir.clone()],
            network: Vec::new(),
        },
    );
    let config = SandboxToml { version: 1, tools };
    let raw = toml::to_string_pretty(&config).map_err(|err| {
        DeadreckonError::InvalidInput(format!("sandbox.toml encode error: {err}"))
    })?;
    std::fs::write(&path, raw).with_path(&path)
}

fn load_tool_policy_from_sandbox_toml(
    state: &PipelineState,
    tool_name: &str,
) -> Result<ToolSandboxPolicy> {
    ensure_sandbox_toml(state)?;
    let path = sandbox_toml_path(state);
    let raw = std::fs::read_to_string(&path).with_path(&path)?;
    let config = toml::from_str::<SandboxToml>(&raw).map_err(|err| {
        DeadreckonError::InvalidInput(format!(
            "invalid sandbox.toml: {err}\ntry: fix {}",
            path.display()
        ))
    })?;
    let tool = config
        .tools
        .get(tool_name)
        .cloned()
        .unwrap_or(SandboxTomlTool {
            read: vec![state.working_dir.clone()],
            write: vec![state.working_dir.clone()],
            network: Vec::new(),
        });
    Ok(ToolSandboxPolicy {
        allow_network: !tool.network.is_empty(),
        read_allowlist: tool.read,
        write_allowlist: tool.write,
        network_allowlist: tool.network,
    })
}

fn bash_policy_refusal(
    state: &PipelineState,
    command: &str,
    policy: &ToolSandboxPolicy,
) -> Option<String> {
    if command.contains(".ssh")
        && !policy
            .read_allowlist
            .iter()
            .any(|allowed| allowed.to_string_lossy().contains(".ssh"))
    {
        return Some(format!(
            "bash denied by sandbox.toml: ~/.ssh is outside the read allowlist\ntry: edit {} [tools.bash].read or choose a project-local path",
            sandbox_toml_path(state).display()
        ));
    }
    None
}

async fn policy_seam_refusal(
    seams: &SeamsConfig,
    ctx: &SeamRunCtx,
    run_id: &str,
    function_id: &str,
    command: &str,
    working_dir: &Path,
) -> Option<String> {
    match dispatch_seam(
        SeamKind::Policy,
        &json!({
            "function_id": function_id,
            "command": command,
            "working_dir": working_dir,
        }),
        seams,
        ctx,
    )
    .await
    {
        SeamOutcome::Deny(reason) => {
            Some(policy_seam_refusal_message(run_id, function_id, &reason))
        }
        SeamOutcome::Ok(_) | SeamOutcome::Unconfigured => None,
        SeamOutcome::Fallback => None,
        SeamOutcome::Skipped(reason) => Some(policy_seam_refusal_message(
            run_id,
            function_id,
            &format!("unexpected skipped outcome: {reason}"),
        )),
    }
}

fn policy_seam_refusal_message(run_id: &str, function_id: &str, reason: &str) -> String {
    format!(
        "seam 'policy' denied {function_id}: {reason}\ntry: deadreckon show {run_id} to review, adjust the policy worker, or re-run with --no-seams"
    )
}

async fn emit_tool_event_with_hook(
    state: &PipelineState,
    sender: Option<&broadcast::Sender<RunEvent>>,
    seams: &SeamsConfig,
    ctx: &SeamRunCtx,
    event: RunEventKind,
) -> Result<()> {
    emit_event(state, sender, event.clone())?;
    dispatch_hook_event(seams, ctx, &event).await;
    Ok(())
}

async fn dispatch_hook_event(seams: &SeamsConfig, ctx: &SeamRunCtx, event: &RunEventKind) {
    let Ok(req) = serde_json::to_value(event) else {
        return;
    };
    let _ = dispatch_seam(SeamKind::Hooks, &req, seams, ctx).await;
}

fn spawn_event_sink_forwarder(
    seams: SeamsConfig,
    ctx: SeamRunCtx,
    sender: &broadcast::Sender<RunEvent>,
) -> tokio::task::JoinHandle<()> {
    let mut receiver = sender.subscribe();
    tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => {
                    let Ok(req) = serde_json::to_value(event) else {
                        continue;
                    };
                    let _ = dispatch_seam(SeamKind::EventSink, &req, &seams, &ctx).await;
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
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

fn append_tool_refusal(
    state: &PipelineState,
    turn: u32,
    tool_call_id: &str,
    tool_name: &str,
    model: &str,
    reason: &str,
    sender: Option<&broadcast::Sender<RunEvent>>,
) -> Result<()> {
    append_trace(
        state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn,
            event: "tool.refused".to_string(),
            latency_ms: None,
            detail: json!({
                "tool_call_id": tool_call_id,
                "tool_name": tool_name,
                "reason": reason,
            }),
        },
    )?;
    append_json_line(
        &state.run_root.join("provenance.jsonl"),
        &json!({
            "timestamp": Utc::now(),
            "prompt_id": format!("turn-{turn}"),
            "model": model,
            "tool_call_id": tool_call_id,
            "session_id": state.run_id.clone(),
            "files": [],
            "event": "tool.refused",
            "tool_name": tool_name,
            "reason": reason,
        }),
    )?;
    emit_event(
        state,
        sender,
        RunEventKind::ToolCallResult {
            turn,
            tool_call_id: tool_call_id.to_string(),
            status: "refused".to_string(),
            preview: event_preview(reason),
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
            doc_provider_source: config.docs.doc_provider_source.clone(),
            doc_subskills: config.docs.doc_subskills.clone(),
            token_budget: config.docs.token_budget,
            budget_cap_usd: config.docs.budget_cap_usd,
            no_llm: config.docs.no_docs,
            force: false,
        },
    )
    .await;
    state.status = previous_status;
    result
}

fn append_turn_doc_checkpoint(
    state: &PipelineState,
    sender: Option<&broadcast::Sender<RunEvent>>,
    input: TurnDocInput,
) -> Result<()> {
    let turn = input.turn;
    append_turn_doc(state, input)?;
    emit_event(
        state,
        sender,
        RunEventKind::DocsCheckpoint {
            turn,
            path: incremental_path(&state.working_dir),
            status: "turn-end".to_string(),
        },
    )
}

fn implementation_notes_ready_or_request_followup(
    state: &PipelineState,
    sender: Option<&broadcast::Sender<RunEvent>>,
    turn: u32,
    history: &mut Vec<String>,
) -> Result<bool> {
    let status = check_implementation_notes_current(state)?;
    if status.is_current() {
        rewrite_templated_docs(state, "templated only")?;
        return Ok(true);
    }
    let reason = status.reason();
    let try_line = implementation_notes_try_line(&status, state);
    append_trace(
        state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn,
            event: "docs.implementation_notes_required".to_string(),
            latency_ms: None,
            detail: json!({
                "reason": reason,
                "try": try_line,
            }),
        },
    )?;
    emit_event(
        state,
        sender,
        RunEventKind::Error {
            turn: Some(turn),
            message: event_preview(format!("{reason}; try: {try_line}")),
        },
    )?;
    history.push(format!(
        "implementation notes are required before done: {reason}. Update {IMPLEMENTATION_NOTES_HTML} with Design decisions, Deviations, Tradeoffs, and Open questions, then report done again."
    ));
    Ok(false)
}

fn implementation_notes_try_line(
    status: &ImplementationNotesStatus,
    state: &PipelineState,
) -> String {
    match status {
        ImplementationNotesStatus::Missing => format!(
            "create {}/{}",
            state.working_dir.display(),
            IMPLEMENTATION_NOTES_HTML
        ),
        ImplementationNotesStatus::MissingSections(_) => {
            "add Design decisions, Deviations, Tradeoffs, and Open questions sections".to_string()
        }
        ImplementationNotesStatus::Stale { .. } => {
            format!(
                "edit {}",
                state.working_dir.join(IMPLEMENTATION_NOTES_HTML).display()
            )
        }
        ImplementationNotesStatus::Current => {
            let prefix = state.run_id.chars().take(8).collect::<String>();
            format!("deadreckon doc {prefix} --kind decisions")
        }
    }
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

fn provider_output_name(provider: &str) -> String {
    if provider == "cli:claude-code" {
        return "claude.out".to_string();
    }
    let Some(cli_id) = provider.strip_prefix("cli:") else {
        return "provider.out".to_string();
    };
    let slug = cli_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() {
        "provider.out".to_string()
    } else {
        format!("{slug}.out")
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

fn acceptance_gate_passed_or_record_failure(
    state: &mut PipelineState,
    sender: Option<&broadcast::Sender<RunEvent>>,
    turn: u32,
    history: &mut Vec<String>,
) -> Result<bool> {
    match run_acceptance_gate(state).and_then(|()| validate_acceptance_marker(state)) {
        Ok(_) => Ok(true),
        Err(err) => {
            let reason = err.to_string();
            append_trace(
                state,
                &TraceRecord {
                    timestamp: Utc::now(),
                    run_id: state.run_id.clone(),
                    turn,
                    event: "acceptance.failed".to_string(),
                    latency_ms: None,
                    detail: json!({ "reason": reason }),
                },
            )?;
            emit_event(
                state,
                sender,
                RunEventKind::Error {
                    turn: Some(turn),
                    message: event_preview(format!("acceptance failed: {reason}")),
                },
            )?;
            history.push(format!(
                "acceptance failed after turn {turn}: {reason}. Continue by fixing the failing done criteria; do not declare done until dr-gate passes."
            ));
            state.failure_reason = Some(format!("acceptance failed after turn {turn}: {reason}"));
            save_history(state, history)?;
            save_state(state)?;
            Ok(false)
        }
    }
}

fn commit_worktree_turn(state: &PipelineState, turn: u32, label: &str) -> Result<()> {
    let Ok(record) = read_codebase_record(&state.working_dir) else {
        return Ok(());
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
    let output = run_git(cwd, args)?;
    Ok(output.status.success())
}

fn git_status(cwd: &Path, args: &[&str]) -> Result<()> {
    let output = run_git(cwd, args)?;
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
        path: PathBuf::from("current-exe"),
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
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use deadreckon_providers::{ProviderKind, ProviderRouter};
    use deadreckon_sandbox::SandboxBackend;
    use serde_json::Value;
    use tempfile::TempDir;

    use crate::seam::{SeamRunCtx, read_seams_config};

    use deadreckon_core::events::{RunEventBus, RunEventKind};
    use deadreckon_core::flight::{
        FlightEventKind, FlightSessionStatus, list_checkpoint_manifests, read_flight_events,
        read_flight_manifest,
    };
    use deadreckon_core::gate::{run_acceptance_gate_and_write_marker, validate_acceptance_marker};
    use deadreckon_core::paths::DeadreckonPaths;
    use deadreckon_core::state::{PipelineState, RunOptions, RunStatus, create_run};
    use deadreckon_core::{TurnDocInput, append_turn_doc, implementation_notes_path};

    use super::{
        RunLoopConfig, RunLoopDocsConfig, RunLoopOutcome, append_tool_refusal, bash_policy_refusal,
        build_cli_subagent_prompt, build_prompt, ensure_sandbox_toml,
        implementation_notes_ready_or_request_followup, is_direct_api_provider_kind,
        load_or_reconstruct_history, load_tool_policy_from_sandbox_toml, policy_seam_refusal,
        policy_seam_refusal_message, provider_output_name, run_turn_loop, safe_working_path,
        safe_working_path_with_policy, save_history,
    };

    fn create_smoke_run(temp: &TempDir, goal: &str) -> (DeadreckonPaths, PipelineState) {
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = temp.path().join("cwd");
        std::fs::create_dir_all(&cwd).expect("cwd");
        let state = create_run(
            &paths,
            RunOptions {
                goal: goal.to_string(),
                cwd,
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
        (paths, state)
    }

    fn create_direct_api_run(temp: &TempDir, goal: &str) -> (DeadreckonPaths, PipelineState) {
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = temp.path().join("cwd");
        std::fs::create_dir_all(&cwd).expect("cwd");
        let state = create_run(
            &paths,
            RunOptions {
                goal: goal.to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: Some("openai-compatible".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        (paths, state)
    }

    fn write_seams_config(paths: &DeadreckonPaths, raw: &str) -> PathBuf {
        let config_path = paths.config_path();
        std::fs::create_dir_all(config_path.parent().expect("config parent"))
            .expect("config parent");
        std::fs::write(&config_path, raw).expect("config");
        config_path
    }

    fn sh_quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }

    async fn run_smoke_turn(
        state: &mut PipelineState,
        paths: &DeadreckonPaths,
        config_path: Option<PathBuf>,
        no_seams: bool,
    ) -> RunLoopOutcome {
        let router = ProviderRouter::smoke();
        run_turn_loop(
            state,
            &router,
            RunLoopConfig {
                provider: Some("smoke".to_string()),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                sandbox_backend: SandboxBackend::None,
                no_seams,
                max_turns: 1,
                from_turn: None,
                event_sender: None,
                cancellation_token: None,
                docs: RunLoopDocsConfig {
                    home: paths.home().to_path_buf(),
                    config_path,
                    doc_provider: None,
                    doc_provider_source: None,
                    doc_subskills: Vec::new(),
                    token_budget: 0,
                    budget_cap_usd: None,
                    doc_skill: "run-narrator".to_string(),
                    no_docs: true,
                },
            },
        )
        .await
        .expect("loop")
    }

    async fn run_direct_api_until_missing_credential(
        state: &mut PipelineState,
        paths: &DeadreckonPaths,
        config_path: PathBuf,
    ) -> String {
        let router = ProviderRouter::from_config_path(&config_path, Some("openai-compatible"))
            .expect("router");
        run_turn_loop(
            state,
            &router,
            RunLoopConfig {
                provider: Some("openai-compatible".to_string()),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                sandbox_backend: SandboxBackend::None,
                no_seams: false,
                max_turns: 1,
                from_turn: None,
                event_sender: None,
                cancellation_token: None,
                docs: RunLoopDocsConfig {
                    home: paths.home().to_path_buf(),
                    config_path: Some(config_path),
                    doc_provider: None,
                    doc_provider_source: None,
                    doc_subskills: Vec::new(),
                    token_budget: 0,
                    budget_cap_usd: None,
                    doc_skill: "run-narrator".to_string(),
                    no_docs: true,
                },
            },
        )
        .await
        .expect_err("missing credential")
        .to_string()
    }

    fn read_seams_json(state: &PipelineState) -> Value {
        let raw = std::fs::read_to_string(state.run_root.join("seams.json")).expect("seams.json");
        serde_json::from_str(&raw).expect("seams json")
    }

    fn read_compaction_lines(state: &PipelineState) -> Vec<String> {
        std::fs::read_to_string(state.run_root.join("compaction.jsonl"))
            .expect("compaction")
            .lines()
            .map(str::to_string)
            .collect()
    }

    async fn read_until_contains(path: &Path, needle: &str) -> String {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Ok(raw) = std::fs::read_to_string(path)
                && raw.contains(needle)
            {
                return raw;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for {needle} in {}",
                path.display()
            );
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    #[test]
    fn provider_output_name_slugifies_cli_descriptor_id() {
        assert_eq!(provider_output_name("cli:codex"), "codex.out");
        assert_eq!(provider_output_name("cli:claude-code"), "claude.out");
        assert_eq!(provider_output_name("cli:gemini"), "gemini.out");
        assert_eq!(provider_output_name("cli:opencode"), "opencode.out");
        assert_eq!(provider_output_name("cli:copilot"), "copilot.out");
        assert_eq!(provider_output_name("cli:pi"), "pi.out");
        assert_eq!(provider_output_name("cli:local/test"), "local-test.out");
        assert_eq!(provider_output_name("anthropic"), "provider.out");
    }

    #[tokio::test]
    async fn policy_seam_deny_blocks_tool_call_and_records_denial() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "policy seam deny");
        let config_path = write_seams_config(
            &paths,
            r#"
[seams.policy]
command = ["/bin/sh", "-c", "cat >/dev/null; echo '{\"decision\":\"deny\",\"reason\":\"blocked by test\"}'"]
timeout_ms = 1000
"#,
        );

        let _ = run_smoke_turn(&mut state, &paths, Some(config_path), false).await;

        assert!(!state.working_dir.join("Cargo.toml").exists());
        let traces = std::fs::read_to_string(state.run_root.join("traces.jsonl")).expect("traces");
        let provenance =
            std::fs::read_to_string(state.run_root.join("provenance.jsonl")).expect("provenance");
        assert!(traces.contains(r#""event":"tool.refused""#));
        assert!(provenance.contains(r#""event":"tool.refused""#));
        assert!(traces.contains("blocked by test"));
        assert_eq!(
            read_seams_json(&state)["kinds"]["policy"]["source"].as_str(),
            Some("external")
        );
    }

    #[tokio::test]
    async fn no_seams_flag_forces_builtin_for_all_kinds() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "no seams forces builtin");
        let config_path = write_seams_config(
            &paths,
            r#"
[seams.policy]
command = ["/bin/sh", "-c", "cat >/dev/null; echo '{\"decision\":\"deny\",\"reason\":\"should be ignored\"}'"]
timeout_ms = 1000

[seams.hooks]
command = ["/bin/sh", "-c", "exit 99"]
timeout_ms = 1000

[seams.event_sink]
command = ["/bin/sh", "-c", "exit 99"]
timeout_ms = 1000
"#,
        );

        let _ = run_smoke_turn(&mut state, &paths, Some(config_path), true).await;

        assert!(state.working_dir.join("Cargo.toml").exists());
        let audit = read_seams_json(&state);
        assert_eq!(audit["no_seams"].as_bool(), Some(true));
        for kind in ["policy", "catalog", "hooks", "event_sink"] {
            assert_eq!(audit["kinds"][kind]["source"].as_str(), Some("builtin"));
        }
    }

    #[test]
    fn seam_failure_renders_error_footer() {
        let message = policy_seam_refusal_message("run123", "bash", "blocked by test");

        assert!(message.contains("seam 'policy' denied bash: blocked by test"));
        assert!(
            message.contains(
                "try: deadreckon show run123 to review, adjust the policy worker, or re-run with --no-seams"
            )
        );
    }

    #[tokio::test]
    async fn policy_seam_allow_proceeds() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "policy seam allow");
        let config_path = write_seams_config(
            &paths,
            r#"
[seams.policy]
command = ["/bin/sh", "-c", "cat >/dev/null; echo '{\"decision\":\"allow\"}'"]
timeout_ms = 1000
"#,
        );

        let _ = run_smoke_turn(&mut state, &paths, Some(config_path), false).await;

        assert!(state.working_dir.join("Cargo.toml").exists());
        let traces = std::fs::read_to_string(state.run_root.join("traces.jsonl")).expect("traces");
        assert!(!traces.contains(r#""event":"tool.refused""#));
        assert_eq!(
            read_seams_json(&state)["kinds"]["policy"]["source"].as_str(),
            Some("external")
        );
    }

    #[tokio::test]
    async fn policy_seam_timeout_denies_fail_closed() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "policy seam timeout");
        let config_path = write_seams_config(
            &paths,
            r#"
[seams.policy]
command = ["/bin/sh", "-c", "sleep 2"]
timeout_ms = 10
"#,
        );

        let _ = run_smoke_turn(&mut state, &paths, Some(config_path), false).await;

        assert!(!state.working_dir.join("Cargo.toml").exists());
        let traces = std::fs::read_to_string(state.run_root.join("traces.jsonl")).expect("traces");
        assert!(traces.contains(r#""event":"tool.refused""#));
        assert!(traces.contains("failed closed: timeout"));
    }

    #[tokio::test]
    async fn policy_seam_cannot_widen_sandbox_floor() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, state) = create_smoke_run(&temp, "policy seam floor");
        let config_path = write_seams_config(
            &paths,
            r#"
[seams.policy]
command = ["/bin/sh", "-c", "cat >/dev/null; echo '{\"decision\":\"allow\"}'"]
timeout_ms = 1000
"#,
        );
        let seams = read_seams_config(&config_path, false).expect("seams");
        let seam_ctx = SeamRunCtx {
            run_root: state.run_root.clone(),
            working_dir: state.working_dir.clone(),
            sandbox_backend: SandboxBackend::None,
        };

        let seam_refusal = policy_seam_refusal(
            &seams,
            &seam_ctx,
            &state.run_id,
            "write_file",
            "../outside.txt",
            &state.working_dir,
        )
        .await;
        assert!(seam_refusal.is_none());

        ensure_sandbox_toml(&state).expect("sandbox.toml");
        let policy =
            load_tool_policy_from_sandbox_toml(&state, "write_file").expect("write policy");
        let err =
            safe_working_path_with_policy(&state.working_dir, Path::new("../outside.txt"), &policy)
                .expect_err("sandbox floor blocks parent path");
        assert!(err.to_string().contains("unsafe write path"));
    }

    #[tokio::test]
    async fn unconfigured_policy_seam_is_identical_to_today() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "policy seam unconfigured");

        let _ = run_smoke_turn(&mut state, &paths, None, false).await;

        assert!(state.working_dir.join("Cargo.toml").exists());
        let traces = std::fs::read_to_string(state.run_root.join("traces.jsonl")).expect("traces");
        assert!(!traces.contains(r#""event":"tool.refused""#));
        assert_eq!(
            read_seams_json(&state)["kinds"]["policy"]["source"].as_str(),
            Some("builtin")
        );
    }

    #[tokio::test]
    async fn hook_seam_receives_started_and_result_events() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "hook seam capture");
        let capture = temp.path().join("hook-events.jsonl");
        let config_path = write_seams_config(
            &paths,
            &format!(
                r#"
[seams.hooks]
command = ["/bin/sh", "-c", "cat >> {}; printf '{{\"ok\":true}}\n'"]
timeout_ms = 1000
"#,
                sh_quote(&capture)
            ),
        );

        let _ = run_smoke_turn(&mut state, &paths, Some(config_path), false).await;

        let captured = std::fs::read_to_string(capture).expect("hook capture");
        assert!(captured.contains(r#""kind":"tool_call_started""#));
        assert!(captured.contains(r#""kind":"tool_call_result""#));
        assert!(captured.contains(r#""tool_name":"bash""#));
        assert!(state.working_dir.join("Cargo.toml").exists());
    }

    #[tokio::test]
    async fn hook_seam_failure_is_non_fatal() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "hook seam failure");
        let config_path = write_seams_config(
            &paths,
            r#"
[seams.hooks]
command = ["/bin/sh", "-c", "cat >/dev/null; exit 42"]
timeout_ms = 1000
"#,
        );

        let _ = run_smoke_turn(&mut state, &paths, Some(config_path), false).await;

        assert!(state.working_dir.join("Cargo.toml").exists());
        let traces = std::fs::read_to_string(state.run_root.join("traces.jsonl")).expect("traces");
        assert!(!traces.contains(r#""event":"tool.refused""#));
    }

    #[tokio::test]
    async fn hook_seam_cannot_alter_dispatch_decision() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "hook seam cannot deny");
        let config_path = write_seams_config(
            &paths,
            r#"
[seams.hooks]
command = ["/bin/sh", "-c", "cat >/dev/null; printf '{\"decision\":\"deny\",\"reason\":\"ignored\"}\n'"]
timeout_ms = 1000
"#,
        );

        let _ = run_smoke_turn(&mut state, &paths, Some(config_path), false).await;

        assert!(state.working_dir.join("Cargo.toml").exists());
        let traces = std::fs::read_to_string(state.run_root.join("traces.jsonl")).expect("traces");
        assert!(!traces.contains(r#""event":"tool.refused""#));
    }

    #[tokio::test]
    async fn event_sink_receives_mirrored_events() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "event sink mirror");
        let capture = temp.path().join("event-sink.jsonl");
        let config_path = write_seams_config(
            &paths,
            &format!(
                r#"
[seams.event_sink]
command = ["/bin/sh", "-c", "cat >> {}; printf '{{\"ok\":true}}\n'"]
timeout_ms = 1000
"#,
                sh_quote(&capture)
            ),
        );

        let _ = run_smoke_turn(&mut state, &paths, Some(config_path), false).await;

        let captured = read_until_contains(&capture, r#""kind":"tool_call_result""#).await;
        assert!(captured.contains(r#""run_id":"#));
        assert!(captured.contains(r#""kind":"turn_started""#));
        assert!(captured.contains(r#""kind":"tool_call_started""#));
    }

    #[tokio::test]
    async fn event_sink_failure_keeps_events_jsonl_complete() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "event sink failure");
        let config_path = write_seams_config(
            &paths,
            r#"
[seams.event_sink]
command = ["/bin/sh", "-c", "cat >/dev/null; exit 42"]
timeout_ms = 1000
"#,
        );

        let _ = run_smoke_turn(&mut state, &paths, Some(config_path), false).await;

        let events = std::fs::read_to_string(state.run_root.join("events.jsonl")).expect("events");
        assert!(events.contains(r#""kind":"turn_started""#));
        assert!(events.contains(r#""kind":"tool_call_started""#));
        assert!(events.contains(r#""kind":"tool_call_result""#));
    }

    #[tokio::test]
    async fn attach_feed_unchanged_with_event_sink() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "event sink attach");
        let capture = temp.path().join("event-sink-attach.jsonl");
        let config_path = write_seams_config(
            &paths,
            &format!(
                r#"
[seams.event_sink]
command = ["/bin/sh", "-c", "cat >> {}; printf '{{\"ok\":true}}\n'"]
timeout_ms = 1000
"#,
                sh_quote(&capture)
            ),
        );

        let _ = run_smoke_turn(&mut state, &paths, Some(config_path), false).await;

        let events = std::fs::read_to_string(state.run_root.join("events.jsonl")).expect("events");
        assert!(events.contains(r#""kind":"tool_call_started""#));
        assert!(events.contains(r#""kind":"tool_call_result""#));
        assert!(
            read_until_contains(&capture, r#""kind":"tool_call_result""#)
                .await
                .contains(r#""event":{"kind":"tool_call_result""#)
        );
    }

    #[tokio::test]
    async fn unconfigured_event_sink_is_identical_to_today() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "event sink unconfigured");

        let _ = run_smoke_turn(&mut state, &paths, None, false).await;

        let events = std::fs::read_to_string(state.run_root.join("events.jsonl")).expect("events");
        assert!(events.contains(r#""kind":"turn_started""#));
        assert!(events.contains(r#""kind":"tool_call_started""#));
        assert_eq!(
            read_seams_json(&state)["kinds"]["event_sink"]["source"].as_str(),
            Some("builtin")
        );
    }

    #[tokio::test]
    async fn full_stack_seam_run_produces_gated_result_and_seams_json() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "full stack seams");
        let hooks_capture = temp.path().join("hooks.jsonl");
        let sink_capture = temp.path().join("sink.jsonl");
        let config_path = write_seams_config(
            &paths,
            &format!(
                r#"
[seams.policy]
command = ["/bin/sh", "-c", "cat >/dev/null; printf '{{\"decision\":\"allow\"}}\n'"]
timeout_ms = 1000

[seams.catalog]
command = ["/bin/sh", "-c", "cat >/dev/null; printf '{{\"models\":[{{\"id\":\"local-scripted-smoke\",\"context_window\":4000}}]}}\n'"]
timeout_ms = 1000

[seams.hooks]
command = ["/bin/sh", "-c", "cat >> {}; printf '{{\"ok\":true}}\n'"]
timeout_ms = 1000

[seams.event_sink]
command = ["/bin/sh", "-c", "cat >> {}; printf '{{\"ok\":true}}\n'"]
timeout_ms = 1000

[compaction]
fraction = 0.5
keep_recent_turns = 2
fallback_context_window = 4000
"#,
                sh_quote(&hooks_capture),
                sh_quote(&sink_capture)
            ),
        );

        let _ = run_smoke_turn(&mut state, &paths, Some(config_path), false).await;

        assert!(state.working_dir.join("Cargo.toml").exists());
        let audit = read_seams_json(&state);
        for kind in ["policy", "catalog", "hooks", "event_sink"] {
            assert_eq!(audit["kinds"][kind]["source"].as_str(), Some("external"));
        }
        let hooks = read_until_contains(&hooks_capture, r#""kind":"tool_call_result""#).await;
        assert!(hooks.contains(r#""tool_name":"bash""#));
        let sink = read_until_contains(&sink_capture, r#""kind":"tool_call_result""#).await;
        assert!(sink.contains(r#""event":{"kind":"tool_call_result""#));

        run_acceptance_gate_and_write_marker(&state.run_root, &state.run_id, &state.working_dir)
            .expect("gate marker");
        let marker = validate_acceptance_marker(&state).expect("validated marker");
        assert_eq!(marker.status, "pass");
        assert_eq!(marker.produced_by, "dr-gate");
    }

    #[tokio::test]
    async fn direct_api_history_compacts_to_jsonl_with_fallback_window() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let config_path = write_seams_config(
            &paths,
            r#"
default_provider = "openai-compatible"

[compaction]
fraction = 0.5
keep_recent_turns = 2
fallback_context_window = 80
"#,
        );
        let cwd = temp.path().join("cwd");
        std::fs::create_dir_all(&cwd).expect("cwd");
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "keep compacted direct api prompt".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: Some("openai-compatible".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "checks:\n  - kind: file_exists\n    path: README.md\n",
        )
        .expect("acceptance");
        let history = (0..8)
            .map(|idx| format!("old-turn-{idx}: {}", "x".repeat(180)))
            .collect::<Vec<_>>();
        save_history(&state, &history).expect("history");
        let router = ProviderRouter::from_config_path(&config_path, Some("openai-compatible"))
            .expect("router");

        let err = run_turn_loop(
            &mut state,
            &router,
            RunLoopConfig {
                provider: Some("openai-compatible".to_string()),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                sandbox_backend: SandboxBackend::None,
                no_seams: false,
                max_turns: 1,
                from_turn: None,
                event_sender: None,
                cancellation_token: None,
                docs: RunLoopDocsConfig {
                    home: paths.home().to_path_buf(),
                    config_path: Some(config_path),
                    doc_provider: None,
                    doc_provider_source: None,
                    doc_subskills: Vec::new(),
                    token_budget: 0,
                    budget_cap_usd: None,
                    doc_skill: "run-narrator".to_string(),
                    no_docs: true,
                },
            },
        )
        .await
        .expect_err("missing credential after compaction");

        assert!(err.to_string().contains("missing credential"));
        let compaction =
            std::fs::read_to_string(state.run_root.join("compaction.jsonl")).expect("compaction");
        assert!(compaction.contains(r#""context_window":80"#));
        assert!(compaction.contains(r#""context_window_source":"fallback""#));
        let full_history =
            std::fs::read_to_string(state.run_root.join("history.json")).expect("history");
        assert!(full_history.contains("old-turn-0"));
    }

    #[tokio::test]
    async fn resume_re_resolves_seams_and_keeps_audit() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_direct_api_run(&temp, "resume re-resolves seams");
        let config_path = write_seams_config(
            &paths,
            r#"
default_provider = "openai-compatible"

[seams.policy]
command = ["/bin/sh", "-c", "cat >/dev/null; printf '{\"decision\":\"allow\"}\n'"]
timeout_ms = 1000
"#,
        );

        let err =
            run_direct_api_until_missing_credential(&mut state, &paths, config_path.clone()).await;
        assert!(err.contains("missing credential"));
        let first_audit = read_seams_json(&state);
        assert_eq!(
            first_audit["kinds"]["policy"]["source"].as_str(),
            Some("external")
        );

        let config_path = write_seams_config(
            &paths,
            r#"
default_provider = "openai-compatible"

[seams.hooks]
command = ["/bin/sh", "-c", "cat >/dev/null; printf '{\"ok\":true}\n'"]
timeout_ms = 1000
"#,
        );
        let err = run_direct_api_until_missing_credential(&mut state, &paths, config_path).await;
        assert!(err.contains("missing credential"));
        let second_audit = read_seams_json(&state);

        assert_eq!(
            second_audit["kinds"]["policy"]["source"].as_str(),
            Some("builtin")
        );
        assert_eq!(
            second_audit["kinds"]["hooks"]["source"].as_str(),
            Some("external")
        );
    }

    #[tokio::test]
    async fn resume_produces_identical_compaction() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_direct_api_run(&temp, "resume compaction determinism");
        let config_path = write_seams_config(
            &paths,
            r#"
default_provider = "openai-compatible"

[compaction]
fraction = 0.5
keep_recent_turns = 2
fallback_context_window = 80
"#,
        );
        let history = (0..8)
            .map(|idx| format!("old-turn-{idx}: {}", "x".repeat(180)))
            .collect::<Vec<_>>();
        save_history(&state, &history).expect("history");

        let err =
            run_direct_api_until_missing_credential(&mut state, &paths, config_path.clone()).await;
        assert!(err.contains("missing credential"));
        let err =
            run_direct_api_until_missing_credential(&mut state, &paths, config_path.clone()).await;
        assert!(err.contains("missing credential"));
        let lines = read_compaction_lines(&state);

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], lines[1]);
    }

    #[tokio::test]
    async fn seams_json_and_compaction_jsonl_survive_resume() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_direct_api_run(&temp, "resume audit files");
        let config_path = write_seams_config(
            &paths,
            r#"
default_provider = "openai-compatible"

[seams.event_sink]
command = ["/bin/sh", "-c", "cat >/dev/null; printf '{\"ok\":true}\n'"]
timeout_ms = 1000

[compaction]
fraction = 0.5
keep_recent_turns = 2
fallback_context_window = 80
"#,
        );
        let history = (0..8)
            .map(|idx| format!("resume-turn-{idx}: {}", "x".repeat(180)))
            .collect::<Vec<_>>();
        save_history(&state, &history).expect("history");

        let _ =
            run_direct_api_until_missing_credential(&mut state, &paths, config_path.clone()).await;
        let first_compaction = read_compaction_lines(&state);
        assert_eq!(first_compaction.len(), 1);
        assert_eq!(
            read_seams_json(&state)["kinds"]["event_sink"]["source"].as_str(),
            Some("external")
        );

        let _ = run_direct_api_until_missing_credential(&mut state, &paths, config_path).await;
        let second_compaction = read_compaction_lines(&state);

        assert!(state.run_root.join("seams.json").exists());
        assert_eq!(second_compaction.len(), 2);
        assert_eq!(second_compaction[0], first_compaction[0]);
        assert_eq!(
            read_seams_json(&state)["kinds"]["event_sink"]["source"].as_str(),
            Some("external")
        );
    }

    #[test]
    fn cli_provider_path_is_never_compacted() {
        assert!(!is_direct_api_provider_kind(&ProviderKind::CliCodex));
        assert!(!is_direct_api_provider_kind(&ProviderKind::CliClaudeCode));
        assert!(is_direct_api_provider_kind(&ProviderKind::OpenAi));
    }

    #[test]
    fn run_prompt_names_implement_spec_and_implementation_notes_contract() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "ship feature".to_string(),
                cwd: temp.path().to_path_buf(),
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
        let prompt = build_prompt(&state, &[]);
        assert!(prompt.contains("Implement the SPEC"), "{prompt}");
        assert!(prompt.contains("implementation-notes.html"), "{prompt}");
        assert!(prompt.contains("RUN-DECISIONS.md"), "{prompt}");
        assert!(
            prompt
                .trim_end()
                .ends_with("Return exactly one JSON object with action bash, write_file, or done.")
        );
    }

    #[test]
    fn cli_subagent_prompt_includes_same_notes_contract() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "ship cli feature".to_string(),
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
        .expect("run");
        let prompt = build_cli_subagent_prompt(&state, &[]);
        assert!(prompt.contains("Implement the SPEC"), "{prompt}");
        assert!(prompt.contains("implementation-notes.html"), "{prompt}");
        assert!(prompt.contains("RUN-DECISIONS.md"), "{prompt}");
    }

    #[test]
    fn done_without_current_implementation_notes_is_rejected() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "ship stale notes check".to_string(),
                cwd: temp.path().to_path_buf(),
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
        let source = state.working_dir.join("src/lib.rs");
        std::fs::create_dir_all(source.parent().expect("parent")).expect("src");
        std::fs::write(&source, "pub fn value() -> u8 { 1 }\n").expect("source");
        append_turn_doc(
            &state,
            TurnDocInput {
                turn: 1,
                tool_kind: "write_file".to_string(),
                latency_ms: None,
                files: vec![source],
                outcome: "code".to_string(),
                response_text: "code".to_string(),
                tool_stdout: None,
                tool_stderr: None,
            },
        )
        .expect("turn doc");
        let mut history = Vec::new();
        let ready = implementation_notes_ready_or_request_followup(&state, None, 2, &mut history)
            .expect("notes check");
        assert!(!ready);
        assert!(history[0].contains("implementation notes are required"));
    }

    #[test]
    fn current_implementation_notes_update_run_decisions() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "ship current notes check".to_string(),
                cwd: temp.path().to_path_buf(),
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
        let source = state.working_dir.join("src/lib.rs");
        std::fs::create_dir_all(source.parent().expect("parent")).expect("src");
        std::fs::write(&source, "pub fn value() -> u8 { 1 }\n").expect("source");
        append_turn_doc(
            &state,
            TurnDocInput {
                turn: 1,
                tool_kind: "write_file".to_string(),
                latency_ms: None,
                files: vec![source],
                outcome: "code".to_string(),
                response_text: "code".to_string(),
                tool_stdout: None,
                tool_stderr: None,
            },
        )
        .expect("source turn");
        let notes = implementation_notes_path(&state.working_dir);
        std::fs::write(
            &notes,
            r#"<!doctype html><html lang="en"><body>
<section id="design-decisions"><h2>Design decisions</h2><p>Project decisions converge into RUN-DECISIONS.</p></section>
<section id="deviations"><h2>Deviations</h2><p>None.</p></section>
<section id="tradeoffs"><h2>Tradeoffs</h2><p>HTML remains the live working copy.</p></section>
<section id="open-questions"><h2>Open questions</h2><p>None.</p></section>
</body></html>"#,
        )
        .expect("notes");
        append_turn_doc(
            &state,
            TurnDocInput {
                turn: 2,
                tool_kind: "write_file".to_string(),
                latency_ms: None,
                files: vec![notes],
                outcome: "notes".to_string(),
                response_text: "notes".to_string(),
                tool_stdout: None,
                tool_stderr: None,
            },
        )
        .expect("notes turn");
        let mut history = Vec::new();
        let ready = implementation_notes_ready_or_request_followup(&state, None, 3, &mut history)
            .expect("notes check");
        assert!(ready);
        let decisions =
            std::fs::read_to_string(deadreckon_core::decisions_path(&state.working_dir))
                .expect("decisions");
        assert!(decisions.contains("Project decisions converge into RUN-DECISIONS"));
    }

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
                    no_seams: false,
                    max_turns: 1,
                    from_turn: None,
                    event_sender: Some(bus.sender()),
                    cancellation_token: Some(cancel_for_loop),
                    docs: RunLoopDocsConfig {
                        home: paths.home().to_path_buf(),
                        config_path: None,
                        doc_provider: None,
                        doc_provider_source: None,
                        doc_subskills: Vec::new(),
                        token_budget: 0,
                        budget_cap_usd: None,
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
                    no_seams: false,
                    max_turns: 1,
                    from_turn: None,
                    event_sender: Some(bus.sender()),
                    cancellation_token: None,
                    docs: RunLoopDocsConfig {
                        home: paths.home().to_path_buf(),
                        config_path: None,
                        doc_provider: None,
                        doc_provider_source: None,
                        doc_subskills: Vec::new(),
                        token_budget: 0,
                        budget_cap_usd: None,
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

    #[tokio::test]
    async fn turn_end_docs_checkpoint_is_explicit_event() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "docs checkpoint".to_string(),
                cwd: temp.path().to_path_buf(),
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
        let router = ProviderRouter::smoke();
        run_turn_loop(
            &mut state,
            &router,
            RunLoopConfig {
                provider: Some("smoke".to_string()),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                sandbox_backend: SandboxBackend::None,
                no_seams: false,
                max_turns: 1,
                from_turn: None,
                event_sender: Some(bus.sender()),
                cancellation_token: None,
                docs: RunLoopDocsConfig {
                    home: paths.home().to_path_buf(),
                    config_path: None,
                    doc_provider: None,
                    doc_provider_source: None,
                    doc_subskills: Vec::new(),
                    token_budget: 0,
                    budget_cap_usd: None,
                    doc_skill: "run-narrator".to_string(),
                    no_docs: true,
                },
            },
        )
        .await
        .expect("loop");
        let events = std::fs::read_to_string(state.run_root.join("events.jsonl")).expect("events");
        assert!(events.contains("\"kind\":\"docs_checkpoint\""));
        assert!(events.contains("\"status\":\"turn-end\""));
    }

    #[tokio::test]
    async fn cli_provider_run_writes_flight_manifest_events_and_checkpoint() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        std::fs::create_dir_all(paths.home().join("providers.d")).expect("providers");
        let provider_logs = temp.path().join("provider-logs");
        std::fs::create_dir_all(&provider_logs).expect("provider logs");
        let script = temp.path().join("flight-provider.sh");
        std::fs::write(
            &script,
            format!(
                r#"mkdir -p src
printf 'pub fn value() -> u8 {{ 42 }}\n' > src/lib.rs
mkdir -p "{}"
printf '%s\n' '{{"type":"tool_call","tool_name":"write_file","path":"src/lib.rs","message":"wrote source"}}' > "{}/session.jsonl"
printf 'done\n'
"#,
                provider_logs.display(),
                provider_logs.display()
            ),
        )
        .expect("script");
        std::fs::write(
            paths.home().join("providers.d/test-flight.toml"),
            format!(
                r#"
id = "cli:test-flight"
display_name = "Test Flight"
kind = "cli"
default_binary = "/bin/sh"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["{}", "{{prompt}}"]

[ingest]
default_dirs = ["{}"]
schema = "test-flight"
file_glob = "*.jsonl"
storage = "jsonl"
"#,
                script.display(),
                provider_logs.display()
            ),
        )
        .expect("descriptor");
        let config_path = paths.home().join("config.toml");
        std::fs::write(&config_path, "default_provider = \"cli:test-flight\"\n").expect("config");
        let cwd = temp.path().join("cwd");
        std::fs::create_dir_all(&cwd).expect("cwd");
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "record cli provider".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        let router = ProviderRouter::from_config_path(&config_path, None).expect("router");

        let _ = run_turn_loop(
            &mut state,
            &router,
            RunLoopConfig {
                provider: None,
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                sandbox_backend: SandboxBackend::None,
                no_seams: false,
                max_turns: 1,
                from_turn: None,
                event_sender: None,
                cancellation_token: None,
                docs: RunLoopDocsConfig {
                    home: paths.home().to_path_buf(),
                    config_path: Some(config_path),
                    doc_provider: None,
                    doc_provider_source: None,
                    doc_subskills: Vec::new(),
                    token_budget: 0,
                    budget_cap_usd: None,
                    doc_skill: "run-narrator".to_string(),
                    no_docs: true,
                },
            },
        )
        .await
        .expect("loop");

        let manifest = read_flight_manifest(&state)
            .expect("manifest")
            .expect("manifest exists");
        assert_eq!(manifest.sessions.len(), 1);
        assert_eq!(manifest.sessions[0].provider, "cli:test-flight");
        assert_eq!(manifest.sessions[0].status, FlightSessionStatus::Completed);
        let events = read_flight_events(&state).expect("flight events");
        assert!(events.iter().any(|event| {
            event.kind == FlightEventKind::Tool && event.files == vec![PathBuf::from("src/lib.rs")]
        }));
        assert!(
            events
                .iter()
                .any(|event| event.kind == FlightEventKind::Checkpoint)
        );
        let checkpoints = list_checkpoint_manifests(&state).expect("checkpoints");
        assert_eq!(checkpoints.len(), 1);
        assert!(
            checkpoints[0]
                .files
                .iter()
                .any(|change| change.path == Path::new("src/lib.rs"))
        );
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

    #[test]
    fn per_tool_policy_refuses_write_outside_working_dir() {
        let root = PathBuf::from("/tmp/deadreckon-safe-root");
        let absolute = safe_working_path(&root, Path::new("/Users/victim/.ssh/id_rsa"))
            .expect_err("absolute path refused");
        let parent =
            safe_working_path(&root, Path::new("../outside.txt")).expect_err("parent path refused");

        assert!(absolute.to_string().contains("unsafe write path"));
        assert!(absolute.to_string().contains("try:"));
        assert!(parent.to_string().contains("unsafe write path"));
        assert!(parent.to_string().contains("try:"));
    }

    #[test]
    fn sandbox_toml_gates_write_file_policy() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "sandbox toml".to_string(),
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
        ensure_sandbox_toml(&state).expect("sandbox.toml");
        let policy =
            load_tool_policy_from_sandbox_toml(&state, "write_file").expect("write policy");
        let ok = safe_working_path_with_policy(&state.working_dir, Path::new("ok.txt"), &policy)
            .expect("allowed");
        assert!(ok.starts_with(&state.working_dir));

        std::fs::write(
            state.run_root.join("sandbox.toml"),
            r#"
version = 1

[tools.write_file]
read = []
write = []
network = []
"#,
        )
        .expect("sandbox override");
        let policy =
            load_tool_policy_from_sandbox_toml(&state, "write_file").expect("write policy");
        let err =
            safe_working_path_with_policy(&state.working_dir, Path::new("blocked.txt"), &policy)
                .expect_err("blocked by sandbox.toml");
        assert!(err.to_string().contains("sandbox.toml"));
        assert!(err.to_string().contains("try:"));
    }

    #[test]
    fn bash_ssh_policy_refusal_contains_try_and_records_provenance() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "bash ssh refusal".to_string(),
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
        let policy = load_tool_policy_from_sandbox_toml(&state, "bash").expect("bash policy");
        let reason = bash_policy_refusal(&state, "cat ~/.ssh/id_rsa", &policy).expect("refused");
        assert!(reason.contains("try:"));
        append_tool_refusal(
            &state,
            1,
            "bash-refused",
            "bash",
            "mock-agent",
            &reason,
            None,
        )
        .expect("refusal");
        let provenance =
            std::fs::read_to_string(state.run_root.join("provenance.jsonl")).expect("provenance");
        assert!(provenance.contains(r#""event":"tool.refused""#));
        assert!(provenance.contains("bash denied by sandbox.toml"));
        assert!(provenance.contains("try:"));
    }

    #[test]
    fn tool_refusal_records_provenance_event() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "refusal".to_string(),
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

        append_tool_refusal(
            &state,
            1,
            "tool-refused",
            "write_file",
            "mock-agent",
            "unsafe write path ../secret",
            None,
        )
        .expect("refusal");

        let provenance =
            std::fs::read_to_string(state.run_root.join("provenance.jsonl")).expect("provenance");
        let traces = std::fs::read_to_string(state.run_root.join("traces.jsonl")).expect("traces");
        assert!(provenance.contains(r#""event":"tool.refused""#));
        assert!(provenance.contains("unsafe write path"));
        assert!(traces.contains(r#""event":"tool.refused""#));
    }

    #[test]
    fn provider_prompt_includes_run_acceptance_spec() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "use acceptance".to_string(),
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
            state.run_root.join("acceptance.yaml"),
            "checks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("acceptance");

        let prompt = build_cli_subagent_prompt(&state, &[]);

        assert!(prompt.contains("Acceptance criteria:"));
        assert!(prompt.contains("acceptance.yaml:"));
        assert!(prompt.contains("README.md"));
    }
}
