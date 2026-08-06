use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use deadreckon_protocol::{
    GateEvaluatorIdentity, JobAuthority, JobGateNetworkAccess, JobSchemaVersion, RunEvent,
    RunEventKind, SandboxBoundaryObservation, SandboxBoundaryObservationIssuer, SpendRecord,
    TraceRecord,
};
use deadreckon_providers::{
    ProviderCleanup, ProviderKind, ProviderPhaseDeadline, ProviderPhaseOutcome, ProviderRequest,
    ProviderResponse, ProviderRouter, complete_provider_phase,
};
use deadreckon_sandbox::{
    DockerExecution, DockerImage, DockerPlatform, GuardedLaunchSpec, ProtectedPathPolicy,
    SandboxBackend, SandboxError, SandboxRunOutput, SandboxSpec, ToolSandboxPolicy,
    WorkspaceAccess, inspect_docker_image, reconcile_docker_execution_record, resolve_backend,
    run as run_sandbox, write_docker_execution_record,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::{broadcast, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::compaction::{append_compaction_record, compact_history, read_compaction_config};
use crate::error::IoContext;
use crate::flight::{ProviderFlightRecorder, ProviderFlightRecorderHandle};
use crate::polish::{PolishConfig, polish_run_docs};
use deadreckon_core::artifact_policy::{
    WorkspacePathClass, classify_workspace_path, evidence_only_roots, is_deliverable_workspace_path,
};
use deadreckon_core::artifacts::{
    ProvenanceRecord, append_provenance, append_spend, append_trace,
    copy_recoverable_tree_with_policy, copy_tree, inventory_files,
    inventory_recoverable_files_for_state, snapshot_working,
};
use deadreckon_core::cancel::{cancel_marker_path_for_run_root, cancel_marker_present};
use deadreckon_core::codebase::{
    CodebaseMode, CodebaseRecord, read_run_codebase_record, read_trusted_codebase_record,
    write_trusted_codebase_record,
};
use deadreckon_core::docs::{
    IMPLEMENTATION_NOTES_HTML, ImplementationNotesStatus, TurnDocInput, append_turn_doc,
    check_implementation_notes_current, incremental_path, rewrite_templated_docs,
};
use deadreckon_core::error::{DeadreckonError, Result};
use deadreckon_core::events::{emit_event, event_preview, tool_args_json};
use deadreckon_core::flight::FlightSessionStatus;
use deadreckon_core::gate::{acceptance_spec_path_for_run_root, validate_acceptance_marker};
use deadreckon_core::git::{
    BoundedGitOutcome, GitCommandBoundary, GitCommandDeadline, run_git, run_git_bounded,
    run_git_with_input, run_git_with_input_bounded,
};
use deadreckon_core::paths::DeadreckonPaths;
use deadreckon_core::promotion::promote_completed_run_bounded;
use deadreckon_core::state::{
    PhaseId, PhaseStatus, PipelineState, RunStatus, append_json_line, save_state,
};

use crate::seam::{
    SeamKind, SeamOutcome, SeamPhaseOutcome, SeamRunCtx, SeamsConfig, dispatch_seam_phase,
    lost_containment_error, read_seams_config, write_seams_audit,
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
    /// Authenticated outer work boundary for a guarded durable launch. When
    /// absent, compatibility Runs derive their historical cumulative wall-cap
    /// boundary from `max_wall_seconds`.
    pub work_boundary: Option<RunWorkBoundary>,
    pub docs: RunLoopDocsConfig,
    /// Live-narration settings. `None` disables narration (every existing
    /// constructor leaves it `None`); the CLI sets `Some(..)` to spawn the
    /// in-process narrator sidecar that subscribes to `event_sender`.
    pub narrate: Option<NarratorConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunWorkExpiry {
    WallCap,
    Deadline,
}

impl RunWorkExpiry {
    fn reached_label(self) -> &'static str {
        match self {
            Self::WallCap => "wall-clock cap reached",
            Self::Deadline => "calendar deadline reached",
        }
    }
}

/// One monotonic cutoff authenticated by the guarded caller before work
/// begins. Every nested phase reuses this exact instant.
#[derive(Debug, Clone, Copy)]
pub struct RunWorkBoundary {
    pub work_expires_at: tokio::time::Instant,
    pub expiry: RunWorkExpiry,
}

impl RunWorkBoundary {
    pub const fn new(work_expires_at: tokio::time::Instant, expiry: RunWorkExpiry) -> Self {
        Self {
            work_expires_at,
            expiry,
        }
    }
}

/// Durable outer launch that owns one strict deterministic-gate evaluation.
///
/// A gate evaluator deliberately leaves the worker's process group so the
/// supervisor can recover it independently. The attempt and outer launch ID
/// bind that escaped process back to the append-only Job history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateLaunchOwner {
    attempt: u32,
    outer_launch_id: String,
}

impl GateLaunchOwner {
    pub fn new(attempt: u32, outer_launch_id: impl Into<String>) -> Result<Self> {
        let outer_launch_id = outer_launch_id.into();
        if attempt == 0 || Uuid::parse_str(&outer_launch_id).is_err() {
            return Err(DeadreckonError::InvalidInput(
                "strict gate requires a valid durable attempt and outer launch identity"
                    .to_string(),
            ));
        }
        Ok(Self {
            attempt,
            outer_launch_id,
        })
    }

    pub const fn attempt(&self) -> u32 {
        self.attempt
    }

    pub fn outer_launch_id(&self) -> &str {
        &self.outer_launch_id
    }
}

/// Settings for the live narrator sidecar. Defaults mirror the
/// `[defaults] narrate_*` config knobs documented in the Live Narrator rider.
#[derive(Debug, Clone, PartialEq)]
pub struct NarratorConfig {
    /// Foreground calm block on a TTY (default on for run/orchestrate/campaign).
    pub foreground: bool,
    /// Headless append-only beats to stderr (the `--narrate` opt-in).
    pub headless_append: bool,
    /// Pin a narrator model id; `None` uses the subscription-first preference order.
    pub model_override: Option<String>,
    /// Per-run narrator spend cap (subscription backends record $0).
    pub budget_usd: f64,
    /// Max lines in the calm foreground block.
    pub lines: usize,
    /// Minimum seconds between model beats.
    pub min_gap_seconds: u64,
    /// Force a beat after this many turns even under the gap.
    pub turn_burst: u32,
    /// A long single turn gets a beat after this many quiet seconds.
    pub quiet_seconds: u64,
    /// Per-run beat cap.
    pub max_beats: u32,
}

impl Default for NarratorConfig {
    fn default() -> Self {
        Self {
            foreground: true,
            headless_append: false,
            model_override: None,
            budget_usd: 0.50,
            lines: 4,
            min_gap_seconds: 30,
            turn_burst: 8,
            quiet_seconds: 45,
            max_beats: 200,
        }
    }
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

/// Attempt-local monotonic work clock backed by the last durable cumulative
/// value. Supervisor downtime is not active work: a recovered attempt starts
/// a fresh monotonic interval from the wall time persisted by its predecessor.
#[derive(Debug)]
struct RunWorkClock {
    baseline_seconds: f64,
    started: Instant,
    work_boundary: Option<RunWorkBoundary>,
    boundary_cap_seconds: Option<f64>,
}

impl RunWorkClock {
    #[cfg(test)]
    fn new(state: &PipelineState) -> Result<Self> {
        Self::with_boundary(state, None)
    }

    fn with_boundary(
        state: &PipelineState,
        work_boundary: Option<RunWorkBoundary>,
    ) -> Result<Self> {
        if !state.total_wall_seconds.is_finite() || state.total_wall_seconds < 0.0 {
            return Err(DeadreckonError::InvalidInput(
                "run total_wall_seconds must be finite and non-negative".to_string(),
            ));
        }
        let boundary_cap_seconds = work_boundary.map(|boundary| {
            state.total_wall_seconds
                + boundary
                    .work_expires_at
                    .saturating_duration_since(tokio::time::Instant::now())
                    .as_secs_f64()
        });
        Ok(Self {
            baseline_seconds: state.total_wall_seconds,
            started: Instant::now(),
            work_boundary,
            boundary_cap_seconds,
        })
    }

    fn total_seconds(&self) -> f64 {
        self.baseline_seconds + self.started.elapsed().as_secs_f64()
    }

    fn sync(&self, state: &mut PipelineState) {
        state.total_wall_seconds = state.total_wall_seconds.max(self.total_seconds());
    }

    fn save(&self, state: &mut PipelineState) -> Result<()> {
        self.sync(state);
        save_state(state)
    }

    fn remaining(&self, cap_seconds: Option<f64>) -> Result<Option<Duration>> {
        let Some(cap_seconds) = cap_seconds else {
            return Ok(None);
        };
        if !cap_seconds.is_finite() || cap_seconds < 0.0 {
            return Err(DeadreckonError::InvalidInput(
                "run max_wall_seconds must be finite and non-negative".to_string(),
            ));
        }
        Ok(Some(Duration::from_secs_f64(
            (cap_seconds - self.total_seconds()).max(0.0),
        )))
    }

    fn remaining_seconds(&self, cap_seconds: Option<f64>) -> Result<Option<f64>> {
        if let Some(boundary) = self.work_boundary {
            return Ok(Some(
                boundary
                    .work_expires_at
                    .saturating_duration_since(tokio::time::Instant::now())
                    .as_secs_f64(),
            ));
        }
        self.remaining(cap_seconds)
            .map(|remaining| remaining.map(|duration| duration.as_secs_f64()))
    }

    fn wall_time_cap_seconds(&self, compatibility_cap: Option<f64>) -> Option<f64> {
        self.boundary_cap_seconds.or(compatibility_cap)
    }

    fn provider_phase_deadline(&self, cap_seconds: Option<f64>) -> Result<ProviderPhaseDeadline> {
        let work_expires_at = if let Some(boundary) = self.work_boundary {
            boundary.work_expires_at
        } else if let Some(cap_seconds) = cap_seconds {
            if !cap_seconds.is_finite() || cap_seconds < 0.0 {
                return Err(DeadreckonError::InvalidInput(
                    "run max_wall_seconds must be finite and non-negative".to_string(),
                ));
            }
            let active_budget =
                Duration::from_secs_f64((cap_seconds - self.baseline_seconds).max(0.0));
            let absolute = self.started.checked_add(active_budget).ok_or_else(|| {
                DeadreckonError::InvalidInput(
                    "run max_wall_seconds exceeds the monotonic clock range".to_string(),
                )
            })?;
            tokio::time::Instant::from_std(absolute)
        } else {
            // Compatibility Runs may omit a wall cap. ProviderPhaseDeadline is
            // intentionally non-optional, so use one fixed practical infinity
            // for the entire attempt instead of rebuilding a relative timeout.
            tokio::time::Instant::now() + PROVIDER_UNBOUNDED_WORK_WINDOW
        };
        Ok(ProviderPhaseDeadline::new(
            work_expires_at,
            PROVIDER_CLEANUP_BUDGET,
        ))
    }

    fn expiry(&self) -> RunWorkExpiry {
        self.work_boundary
            .map_or(RunWorkExpiry::WallCap, |boundary| boundary.expiry)
    }
}

/// Persist the authoritative clock after a local phase has returned, before
/// its success or failure is propagated to the caller. This keeps failures in
/// snapshotting, Git post-processing, documentation, verification, and
/// promotion from leaving the durable run clock at the previous boundary.
fn persist_work_boundary<T>(
    state: &mut PipelineState,
    work_clock: &RunWorkClock,
    result: Result<T>,
) -> Result<T> {
    work_clock.save(state)?;
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunLoopOutcome {
    Done,
    PausedAtCap,
    Killed,
    Failed,
}

/// Trusted context for the candidate produced by one fenced parent-repair
/// attempt. The runtime writes this outside the provider-visible workspace
/// immediately before returning `Done`, closing the candidate-ready crash
/// window without giving the worker authority over lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParentRepairCandidateContext {
    pub path: PathBuf,
    pub job_id: String,
    pub round: u32,
    pub attempt: u32,
    pub launch_id: String,
    pub lease_epoch: u64,
    pub intent_sha256: String,
    pub manifest_sha256: String,
    pub feedback: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParentRepairCandidate {
    pub schema_version: u32,
    pub job_id: String,
    pub run_id: String,
    pub round: u32,
    pub attempt: u32,
    pub launch_id: String,
    pub lease_epoch: u64,
    pub intent_sha256: String,
    pub manifest_sha256: String,
    pub result_tree_sha256: String,
    pub turn: u32,
    pub ready_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone)]
enum CompletionMode {
    VerifyAndPromote,
    ParentRepairCandidate(ParentRepairCandidateContext),
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
    /// C-P12: the worker proposes decomposing the goal into independent
    /// pieces. Recording it is non-terminal and the proposal is INERT — it
    /// executes only when an operator accepts it via `deadreckon reshape`.
    Reshape {
        tool_call_id: String,
        #[serde(default)]
        pieces: Vec<ReshapePieceDraft>,
    },
    Done {
        summary: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
struct ReshapePieceDraft {
    goal: String,
    #[serde(default)]
    done_hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SandboxToml {
    version: u32,
    tools: BTreeMap<String, SandboxTomlTool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    run_turn_loop_with_semantic_router(state, router, router, config).await
}

/// Run the mutation loop with independently resolved worker and semantic
/// reviewer routes. Durable launch plans freeze both roles before admission;
/// keeping the routers distinct here prevents a generic worker adapter from
/// being reused for the schema-only completion judgment.
pub async fn run_turn_loop_with_semantic_router(
    state: &mut PipelineState,
    worker_router: &ProviderRouter,
    semantic_router: &ProviderRouter,
    config: RunLoopConfig,
) -> Result<RunLoopOutcome> {
    run_turn_loop_inner(
        state,
        worker_router,
        semantic_router,
        config,
        CompletionMode::VerifyAndPromote,
    )
    .await
}

/// Run the ordinary bounded mutation loop for a composed parent, but stop at
/// a durable candidate boundary. The supervisor—not this repair worker—then
/// revalidates Graph/Campaign lineage, runs both completion keys and seals.
pub async fn run_parent_repair_turn_loop(
    state: &mut PipelineState,
    router: &ProviderRouter,
    config: RunLoopConfig,
    candidate: ParentRepairCandidateContext,
) -> Result<RunLoopOutcome> {
    run_turn_loop_inner(
        state,
        router,
        router,
        config,
        CompletionMode::ParentRepairCandidate(candidate),
    )
    .await
}

async fn run_turn_loop_inner(
    state: &mut PipelineState,
    worker_router: &ProviderRouter,
    semantic_router: &ProviderRouter,
    config: RunLoopConfig,
    completion_mode: CompletionMode,
) -> Result<RunLoopOutcome> {
    let mut config = config;
    let work_clock = RunWorkClock::with_boundary(state, config.work_boundary)?;
    let provider_phase_deadline = work_clock.provider_phase_deadline(config.max_wall_seconds)?;
    // AS-BUILT §9: the harness, not the model, owns the bounded mutation loop
    // and writes state after every turn boundary.
    let mut history = load_or_reconstruct_history(state, config.from_turn, &work_clock)?;
    if let CompletionMode::ParentRepairCandidate(candidate) = &completion_mode
        && !history.iter().any(|entry| entry == &candidate.feedback)
    {
        history.push(candidate.feedback.clone());
        state.failure_reason = Some(candidate.feedback.clone());
        save_history(state, &history)?;
        work_clock.save(state)?;
    }
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
    let run_token = config.cancellation_token.clone().unwrap_or_default();
    let event_sink_forwarder = if seams.command_for(SeamKind::EventSink).is_some() {
        if config.event_sender.is_none() {
            let (sender, _) = broadcast::channel(256);
            config.event_sender = Some(sender);
        }
        config.event_sender.as_ref().map(|sender| {
            spawn_event_sink_forwarder(
                seams.clone(),
                seam_ctx.clone(),
                sender,
                provider_phase_deadline,
                &run_token,
            )
        })
    } else {
        None
    };
    let _cancel_marker_guard = CancelMarkerGuard::spawn(&state.run_root, run_token.clone());
    let run_result = async {
    if should_cancel_run(state, &run_token) {
        state.status = deadreckon_core::state::RunStatus::Killed;
        state.failure_reason = Some("run cancelled before turn loop".to_string());
        work_clock.save(state)?;
        emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Killed)?;
        return Ok(RunLoopOutcome::Killed);
    }
    state.set_phase_status(PhaseId(40), PhaseStatus::Executing)?;
    // A new loop attempt supersedes any provider disposition from the prior
    // process attempt. If this attempt reaches a provider failure it records
    // a fresh typed classification below.
    state.provider_failure = None;
    work_clock.save(state)?;
    if let Some(from_turn) = config.from_turn {
        state.turn = from_turn;
        work_clock.save(state)?;
    }

    for _ in 0..config.max_turns {
        if should_cancel_run(state, &run_token) {
            state.status = deadreckon_core::state::RunStatus::Killed;
            state.failure_reason = Some("run cancelled".to_string());
            work_clock.save(state)?;
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
        let head_result =
            capture_trusted_turn_head_bounded(state, turn, provider_phase_deadline, &run_token);
        if let ControlFlow::Break(outcome) = settle_trusted_git_phase(
            state,
            turn,
            "pre-provider trusted Git capture",
            PhaseId(40),
            config.event_sender.as_ref(),
            &work_clock,
            head_result,
        )? {
            return Ok(outcome);
        }
        let snapshot_result = snapshot_working_bounded(
            state,
            turn.saturating_sub(1),
            provider_phase_deadline,
            &run_token,
        );
        let snapshot_result = persist_work_boundary(state, &work_clock, snapshot_result);
        if let Err(error) = &snapshot_result
            && let Some(outcome) = settle_local_work_boundary(
                state,
                turn,
                &mut history,
                error,
                PhaseId(40),
                "pre-provider workspace snapshot",
                config.event_sender.as_ref(),
                &work_clock,
            )?
        {
            return Ok(outcome);
        }
        snapshot_result?;
        let selected_route = worker_router.selected_route_info();
        let selected_provider = config
            .provider
            .clone()
            .or_else(|| selected_route.as_ref().map(|route| route.name.clone()));
        let mut prompt_history = history.clone();
        if selected_route
            .as_ref()
            .is_some_and(|route| is_direct_api_provider_kind(&route.kind))
        {
            let (context_window, source) = worker_router
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
        let mut request = ProviderRequest {
            prompt,
            max_output_tokens: 2048,
            cwd: Some(state.working_dir.clone()),
            output_path: Some(turn_dir.join(stdout_name)),
            sandbox_backend: Some(config.sandbox_backend),
            workspace_access: deadreckon_sandbox::WorkspaceAccess::ReadWrite,
            pid_file: Some(
                state
                    .run_root
                    .join("child-pids")
                    .join(format!("provider-turn-{turn}.pid")),
            ),
            cancellation_token: Some(turn_token.clone()),
            // Semaphore: the run root holds this run's provider-session.json and
            // any per-turn output-schema file. Non-CLI providers ignore it.
            session_dir: Some(state.run_root.clone()),
            output_schema: None,
            capability_posture: None,
        };
        apply_provider_capability_posture(&mut request, state, &config.docs.home)?;

        let mut flight_recorder: Option<ProviderFlightRecorderHandle> =
            match selected_provider.as_deref() {
                Some(provider) if is_cli_provider_name(provider) => {
                    ProviderFlightRecorder::start(state, provider, &config.docs.home, turn)?
                        .map(|recorder| recorder.spawn(state.clone()))
                }
                _ => None,
            };
        let started = Instant::now();
        // One absolute cumulative work cutoff is shared by every provider
        // attempt in this run-loop process. Cleanup has its own bounded window
        // and never grants a retry more provider work.
        request.cancellation_token = Some(turn_token.clone());
        let mut completion =
            complete_provider_phase(worker_router, &mut request, provider_phase_deadline).await;
        // Self-healing: one bounded retry on transient provider errors (429,
        // 5xx, transport blips, CLI rate limits). The retry is recorded in
        // events.jsonl so "turn N hit a transient error; retried" is visible
        // in attach and the audit trail, and the wall budget shrinks by the
        // time the failed attempt and backoff consumed.
        if let ProviderPhaseOutcome::Completed(Err(err)) = &completion
            && err.is_retryable()
            && !should_cancel_run(state, &run_token)
        {
            emit_event(
                state,
                config.event_sender.as_ref(),
                RunEventKind::Error {
                    turn: Some(turn),
                    message: format!(
                        "turn {turn} hit a transient provider error; retrying once: {err}"
                    ),
                },
            )?;
            wait_for_provider_retry(
                provider_phase_deadline.work_expires_at,
                &turn_token,
                PROVIDER_RETRY_BACKOFF,
            )
            .await;
            request.cancellation_token = Some(turn_token.clone());
            completion =
                complete_provider_phase(worker_router, &mut request, provider_phase_deadline).await;
            if matches!(&completion, ProviderPhaseOutcome::Completed(Ok(_))) {
                emit_event(
                    state,
                    config.event_sender.as_ref(),
                    RunEventKind::Error {
                        turn: Some(turn),
                        message: format!("turn {turn} retry succeeded; continuing"),
                    },
                )?;
            }
        }
        let response = match completion {
            ProviderPhaseOutcome::WorkExpired { cleanup } => {
                // The cut turn produced no provider result, but its wall time
                // was really consumed: account for it honestly.
                let elapsed = started.elapsed().as_secs_f64();
                work_clock.sync(state);
                append_spend(
                    state,
                    &SpendRecord {
                        timestamp: Utc::now(),
                        turn,
                        provider: selected_provider
                            .clone()
                            .unwrap_or_else(|| "unknown".to_string()),
                        model: selected_route
                            .as_ref()
                            .map(|route| route.model.clone())
                            .unwrap_or_else(|| "unknown".to_string()),
                        input_tokens: 0,
                        output_tokens: 0,
                        cost_usd: 0.0,
                        total_cost_usd: state.total_spend_usd,
                        cap_usd: config.max_spend_usd,
                        subscription: selected_provider
                            .as_deref()
                            .is_some_and(is_cli_provider_name),
                        estimated: false,
                        wall_time_seconds: Some(elapsed),
                        wall_time_cap_seconds: work_clock
                            .wall_time_cap_seconds(config.max_wall_seconds),
                        kind: "loop".to_string(),
                    },
                )?;
                let outcome = record_provider_interruption(
                    state,
                    turn,
                    ProviderInterruption::WorkExpired,
                    &cleanup,
                    &work_clock,
                )?;
                if let Some(recorder) = flight_recorder.take() {
                    let status = if matches!(&cleanup, ProviderCleanup::RetainedAuthority { .. }) {
                        FlightSessionStatus::Failed
                    } else {
                        FlightSessionStatus::Killed
                    };
                    recorder.finish(state, status, &[]).await?;
                }
                emit_run_completed(state, config.event_sender.as_ref(), outcome.clone())?;
                return Ok(outcome);
            }
            ProviderPhaseOutcome::Cancelled { cleanup } => {
                let outcome = record_provider_interruption(
                    state,
                    turn,
                    ProviderInterruption::Cancelled,
                    &cleanup,
                    &work_clock,
                )?;
                if let Some(recorder) = flight_recorder.take() {
                    let status = if matches!(&cleanup, ProviderCleanup::RetainedAuthority { .. }) {
                        FlightSessionStatus::Failed
                    } else {
                        FlightSessionStatus::Killed
                    };
                    recorder.finish(state, status, &[]).await?;
                }
                emit_run_completed(state, config.event_sender.as_ref(), outcome.clone())?;
                return Ok(outcome);
            }
            ProviderPhaseOutcome::Completed(Ok(response)) => {
                state.provider_failure = None;
                response
            }
            ProviderPhaseOutcome::Completed(Err(
                deadreckon_providers::ProviderError::CleanupIncomplete {
                    authority, detail, ..
                },
            )) => {
                let outcome = record_provider_lost_containment(
                    state,
                    turn,
                    "completion returned without cleanup proof",
                    authority.as_deref(),
                    &detail,
                    &work_clock,
                )?;
                if let Some(recorder) = flight_recorder.take() {
                    recorder
                        .finish(state, FlightSessionStatus::Failed, &[])
                        .await?;
                }
                emit_run_completed(state, config.event_sender.as_ref(), outcome.clone())?;
                return Ok(outcome);
            }
            ProviderPhaseOutcome::Completed(Err(err)) if should_cancel_run(state, &run_token) => {
                if let Some(recorder) = flight_recorder.take() {
                    recorder
                        .finish(state, FlightSessionStatus::Killed, &[])
                        .await?;
                }
                state.status = deadreckon_core::state::RunStatus::Killed;
                state.failure_reason = Some(format!("run cancelled during provider call: {err}"));
                work_clock.save(state)?;
                emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Killed)?;
                return Ok(RunLoopOutcome::Killed);
            }
            ProviderPhaseOutcome::Completed(Err(err)) => {
                if let Some(recorder) = flight_recorder.take() {
                    recorder
                        .finish(state, FlightSessionStatus::Failed, &[])
                        .await?;
                }
                // Persist the failure before surfacing it: a dead run must
                // show as Failed with a reason in list/status, never linger
                // as a zombie Executing until someone probes pid liveness.
                state.failure_reason = Some(format!("provider error: {err}"));
                state.provider_failure = Some(provider_failure_disposition(&err));
                let _ = state.set_phase_status(PhaseId(40), PhaseStatus::Failed);
                work_clock.save(state)?;
                emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Failed)?;
                return Err(provider_error(&err));
            }
        };
        if should_cancel_run(state, &run_token) {
            if let Some(recorder) = flight_recorder.take() {
                recorder
                    .finish(state, FlightSessionStatus::Killed, &[])
                    .await?;
            }
            state.status = deadreckon_core::state::RunStatus::Killed;
            state.failure_reason = Some("run cancelled after provider call".to_string());
            work_clock.save(state)?;
            emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Killed)?;
            return Ok(RunLoopOutcome::Killed);
        }
        if let Some(recorder) = flight_recorder.take() {
            recorder
                .finish(
                    state,
                    FlightSessionStatus::Completed,
                    &provider_flight_rows(&response.trace),
                )
                .await?;
        }
        // Semaphore: a degraded provider contract is not fatal, but it means we
        // fell back to raw stdout — surface it on the attention/"anything wrong"
        // channel (events.jsonl) so it isn't silently swallowed.
        if let Some(message) = degraded_caveat_message(&response.trace, turn) {
            emit_event(
                state,
                config.event_sender.as_ref(),
                RunEventKind::Error {
                    turn: Some(turn),
                    message,
                },
            )?;
        }
        append_provider_approval_traces(state, turn, &response.trace)?;
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
        // Provider-reported wall time remains phase evidence. The authoritative
        // cumulative run clock is controller-measured and synchronized below.
        work_clock.sync(state);
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
                wall_time_cap_seconds: work_clock.wall_time_cap_seconds(config.max_wall_seconds),
                kind: "loop".to_string(),
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
        // Provider completion is a durable progress boundary. Everything that
        // follows (snapshotting, provenance, Git post-processing, docs, and
        // gates) is local work that can fail or be interrupted independently.
        // Persist the provider accounting before entering that boundary so a
        // supervisor/operator never sees a turn-0, zero-spend zombie after the
        // provider has already returned useful work.
        work_clock.save(state)?;
        if config
            .max_spend_usd
            .is_some_and(|cap| state.total_spend_usd > cap)
        {
            state.pause_reason = Some("spend cap reached".to_string());
            work_clock.save(state)?;
            emit_run_completed(
                state,
                config.event_sender.as_ref(),
                RunLoopOutcome::PausedAtCap,
            )?;
            return Ok(RunLoopOutcome::PausedAtCap);
        }
        if config.work_boundary.is_none()
            && config
                .max_wall_seconds
                .is_some_and(|cap| state.total_wall_seconds >= cap)
        {
            state.pause_reason = Some("wall-clock cap reached".to_string());
            work_clock.save(state)?;
            emit_run_completed(
                state,
                config.event_sender.as_ref(),
                RunLoopOutcome::PausedAtCap,
            )?;
            return Ok(RunLoopOutcome::PausedAtCap);
        }

        if is_cli_subagent(&response) {
            let tool_call_id = format!("cli-subagent-turn-{turn}");
            let hook_outcome = emit_tool_event_with_hook(
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
                provider_phase_deadline,
                &run_token,
            )
            .await?;
            if let ControlFlow::Break(outcome) = settle_seam_phase(
                state,
                turn,
                "CLI provider start hook",
                PhaseId(40),
                config.event_sender.as_ref(),
                &work_clock,
                hook_outcome,
            )? {
                return Ok(outcome);
            }
            let changed_result = changed_files_since_snapshot(state, turn.saturating_sub(1));
            let raw_changed = persist_work_boundary(state, &work_clock, changed_result)?;
            let deliverable_result = deliverable_changed_files(state, &raw_changed);
            let changed = persist_work_boundary(state, &work_clock, deliverable_result)?;
            let snapshot_result =
                snapshot_working_bounded(state, turn, provider_phase_deadline, &run_token);
            let snapshot_result = persist_work_boundary(state, &work_clock, snapshot_result);
            if let Err(error) = &snapshot_result
                && let Some(outcome) = settle_local_work_boundary(
                    state,
                    turn,
                    &mut history,
                    error,
                    PhaseId(40),
                    "CLI provider workspace snapshot",
                    config.event_sender.as_ref(),
                    &work_clock,
                )?
            {
                return Ok(outcome);
            }
            snapshot_result?;
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
            let provenance_result = append_provenance_for_files(
                state,
                turn,
                &tool_call_id,
                &response.model,
                raw_changed,
            );
            persist_work_boundary(state, &work_clock, provenance_result)?;
            let commit_result = commit_worktree_turn_bounded(
                state,
                turn,
                "cli_subagent",
                provider_phase_deadline,
                &run_token,
            );
            if let ControlFlow::Break(outcome) = settle_trusted_git_phase(
                state,
                turn,
                "post-provider trusted Git commit",
                PhaseId(40),
                config.event_sender.as_ref(),
                &work_clock,
                commit_result,
            )? {
                return Ok(outcome);
            }
            if changed.is_empty() {
                classify_cli_no_deliverable_changes(state, &history, turn);
                state.set_phase_status(PhaseId(40), PhaseStatus::Failed)?;
                work_clock.save(state)?;
                emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Failed)?;
                return Ok(RunLoopOutcome::Failed);
            }
            let docs_result = append_turn_doc_checkpoint(
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
            );
            persist_work_boundary(state, &work_clock, docs_result)?;
            let hook_outcome = emit_tool_event_with_hook(
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
                provider_phase_deadline,
                &run_token,
            )
            .await?;
            if let ControlFlow::Break(outcome) = settle_seam_phase(
                state,
                turn,
                "CLI provider result hook",
                PhaseId(40),
                config.event_sender.as_ref(),
                &work_clock,
                hook_outcome,
            )? {
                return Ok(outcome);
            }
            if !implementation_notes_ready_or_request_followup(
                state,
                config.event_sender.as_ref(),
                turn,
                &mut history,
            )? {
                state.turn = turn;
                save_history(state, &history)?;
                work_clock.save(state)?;
                continue;
            }
            state.turn = turn;
            save_history(state, &history)?;
            work_clock.save(state)?;
            begin_verification(state, &work_clock)?;
            let docs_result =
                complete_run_docs(
                    state,
                    worker_router,
                    &config,
                    &work_clock,
                    provider_phase_deadline,
                )
                    .await;
            verification_result(state, &work_clock, docs_result)?;
            if should_cancel_run(state, &run_token) {
                fail_verification(state, &work_clock)?;
                finish_cancelled_run_if_requested(
                    state,
                    &run_token,
                    config.event_sender.as_ref(),
                    "run cancelled during documentation finalization",
                    &work_clock,
                )?;
                return Ok(RunLoopOutcome::Killed);
            }
            if pause_verification_if_work_expired(
                state,
                config.event_sender.as_ref(),
                provider_phase_deadline.work_expires_at,
                "documentation finalization",
                &work_clock,
            )? {
                return Ok(RunLoopOutcome::PausedAtCap);
            }
            let commit_result =
                commit_finalized_turn_bounded(state, turn, provider_phase_deadline, &run_token);
            if let ControlFlow::Break(outcome) = settle_trusted_git_phase(
                state,
                turn,
                "finalized turn trusted Git commit",
                PhaseId(50),
                config.event_sender.as_ref(),
                &work_clock,
                commit_result,
            )? {
                return Ok(outcome);
            }
            if pause_verification_if_work_expired(
                state,
                config.event_sender.as_ref(),
                provider_phase_deadline.work_expires_at,
                "finalized turn commit",
                &work_clock,
            )? {
                return Ok(RunLoopOutcome::PausedAtCap);
            }
            if let CompletionMode::ParentRepairCandidate(candidate) = &completion_mode {
                let candidate_result = persist_parent_repair_candidate(state, turn, candidate);
                verification_result(state, &work_clock, candidate_result)?;
                complete_verification(state, &work_clock)?;
                state.failure_reason = None;
                work_clock.save(state)?;
                emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Done)?;
                return Ok(RunLoopOutcome::Done);
            }
            let gate_result = acceptance_gate_passed_or_record_failure(
                state,
                config.event_sender.as_ref(),
                turn,
                &mut history,
                config.sandbox_backend,
                &run_token,
                &work_clock,
                provider_phase_deadline.work_expires_at,
            )
            .await;
            let marker = match verification_result(state, &work_clock, gate_result)? {
                DeterministicGateDisposition::Passed(marker) => marker,
                DeterministicGateDisposition::Revise => {
                    revise_verification(state, &work_clock)?;
                    continue;
                }
                DeterministicGateDisposition::PausedAtCap => {
                    fail_verification(state, &work_clock)?;
                    emit_run_completed(
                        state,
                        config.event_sender.as_ref(),
                        RunLoopOutcome::PausedAtCap,
                    )?;
                    return Ok(RunLoopOutcome::PausedAtCap);
                }
                DeterministicGateDisposition::Cancelled => {
                    fail_verification(state, &work_clock)?;
                    finish_cancelled_run_if_requested(
                        state,
                        &run_token,
                        config.event_sender.as_ref(),
                        "run cancelled during deterministic verification",
                        &work_clock,
                    )?;
                    return Ok(RunLoopOutcome::Killed);
                }
                DeterministicGateDisposition::LostContainment => {
                    fail_verification(state, &work_clock)?;
                    emit_run_completed(
                        state,
                        config.event_sender.as_ref(),
                        RunLoopOutcome::Failed,
                    )?;
                    return Ok(RunLoopOutcome::Failed);
                }
            };
            if should_cancel_run(state, &run_token) {
                fail_verification(state, &work_clock)?;
                finish_cancelled_run_if_requested(
                    state,
                    &run_token,
                    config.event_sender.as_ref(),
                    "run cancelled before semantic verification",
                    &work_clock,
                )?;
                return Ok(RunLoopOutcome::Killed);
            }
            let semantic_result = semantic_completion_disposition(
                state,
                semantic_router,
                &config,
                turn,
                &marker,
                &mut history,
                &run_token,
                &work_clock,
                provider_phase_deadline,
            )
            .await;
            match verification_result(state, &work_clock, semantic_result)? {
                SemanticCompletionDisposition::Achieved => {}
                SemanticCompletionDisposition::Revise => {
                    revise_verification(state, &work_clock)?;
                    continue;
                }
                SemanticCompletionDisposition::NeedsReview => {
                    fail_verification(state, &work_clock)?;
                    emit_run_completed(
                        state,
                        config.event_sender.as_ref(),
                        RunLoopOutcome::Failed,
                    )?;
                    return Ok(RunLoopOutcome::Failed);
                }
                SemanticCompletionDisposition::LostContainment => {
                    fail_verification(state, &work_clock)?;
                    emit_run_completed(
                        state,
                        config.event_sender.as_ref(),
                        RunLoopOutcome::Failed,
                    )?;
                    return Ok(RunLoopOutcome::Failed);
                }
                SemanticCompletionDisposition::BudgetExhausted => {
                    fail_verification(state, &work_clock)?;
                    emit_run_completed(
                        state,
                        config.event_sender.as_ref(),
                        RunLoopOutcome::PausedAtCap,
                    )?;
                    return Ok(RunLoopOutcome::PausedAtCap);
                }
                SemanticCompletionDisposition::Cancelled => {
                    fail_verification(state, &work_clock)?;
                    finish_cancelled_run_if_requested(
                        state,
                        &run_token,
                        config.event_sender.as_ref(),
                        "run cancelled during semantic verification",
                        &work_clock,
                    )?;
                    return Ok(RunLoopOutcome::Killed);
                }
            }
            if pause_verification_if_work_expired(
                state,
                config.event_sender.as_ref(),
                provider_phase_deadline.work_expires_at,
                "semantic verification",
                &work_clock,
            )? {
                return Ok(RunLoopOutcome::PausedAtCap);
            }
            complete_verification(state, &work_clock)?;
            if finish_cancelled_run_if_requested(
                state,
                &run_token,
                config.event_sender.as_ref(),
                "run cancelled before promotion",
                &work_clock,
            )? {
                return Ok(RunLoopOutcome::Killed);
            }
            state.set_phase_status(PhaseId(60), PhaseStatus::Executing)?;
            work_clock.save(state)?;
            let promotion_result = promote_if_ready(state, provider_phase_deadline, &run_token);
            let promotion_result = persist_work_boundary(state, &work_clock, promotion_result);
            if let Err(error) = &promotion_result
                && let Some(outcome) = settle_local_work_boundary(
                    state,
                    turn,
                    &mut history,
                    error,
                    PhaseId(60),
                    "result promotion",
                    config.event_sender.as_ref(),
                    &work_clock,
                )?
            {
                return Ok(outcome);
            }
            if promotion_result.is_err() {
                state.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
                work_clock.save(state)?;
            }
            promotion_result?;
            state.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
            state.failure_reason = None;
            work_clock.save(state)?;
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
                let hook_outcome = emit_tool_event_with_hook(
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
                    provider_phase_deadline,
                    &run_token,
                )
                .await?;
                if let ControlFlow::Break(outcome) = settle_seam_phase(
                    state,
                    turn,
                    "bash tool start hook",
                    PhaseId(40),
                    config.event_sender.as_ref(),
                    &work_clock,
                    hook_outcome,
                )? {
                    return Ok(outcome);
                }
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
                    work_clock.save(state)?;
                    continue;
                }
                let policy_outcome = policy_seam_refusal(
                    &seams,
                    &seam_ctx,
                    &state.run_id,
                    "bash",
                    &command,
                    &state.working_dir,
                    provider_phase_deadline,
                    &run_token,
                )
                .await?;
                let refusal = match settle_seam_phase(
                    state,
                    turn,
                    "bash policy seam",
                    PhaseId(40),
                    config.event_sender.as_ref(),
                    &work_clock,
                    policy_outcome,
                )? {
                    ControlFlow::Continue(refusal) => refusal,
                    ControlFlow::Break(outcome) => return Ok(outcome),
                };
                if let Some(reason) = refusal {
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
                    work_clock.save(state)?;
                    continue;
                }
                let sandboxed = run_sandboxed_work_phase(
                    SandboxSpec {
                        backend: config.sandbox_backend,
                        docker: None,
                        cwd: state.working_dir.clone(),
                        program: OsString::from("sh"),
                        // A login shell rewrites PATH from /etc/profile. That
                        // discards the supervisor's approved toolchain paths
                        // (for example ~/.cargo/bin) inside an otherwise valid
                        // sandbox. The private sandbox HOME has no user profile
                        // to load, so preserve the inherited PATH with `-c`.
                        args: vec![OsString::from("-c"), OsString::from(command.clone())],
                        stdin: None,
                        env: BTreeMap::new(),
                        allow_network: policy.allow_network,
                        pid_file: Some(
                            state
                                .run_root
                                .join("child-pids")
                                .join(format!("tool-{tool_call_id}.pid")),
                        ),
                        cancellation_token: None,
                        profile_dir: Some(state.run_root.join("sandbox")),
                        read_allowlist: policy.read_allowlist,
                        write_allowlist: policy.write_allowlist,
                        read_denylist: Vec::new(),
                        write_denylist: Vec::new(),
                        network_allowlist: policy.network_allowlist,
                        workspace_access: deadreckon_sandbox::WorkspaceAccess::ReadWrite,
                        cleanup_process_group: false,
                        guarded_launch: None,
                    },
                    provider_phase_deadline.work_expires_at,
                    &tool_token,
                )
                .await;
                let output = match sandboxed {
                    SandboxedPhaseOutcome::Completed {
                        cleanup: ProviderCleanup::RetainedAuthority { path, detail },
                        ..
                    } => {
                        let outcome = record_runtime_lost_containment(
                            state,
                            turn,
                            "bash tool phase",
                            PhaseId(40),
                            Some(&path),
                            &detail,
                            &work_clock,
                        )?;
                        emit_run_completed(state, config.event_sender.as_ref(), outcome.clone())?;
                        return Ok(outcome);
                    }
                    SandboxedPhaseOutcome::Completed {
                        result: Ok(output), ..
                    } => output,
                    SandboxedPhaseOutcome::Completed {
                        result: Err(error), ..
                    } => {
                        work_clock.save(state)?;
                        return Err(sandbox_error(&error));
                    }
                    SandboxedPhaseOutcome::WorkExpired { cleanup } => {
                        let outcome = record_runtime_phase_interruption(
                            state,
                            turn,
                            "bash tool phase",
                            PhaseId(40),
                            RuntimePhaseInterruption::WorkExpired,
                            &cleanup,
                            &work_clock,
                        )?;
                        emit_run_completed(state, config.event_sender.as_ref(), outcome.clone())?;
                        return Ok(outcome);
                    }
                    SandboxedPhaseOutcome::Cancelled { cleanup } => {
                        let outcome = record_runtime_phase_interruption(
                            state,
                            turn,
                            "bash tool phase",
                            PhaseId(40),
                            RuntimePhaseInterruption::Cancelled,
                            &cleanup,
                            &work_clock,
                        )?;
                        emit_run_completed(state, config.event_sender.as_ref(), outcome.clone())?;
                        return Ok(outcome);
                    }
                };
                work_clock.save(state)?;
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
                let changed_result = changed_files_since_snapshot(state, turn.saturating_sub(1));
                let raw_changed = persist_work_boundary(state, &work_clock, changed_result)?;
                let deliverable_result = deliverable_changed_files(state, &raw_changed);
                let changed = persist_work_boundary(state, &work_clock, deliverable_result)?;
                let snapshot_result =
                    snapshot_working_bounded(state, turn, provider_phase_deadline, &run_token);
                let snapshot_result = persist_work_boundary(state, &work_clock, snapshot_result);
                if let Err(error) = &snapshot_result
                    && let Some(outcome) = settle_local_work_boundary(
                        state,
                        turn,
                        &mut history,
                        error,
                        PhaseId(40),
                        "bash tool workspace snapshot",
                        config.event_sender.as_ref(),
                        &work_clock,
                    )?
                {
                    return Ok(outcome);
                }
                snapshot_result?;
                let provenance_result = append_provenance_for_files(
                    state,
                    turn,
                    &tool_call_id,
                    &response.model,
                    raw_changed,
                );
                persist_work_boundary(state, &work_clock, provenance_result)?;
                let commit_label = format!("bash {tool_call_id}");
                let commit_result = commit_worktree_turn_bounded(
                    state,
                    turn,
                    &commit_label,
                    provider_phase_deadline,
                    &run_token,
                );
                if let ControlFlow::Break(outcome) = settle_trusted_git_phase(
                    state,
                    turn,
                    "post-tool trusted Git commit",
                    PhaseId(40),
                    config.event_sender.as_ref(),
                    &work_clock,
                    commit_result,
                )? {
                    return Ok(outcome);
                }
                let docs_result = append_turn_doc_checkpoint(
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
                );
                persist_work_boundary(state, &work_clock, docs_result)?;
                let hook_outcome = emit_tool_event_with_hook(
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
                    provider_phase_deadline,
                    &run_token,
                )
                .await?;
                if let ControlFlow::Break(outcome) = settle_seam_phase(
                    state,
                    turn,
                    "bash tool result hook",
                    PhaseId(40),
                    config.event_sender.as_ref(),
                    &work_clock,
                    hook_outcome,
                )? {
                    return Ok(outcome);
                }
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
                let hook_outcome = emit_tool_event_with_hook(
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
                    provider_phase_deadline,
                    &run_token,
                )
                .await?;
                if let ControlFlow::Break(outcome) = settle_seam_phase(
                    state,
                    turn,
                    "write-file start hook",
                    PhaseId(40),
                    config.event_sender.as_ref(),
                    &work_clock,
                    hook_outcome,
                )? {
                    return Ok(outcome);
                }
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
                            work_clock.save(state)?;
                            continue;
                        }
                    };
                let target_label = target.display().to_string();
                let policy_outcome = policy_seam_refusal(
                    &seams,
                    &seam_ctx,
                    &state.run_id,
                    "write_file",
                    &target_label,
                    &state.working_dir,
                    provider_phase_deadline,
                    &run_token,
                )
                .await?;
                let refusal = match settle_seam_phase(
                    state,
                    turn,
                    "write-file policy seam",
                    PhaseId(40),
                    config.event_sender.as_ref(),
                    &work_clock,
                    policy_outcome,
                )? {
                    ControlFlow::Continue(refusal) => refusal,
                    ControlFlow::Break(outcome) => return Ok(outcome),
                };
                if let Some(reason) = refusal {
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
                    work_clock.save(state)?;
                    continue;
                }
                write_workspace_file_no_follow(&state.working_dir, &path, content.as_bytes())?;
                work_clock.save(state)?;
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
                let snapshot_result =
                    snapshot_working_bounded(state, turn, provider_phase_deadline, &run_token);
                let snapshot_result = persist_work_boundary(state, &work_clock, snapshot_result);
                if let Err(error) = &snapshot_result
                    && let Some(outcome) = settle_local_work_boundary(
                        state,
                        turn,
                        &mut history,
                        error,
                        PhaseId(40),
                        "write-file workspace snapshot",
                        config.event_sender.as_ref(),
                        &work_clock,
                    )?
                {
                    return Ok(outcome);
                }
                snapshot_result?;
                let changed = vec![target.clone()];
                let provenance_result = append_provenance_for_files(
                    state,
                    turn,
                    &tool_call_id,
                    &response.model,
                    changed.clone(),
                );
                persist_work_boundary(state, &work_clock, provenance_result)?;
                let commit_label = format!("write_file {tool_call_id}");
                let commit_result = commit_worktree_turn_bounded(
                    state,
                    turn,
                    &commit_label,
                    provider_phase_deadline,
                    &run_token,
                );
                if let ControlFlow::Break(outcome) = settle_trusted_git_phase(
                    state,
                    turn,
                    "post-tool trusted Git commit",
                    PhaseId(40),
                    config.event_sender.as_ref(),
                    &work_clock,
                    commit_result,
                )? {
                    return Ok(outcome);
                }
                let docs_result = append_turn_doc_checkpoint(
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
                );
                persist_work_boundary(state, &work_clock, docs_result)?;
                let hook_outcome = emit_tool_event_with_hook(
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
                    provider_phase_deadline,
                    &run_token,
                )
                .await?;
                if let ControlFlow::Break(outcome) = settle_seam_phase(
                    state,
                    turn,
                    "write-file result hook",
                    PhaseId(40),
                    config.event_sender.as_ref(),
                    &work_clock,
                    hook_outcome,
                )? {
                    return Ok(outcome);
                }
                history.push(format!("tool {tool_call_id} result: wrote file"));
            }
            Action::Reshape {
                tool_call_id,
                pieces,
            } => {
                let recorded = record_reshape_proposal(state, turn, &pieces)?;
                emit_event(
                    state,
                    config.event_sender.as_ref(),
                    RunEventKind::ToolCallResult {
                        turn,
                        tool_call_id: tool_call_id.clone(),
                        status: "recorded".to_string(),
                        preview: event_preview(format!(
                            "reshape proposal: {} piece(s), inert until accepted",
                            pieces.len()
                        )),
                    },
                )?;
                history.push(format!(
                    "tool {tool_call_id} recorded a reshape proposal at {} ({} pieces). It is inert until an operator runs `deadreckon reshape {}`. Keep working the goal in this run.",
                    recorded.display(),
                    pieces.len(),
                    state.run_id
                ));
                state.turn = turn;
                save_history(state, &history)?;
                work_clock.save(state)?;
                continue;
            }
            Action::Done { summary } => {
                state.turn = turn;
                history.push(format!("done: {}", summary.clone().unwrap_or_default()));
                save_history(state, &history)?;
                work_clock.save(state)?;
                let docs_result = append_turn_doc_checkpoint(
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
                );
                persist_work_boundary(state, &work_clock, docs_result)?;
                if !implementation_notes_ready_or_request_followup(
                    state,
                    config.event_sender.as_ref(),
                    turn,
                    &mut history,
                )? {
                    state.turn = turn;
                    save_history(state, &history)?;
                    work_clock.save(state)?;
                    continue;
                }
                begin_verification(state, &work_clock)?;
                let docs_result =
                    complete_run_docs(
                        state,
                        worker_router,
                        &config,
                        &work_clock,
                        provider_phase_deadline,
                    )
                        .await;
                verification_result(state, &work_clock, docs_result)?;
                if should_cancel_run(state, &run_token) {
                    fail_verification(state, &work_clock)?;
                    finish_cancelled_run_if_requested(
                        state,
                        &run_token,
                        config.event_sender.as_ref(),
                        "run cancelled during documentation finalization",
                        &work_clock,
                    )?;
                    return Ok(RunLoopOutcome::Killed);
                }
                if pause_verification_if_work_expired(
                    state,
                    config.event_sender.as_ref(),
                    provider_phase_deadline.work_expires_at,
                    "documentation finalization",
                    &work_clock,
                )? {
                    return Ok(RunLoopOutcome::PausedAtCap);
                }
                let commit_result =
                    commit_finalized_turn_bounded(state, turn, provider_phase_deadline, &run_token);
                if let ControlFlow::Break(outcome) = settle_trusted_git_phase(
                    state,
                    turn,
                    "finalized turn trusted Git commit",
                    PhaseId(50),
                    config.event_sender.as_ref(),
                    &work_clock,
                    commit_result,
                )? {
                    return Ok(outcome);
                }
                if pause_verification_if_work_expired(
                    state,
                    config.event_sender.as_ref(),
                    provider_phase_deadline.work_expires_at,
                    "finalized turn commit",
                    &work_clock,
                )? {
                    return Ok(RunLoopOutcome::PausedAtCap);
                }
                if let CompletionMode::ParentRepairCandidate(candidate) = &completion_mode {
                    let candidate_result = persist_parent_repair_candidate(state, turn, candidate);
                    verification_result(state, &work_clock, candidate_result)?;
                    complete_verification(state, &work_clock)?;
                    state.failure_reason = None;
                    work_clock.save(state)?;
                    emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Done)?;
                    return Ok(RunLoopOutcome::Done);
                }
                let gate_result = acceptance_gate_passed_or_record_failure(
                    state,
                    config.event_sender.as_ref(),
                    turn,
                    &mut history,
                    config.sandbox_backend,
                    &run_token,
                    &work_clock,
                    provider_phase_deadline.work_expires_at,
                )
                .await;
                let marker = match verification_result(state, &work_clock, gate_result)? {
                    DeterministicGateDisposition::Passed(marker) => marker,
                    DeterministicGateDisposition::Revise => {
                        revise_verification(state, &work_clock)?;
                        continue;
                    }
                    DeterministicGateDisposition::PausedAtCap => {
                        fail_verification(state, &work_clock)?;
                        emit_run_completed(
                            state,
                            config.event_sender.as_ref(),
                            RunLoopOutcome::PausedAtCap,
                        )?;
                        return Ok(RunLoopOutcome::PausedAtCap);
                    }
                    DeterministicGateDisposition::Cancelled => {
                        fail_verification(state, &work_clock)?;
                        finish_cancelled_run_if_requested(
                            state,
                            &run_token,
                            config.event_sender.as_ref(),
                            "run cancelled during deterministic verification",
                            &work_clock,
                        )?;
                        return Ok(RunLoopOutcome::Killed);
                    }
                    DeterministicGateDisposition::LostContainment => {
                        fail_verification(state, &work_clock)?;
                        emit_run_completed(
                            state,
                            config.event_sender.as_ref(),
                            RunLoopOutcome::Failed,
                        )?;
                        return Ok(RunLoopOutcome::Failed);
                    }
                };
                if should_cancel_run(state, &run_token) {
                    fail_verification(state, &work_clock)?;
                    finish_cancelled_run_if_requested(
                        state,
                        &run_token,
                        config.event_sender.as_ref(),
                        "run cancelled before semantic verification",
                        &work_clock,
                    )?;
                    return Ok(RunLoopOutcome::Killed);
                }
                let semantic_result = semantic_completion_disposition(
                    state,
                    semantic_router,
                    &config,
                    turn,
                    &marker,
                    &mut history,
                    &run_token,
                    &work_clock,
                    provider_phase_deadline,
                )
                .await;
                match verification_result(state, &work_clock, semantic_result)? {
                    SemanticCompletionDisposition::Achieved => {}
                    SemanticCompletionDisposition::Revise => {
                        revise_verification(state, &work_clock)?;
                        continue;
                    }
                    SemanticCompletionDisposition::NeedsReview => {
                        fail_verification(state, &work_clock)?;
                        emit_run_completed(
                            state,
                            config.event_sender.as_ref(),
                            RunLoopOutcome::Failed,
                        )?;
                        return Ok(RunLoopOutcome::Failed);
                    }
                    SemanticCompletionDisposition::LostContainment => {
                        fail_verification(state, &work_clock)?;
                        emit_run_completed(
                            state,
                            config.event_sender.as_ref(),
                            RunLoopOutcome::Failed,
                        )?;
                        return Ok(RunLoopOutcome::Failed);
                    }
                    SemanticCompletionDisposition::BudgetExhausted => {
                        fail_verification(state, &work_clock)?;
                        emit_run_completed(
                            state,
                            config.event_sender.as_ref(),
                            RunLoopOutcome::PausedAtCap,
                        )?;
                        return Ok(RunLoopOutcome::PausedAtCap);
                    }
                    SemanticCompletionDisposition::Cancelled => {
                        fail_verification(state, &work_clock)?;
                        finish_cancelled_run_if_requested(
                            state,
                            &run_token,
                            config.event_sender.as_ref(),
                            "run cancelled during semantic verification",
                            &work_clock,
                        )?;
                        return Ok(RunLoopOutcome::Killed);
                    }
                }
                if pause_verification_if_work_expired(
                    state,
                    config.event_sender.as_ref(),
                    provider_phase_deadline.work_expires_at,
                    "semantic verification",
                    &work_clock,
                )? {
                    return Ok(RunLoopOutcome::PausedAtCap);
                }
                complete_verification(state, &work_clock)?;
                if finish_cancelled_run_if_requested(
                    state,
                    &run_token,
                    config.event_sender.as_ref(),
                    "run cancelled before promotion",
                    &work_clock,
                )? {
                    return Ok(RunLoopOutcome::Killed);
                }
                state.set_phase_status(PhaseId(60), PhaseStatus::Executing)?;
                work_clock.save(state)?;
                let promotion_result = promote_if_ready(state, provider_phase_deadline, &run_token);
                let promotion_result = persist_work_boundary(state, &work_clock, promotion_result);
                if let Err(error) = &promotion_result
                    && let Some(outcome) = settle_local_work_boundary(
                        state,
                        turn,
                        &mut history,
                        error,
                        PhaseId(60),
                        "result promotion",
                        config.event_sender.as_ref(),
                        &work_clock,
                    )?
                {
                    return Ok(outcome);
                }
                if promotion_result.is_err() {
                    state.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
                    work_clock.save(state)?;
                }
                promotion_result?;
                state.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
                state.failure_reason = None;
                work_clock.save(state)?;
                emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Done)?;
                return Ok(RunLoopOutcome::Done);
            }
        }
        state.turn = turn;
        save_history(state, &history)?;
        work_clock.save(state)?;
    }

    state.failure_reason = Some(match state.failure_reason.take() {
        Some(reason) => format!("{reason}; max turn budget exhausted"),
        None => "max turn budget exhausted".to_string(),
    });
    state.set_phase_status(PhaseId(40), PhaseStatus::Failed)?;
    work_clock.save(state)?;
    emit_run_completed(state, config.event_sender.as_ref(), RunLoopOutcome::Failed)?;
    Ok(RunLoopOutcome::Failed)
    }
    .await;
    if let Some(forwarder) = event_sink_forwarder {
        forwarder
            .shutdown(provider_phase_deadline.cleanup_budget)
            .await;
    }
    run_result
}

const ACCEPTANCE_FAILURE_PREFIX: &str = "acceptance failed after turn ";
const CLI_NO_DELIVERABLE_CHANGES: &str =
    "cli subagent completed without file changes in the deliverable";

fn classify_cli_no_deliverable_changes(state: &mut PipelineState, history: &[String], turn: u32) {
    // A no-op provider turn after a deterministic-gate failure has not made
    // that gate failure disappear. Preserve the actionable cause and mark the
    // no-progress result instead of replacing it with a false provider cause.
    // The durable supervisor can then classify the preserved gate failure and
    // decide whether another approved, bounded attempt is available.
    let acceptance_failure = state
        .failure_reason
        .as_ref()
        .filter(|reason| reason.starts_with(ACCEPTANCE_FAILURE_PREFIX))
        .cloned()
        .or_else(|| {
            history
                .iter()
                .rev()
                .find(|entry| entry.starts_with(ACCEPTANCE_FAILURE_PREFIX))
                .cloned()
        });
    state.turn = turn;
    if let Some(reason) = acceptance_failure {
        state.failure_reason = Some(reason);
    } else {
        state.failure_reason = Some(CLI_NO_DELIVERABLE_CHANGES.to_string());
    }
}

fn persist_parent_repair_candidate(
    state: &PipelineState,
    turn: u32,
    context: &ParentRepairCandidateContext,
) -> Result<()> {
    if context.job_id != state.run_id
        || context.round == 0
        || context.attempt == 0
        || context.lease_epoch == 0
        || Uuid::parse_str(&context.launch_id).is_err()
        || context.intent_sha256.is_empty()
        || context.manifest_sha256.is_empty()
    {
        return Err(DeadreckonError::InvalidInput(
            "parent repair candidate context does not match the same-ID result run".to_string(),
        ));
    }
    let expected_path = deadreckon_core::parent_repair_candidate_path_for_run_root(&state.run_root);
    if context.path != expected_path {
        return Err(DeadreckonError::InvalidInput(format!(
            "parent repair candidate path is outside the trusted proof location: {}",
            context.path.display()
        )));
    }
    let mut index = deadreckon_core::flight::build_deliverable_file_index_for_state(state)?;
    index.files.remove(Path::new("manifest.json"));
    let candidate = ParentRepairCandidate {
        schema_version: 1,
        job_id: context.job_id.clone(),
        run_id: state.run_id.clone(),
        round: context.round,
        attempt: context.attempt,
        launch_id: context.launch_id.clone(),
        lease_epoch: context.lease_epoch,
        intent_sha256: context.intent_sha256.clone(),
        manifest_sha256: context.manifest_sha256.clone(),
        result_tree_sha256: index.tree_hash(),
        turn,
        ready_at: Utc::now(),
    };
    let parent = context.path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "parent repair candidate path has no parent: {}",
            context.path.display()
        ))
    })?;
    std::fs::create_dir_all(parent).with_path(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).with_path(parent)?;
    serde_json::to_writer_pretty(&mut temp, &candidate).map_err(|source| {
        DeadreckonError::Json {
            path: context.path.clone(),
            source,
        }
    })?;
    temp.write_all(b"\n").with_path(&context.path)?;
    temp.as_file_mut().sync_all().with_path(&context.path)?;
    temp.persist(&context.path)
        .map_err(|error| DeadreckonError::Io {
            path: context.path.clone(),
            source: error.error,
        })?;
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_path(parent)?;
    Ok(())
}

fn should_cancel_run(state: &PipelineState, token: &CancellationToken) -> bool {
    state.status == RunStatus::Killed || token.is_cancelled() || cancel_marker_present(state)
}

fn finish_cancelled_run_if_requested(
    state: &mut PipelineState,
    token: &CancellationToken,
    sender: Option<&broadcast::Sender<RunEvent>>,
    reason: &str,
    work_clock: &RunWorkClock,
) -> Result<bool> {
    if !should_cancel_run(state, token) {
        return Ok(false);
    }
    state.status = RunStatus::Killed;
    state.failure_reason = Some(reason.to_string());
    work_clock.save(state)?;
    emit_run_completed(state, sender, RunLoopOutcome::Killed)?;
    Ok(true)
}

fn finish_work_expired_if_reached(
    state: &mut PipelineState,
    work_expires_at: tokio::time::Instant,
    phase: &str,
    work_clock: &RunWorkClock,
) -> Result<bool> {
    work_clock.sync(state);
    if tokio::time::Instant::now() < work_expires_at {
        return Ok(false);
    }
    let reason = format!("{} during {phase}", work_clock.expiry().reached_label());
    state.pause_reason = Some(reason.clone());
    state.failure_reason = Some(reason);
    work_clock.save(state)?;
    Ok(true)
}

fn pause_verification_if_work_expired(
    state: &mut PipelineState,
    sender: Option<&broadcast::Sender<RunEvent>>,
    work_expires_at: tokio::time::Instant,
    phase: &str,
    work_clock: &RunWorkClock,
) -> Result<bool> {
    if !finish_work_expired_if_reached(state, work_expires_at, phase, work_clock)? {
        return Ok(false);
    }
    state.set_phase_status(PhaseId(50), PhaseStatus::Failed)?;
    work_clock.save(state)?;
    emit_run_completed(state, sender, RunLoopOutcome::PausedAtCap)?;
    Ok(true)
}

/// Backoff before the single transient-error retry: long enough for a rate
/// limit window to move, short enough to be negligible against a turn.
const PROVIDER_RETRY_BACKOFF: Duration = Duration::from_secs(2);
const PROVIDER_CLEANUP_BUDGET: Duration = Duration::from_secs(30);
const PROVIDER_UNBOUNDED_WORK_WINDOW: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);
const RUNTIME_PHASE_CLEANUP_BUDGET: Duration = Duration::from_secs(30);
const EVENT_SINK_DRAIN_GRACE: Duration = Duration::from_millis(250);

async fn wait_for_provider_retry(
    work_expires_at: tokio::time::Instant,
    cancellation_token: &CancellationToken,
    backoff: Duration,
) {
    tokio::select! {
        biased;
        () = cancellation_token.cancelled() => {}
        () = tokio::time::sleep_until(work_expires_at) => {}
        () = tokio::time::sleep(backoff) => {}
    }
}

#[derive(Debug)]
enum SandboxedPhaseOutcome {
    Completed {
        result: std::result::Result<SandboxRunOutput, SandboxError>,
        cleanup: ProviderCleanup,
    },
    WorkExpired {
        cleanup: ProviderCleanup,
    },
    Cancelled {
        cleanup: ProviderCleanup,
    },
}

enum SandboxedPhaseBoundary<T> {
    Completed(T),
    WorkExpired,
    Cancelled,
}

async fn run_sandboxed_work_phase(
    mut spec: SandboxSpec,
    work_expires_at: tokio::time::Instant,
    external_cancellation: &CancellationToken,
) -> SandboxedPhaseOutcome {
    let authority = spec.pid_file.clone();
    if external_cancellation.is_cancelled() {
        return SandboxedPhaseOutcome::Cancelled {
            cleanup: classify_runtime_phase_cleanup(authority.as_deref(), true),
        };
    }
    if tokio::time::Instant::now() >= work_expires_at {
        return SandboxedPhaseOutcome::WorkExpired {
            cleanup: classify_runtime_phase_cleanup(authority.as_deref(), true),
        };
    }

    let phase_token = CancellationToken::new();
    spec.cancellation_token = Some(phase_token.clone());
    let execution = run_sandbox(spec);
    tokio::pin!(execution);
    let boundary = tokio::select! {
        biased;
        () = external_cancellation.cancelled() => SandboxedPhaseBoundary::Cancelled,
        result = &mut execution => SandboxedPhaseBoundary::Completed(result),
        () = tokio::time::sleep_until(work_expires_at) => SandboxedPhaseBoundary::WorkExpired,
    };

    match boundary {
        SandboxedPhaseBoundary::Completed(result) => SandboxedPhaseOutcome::Completed {
            cleanup: classify_runtime_phase_cleanup(authority.as_deref(), true),
            result,
        },
        SandboxedPhaseBoundary::WorkExpired => {
            phase_token.cancel();
            let cleanup_resolved =
                tokio::time::timeout(RUNTIME_PHASE_CLEANUP_BUDGET, &mut execution)
                    .await
                    .is_ok();
            let cleanup = classify_runtime_phase_cleanup(authority.as_deref(), cleanup_resolved);
            SandboxedPhaseOutcome::WorkExpired { cleanup }
        }
        SandboxedPhaseBoundary::Cancelled => {
            phase_token.cancel();
            let cleanup_resolved =
                tokio::time::timeout(RUNTIME_PHASE_CLEANUP_BUDGET, &mut execution)
                    .await
                    .is_ok();
            let cleanup = classify_runtime_phase_cleanup(authority.as_deref(), cleanup_resolved);
            SandboxedPhaseOutcome::Cancelled { cleanup }
        }
    }
}

fn classify_runtime_phase_cleanup(
    authority: Option<&Path>,
    execution_resolved: bool,
) -> ProviderCleanup {
    let Some(authority) = authority else {
        return if execution_resolved {
            ProviderCleanup::NotApplicable
        } else {
            ProviderCleanup::RetainedAuthority {
                path: PathBuf::from("<unavailable-runtime-process-authority>"),
                detail:
                    "runtime subprocess cleanup did not resolve and no authority path was available"
                        .to_string(),
            }
        };
    };
    match std::fs::symlink_metadata(authority) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && execution_resolved => {
            ProviderCleanup::Proven
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ProviderCleanup::RetainedAuthority {
                path: authority.to_path_buf(),
                detail: "runtime subprocess cleanup future did not resolve before its separate cleanup deadline"
                    .to_string(),
            }
        }
        Ok(_) => ProviderCleanup::RetainedAuthority {
            path: authority.to_path_buf(),
            detail: "runtime subprocess authority remains after the phase boundary".to_string(),
        },
        Err(error) => ProviderCleanup::RetainedAuthority {
            path: authority.to_path_buf(),
            detail: format!("runtime subprocess authority could not be inspected: {error}"),
        },
    }
}

fn provider_failure_disposition(
    error: &deadreckon_providers::ProviderError,
) -> deadreckon_core::ProviderFailureDisposition {
    if error.is_retryable() {
        deadreckon_core::ProviderFailureDisposition::Retryable
    } else {
        deadreckon_core::ProviderFailureDisposition::Fatal
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderInterruption {
    WorkExpired,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimePhaseInterruption {
    WorkExpired,
    Cancelled,
}

fn record_runtime_phase_interruption(
    state: &mut PipelineState,
    turn: u32,
    phase: &str,
    phase_id: PhaseId,
    interruption: RuntimePhaseInterruption,
    cleanup: &ProviderCleanup,
    work_clock: &RunWorkClock,
) -> Result<RunLoopOutcome> {
    match cleanup {
        ProviderCleanup::Proven | ProviderCleanup::NotApplicable => {
            let outcome = match interruption {
                RuntimePhaseInterruption::WorkExpired => {
                    state.pause_reason = Some(format!(
                        "{} during {phase}; subprocess cleanup was proven",
                        work_clock.expiry().reached_label()
                    ));
                    RunLoopOutcome::PausedAtCap
                }
                RuntimePhaseInterruption::Cancelled => {
                    state.status = RunStatus::Killed;
                    state.failure_reason = Some(format!(
                        "run cancelled during {phase}; subprocess cleanup was proven"
                    ));
                    RunLoopOutcome::Killed
                }
            };
            work_clock.save(state)?;
            Ok(outcome)
        }
        ProviderCleanup::RetainedAuthority { path, detail } => record_runtime_lost_containment(
            state,
            turn,
            phase,
            phase_id,
            Some(path.as_path()),
            detail,
            work_clock,
        ),
    }
}

fn record_runtime_lost_containment(
    state: &mut PipelineState,
    turn: u32,
    phase: &str,
    phase_id: PhaseId,
    authority: Option<&Path>,
    detail: &str,
    work_clock: &RunWorkClock,
) -> Result<RunLoopOutcome> {
    let authority_label = authority
        .map(|path| format!(" at {}", path.display()))
        .unwrap_or_else(|| " with an unavailable authority path".to_string());
    let reason =
        format!("LOST_CONTAINMENT: {phase} retained process authority{authority_label}: {detail}");
    append_trace(
        state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn,
            event: "runtime.lost_containment".to_string(),
            latency_ms: None,
            detail: json!({
                "phase": phase,
                "authority": authority,
                "reason": detail,
            }),
        },
    )?;
    state.pause_reason = None;
    state.failure_reason = Some(reason);
    state.set_phase_status(phase_id, PhaseStatus::Failed)?;
    work_clock.save(state)?;
    Ok(RunLoopOutcome::Failed)
}

#[allow(clippy::too_many_arguments)]
fn settle_trusted_git_phase<T>(
    state: &mut PipelineState,
    turn: u32,
    phase: &str,
    phase_id: PhaseId,
    sender: Option<&broadcast::Sender<RunEvent>>,
    work_clock: &RunWorkClock,
    result: Result<TrustedGitPhaseOutcome<T>>,
) -> Result<ControlFlow<RunLoopOutcome, T>> {
    let phase_outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            work_clock.save(state)?;
            return Err(error);
        }
    };
    let terminal = match phase_outcome {
        TrustedGitPhaseOutcome::Completed(value) => {
            work_clock.save(state)?;
            return Ok(ControlFlow::Continue(value));
        }
        TrustedGitPhaseOutcome::WorkExpired { cleanup } => record_runtime_phase_interruption(
            state,
            turn,
            phase,
            phase_id,
            RuntimePhaseInterruption::WorkExpired,
            &cleanup,
            work_clock,
        )?,
        TrustedGitPhaseOutcome::Cancelled { cleanup } => record_runtime_phase_interruption(
            state,
            turn,
            phase,
            phase_id,
            RuntimePhaseInterruption::Cancelled,
            &cleanup,
            work_clock,
        )?,
        TrustedGitPhaseOutcome::LostContainment {
            boundary,
            authority,
            detail,
        } => {
            let detail = format!("{detail}; controller boundary: {boundary:?}");
            record_runtime_lost_containment(
                state,
                turn,
                phase,
                phase_id,
                authority.as_deref(),
                &detail,
                work_clock,
            )?
        }
    };
    emit_run_completed(state, sender, terminal.clone())?;
    Ok(ControlFlow::Break(terminal))
}

#[allow(clippy::too_many_arguments)]
fn settle_seam_phase<T>(
    state: &mut PipelineState,
    turn: u32,
    phase: &str,
    phase_id: PhaseId,
    sender: Option<&broadcast::Sender<RunEvent>>,
    work_clock: &RunWorkClock,
    outcome: SeamPhaseOutcome<T>,
) -> Result<ControlFlow<RunLoopOutcome, T>> {
    let terminal = match outcome {
        SeamPhaseOutcome::Completed(value) => {
            work_clock.save(state)?;
            return Ok(ControlFlow::Continue(value));
        }
        SeamPhaseOutcome::WorkExpired { cleanup } => record_runtime_phase_interruption(
            state,
            turn,
            phase,
            phase_id,
            RuntimePhaseInterruption::WorkExpired,
            &cleanup,
            work_clock,
        )?,
        SeamPhaseOutcome::Cancelled { cleanup } => record_runtime_phase_interruption(
            state,
            turn,
            phase,
            phase_id,
            RuntimePhaseInterruption::Cancelled,
            &cleanup,
            work_clock,
        )?,
    };
    emit_run_completed(state, sender, terminal.clone())?;
    Ok(ControlFlow::Break(terminal))
}

fn record_provider_interruption(
    state: &mut PipelineState,
    turn: u32,
    interruption: ProviderInterruption,
    cleanup: &ProviderCleanup,
    work_clock: &RunWorkClock,
) -> Result<RunLoopOutcome> {
    let boundary = match interruption {
        ProviderInterruption::WorkExpired => work_clock.expiry().reached_label(),
        ProviderInterruption::Cancelled => "controller cancellation",
    };
    match cleanup {
        ProviderCleanup::Proven | ProviderCleanup::NotApplicable => {
            let outcome = match interruption {
                ProviderInterruption::WorkExpired => {
                    state.pause_reason =
                        Some(format!("{} mid-turn", work_clock.expiry().reached_label()));
                    RunLoopOutcome::PausedAtCap
                }
                ProviderInterruption::Cancelled => {
                    state.status = RunStatus::Killed;
                    state.failure_reason = Some(
                        "run cancelled during provider call after cleanup was proven".to_string(),
                    );
                    RunLoopOutcome::Killed
                }
            };
            work_clock.save(state)?;
            Ok(outcome)
        }
        ProviderCleanup::RetainedAuthority { path, detail } => record_provider_lost_containment(
            state,
            turn,
            boundary,
            Some(path.as_path()),
            detail,
            work_clock,
        ),
    }
}

fn record_provider_lost_containment(
    state: &mut PipelineState,
    turn: u32,
    boundary: &str,
    authority: Option<&Path>,
    detail: &str,
    work_clock: &RunWorkClock,
) -> Result<RunLoopOutcome> {
    let authority_label = authority
        .map(|path| format!(" at {}", path.display()))
        .unwrap_or_else(|| " with an unavailable authority path".to_string());
    let reason = format!(
        "LOST_CONTAINMENT: provider {boundary} with retained process authority{authority_label}: {detail}"
    );
    append_trace(
        state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn,
            event: "provider.lost_containment".to_string(),
            latency_ms: None,
            detail: json!({
                "boundary": boundary,
                "authority": authority,
                "reason": detail,
            }),
        },
    )?;
    state.pause_reason = None;
    state.failure_reason = Some(reason);
    state.provider_failure = Some(deadreckon_core::ProviderFailureDisposition::Fatal);
    state.set_phase_status(PhaseId(40), PhaseStatus::Failed)?;
    work_clock.save(state)?;
    Ok(RunLoopOutcome::Failed)
}

fn begin_verification(state: &mut PipelineState, work_clock: &RunWorkClock) -> Result<()> {
    state.set_phase_status(PhaseId(40), PhaseStatus::Completed)?;
    state.set_phase_status(PhaseId(50), PhaseStatus::Executing)?;
    work_clock.save(state)
}

fn revise_verification(state: &mut PipelineState, work_clock: &RunWorkClock) -> Result<()> {
    state.set_phase_status(PhaseId(50), PhaseStatus::Pending)?;
    state.set_phase_status(PhaseId(40), PhaseStatus::Executing)?;
    work_clock.save(state)
}

fn fail_verification(state: &mut PipelineState, work_clock: &RunWorkClock) -> Result<()> {
    state.set_phase_status(PhaseId(50), PhaseStatus::Failed)?;
    work_clock.save(state)
}

fn complete_verification(state: &mut PipelineState, work_clock: &RunWorkClock) -> Result<()> {
    state.set_phase_status(PhaseId(50), PhaseStatus::Completed)?;
    work_clock.save(state)
}

fn verification_result<T>(
    state: &mut PipelineState,
    work_clock: &RunWorkClock,
    result: Result<T>,
) -> Result<T> {
    if result.is_err() {
        fail_verification(state, work_clock)?;
    }
    result
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
        "You are deadreckon running unattended coding work.\nWorking directory: {}\nSPEC:\n{}\n\nSkill and implementation-notes contract:\n{}\n\nHistory:\n{}\n\nReturn exactly one JSON object with action bash, write_file, reshape (propose splitting the goal into 2-6 independent pieces: {{\"action\":\"reshape\",\"tool_call_id\":\"...\",\"pieces\":[{{\"goal\":\"...\",\"done_hint\":\"...\"}}]}} - recorded for the operator, never executed by you), or done.",
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

/// Tool rows a Semaphore driver lifted live from its structured stream, carried
/// on the response trace as `flight_rows`. Empty for providers that don't emit
/// them (the post-hoc file scraper covers those).
fn provider_flight_rows(trace: &serde_json::Value) -> Vec<serde_json::Value> {
    trace
        .get("flight_rows")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A user-facing notice when either the provider contract or route degraded
/// this turn, for the attention / "anything wrong" surface.
fn degraded_caveat_message(trace: &serde_json::Value, turn: u32) -> Option<String> {
    let caveats = trace.get("caveats").and_then(serde_json::Value::as_array)?;
    let caveat = caveats
        .iter()
        .find(|caveat| {
            caveat.get("code").and_then(serde_json::Value::as_str)
                == Some("provider.route.degraded")
        })
        .or_else(|| {
            caveats.iter().find(|caveat| {
                caveat.get("code").and_then(serde_json::Value::as_str)
                    == Some("provider.contract.degraded")
            })
        })?;
    let kind = if caveat.get("code").and_then(serde_json::Value::as_str)
        == Some("provider.route.degraded")
    {
        "provider route degraded"
    } else {
        "provider contract degraded"
    };
    let detail = caveat
        .get("message")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(kind);
    Some(format!("turn {turn}: {kind} — {detail}"))
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

fn append_provider_approval_traces(
    state: &PipelineState,
    turn: u32,
    provider_trace: &Value,
) -> Result<()> {
    let Some(approvals) = provider_trace.get("approvals").and_then(Value::as_array) else {
        return Ok(());
    };
    for approval in approvals {
        append_trace(
            state,
            &TraceRecord {
                timestamp: Utc::now(),
                run_id: state.run_id.clone(),
                turn,
                event: "provider.approval".to_string(),
                latency_ms: None,
                detail: approval.clone(),
            },
        )?;
    }
    Ok(())
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
    if !is_deliverable_workspace_path(relative) {
        return Err(DeadreckonError::InvalidInput(format!(
            "write_file path {} is reserved for evidence, lifecycle metadata, or runtime state and is not part of the deliverable\ntry: write a project source, test, or documentation path instead",
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

fn write_workspace_file_no_follow(root: &Path, relative: &Path, content: &[u8]) -> Result<PathBuf> {
    let target = safe_working_path(root, relative)?;
    ensure_real_directory_no_follow(root)?;

    let parent_relative = relative.parent().unwrap_or_else(|| Path::new(""));
    let mut parent = root.to_path_buf();
    for component in parent_relative.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => {
                parent.push(part);
                match std::fs::symlink_metadata(&parent) {
                    Ok(metadata) => refuse_non_directory_or_symlink(&parent, &metadata)?,
                    Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                        std::fs::create_dir(&parent).with_path(&parent)?;
                        let metadata = std::fs::symlink_metadata(&parent).with_path(&parent)?;
                        refuse_non_directory_or_symlink(&parent, &metadata)?;
                    }
                    Err(source) => {
                        return Err(DeadreckonError::Io {
                            path: parent.clone(),
                            source,
                        });
                    }
                }
            }
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err(DeadreckonError::InvalidInput(format!(
                    "unsafe write path {}",
                    relative.display()
                )));
            }
        }
    }

    let file_name = relative.file_name().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "write_file path {} does not name a file",
            relative.display()
        ))
    })?;
    let leaf = parent.join(file_name);
    match std::fs::symlink_metadata(&leaf) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(symlink_write_refusal(&leaf));
        }
        Ok(metadata) if metadata.is_file() => {
            let mut temp = tempfile::NamedTempFile::new_in(&parent).with_path(&parent)?;
            temp.as_file()
                .set_permissions(metadata.permissions())
                .with_path(temp.path())?;
            temp.write_all(content).with_path(temp.path())?;
            temp.flush().with_path(temp.path())?;
            temp.persist(&leaf).map_err(|err| DeadreckonError::Io {
                path: leaf.clone(),
                source: err.error,
            })?;
        }
        Ok(_) => {
            return Err(DeadreckonError::InvalidInput(format!(
                "write_file path {} is not a regular file",
                leaf.display()
            )));
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&leaf)
                .with_path(&leaf)?;
            file.write_all(content).with_path(&leaf)?;
        }
        Err(source) => {
            return Err(DeadreckonError::Io { path: leaf, source });
        }
    }
    Ok(target)
}

fn ensure_real_directory_no_follow(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).with_path(path)?;
    refuse_non_directory_or_symlink(path, &metadata)
}

fn refuse_non_directory_or_symlink(path: &Path, metadata: &std::fs::Metadata) -> Result<()> {
    if metadata.file_type().is_symlink() {
        return Err(symlink_write_refusal(path));
    }
    if !metadata.is_dir() {
        return Err(DeadreckonError::InvalidInput(format!(
            "write_file ancestor {} is not a directory",
            path.display()
        )));
    }
    Ok(())
}

fn symlink_write_refusal(path: &Path) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!(
        "write_file refuses symlink path {} to prevent writes outside the working directory",
        path.display()
    ))
}

fn sandbox_toml_path(state: &PipelineState) -> PathBuf {
    state.run_root.join("sandbox.toml")
}

fn ensure_sandbox_toml(state: &PipelineState) -> Result<()> {
    let path = sandbox_toml_path(state);
    let approved = approved_sandbox_toml(state)?;
    if path.exists() {
        if let Some(expected) = approved {
            let raw = std::fs::read_to_string(&path).with_path(&path)?;
            let actual = toml::from_str::<SandboxToml>(&raw).map_err(|err| {
                DeadreckonError::InvalidInput(format!(
                    "invalid sandbox.toml: {err}\ntry: inspect {}",
                    path.display()
                ))
            })?;
            if actual != expected {
                return Err(DeadreckonError::InvalidInput(
                    "sandbox.toml no longer matches the immutable approved Job execution policy"
                        .to_string(),
                ));
            }
        }
        return Ok(());
    }
    let config = approved.unwrap_or_else(|| default_sandbox_toml(state));
    let raw = toml::to_string_pretty(&config).map_err(|err| {
        DeadreckonError::InvalidInput(format!("sandbox.toml encode error: {err}"))
    })?;
    std::fs::write(&path, raw).with_path(&path)
}

fn default_sandbox_toml(state: &PipelineState) -> SandboxToml {
    let mut tools = BTreeMap::new();
    for name in ["bash", "write_file"] {
        tools.insert(
            name.to_string(),
            SandboxTomlTool {
                read: vec![state.working_dir.clone()],
                write: vec![state.working_dir.clone()],
                network: Vec::new(),
            },
        );
    }
    SandboxToml { version: 1, tools }
}

fn approved_gate_network_access(
    state: &PipelineState,
    strict_job: bool,
) -> Result<JobGateNetworkAccess> {
    let contract_path = acceptance_spec_path_for_run_root(&state.run_root);
    let contract_raw = std::fs::read_to_string(&contract_path).with_path(&contract_path)?;
    let declared = deadreckon_core::gate::acceptance_capabilities_from_yaml(&contract_raw)?.network;
    let declared = match declared {
        deadreckon_core::gate::AcceptanceNetworkAccess::Deny => JobGateNetworkAccess::Deny,
        deadreckon_core::gate::AcceptanceNetworkAccess::Loopback => JobGateNetworkAccess::Loopback,
        deadreckon_core::gate::AcceptanceNetworkAccess::Full => JobGateNetworkAccess::Full,
    };
    if !strict_job {
        return Ok(declared);
    }

    // Reuse the provider-policy sibling's authority/digest verification before
    // trusting the gate-specific projection from the same immutable Job.
    approved_sandbox_toml(state)?.ok_or_else(|| {
        DeadreckonError::InvalidInput(
            "strict Job is missing its immutable execution policy".to_string(),
        )
    })?;
    let paths = paths_for_state(state)?;
    let job = deadreckon_core::load_job(&paths, &state.run_id)?;
    let approved = job
        .policy
        .execution
        .as_ref()
        .ok_or_else(|| {
            DeadreckonError::InvalidInput(
                "strict Job predates immutable execution policy".to_string(),
            )
        })?
        .gate
        .network;
    if approved != declared {
        return Err(DeadreckonError::InvalidInput(format!(
            "approved gate network policy {approved:?} does not match frozen done contract {declared:?}"
        )));
    }
    Ok(approved)
}

fn approved_sandbox_toml(state: &PipelineState) -> Result<Option<SandboxToml>> {
    let paths = paths_for_state(state)?;
    if !paths.job_json(&state.run_id).is_file() {
        return Ok(None);
    }
    let job = deadreckon_core::load_job(&paths, &state.run_id)?;
    let authority_path = paths.job_authority(&state.run_id);
    if deadreckon_core::flight::sha256_file(&authority_path)? != job.authority_sha256 {
        return Err(DeadreckonError::InvalidInput(
            "Job authority changed before execution policy materialization".to_string(),
        ));
    }
    let authority_raw = std::fs::read(&authority_path).with_path(&authority_path)?;
    let authority: JobAuthority =
        serde_json::from_slice(&authority_raw).map_err(|source| DeadreckonError::Json {
            path: authority_path,
            source,
        })?;
    let policy_raw =
        serde_json::to_string(&job.policy).map_err(|source| DeadreckonError::Json {
            path: paths.job_json(&state.run_id),
            source,
        })?;
    if deadreckon_core::flight::sha256_text(&policy_raw) != authority.effective_policy_sha256 {
        return Err(DeadreckonError::InvalidInput(
            "Job execution policy no longer matches approved authority".to_string(),
        ));
    }
    let execution = job.policy.execution.as_ref().ok_or_else(|| {
        DeadreckonError::InvalidInput(
            "Job predates immutable execution policy; refusing unattended execution".to_string(),
        )
    })?;
    if !execution.require_containment || execution.sandbox_requested != authority.sandbox_requested
    {
        return Err(DeadreckonError::InvalidInput(
            "Job containment policy no longer matches approved authority".to_string(),
        ));
    }
    let tools = execution
        .tools
        .iter()
        .map(|(name, tool)| {
            (
                name.clone(),
                SandboxTomlTool {
                    read: tool
                        .workspace_read
                        .then(|| state.working_dir.clone())
                        .into_iter()
                        .collect(),
                    write: tool
                        .workspace_write
                        .then(|| state.working_dir.clone())
                        .into_iter()
                        .collect(),
                    network: tool.network_allowlist.clone(),
                },
            )
        })
        .collect();
    Ok(Some(SandboxToml { version: 1, tools }))
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

fn apply_provider_capability_posture(
    request: &mut ProviderRequest,
    state: &PipelineState,
    home: &Path,
) -> Result<()> {
    let command_policy = load_tool_policy_from_sandbox_toml(state, "bash")?;
    let file_policy = load_tool_policy_from_sandbox_toml(state, "write_file")?;
    let preview = capability_preview_for_run(state, home);
    let network = if command_policy
        .network_allowlist
        .iter()
        .any(|host| host == "*")
    {
        deadreckon_core::NetworkCapability::Full
    } else if command_policy.network_allowlist.is_empty() {
        preview.network
    } else {
        deadreckon_core::NetworkCapability::Allowlist
    };
    let working_dir = state.working_dir.clone();
    let mut additional_write_roots = command_policy.write_allowlist;
    additional_write_roots.extend(file_policy.write_allowlist);
    additional_write_roots.retain(|root| root != &working_dir);
    additional_write_roots.sort();
    additional_write_roots.dedup();
    request.set_capability_posture(
        network,
        command_policy.network_allowlist,
        preview.deploy,
        preview.global_install,
        working_dir,
        additional_write_roots,
    );
    Ok(())
}

fn capability_preview_for_run(
    state: &PipelineState,
    home: &Path,
) -> deadreckon_core::CapabilityPreview {
    let marker_path = state
        .working_dir
        .join(deadreckon_core::PLAN_CHILD_PARENT_JSON);
    let Ok(raw) = std::fs::read_to_string(marker_path) else {
        return deadreckon_core::CapabilityPreview::default();
    };
    let Ok(marker) = serde_json::from_str::<deadreckon_core::PlanChildMarker>(&raw) else {
        return deadreckon_core::CapabilityPreview::default();
    };
    let paths = DeadreckonPaths::from_home(home.to_path_buf());
    let Ok(raw) = std::fs::read_to_string(paths.plan_json(&marker.parent_plan_id)) else {
        return deadreckon_core::CapabilityPreview::default();
    };
    serde_json::from_str::<deadreckon_core::Plan>(&raw)
        .map(|plan| plan.capability_preview)
        .unwrap_or_default()
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

// Seam refusal evidence is assembled from explicit, independently reviewed
// authority inputs rather than an opaque context bag.
#[allow(clippy::too_many_arguments)]
async fn policy_seam_refusal(
    seams: &SeamsConfig,
    ctx: &SeamRunCtx,
    run_id: &str,
    function_id: &str,
    command: &str,
    working_dir: &Path,
    deadline: ProviderPhaseDeadline,
    cancellation: &CancellationToken,
) -> Result<SeamPhaseOutcome<Option<String>>> {
    let outcome = dispatch_seam_phase(
        SeamKind::Policy,
        &json!({
            "function_id": function_id,
            "command": command,
            "working_dir": working_dir,
        }),
        seams,
        ctx,
        deadline,
        cancellation,
    )
    .await;
    outcome
        .map(|outcome| match outcome {
            SeamOutcome::Deny(reason) => Ok(Some(policy_seam_refusal_message(
                run_id,
                function_id,
                &reason,
            ))),
            SeamOutcome::Ok(_) | SeamOutcome::Unconfigured | SeamOutcome::Fallback => Ok(None),
            SeamOutcome::Skipped(reason) => Ok(Some(policy_seam_refusal_message(
                run_id,
                function_id,
                &format!("unexpected skipped outcome: {reason}"),
            ))),
            SeamOutcome::LostContainment(reason) => {
                Err(lost_containment_error(SeamKind::Policy, &reason))
            }
        })
        .transpose()
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
    deadline: ProviderPhaseDeadline,
    cancellation: &CancellationToken,
) -> Result<SeamPhaseOutcome<()>> {
    emit_event(state, sender, event.clone())?;
    dispatch_hook_event(seams, ctx, &event, deadline, cancellation).await
}

async fn dispatch_hook_event(
    seams: &SeamsConfig,
    ctx: &SeamRunCtx,
    event: &RunEventKind,
    deadline: ProviderPhaseDeadline,
    cancellation: &CancellationToken,
) -> Result<SeamPhaseOutcome<()>> {
    let Ok(req) = serde_json::to_value(event) else {
        return Ok(SeamPhaseOutcome::Completed(()));
    };
    dispatch_seam_phase(SeamKind::Hooks, &req, seams, ctx, deadline, cancellation)
        .await
        .map(|outcome| match outcome {
            SeamOutcome::LostContainment(reason) => {
                Err(lost_containment_error(SeamKind::Hooks, &reason))
            }
            _ => Ok(()),
        })
        .transpose()
}

fn event_sink_must_stop(outcome: &SeamOutcome) -> bool {
    matches!(outcome, SeamOutcome::LostContainment(_))
}

struct EventSinkForwarder {
    force_shutdown: CancellationToken,
    graceful_shutdown: Option<oneshot::Sender<()>>,
    handle: tokio::task::JoinHandle<()>,
}

impl EventSinkForwarder {
    async fn shutdown(mut self, cleanup_budget: Duration) {
        if let Some(shutdown) = self.graceful_shutdown.take() {
            let _ = shutdown.send(());
        }
        if tokio::time::timeout(EVENT_SINK_DRAIN_GRACE, &mut self.handle)
            .await
            .is_err()
        {
            self.force_shutdown.cancel();
            if tokio::time::timeout(cleanup_budget, &mut self.handle)
                .await
                .is_err()
            {
                self.handle.abort();
                let _ = tokio::time::timeout(Duration::from_millis(100), &mut self.handle).await;
            }
        }
    }
}

impl Drop for EventSinkForwarder {
    fn drop(&mut self) {
        self.force_shutdown.cancel();
        self.handle.abort();
    }
}

async fn forward_event_to_sink(
    event: RunEvent,
    seams: &SeamsConfig,
    ctx: &SeamRunCtx,
    deadline: ProviderPhaseDeadline,
    cancellation: &CancellationToken,
) -> bool {
    let Ok(req) = serde_json::to_value(event) else {
        return false;
    };
    match dispatch_seam_phase(
        SeamKind::EventSink,
        &req,
        seams,
        ctx,
        deadline,
        cancellation,
    )
    .await
    {
        SeamPhaseOutcome::Completed(outcome) => event_sink_must_stop(&outcome),
        SeamPhaseOutcome::WorkExpired { .. } | SeamPhaseOutcome::Cancelled { .. } => true,
    }
}

fn spawn_event_sink_forwarder(
    seams: SeamsConfig,
    ctx: SeamRunCtx,
    sender: &broadcast::Sender<RunEvent>,
    deadline: ProviderPhaseDeadline,
    run_cancellation: &CancellationToken,
) -> EventSinkForwarder {
    let mut receiver = sender.subscribe();
    let force_shutdown = run_cancellation.child_token();
    let task_cancellation = force_shutdown.clone();
    let (graceful_shutdown, mut graceful_receiver) = oneshot::channel();
    let handle = tokio::spawn(async move {
        loop {
            let received = tokio::select! {
                biased;
                () = task_cancellation.cancelled() => break,
                _ = &mut graceful_receiver => {
                    while let Ok(event) = receiver.try_recv() {
                        if forward_event_to_sink(
                            event,
                            &seams,
                            &ctx,
                            deadline,
                            &task_cancellation,
                        ).await {
                            break;
                        }
                    }
                    break;
                },
                received = receiver.recv() => received,
            };
            match received {
                Ok(event) => {
                    if forward_event_to_sink(event, &seams, &ctx, deadline, &task_cancellation)
                        .await
                    {
                        // The detached observer cannot safely dispatch another
                        // worker while authority from this one remains live.
                        // Its durable process record is left for supervisor or
                        // operator reconciliation.
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    EventSinkForwarder {
        force_shutdown,
        graceful_shutdown: Some(graceful_shutdown),
        handle,
    }
}

fn changed_files_since_snapshot(state: &PipelineState, snapshot_turn: u32) -> Result<Vec<PathBuf>> {
    let snapshot_dir = state
        .run_root
        .join("snapshots")
        .join(format!("turn-{snapshot_turn}"));
    let before = file_set(&snapshot_dir)?;
    let after_files = inventory_recoverable_files_for_state(state)?;
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

fn deliverable_changed_files(state: &PipelineState, changed: &[PathBuf]) -> Result<Vec<PathBuf>> {
    changed
        .iter()
        .filter_map(|path| {
            let relative = path.strip_prefix(&state.working_dir).map_err(|err| {
                DeadreckonError::InvalidInput(format!(
                    "changed path {} is outside working directory {}: {err}",
                    path.display(),
                    state.working_dir.display()
                ))
            });
            match relative {
                Ok(relative) if is_deliverable_workspace_path(relative) => Some(Ok(path.clone())),
                Ok(_) => None,
                Err(err) => Some(Err(err)),
            }
        })
        .collect()
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

/// C-P12: write the inert reshape proposal into the run root in the
/// launch-plan schema (parent set, accepted_by ABSENT — inert by
/// construction) and trace the event. The proposal never executes here.
fn record_reshape_proposal(
    state: &PipelineState,
    turn: u32,
    pieces: &[ReshapePieceDraft],
) -> Result<PathBuf> {
    let path = state.run_root.join("reshape-proposal.json");
    let n = pieces.len().clamp(2, 6) as u8;
    let piece_values: Vec<serde_json::Value> = pieces
        .iter()
        .enumerate()
        .map(|(idx, piece)| {
            json!({
                "id": format!("p{}", idx + 1),
                "goal": piece.goal,
                "done_hint": piece.done_hint,
            })
        })
        .collect();
    let proposal = json!({
        "schema": 1,
        "created_at": Utc::now().to_rfc3339(),
        "goal": state.goal,
        "shape": "plan",
        "n": n,
        "pieces": piece_values,
        "resolution": {
            "source": "provider",
            "confidence": 0.6,
            "rationale": format!("worker proposed decomposition at turn {turn}"),
        },
        "parent": state.run_id,
    });
    let bytes = serde_json::to_vec_pretty(&proposal).map_err(|source| {
        deadreckon_core::DeadreckonError::Json {
            path: path.clone(),
            source,
        }
    })?;
    std::fs::write(&path, bytes).map_err(|source| deadreckon_core::DeadreckonError::Io {
        path: path.clone(),
        source,
    })?;
    append_trace(
        state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn,
            event: "reshape.proposed".to_string(),
            latency_ms: None,
            detail: json!({
                "pieces": pieces.len(),
                "path": path.display().to_string(),
            }),
        },
    )?;
    Ok(path)
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
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent).with_path(parent)?;
    std::io::Write::write_all(&mut temp, &data).with_path(&path)?;
    temp.as_file_mut().sync_all().with_path(&path)?;
    temp.persist(&path).map_err(|err| DeadreckonError::Io {
        path: path.clone(),
        source: err.error,
    })?;
    Ok(())
}

fn history_path(state: &PipelineState) -> PathBuf {
    state.run_root.join("history.json")
}

// stderr is the only surface available this early in resume: the flight
// recorder for the run is not open yet when history.json proves unreadable.
#[allow(clippy::print_stderr)]
fn load_or_reconstruct_history(
    state: &mut PipelineState,
    from_turn: Option<u32>,
    work_clock: &RunWorkClock,
) -> Result<Vec<String>> {
    let trace_reconstruction = reconstruct_history_from_traces(state)?;
    let loaded = if history_path(state).exists() {
        match load_history(state) {
            Ok(history) => Some(history),
            Err(error) => {
                eprintln!(
                    "warning: {} is unreadable ({error}); reconstructing history from traces.jsonl",
                    history_path(state).display()
                );
                None
            }
        }
    } else {
        None
    };
    let recovered_from_traces = loaded.is_none();
    let mut history = loaded.unwrap_or(trace_reconstruction.history);
    if let Some(from_turn) = from_turn {
        history.truncate(from_turn as usize);
        state.turn = from_turn;
        truncate_run_artifacts_after_turn(state, from_turn)?;
        save_history(state, &history)?;
        work_clock.save(state)?;
    } else if recovered_from_traces {
        if trace_reconstruction.last_complete_turn > state.turn {
            state.turn = trace_reconstruction.last_complete_turn;
        }
        save_history(state, &history)?;
        work_clock.save(state)?;
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
    let snapshot_manifests = state
        .run_root
        .join(deadreckon_core::SNAPSHOT_CAPTURE_MANIFESTS_DIR);
    if let Ok(entries) = std::fs::read_dir(&snapshot_manifests) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(turn) = path
                .file_stem()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("turn-"))
                .and_then(|turn| turn.parse::<u32>().ok())
            else {
                continue;
            };
            if turn > from_turn {
                std::fs::remove_file(&path).with_path(&path)?;
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

fn run_work_boundary_scope(
    state: &PipelineState,
    phase_deadline: ProviderPhaseDeadline,
    cancellation_token: &CancellationToken,
    operation: &str,
) -> deadreckon_core::git::WorkBoundaryScope {
    let cancellation = cancellation_token.clone();
    deadreckon_core::git::WorkBoundaryScope::new(
        phase_deadline.work_expires_at.into_std(),
        phase_deadline.cleanup_budget,
        move || cancellation.is_cancelled(),
        operation,
    )
    .with_authority_dir(state.run_root.join("child-pids"))
}

fn snapshot_working_bounded(
    state: &PipelineState,
    turn: u32,
    phase_deadline: ProviderPhaseDeadline,
    cancellation_token: &CancellationToken,
) -> Result<()> {
    let scope = run_work_boundary_scope(
        state,
        phase_deadline,
        cancellation_token,
        "workspace snapshot",
    );
    deadreckon_core::git::with_git_command_scope(scope, || snapshot_working(state, turn))
        .map(|_| ())
}

fn promote_if_ready(
    state: &mut PipelineState,
    phase_deadline: ProviderPhaseDeadline,
    cancellation_token: &CancellationToken,
) -> Result<()> {
    let paths = paths_for_state(state)?;
    let scope = run_work_boundary_scope(
        state,
        phase_deadline,
        cancellation_token,
        "result promotion",
    );
    promote_completed_run_bounded(&paths, state, scope).map(|_| ())
}

// Settlement keeps the mutable run artifacts explicit so no phase authority
// is accidentally retained in a shared aggregate.
#[allow(clippy::too_many_arguments)]
fn settle_local_work_boundary(
    state: &mut PipelineState,
    turn: u32,
    history: &mut Vec<String>,
    error: &DeadreckonError,
    phase: PhaseId,
    context: &str,
    sender: Option<&broadcast::Sender<RunEvent>>,
    work_clock: &RunWorkClock,
) -> Result<Option<RunLoopOutcome>> {
    let DeadreckonError::ProcessBoundary {
        kind,
        authority,
        detail,
        ..
    } = error
    else {
        return Ok(None);
    };
    let outcome = match kind {
        deadreckon_core::ProcessBoundaryKind::WorkExpired => {
            let reason = format!(
                "{} during {context} (approved Job work cutoff)",
                work_clock.expiry().reached_label()
            );
            state.pause_reason = Some(reason.clone());
            state.failure_reason = Some(reason);
            state.set_phase_status(phase, PhaseStatus::Failed)?;
            work_clock.save(state)?;
            RunLoopOutcome::PausedAtCap
        }
        deadreckon_core::ProcessBoundaryKind::Cancelled => {
            state.status = RunStatus::Killed;
            state.failure_reason = Some(format!("operator cancelled during {context}"));
            state.set_phase_status(phase, PhaseStatus::Failed)?;
            work_clock.save(state)?;
            RunLoopOutcome::Killed
        }
        deadreckon_core::ProcessBoundaryKind::CleanupIncomplete => {
            let reason = format!(
                "{context} lost process containment{}: {detail}",
                authority
                    .as_deref()
                    .map(|path| format!("; authority retained at {}", path.display()))
                    .unwrap_or_default()
            );
            record_semantic_lost_containment(state, turn, history, &reason, work_clock)?;
            state.set_phase_status(phase, PhaseStatus::Failed)?;
            RunLoopOutcome::Failed
        }
        deadreckon_core::ProcessBoundaryKind::SupervisionFailed => {
            record_needs_review(
                state,
                turn,
                history,
                &format!(
                    "{context} subprocess supervision failed after cleanup was proven: {detail}"
                ),
                work_clock,
            )?;
            state.set_phase_status(phase, PhaseStatus::Failed)?;
            RunLoopOutcome::Failed
        }
    };
    work_clock.save(state)?;
    emit_run_completed(state, sender, outcome.clone())?;
    Ok(Some(outcome))
}

async fn complete_run_docs(
    state: &mut PipelineState,
    router: &ProviderRouter,
    config: &RunLoopConfig,
    work_clock: &RunWorkClock,
    phase_deadline: ProviderPhaseDeadline,
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
            sandbox_backend: config.sandbox_backend,
            commit_docs: false,
            no_llm: config.docs.no_docs,
            force: false,
            max_wall_seconds: work_clock.remaining_seconds(config.max_wall_seconds)?,
            phase_deadline: Some(phase_deadline),
            cancellation_token: config.cancellation_token.clone(),
        },
    )
    .await;
    state.status = previous_status;
    work_clock.save(state)?;
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

/// Execute the real `dr-gate` binary for an already-materialized result.
///
/// This is also used by the durable graph supervisor after it has copied the
/// merged artifact into the parent job's result run. It intentionally does
/// not expose marker construction: only `dr-gate` receives the signing key.
pub async fn run_deterministic_completion_gate(
    state: &PipelineState,
    requested_backend: SandboxBackend,
    launch_owner: Option<&GateLaunchOwner>,
    cancellation_token: Option<&CancellationToken>,
) -> Result<()> {
    if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(DeadreckonError::InvalidInput(
            "deterministic completion gate cancelled before evaluation".to_string(),
        ));
    }
    let paths = paths_for_state(state)?;
    let strict_job = paths.job_json(&state.run_id).is_file();
    let resolved_backend = if strict_job {
        let (resolved, _) = resolve_backend(requested_backend).map_err(|error| {
            DeadreckonError::InvalidInput(format!(
                "strict gate requires an available sandbox backend ({error})"
            ))
        })?;
        if resolved == SandboxBackend::None {
            return Err(DeadreckonError::InvalidInput(
                "strict gate requires an available containment backend".to_string(),
            ));
        }
        resolved
    } else {
        requested_backend
    };
    let launch_owner = if strict_job {
        Some(launch_owner.ok_or_else(|| {
            DeadreckonError::InvalidInput(
                "strict gate is missing its durable outer launch identity".to_string(),
            )
        })?)
    } else {
        None
    };
    let gate_toolchain = resolve_gate_toolchain(state, &paths, strict_job, resolved_backend)?;

    // Default detection and contract persistence are trusted-controller work.
    // The keyless evaluator is deliberately read-only outside `working_dir`
    // and refuses to create or replace this approved input.
    deadreckon_core::gate::compiled_acceptance_checks(&state.run_root, &state.working_dir)?;
    let gate_network_access = approved_gate_network_access(state, strict_job)?;

    let boundary_observation = if let Some(launch_owner) = launch_owner {
        Some(
            run_strict_sandbox_boundary_probe(
                state,
                &paths,
                resolved_backend,
                launch_owner,
                &gate_toolchain,
                cancellation_token,
            )
            .await?,
        )
    } else {
        None
    };
    let (pid_file, guarded_launch) = if let Some(launch_owner) = launch_owner {
        let launch_id = Uuid::new_v4().to_string();
        (
            state.run_root.join("child-pids").join(format!(
                "dr-gate-evaluate-{}-{launch_id}.json",
                launch_owner.attempt
            )),
            Some(GuardedLaunchSpec {
                program: gate_toolchain.controller.clone().into_os_string(),
                launch_id,
                attempt: launch_owner.attempt,
                owner_launch_id: Some(launch_owner.outer_launch_id.clone()),
            }),
        )
    } else {
        (
            state
                .run_root
                .join("child-pids")
                .join("dr-gate-evaluate.pid"),
            None,
        )
    };

    let evaluation = run_keyless_gate_evaluation(
        state,
        &state.working_dir,
        resolved_backend,
        pid_file,
        guarded_launch,
        strict_job,
        strict_job,
        gate_network_access,
        Some(&gate_toolchain),
        cancellation_token,
    )
    .await?;
    if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(DeadreckonError::InvalidInput(
            "deterministic completion gate cancelled before signing".to_string(),
        ));
    }

    // Read the key only after the sandbox runner has reaped the evaluator and
    // terminated residual descendants. Both signing phases execute no
    // repository-controlled checks.
    if let Some(observation) = boundary_observation.as_ref() {
        let authority_path = paths.job_authority(&state.run_id);
        let authority_raw = std::fs::read(&authority_path).with_path(&authority_path)?;
        let authority: JobAuthority =
            serde_json::from_slice(&authority_raw).map_err(|source| DeadreckonError::Json {
                path: authority_path,
                source,
            })?;
        deadreckon_core::seal_sandbox_boundary_observation(&paths, state, &authority, observation)?;
    }
    revalidate_gate_toolchain(&gate_toolchain)?;
    let gate_key = deadreckon_core::read_gate_key(&paths, &state.run_id)?;
    let containment = deadreckon_core::gate::AcceptanceContainment {
        contained: evaluation.contained,
        sandbox_backend: evaluation.backend.to_string(),
    };
    // Signing executes trusted in-process code only after every evaluator has
    // returned and its process-tree cleanup has been proven. This removes an
    // otherwise unbounded signer subprocess while preserving the key boundary:
    // repository-controlled checks never run in a process that holds the key.
    deadreckon_core::sign_gate_evaluation_with_key(
        &state.run_root,
        &state.run_id,
        &state.working_dir,
        evaluation.evaluation,
        &gate_key,
        containment,
    )?;
    Ok(())
}

const STRICT_SANDBOX_BOUNDARY_PROBE: &str = "dr-gate probe-boundary v1";
const STRICT_SANDBOX_BOUNDARY_SUCCESS: &str = "deadreckon-sandbox-boundary-v1";

async fn run_strict_sandbox_boundary_probe(
    state: &PipelineState,
    paths: &DeadreckonPaths,
    resolved_backend: SandboxBackend,
    launch_owner: &GateLaunchOwner,
    gate_toolchain: &ResolvedGateToolchain,
    cancellation_token: Option<&CancellationToken>,
) -> Result<SandboxBoundaryObservation> {
    if resolved_backend == SandboxBackend::None || resolved_backend == SandboxBackend::Auto {
        return Err(DeadreckonError::InvalidInput(
            "strict sandbox boundary probe requires one resolved containment backend".to_string(),
        ));
    }
    if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(DeadreckonError::InvalidInput(
            "strict sandbox boundary probe cancelled before launch".to_string(),
        ));
    }

    let probe_id = Uuid::new_v4().to_string();
    let proof_sentinel = state
        .run_root
        .join("proofs")
        .join("sandbox-boundary-probes")
        .join(format!("{probe_id}.proof"));
    let control_sentinel = paths
        .job_dir(&state.run_id)
        .join("sandbox-boundary-probes")
        .join(format!("{probe_id}.control"));
    let operator_capture_sentinel = paths
        .operator_captures_dir()
        .join("sandbox-boundary-probes")
        .join(&state.run_id)
        .join(format!("{probe_id}.capture"));
    let sentinel = format!("deadreckon-controller-probe:{probe_id}\n");
    create_probe_sentinel(&proof_sentinel, sentinel.as_bytes())?;
    create_probe_sentinel(&control_sentinel, sentinel.as_bytes())?;
    create_probe_sentinel(&operator_capture_sentinel, sentinel.as_bytes())?;

    let runtime = TempDir::new().with_path(PathBuf::from("sandbox-boundary-runtime"))?;
    let mut env = BTreeMap::new();
    let mut read_allowlist = vec![
        state.run_root.clone(),
        paths.job_dir(&state.run_id),
        paths.home().join("gate-keys"),
        paths.operator_captures_dir(),
    ];
    let mut write_allowlist = vec![state.working_dir.clone()];
    prepare_strict_gate_environment(
        &state.working_dir,
        runtime.path(),
        resolved_backend,
        &mut env,
        &mut read_allowlist,
        &mut write_allowlist,
    )?;
    for (name, path) in [
        (
            "DR_BOUNDARY_GATE_KEY",
            deadreckon_core::gate_key_path(paths, &state.run_id),
        ),
        ("DR_BOUNDARY_PROOF", proof_sentinel.clone()),
        ("DR_BOUNDARY_CONTROL", control_sentinel.clone()),
        (
            "DR_BOUNDARY_OPERATOR_CAPTURE",
            operator_capture_sentinel.clone(),
        ),
    ] {
        env.insert(name.to_string(), path.to_string_lossy().into_owned());
    }
    for (name, value) in [
        (deadreckon_core::GATE_KEY_ENV, "must-not-cross"),
        (deadreckon_core::GATE_CONTAINED_ENV, "must-not-cross"),
        (deadreckon_core::GATE_SANDBOX_BACKEND_ENV, "must-not-cross"),
    ] {
        env.insert(name.to_string(), value.to_string());
    }
    read_allowlist.sort();
    read_allowlist.dedup();
    write_allowlist.sort();
    write_allowlist.dedup();

    let mut boundary = ProtectedPathPolicy::for_paths(paths);
    boundary.protect_workspace_git_control(&state.working_dir);
    revalidate_gate_toolchain(gate_toolchain)?;
    let guard = gate_toolchain.controller.clone();
    let probe_launch_id = Uuid::new_v4().to_string();
    let pid_file = state.run_root.join("child-pids").join(format!(
        "sandbox-boundary-probe-{}-{probe_launch_id}.json",
        launch_owner.attempt
    ));
    let docker = prepare_gate_docker(
        state,
        paths,
        gate_toolchain,
        resolved_backend,
        "gate-probe",
        &probe_launch_id,
        launch_owner.attempt,
        Some(launch_owner.outer_launch_id.clone()),
        &pid_file,
    )?;
    let output = run_sandbox(SandboxSpec {
        backend: resolved_backend,
        docker: docker.as_ref().map(|prepared| prepared.execution.clone()),
        cwd: state.working_dir.clone(),
        program: gate_toolchain.evaluator.clone().into_os_string(),
        args: vec![OsString::from("probe-boundary")],
        stdin: None,
        env,
        allow_network: false,
        pid_file: Some(pid_file),
        cancellation_token: cancellation_token.cloned(),
        profile_dir: None,
        read_allowlist,
        write_allowlist,
        read_denylist: boundary.read_denylist,
        write_denylist: boundary.write_denylist,
        network_allowlist: Vec::new(),
        workspace_access: WorkspaceAccess::Disposable,
        cleanup_process_group: true,
        guarded_launch: Some(GuardedLaunchSpec {
            program: guard.into_os_string(),
            launch_id: probe_launch_id,
            attempt: launch_owner.attempt,
            owner_launch_id: Some(launch_owner.outer_launch_id.clone()),
        }),
    })
    .await
    .map_err(|error| sandbox_error(&error));
    finish_gate_docker(docker.as_ref())?;
    let output = output?;

    let proof_unchanged = read_probe_sentinel(&proof_sentinel)? == sentinel.as_bytes();
    let control_unchanged = read_probe_sentinel(&control_sentinel)? == sentinel.as_bytes();
    let operator_capture_unchanged =
        read_probe_sentinel(&operator_capture_sentinel)? == sentinel.as_bytes();
    if output.backend != resolved_backend
        || output.status_code != Some(0)
        || output.stdout != STRICT_SANDBOX_BOUNDARY_SUCCESS
        || !proof_unchanged
        || !control_unchanged
        || !operator_capture_unchanged
    {
        return Err(DeadreckonError::InvalidInput(format!(
            "strict sandbox boundary probe failed under {} with status {:?}: stdout: {} stderr: {}",
            output.backend, output.status_code, output.stdout, output.stderr
        )));
    }

    let authority_path = paths.job_authority(&state.run_id);
    let authority_raw = std::fs::read(&authority_path).with_path(&authority_path)?;
    let authority: JobAuthority =
        serde_json::from_slice(&authority_raw).map_err(|source| DeadreckonError::Json {
            path: authority_path.clone(),
            source,
        })?;
    Ok(SandboxBoundaryObservation {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: authority.job_id.clone(),
        run_id: authority.run_id.clone(),
        observed_at: Utc::now(),
        issuer: SandboxBoundaryObservationIssuer::DeadreckonController,
        probe_id,
        attempt: launch_owner.attempt,
        outer_launch_id: launch_owner.outer_launch_id.clone(),
        authority_sha256: deadreckon_core::flight::sha256_file(&authority_path)?,
        contract_sha256: deadreckon_core::flight::sha256_file(&acceptance_spec_path_for_run_root(
            &state.run_root,
        ))?,
        result_tree_sha256: deadreckon_core::sandbox_boundary_result_tree_sha256(state)?,
        sandbox_requested: authority.sandbox_requested,
        sandbox_backend: output.backend.to_string(),
        gate_evaluator_sha256: gate_toolchain.identity_sha256.clone(),
        contained: true,
        gate_key_read_denied: true,
        proof_write_denied: proof_unchanged,
        control_write_denied: control_unchanged,
        operator_capture_read_denied: true,
        operator_capture_write_denied: operator_capture_unchanged,
        signing_env_scrubbed: true,
        probe_sha256: deadreckon_core::flight::sha256_text(STRICT_SANDBOX_BOUNDARY_PROBE),
        signature: String::new(),
    })
}

fn create_probe_sentinel(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "sandbox boundary sentinel has no parent: {}",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent).with_path(parent)?;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .with_path(path)?;
    file.write_all(bytes).with_path(path)?;
    file.sync_all().with_path(path)
}

fn read_probe_sentinel(path: &Path) -> Result<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(path).with_path(path)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(DeadreckonError::InvalidInput(format!(
            "sandbox boundary sentinel is not a regular non-symlink file: {}",
            path.display()
        )));
    }
    std::fs::read(path).with_path(path)
}

#[derive(Debug)]
struct KeylessGateEvaluation {
    evaluation: deadreckon_core::GateEvaluation,
    backend: SandboxBackend,
    contained: bool,
}

#[allow(clippy::too_many_arguments)]
async fn run_keyless_gate_evaluation(
    state: &PipelineState,
    working_dir: &Path,
    requested_backend: SandboxBackend,
    pid_file: PathBuf,
    guarded_launch: Option<GuardedLaunchSpec>,
    cleanup_process_group: bool,
    require_contained: bool,
    network_access: JobGateNetworkAccess,
    gate_toolchain: Option<&ResolvedGateToolchain>,
    cancellation_token: Option<&CancellationToken>,
) -> Result<KeylessGateEvaluation> {
    let fallback_gate;
    let (gate, gate_evaluator_sha256) = if let Some(toolchain) = gate_toolchain {
        revalidate_gate_toolchain(toolchain)?;
        (
            toolchain.evaluator.as_path(),
            toolchain.identity_sha256.as_deref(),
        )
    } else {
        fallback_gate = gate_binary_path()?;
        (fallback_gate.as_path(), None)
    };
    let paths = paths_for_state(state)?;
    let disposable_copy = require_contained && working_dir != state.working_dir;
    let evaluation_runtime = if require_contained {
        Some(
            tempfile::Builder::new()
                .prefix("deadreckon-gate-")
                .tempdir()
                .with_path(PathBuf::from("gate-runtime"))?,
        )
    } else {
        None
    };
    let mut env = BTreeMap::new();
    let mut boundary = ProtectedPathPolicy::for_paths(&paths);
    boundary.protect_workspace_git_control(working_dir);
    if disposable_copy {
        protect_recorded_workspaces(&paths, state, working_dir, &mut boundary)?;
    }
    let mut read_allowlist = vec![state.run_root.clone()];
    let mut write_allowlist = vec![working_dir.to_path_buf()];
    if let Some(parent) = gate.parent() {
        read_allowlist.push(parent.to_path_buf());
    }
    if let Some(runtime) = evaluation_runtime.as_ref() {
        prepare_strict_gate_environment(
            working_dir,
            runtime.path(),
            requested_backend,
            &mut env,
            &mut read_allowlist,
            &mut write_allowlist,
        )?;
        read_allowlist.push(runtime.path().to_path_buf());
    } else {
        extend_gate_toolchain_reads(&mut read_allowlist);
    }
    read_allowlist.sort();
    read_allowlist.dedup();
    write_allowlist.sort();
    write_allowlist.dedup();
    let mut gate_args = vec![
        "evaluate".into(),
        "--run".into(),
        state.run_id.clone().into(),
        "--run-root".into(),
        state.run_root.clone().into_os_string(),
        "--working-dir".into(),
        working_dir.to_path_buf().into_os_string(),
    ];
    if let Some(identity) = gate_evaluator_sha256 {
        gate_args.push("--gate-evaluator-sha256".into());
        gate_args.push(identity.into());
    }
    let docker = if requested_backend == SandboxBackend::Docker {
        let toolchain = gate_toolchain.ok_or_else(|| {
            DeadreckonError::InvalidInput(
                "Docker verdict evaluation requires a frozen Job evaluator; no check was run"
                    .to_string(),
            )
        })?;
        let guard = guarded_launch.as_ref().ok_or_else(|| {
            DeadreckonError::InvalidInput(
                "strict Docker evaluation is missing guarded launch identity".to_string(),
            )
        })?;
        prepare_gate_docker(
            state,
            &paths,
            toolchain,
            requested_backend,
            "gate-evaluate",
            &guard.launch_id,
            guard.attempt,
            guard.owner_launch_id.clone(),
            &pid_file,
        )?
    } else {
        None
    };
    let (allow_network, network_allowlist) = match network_access {
        JobGateNetworkAccess::Deny => (false, Vec::new()),
        JobGateNetworkAccess::Loopback => (
            true,
            vec![
                "127.0.0.1".to_string(),
                "localhost".to_string(),
                "::1".to_string(),
            ],
        ),
        JobGateNetworkAccess::Full => (true, vec!["*".to_string()]),
    };
    let output = run_sandbox(SandboxSpec {
        backend: requested_backend,
        docker: docker.as_ref().map(|prepared| prepared.execution.clone()),
        cwd: working_dir.to_path_buf(),
        program: gate.as_os_str().to_os_string(),
        args: gate_args,
        stdin: None,
        env,
        allow_network,
        pid_file: Some(pid_file),
        cancellation_token: cancellation_token.cloned(),
        profile_dir: None,
        read_allowlist,
        write_allowlist,
        read_denylist: boundary.read_denylist,
        write_denylist: boundary.write_denylist,
        network_allowlist,
        workspace_access: if require_contained {
            WorkspaceAccess::Disposable
        } else {
            WorkspaceAccess::ReadWrite
        },
        cleanup_process_group,
        guarded_launch,
    })
    .await
    .map_err(|err| sandbox_error(&err));
    finish_gate_docker(docker.as_ref())?;
    let output = output?;
    if output.status_code != Some(0) {
        return Err(DeadreckonError::InvalidInput(format!(
            "sandboxed dr-gate evaluation failed with status {:?} under {}: stdout: {} stderr: {}",
            output.status_code, output.backend, output.stdout, output.stderr
        )));
    }
    let contained = output.backend != SandboxBackend::None;
    if !contained && require_contained {
        return Err(DeadreckonError::InvalidInput(
            "dr-gate evaluation was not contained; independent verification refused".to_string(),
        ));
    }
    let evaluation: deadreckon_core::GateEvaluation = serde_json::from_str(&output.stdout)
        .map_err(|source| DeadreckonError::Json {
            path: PathBuf::from("dr-gate-evaluation-stdout"),
            source,
        })?;
    deadreckon_core::validate_gate_evaluation_integrity(
        &state.run_id,
        &state.run_root,
        working_dir,
        &evaluation,
    )?;
    if evaluation.gate_evaluator_sha256.as_deref() != gate_evaluator_sha256 {
        return Err(DeadreckonError::InvalidInput(format!(
            "sandboxed dr-gate evaluator identity {:?} does not match approved identity {:?}",
            evaluation.gate_evaluator_sha256, gate_evaluator_sha256
        )));
    }
    Ok(KeylessGateEvaluation {
        evaluation,
        backend: output.backend,
        contained,
    })
}

fn protect_recorded_workspaces(
    paths: &DeadreckonPaths,
    state: &PipelineState,
    evaluated_working_dir: &Path,
    boundary: &mut ProtectedPathPolicy,
) -> Result<()> {
    let evaluated = evaluated_working_dir
        .canonicalize()
        .unwrap_or_else(|_| evaluated_working_dir.to_path_buf());
    protect_workspace_except(&state.working_dir, &evaluated, boundary);
    for entry in deadreckon_core::list_runs(paths, None)? {
        let state = deadreckon_core::load_run(paths, &entry.run_id)?;
        protect_workspace_except(&state.working_dir, &evaluated, boundary);
    }
    boundary.write_denylist.sort();
    boundary.write_denylist.dedup();
    Ok(())
}

fn protect_workspace_except(
    workspace: &Path,
    evaluated_working_dir: &Path,
    boundary: &mut ProtectedPathPolicy,
) {
    let canonical = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    if canonical != evaluated_working_dir {
        boundary.write_denylist.push(workspace.to_path_buf());
        boundary.write_denylist.push(canonical);
    }
}

/// Re-run an ordinary Run's acceptance contract inside a disposable workspace.
///
/// Verdict evaluation is keyless and non-authoritative. It requires a real
/// containment backend, protects every recorded Run workspace and control
/// path, reaps descendants, and discards all repository-controlled writes.
pub async fn run_contained_verdict_evaluation(
    state: &PipelineState,
    cancellation_token: Option<&CancellationToken>,
) -> Result<deadreckon_core::GateEvaluation> {
    if !state.working_dir.is_dir() {
        return Err(DeadreckonError::NotFound(format!(
            "working directory {}",
            state.working_dir.display()
        )));
    }
    if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
        return Err(DeadreckonError::InvalidInput(
            "verdict evaluation cancelled before launch".to_string(),
        ));
    }
    let contract = acceptance_spec_path_for_run_root(&state.run_root);
    let metadata = std::fs::symlink_metadata(&contract).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            DeadreckonError::InvalidInput(format!(
                "verdict requires an approved acceptance contract at {}; no check was run",
                contract.display()
            ))
        } else {
            DeadreckonError::Io {
                path: contract.clone(),
                source,
            }
        }
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(DeadreckonError::InvalidInput(format!(
            "verdict requires a regular, non-symlink approved acceptance contract at {}; no check was run",
            contract.display()
        )));
    }

    let configured = state
        .sandbox
        .parse::<SandboxBackend>()
        .map_err(|error| sandbox_error(&error))?;
    let requested = if configured == SandboxBackend::None {
        SandboxBackend::Auto
    } else {
        configured
    };
    let (resolved, _) = resolve_backend(requested).map_err(|error| {
        DeadreckonError::InvalidInput(format!(
            "verdict requires an available sandbox backend ({error}); no repository-controlled check was run"
        ))
    })?;
    if resolved == SandboxBackend::None {
        return Err(DeadreckonError::InvalidInput(
            "verdict requires an available sandbox backend; no repository-controlled check was run"
                .to_string(),
        ));
    }

    let scratch = TempDir::new().with_path(PathBuf::from("verdict-scratch"))?;
    let scratch_working_dir = scratch.path().join("workspace");
    let capture_policy = deadreckon_core::require_workspace_capture_policy(state)?;
    copy_recoverable_tree_with_policy(&state.working_dir, &scratch_working_dir, &capture_policy)?;
    let pid_file = scratch.path().join("dr-gate-evaluate.pid");
    let strict_job = paths_for_state(state)?.job_json(&state.run_id).is_file();
    let gate_network_access = approved_gate_network_access(state, strict_job)?;
    let result = run_keyless_gate_evaluation(
        state,
        &scratch_working_dir,
        resolved,
        pid_file,
        None,
        true,
        true,
        gate_network_access,
        None,
        cancellation_token,
    )
    .await?;
    Ok(result.evaluation)
}

fn extend_gate_toolchain_reads(read_allowlist: &mut Vec<PathBuf>) {
    for variable in ["CARGO_HOME", "RUSTUP_HOME"] {
        if let Some(path) = std::env::var_os(variable).map(PathBuf::from) {
            read_allowlist.push(path);
        }
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        read_allowlist.push(home.join(".cargo"));
        read_allowlist.push(home.join(".rustup"));
    }
    for program in ["cargo", "rustc", "sh"] {
        if let Ok(path) = which::which(program) {
            read_allowlist.push(path);
        }
    }
}

fn prepare_strict_gate_environment(
    working_dir: &Path,
    runtime_root: &Path,
    backend: SandboxBackend,
    env: &mut BTreeMap<String, String>,
    read_allowlist: &mut Vec<PathBuf>,
    write_allowlist: &mut Vec<PathBuf>,
) -> Result<()> {
    let isolated_home = runtime_root.join("home");
    let isolated_tmp = runtime_root.join("tmp");
    let isolated_cargo = runtime_root.join("cargo");
    for directory in [&isolated_home, &isolated_tmp, &isolated_cargo] {
        std::fs::create_dir_all(directory).with_path(directory)?;
        write_allowlist.push(directory.clone());
    }
    // Seatbelt requires read access to a writable directory's metadata before
    // tools can create children beneath its allowed subdirectories. Keep only
    // the disposable root readable; it contains no host state and is deleted
    // afterward, while tool-bin remains outside the write allowlist.
    read_allowlist.push(runtime_root.to_path_buf());

    for variable in ["HOME", "TMPDIR", "TMP", "TEMP"] {
        let value = if variable == "HOME" {
            &isolated_home
        } else {
            &isolated_tmp
        };
        env.insert(variable.to_string(), value.to_string_lossy().into_owned());
    }
    env.insert(
        "CARGO_HOME".to_string(),
        isolated_cargo.to_string_lossy().into_owned(),
    );
    env.insert("LC_ALL".to_string(), "C".to_string());
    env.insert("LANG".to_string(), "C".to_string());

    let host_home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    let host_cargo = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| host_home.as_ref().map(|home| home.join(".cargo")));
    let host_rustup = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| host_home.as_ref().map(|home| home.join(".rustup")));

    #[cfg(target_os = "macos")]
    let gate_tool_bin = if backend == SandboxBackend::SandboxExec {
        let bin = runtime_root.join("tool-bin");
        std::fs::create_dir_all(&bin).with_path(&bin)?;
        let wrapper = bin.join("mktemp");
        std::fs::write(
            &wrapper,
            b"#!/bin/sh\nset -eu\ncase \"$0\" in\n  */*) root=$(CDPATH= cd -P \"${0%/*}/../tmp\" && pwd -P) ;;\n  *) exit 70 ;;\nesac\ncase \"$#:$*\" in\n  '0:') exec /usr/bin/mktemp \"$root/tmp.XXXXXXXXXX\" ;;\n  '1:-d') exec /usr/bin/mktemp -d \"$root/tmp.XXXXXXXXXX\" ;;\n  2:-t\\ *) exec /usr/bin/mktemp \"$root/$2.XXXXXXXXXX\" ;;\n  3:-d\\ -t\\ *) exec /usr/bin/mktemp -d \"$root/$3.XXXXXXXXXX\" ;;\n  3:-t\\ *\\ -d) exec /usr/bin/mktemp -d \"$root/$2.XXXXXXXXXX\" ;;\nesac\nexec /usr/bin/mktemp \"$@\"\n",
        )
        .with_path(&wrapper)?;
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o500))
            .with_path(&wrapper)?;
        read_allowlist.push(bin.clone());
        Some(bin)
    } else {
        None
    };
    #[cfg(not(target_os = "macos"))]
    let gate_tool_bin: Option<PathBuf> = None;

    let docker = backend == SandboxBackend::Docker;
    let mut path_entries = if docker {
        [
            "/usr/local/cargo/bin",
            "/usr/local/sbin",
            "/usr/local/bin",
            "/usr/sbin",
            "/usr/bin",
            "/sbin",
            "/bin",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>()
    } else {
        let mut entries = Vec::new();
        for path in [
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
            PathBuf::from("/usr/sbin"),
            PathBuf::from("/sbin"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/Library/Apple/usr/bin"),
        ] {
            push_existing_unique(&mut entries, path);
        }
        entries
    };
    if let Some(bin) = gate_tool_bin.as_ref() {
        path_entries.insert(0, bin.clone());
    }
    if let Some(cargo) = host_cargo.as_ref().filter(|path| path.is_dir()) {
        let bin = cargo.join("bin");
        if !docker {
            push_existing_unique(&mut path_entries, bin.clone());
            push_existing_unique(read_allowlist, bin);
        }
        for cache in ["registry", "git"] {
            let source = cargo.join(cache);
            if source.is_dir() {
                link_gate_cache(&source, &isolated_cargo.join(cache))?;
                push_existing_unique(read_allowlist, source);
            }
        }
    }
    if docker {
        env.insert("RUSTUP_HOME".to_string(), "/usr/local/rustup".to_string());
    } else if let Some(rustup) = host_rustup.as_ref().filter(|path| path.is_dir()) {
        env.insert(
            "RUSTUP_HOME".to_string(),
            rustup.to_string_lossy().into_owned(),
        );
        push_existing_unique(read_allowlist, rustup.clone());
    }
    push_existing_unique(
        &mut path_entries,
        working_dir.join("node_modules").join(".bin"),
    );
    let path = std::env::join_paths(&path_entries).map_err(|error| {
        DeadreckonError::InvalidInput(format!(
            "strict gate could not construct a safe executable PATH: {error}"
        ))
    })?;
    env.insert("PATH".to_string(), path.to_string_lossy().into_owned());
    if !docker {
        configure_macos_strict_toolchain(gate_tool_bin.as_deref(), env, read_allowlist)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn configure_macos_strict_toolchain(
    gate_tool_bin: Option<&Path>,
    env: &mut BTreeMap<String, String>,
    read_allowlist: &mut Vec<PathBuf>,
) -> Result<()> {
    // `/usr/bin/cc` and `/usr/bin/clang` are xcrun shims. Inside the strict
    // Seatbelt profile they try to update a host-global cache below the Darwin
    // user temp directory, which is intentionally not writable. Resolve the
    // selected developer toolchain in the trusted controller and pass only
    // stable, canonical paths into the disposable evaluator.
    if let Some(clang) = xcrun_path(&["--find", "clang"], false) {
        let clang_value = clang.to_string_lossy().into_owned();
        env.insert("CC".to_string(), clang_value.clone());
        let cargo_linker = match std::env::consts::ARCH {
            "aarch64" => Some("CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER"),
            "x86_64" => Some("CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER"),
            _ => None,
        };
        if let Some(key) = cargo_linker {
            env.insert(key.to_string(), clang_value);
        }
        push_existing_unique(read_allowlist, clang);
    }

    if let Some(clangxx) = xcrun_path(&["--find", "clang++"], false) {
        env.insert("CXX".to_string(), clangxx.to_string_lossy().into_owned());
        push_existing_unique(read_allowlist, clangxx);
    }
    if let Some(sdk_root) = xcrun_path(&["--show-sdk-path"], true) {
        env.insert(
            "SDKROOT".to_string(),
            sdk_root.to_string_lossy().into_owned(),
        );
        push_existing_unique(read_allowlist, sdk_root);
    }
    if let Some(tool_bin) = gate_tool_bin {
        // Shell acceptance checks commonly invoke these names directly. The
        // `/usr/bin` entries are Apple developer-tool shims which may invoke
        // xcrun and update its host temp cache after containment begins. Put
        // controller-resolved, read-only aliases ahead of `/usr/bin` instead.
        for name in [
            "python3",
            "swift",
            "swiftc",
            "clang",
            "clang++",
            "cc",
            "c++",
            "xcodebuild",
        ] {
            let Some(tool) = xcrun_path(&["--find", name], false) else {
                continue;
            };
            let alias = tool_bin.join(name);
            std::os::unix::fs::symlink(&tool, &alias).with_path(&alias)?;
            push_existing_unique(read_allowlist, tool);
        }
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
// Match the fallible macOS implementation so the cross-platform call site
// cannot accidentally omit error propagation when compiled on another host.
#[allow(clippy::unnecessary_wraps)]
fn configure_macos_strict_toolchain(
    _gate_tool_bin: Option<&Path>,
    _env: &mut BTreeMap<String, String>,
    _read_allowlist: &mut Vec<PathBuf>,
) -> Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn xcrun_path(args: &[&str], directory: bool) -> Option<PathBuf> {
    let output = std::process::Command::new("/usr/bin/xcrun")
        .args(args)
        .env_clear()
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.len() > 4096 {
        return None;
    }
    let text = std::str::from_utf8(&output.stdout).ok()?;
    let mut lines = text.lines();
    let path = PathBuf::from(lines.next()?.trim());
    if path.as_os_str().is_empty() || !path.is_absolute() || lines.next().is_some() {
        return None;
    }
    let canonical = path.canonicalize().ok()?;
    let metadata = std::fs::symlink_metadata(&canonical).ok()?;
    if (directory && !metadata.file_type().is_dir())
        || (!directory && !metadata.file_type().is_file())
    {
        return None;
    }
    Some(canonical)
}

fn push_existing_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if path.exists() && !paths.contains(&path) {
        paths.push(path);
    }
}

#[cfg(unix)]
fn link_gate_cache(source: &Path, destination: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, destination).with_path(destination)
}

#[cfg(not(unix))]
fn link_gate_cache(_source: &Path, _destination: &Path) -> Result<()> {
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SemanticCompletionDisposition {
    Achieved,
    Revise,
    NeedsReview,
    LostContainment,
    BudgetExhausted,
    Cancelled,
}

// Completion is a trust boundary; keep its contract, marker, cancellation,
// and deadline inputs explicit at the call site.
#[allow(clippy::too_many_arguments)]
async fn semantic_completion_disposition(
    state: &mut PipelineState,
    router: &ProviderRouter,
    config: &RunLoopConfig,
    turn: u32,
    marker: &deadreckon_core::AcceptanceMarker,
    history: &mut Vec<String>,
    cancellation_token: &CancellationToken,
    work_clock: &RunWorkClock,
    phase_deadline: ProviderPhaseDeadline,
) -> Result<SemanticCompletionDisposition> {
    work_clock.sync(state);
    if should_cancel_run(state, cancellation_token) {
        return Ok(SemanticCompletionDisposition::Cancelled);
    }
    let paths = paths_for_state(state)?;
    if !paths.job_json(&state.run_id).is_file() {
        // Compatibility runs predate Watchkeeper. They keep their historical
        // deterministic completion path and are never presented as a new
        // two-key Job receipt.
        return Ok(SemanticCompletionDisposition::Achieved);
    }
    let job = deadreckon_core::load_job(&paths, &state.run_id)?;
    if job.policy.semantic_judge == deadreckon_protocol::SemanticJudgeMode::Disabled {
        record_needs_review(
            state,
            turn,
            history,
            "semantic judging is disabled; a new job cannot be reported verified without the second key",
            work_clock,
        )?;
        return Ok(SemanticCompletionDisposition::NeedsReview);
    }
    let budget_reason = if state.total_spend_usd >= job.policy.max_spend_usd {
        Some("spend cap reached before semantic judge")
    } else if state.total_wall_seconds >= job.policy.max_wall_seconds as f64 {
        Some("wall-clock cap reached before semantic judge")
    } else if tokio::time::Instant::now() >= phase_deadline.work_expires_at {
        Some(match work_clock.expiry() {
            RunWorkExpiry::WallCap => "wall-clock cap reached before semantic judge",
            RunWorkExpiry::Deadline => "calendar deadline reached before semantic judge",
        })
    } else {
        None
    };
    if let Some(reason) = budget_reason {
        state.pause_reason = Some(reason.to_string());
        state.failure_reason = Some(reason.to_string());
        work_clock.save(state)?;
        return Ok(SemanticCompletionDisposition::BudgetExhausted);
    }

    let semantic_run =
        match crate::semantic_judge::run_semantic_judge_with_deadline_and_cancellation(
            state,
            marker,
            router,
            config.sandbox_backend,
            crate::semantic_judge::SemanticJudgeBudget {
                remaining_spend_usd: Some(job.policy.max_spend_usd - state.total_spend_usd),
                // The exact outer phase deadline below owns time. This relative
                // field remains only for accounting compatibility and must not be
                // used to construct a new work window.
                remaining_wall_seconds: None,
            },
            phase_deadline,
            Some(cancellation_token),
        )
        .await
        {
            Ok(result) => result,
            Err(error) => {
                record_needs_review(
                    state,
                    turn,
                    history,
                    &format!("strict semantic judge unavailable: {error}"),
                    work_clock,
                )?;
                return Ok(SemanticCompletionDisposition::NeedsReview);
            }
        };
    work_clock.sync(state);
    record_semantic_judge_accounting(state, turn, config, &semantic_run, work_clock)?;
    if let crate::semantic_judge::SemanticJudgeResult::LostContainment(reason) =
        &semantic_run.result
    {
        record_semantic_lost_containment(state, turn, history, reason, work_clock)?;
        return Ok(SemanticCompletionDisposition::LostContainment);
    }
    if should_cancel_run(state, cancellation_token) {
        return Ok(SemanticCompletionDisposition::Cancelled);
    }
    let overrun_reason = match semantic_run.budget_exhaustion {
        Some(crate::semantic_judge::SemanticBudgetExhaustion::Spend) => {
            Some("semantic judge exhausted the approved spend cap".to_string())
        }
        Some(crate::semantic_judge::SemanticBudgetExhaustion::Wall) => Some(format!(
            "{} during semantic judge",
            work_clock.expiry().reached_label()
        )),
        None if state.total_spend_usd > job.policy.max_spend_usd => {
            Some("semantic judge exceeded the approved spend cap".to_string())
        }
        None if state.total_wall_seconds > job.policy.max_wall_seconds as f64 => {
            Some("semantic judge exceeded the approved wall-time cap".to_string())
        }
        None => None,
    };
    if let Some(reason) = overrun_reason {
        state.pause_reason = Some(reason.clone());
        state.failure_reason = Some(reason);
        work_clock.save(state)?;
        return Ok(SemanticCompletionDisposition::BudgetExhausted);
    }
    if let Some(judgment) = semantic_run.result.judgment()
        && let Err(error) =
            crate::semantic_judge::persist_semantic_judgment(&state.run_root, judgment)
    {
        record_needs_review(
            state,
            turn,
            history,
            &format!("strict semantic judgment could not be persisted after accounting: {error}"),
            work_clock,
        )?;
        return Ok(SemanticCompletionDisposition::NeedsReview);
    }
    match semantic_run.result {
        crate::semantic_judge::SemanticJudgeResult::Achieved(judgment) => {
            seal_achieved_semantic_completion(
                state,
                &paths,
                turn,
                marker,
                &judgment,
                history,
                work_clock,
                phase_deadline,
                cancellation_token,
            )
        }
        crate::semantic_judge::SemanticJudgeResult::Revise(judgment) => {
            let missing = if judgment.missing.is_empty() {
                "no explicit missing clauses supplied".to_string()
            } else {
                judgment.missing.join("; ")
            };
            let feedback = format!(
                "independent semantic judge requested revision after turn {turn}: {}. Missing: {missing}",
                judgment.summary
            );
            history.push(feedback.clone());
            state.failure_reason = Some(feedback);
            save_history(state, history)?;
            work_clock.save(state)?;
            Ok(SemanticCompletionDisposition::Revise)
        }
        crate::semantic_judge::SemanticJudgeResult::NeedsReview(judgment) => {
            record_needs_review(
                state,
                turn,
                history,
                &format!("semantic judge was uncertain: {}", judgment.summary),
                work_clock,
            )?;
            Ok(SemanticCompletionDisposition::NeedsReview)
        }
        crate::semantic_judge::SemanticJudgeResult::Unavailable(reason) => {
            record_needs_review(state, turn, history, &reason, work_clock)?;
            Ok(SemanticCompletionDisposition::NeedsReview)
        }
        crate::semantic_judge::SemanticJudgeResult::LostContainment(reason) => {
            record_semantic_lost_containment(state, turn, history, &reason, work_clock)?;
            Ok(SemanticCompletionDisposition::LostContainment)
        }
    }
}

// Sealing deliberately accepts the independently derived proof components as
// separate arguments so their provenance stays visible.
#[allow(clippy::too_many_arguments)]
fn seal_achieved_semantic_completion(
    state: &mut PipelineState,
    paths: &DeadreckonPaths,
    turn: u32,
    marker: &deadreckon_core::AcceptanceMarker,
    judgment: &deadreckon_protocol::SemanticJudgment,
    history: &mut Vec<String>,
    work_clock: &RunWorkClock,
    phase_deadline: ProviderPhaseDeadline,
    cancellation_token: &CancellationToken,
) -> Result<SemanticCompletionDisposition> {
    if cancellation_token.is_cancelled() {
        return Ok(SemanticCompletionDisposition::Cancelled);
    }
    if tokio::time::Instant::now() >= phase_deadline.work_expires_at {
        state.pause_reason = Some(format!(
            "{} before completion receipt sealing",
            work_clock.expiry().reached_label()
        ));
        state.failure_reason = state.pause_reason.clone();
        work_clock.save(state)?;
        return Ok(SemanticCompletionDisposition::BudgetExhausted);
    }
    let seal_result = (|| -> Result<()> {
        crate::semantic_judge::validate_semantic_judgment_input(state, marker, judgment)?;
        let authority_path = paths.job_authority(&state.run_id);
        let raw = std::fs::read(&authority_path).map_err(|source| DeadreckonError::Io {
            path: authority_path.clone(),
            source,
        })?;
        let authority: deadreckon_protocol::JobAuthority =
            serde_json::from_slice(&raw).map_err(|source| DeadreckonError::Json {
                path: authority_path,
                source,
            })?;
        let cancellation = cancellation_token.clone();
        let scope = deadreckon_core::git::WorkBoundaryScope::new(
            phase_deadline.work_expires_at.into_std(),
            phase_deadline.cleanup_budget,
            move || cancellation.is_cancelled(),
            "completion receipt sealing",
        )
        .with_authority_dir(state.run_root.join("child-pids"));
        deadreckon_core::seal_completion_receipt_bounded(
            paths, state, &authority, marker, judgment, scope,
        )?;
        Ok(())
    })();
    if let Err(error) = seal_result {
        if let DeadreckonError::ProcessBoundary {
            kind: deadreckon_core::ProcessBoundaryKind::WorkExpired,
            ..
        } = &error
        {
            state.pause_reason = Some(format!(
                "{} while sealing the completion receipt",
                work_clock.expiry().reached_label()
            ));
            state.failure_reason = state.pause_reason.clone();
            work_clock.save(state)?;
            return Ok(SemanticCompletionDisposition::BudgetExhausted);
        }
        if let DeadreckonError::ProcessBoundary {
            kind: deadreckon_core::ProcessBoundaryKind::Cancelled,
            ..
        } = &error
        {
            return Ok(SemanticCompletionDisposition::Cancelled);
        }
        if let DeadreckonError::ProcessBoundary {
            kind: deadreckon_core::ProcessBoundaryKind::CleanupIncomplete,
            authority,
            detail,
            ..
        } = &error
        {
            let reason = format!(
                "completion receipt sealing lost process containment{}: {detail}",
                authority
                    .as_deref()
                    .map(|path| format!("; authority retained at {}", path.display()))
                    .unwrap_or_default()
            );
            record_semantic_lost_containment(state, turn, history, &reason, work_clock)?;
            return Ok(SemanticCompletionDisposition::LostContainment);
        }
        record_needs_review(
            state,
            turn,
            history,
            &format!(
                "semantic judgment achieved, but the combined receipt could not be sealed: {error}"
            ),
            work_clock,
        )?;
        return Ok(SemanticCompletionDisposition::NeedsReview);
    }
    Ok(SemanticCompletionDisposition::Achieved)
}

fn record_semantic_judge_accounting(
    state: &mut PipelineState,
    turn: u32,
    config: &RunLoopConfig,
    semantic_run: &crate::semantic_judge::SemanticJudgeRun,
    work_clock: &RunWorkClock,
) -> Result<()> {
    let accounting = &semantic_run.accounting;
    state.total_spend_usd += accounting.cost_usd;
    work_clock.sync(state);
    append_spend(
        state,
        &SpendRecord {
            timestamp: Utc::now(),
            turn,
            provider: accounting.provider.clone(),
            model: accounting.model.clone(),
            input_tokens: accounting.input_tokens,
            output_tokens: accounting.output_tokens,
            cost_usd: accounting.cost_usd,
            total_cost_usd: state.total_spend_usd,
            cap_usd: config.max_spend_usd,
            subscription: accounting.subscription,
            estimated: false,
            wall_time_seconds: Some(accounting.wall_time_seconds),
            wall_time_cap_seconds: work_clock.wall_time_cap_seconds(config.max_wall_seconds),
            kind: "semantic_judge".to_string(),
        },
    )?;
    let (event, reason) = match &semantic_run.result {
        crate::semantic_judge::SemanticJudgeResult::Achieved(_) => {
            ("semantic_judge.achieved", None)
        }
        crate::semantic_judge::SemanticJudgeResult::Revise(_) => ("semantic_judge.revise", None),
        crate::semantic_judge::SemanticJudgeResult::NeedsReview(_) => {
            ("semantic_judge.uncertain", None)
        }
        crate::semantic_judge::SemanticJudgeResult::Unavailable(reason) => {
            ("semantic_judge.unavailable", Some(reason.as_str()))
        }
        crate::semantic_judge::SemanticJudgeResult::LostContainment(reason) => {
            ("semantic_judge.lost_containment", Some(reason.as_str()))
        }
    };
    let input_sha256 = semantic_run
        .result
        .judgment()
        .map(|judgment| judgment.input_sha256.clone());
    let decision = semantic_run
        .result
        .judgment()
        .map(|judgment| judgment.decision);
    let summary = semantic_run
        .result
        .judgment()
        .map(|judgment| judgment.summary.clone());
    let missing = semantic_run
        .result
        .judgment()
        .map(|judgment| judgment.missing.clone());
    append_trace(
        state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn,
            event: event.to_string(),
            latency_ms: Some((accounting.wall_time_seconds.max(0.0) * 1_000.0).round() as u128),
            detail: json!({
                "provider": accounting.provider,
                "model": accounting.model,
                "input_tokens": accounting.input_tokens,
                "output_tokens": accounting.output_tokens,
                "spend_usd": accounting.cost_usd,
                "wall_time_seconds": accounting.wall_time_seconds,
                "sandbox_backend": accounting.sandbox_backend,
                "workspace_access": "read-only",
                "worker_session": false,
                "input_sha256": input_sha256,
                "decision": decision,
                "summary": summary,
                "missing": missing,
                "reason": reason,
            }),
        },
    )?;
    work_clock.save(state)
}

fn record_needs_review(
    state: &mut PipelineState,
    turn: u32,
    history: &mut Vec<String>,
    reason: &str,
    work_clock: &RunWorkClock,
) -> Result<()> {
    let message = format!("NEEDS_REVIEW: {reason}");
    append_trace(
        state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn,
            event: "semantic_judge.needs_review".to_string(),
            latency_ms: None,
            detail: json!({ "reason": reason }),
        },
    )?;
    history.push(message.clone());
    state.failure_reason = Some(message);
    state.set_phase_status(PhaseId(50), PhaseStatus::Failed)?;
    save_history(state, history)?;
    work_clock.save(state)
}

fn record_semantic_lost_containment(
    state: &mut PipelineState,
    turn: u32,
    history: &mut Vec<String>,
    reason: &str,
    work_clock: &RunWorkClock,
) -> Result<()> {
    let message = format!("LOST_CONTAINMENT: {reason}");
    append_trace(
        state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn,
            event: "semantic_judge.lost_containment".to_string(),
            latency_ms: None,
            detail: json!({ "reason": reason }),
        },
    )?;
    history.push(message.clone());
    state.failure_reason = Some(message);
    state.set_phase_status(PhaseId(50), PhaseStatus::Failed)?;
    save_history(state, history)?;
    work_clock.save(state)
}

#[derive(Debug)]
// This enum crosses the gate once per completion attempt. Keeping the marker
// inline avoids heap indirection in proof-handling code and preserves its
// concrete ownership semantics.
#[allow(clippy::large_enum_variant)]
enum DeterministicGateDisposition {
    Passed(deadreckon_core::AcceptanceMarker),
    Revise,
    PausedAtCap,
    Cancelled,
    LostContainment,
}

#[derive(Debug)]
pub enum DeterministicGatePhaseOutcome {
    Completed {
        result: Result<()>,
        cleanup: ProviderCleanup,
    },
    WorkExpired {
        cleanup: ProviderCleanup,
    },
    Cancelled {
        cleanup: ProviderCleanup,
    },
}

pub async fn run_deterministic_gate_work_phase(
    state: &PipelineState,
    sandbox_backend: SandboxBackend,
    launch_owner: Option<&GateLaunchOwner>,
    external_cancellation: &CancellationToken,
    deadline: ProviderPhaseDeadline,
) -> Result<DeterministicGatePhaseOutcome> {
    if external_cancellation.is_cancelled() {
        return Ok(DeterministicGatePhaseOutcome::Cancelled {
            cleanup: ProviderCleanup::NotApplicable,
        });
    }
    if tokio::time::Instant::now() >= deadline.work_expires_at {
        return Ok(DeterministicGatePhaseOutcome::WorkExpired {
            cleanup: ProviderCleanup::NotApplicable,
        });
    }

    let authority_dir = state.run_root.join("runtime-phase-authority");
    std::fs::create_dir_all(&authority_dir).with_path(&authority_dir)?;
    let phase_authority = authority_dir.join(format!(
        "deterministic-gate-{}.json",
        Uuid::new_v4().simple()
    ));
    write_new_file(
        &phase_authority,
        format!(
            "phase=deterministic_gate\nrun_id={}\nstarted_at={}\n",
            state.run_id,
            Utc::now().to_rfc3339()
        )
        .as_bytes(),
    )?;
    let phase_token = CancellationToken::new();
    let gate =
        run_deterministic_completion_gate(state, sandbox_backend, launch_owner, Some(&phase_token));
    tokio::pin!(gate);
    enum Boundary<T> {
        Completed(T),
        WorkExpired,
        Cancelled,
    }
    let boundary = tokio::select! {
        biased;
        () = external_cancellation.cancelled() => Boundary::Cancelled,
        result = &mut gate => Boundary::Completed(result),
        () = tokio::time::sleep_until(deadline.work_expires_at) => Boundary::WorkExpired,
    };
    match boundary {
        Boundary::Completed(result) => Ok(DeterministicGatePhaseOutcome::Completed {
            cleanup: finish_gate_phase_cleanup(state, &phase_authority, true),
            result,
        }),
        Boundary::WorkExpired => {
            phase_token.cancel();
            let resolved = tokio::time::timeout(deadline.cleanup_budget, &mut gate)
                .await
                .is_ok();
            Ok(DeterministicGatePhaseOutcome::WorkExpired {
                cleanup: finish_gate_phase_cleanup(state, &phase_authority, resolved),
            })
        }
        Boundary::Cancelled => {
            phase_token.cancel();
            let resolved = tokio::time::timeout(deadline.cleanup_budget, &mut gate)
                .await
                .is_ok();
            Ok(DeterministicGatePhaseOutcome::Cancelled {
                cleanup: finish_gate_phase_cleanup(state, &phase_authority, resolved),
            })
        }
    }
}

fn finish_gate_phase_cleanup(
    state: &PipelineState,
    phase_authority: &Path,
    execution_resolved: bool,
) -> ProviderCleanup {
    if !execution_resolved {
        return ProviderCleanup::RetainedAuthority {
            path: phase_authority.to_path_buf(),
            detail: format!(
                "deterministic gate did not resolve within the separate {:.0}s cleanup window",
                RUNTIME_PHASE_CLEANUP_BUDGET.as_secs_f64()
            ),
        };
    }
    if let Some(authority) = outstanding_gate_process_authority(state) {
        return ProviderCleanup::RetainedAuthority {
            path: authority,
            detail: "deterministic gate returned with subprocess authority still present"
                .to_string(),
        };
    }
    match std::fs::remove_file(phase_authority) {
        Ok(()) => ProviderCleanup::Proven,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ProviderCleanup::Proven,
        Err(error) => ProviderCleanup::RetainedAuthority {
            path: phase_authority.to_path_buf(),
            detail: format!("deterministic gate phase authority could not be removed: {error}"),
        },
    }
}

fn outstanding_gate_process_authority(state: &PipelineState) -> Option<PathBuf> {
    let entries = match std::fs::read_dir(state.run_root.join("child-pids")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(_) => return Some(state.run_root.join("child-pids")),
    };
    entries
        .filter_map(std::result::Result::ok)
        .find_map(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            (name.starts_with("dr-gate-evaluate-")
                || name.starts_with("sandbox-boundary-probe-")
                || name.starts_with("docker-gate-probe-")
                || name.starts_with("docker-gate-evaluate-"))
            .then(|| entry.path())
        })
}

// Gate evaluation keeps each authority and deadline input explicit for audit.
#[allow(clippy::too_many_arguments)]
async fn acceptance_gate_passed_or_record_failure(
    state: &mut PipelineState,
    sender: Option<&broadcast::Sender<RunEvent>>,
    turn: u32,
    history: &mut Vec<String>,
    sandbox_backend: SandboxBackend,
    cancellation_token: &CancellationToken,
    work_clock: &RunWorkClock,
    work_expires_at: tokio::time::Instant,
) -> Result<DeterministicGateDisposition> {
    let launch_owner = gate_launch_owner_from_environment(state)?;
    work_clock.sync(state);
    let gate_phase = run_deterministic_gate_work_phase(
        state,
        sandbox_backend,
        launch_owner.as_ref(),
        cancellation_token,
        ProviderPhaseDeadline::new(work_expires_at, RUNTIME_PHASE_CLEANUP_BUDGET),
    )
    .await?;
    let gate_result = match gate_phase {
        DeterministicGatePhaseOutcome::Completed {
            result,
            cleanup: ProviderCleanup::Proven | ProviderCleanup::NotApplicable,
        } => result.and_then(|()| validate_acceptance_marker(state)),
        DeterministicGatePhaseOutcome::Completed {
            cleanup: ProviderCleanup::RetainedAuthority { path, detail },
            ..
        } => {
            record_runtime_lost_containment(
                state,
                turn,
                "deterministic gate",
                PhaseId(50),
                Some(&path),
                &detail,
                work_clock,
            )?;
            return Ok(DeterministicGateDisposition::LostContainment);
        }
        DeterministicGatePhaseOutcome::WorkExpired { cleanup } => {
            let outcome = record_runtime_phase_interruption(
                state,
                turn,
                "deterministic gate",
                PhaseId(50),
                RuntimePhaseInterruption::WorkExpired,
                &cleanup,
                work_clock,
            )?;
            return Ok(match outcome {
                RunLoopOutcome::PausedAtCap => DeterministicGateDisposition::PausedAtCap,
                RunLoopOutcome::Failed => DeterministicGateDisposition::LostContainment,
                RunLoopOutcome::Done | RunLoopOutcome::Killed => {
                    DeterministicGateDisposition::LostContainment
                }
            });
        }
        DeterministicGatePhaseOutcome::Cancelled { cleanup } => {
            let outcome = record_runtime_phase_interruption(
                state,
                turn,
                "deterministic gate",
                PhaseId(50),
                RuntimePhaseInterruption::Cancelled,
                &cleanup,
                work_clock,
            )?;
            return Ok(if outcome == RunLoopOutcome::Killed {
                DeterministicGateDisposition::Cancelled
            } else {
                DeterministicGateDisposition::LostContainment
            });
        }
    };
    work_clock.save(state)?;
    match gate_result {
        Ok(marker) => Ok(DeterministicGateDisposition::Passed(marker)),
        Err(err) => {
            if should_cancel_run(state, cancellation_token) {
                return Ok(DeterministicGateDisposition::Cancelled);
            }
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
            work_clock.save(state)?;
            Ok(DeterministicGateDisposition::Revise)
        }
    }
}

fn gate_launch_owner_from_environment(state: &PipelineState) -> Result<Option<GateLaunchOwner>> {
    let paths = paths_for_state(state)?;
    if !paths.job_json(&state.run_id).is_file() {
        return Ok(None);
    }
    let attempt = std::env::var("DEADRECKON_SUPERVISOR_ATTEMPT")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or_else(|| {
            DeadreckonError::InvalidInput(
                "strict worker gate is missing its durable supervisor attempt identity".to_string(),
            )
        })?;
    let outer_launch_id = std::env::var("DEADRECKON_SUPERVISOR_LAUNCH_ID").map_err(|_| {
        DeadreckonError::InvalidInput(
            "strict worker gate is missing its durable supervisor launch identity".to_string(),
        )
    })?;
    GateLaunchOwner::new(attempt, outer_launch_id).map(Some)
}

fn trusted_turn_head_path(state: &PipelineState, turn: u32) -> PathBuf {
    state
        .run_root
        .join("provider-evidence")
        .join(format!("turn-{turn}"))
        .join("result-head-before-provider.txt")
}

fn trusted_lifecycle_snapshot_path(state: &PipelineState, turn: u32) -> PathBuf {
    state
        .run_root
        .join("provider-evidence")
        .join(format!("turn-{turn}"))
        .join("lifecycle-before-provider")
}

fn trusted_git_control_snapshot_path(state: &PipelineState, turn: u32) -> PathBuf {
    state
        .run_root
        .join("provider-evidence")
        .join(format!("turn-{turn}"))
        .join("git-control-before-provider")
}

#[derive(Debug)]
struct TrustedGitControl {
    workspace: PathBuf,
    workspace_control: PathBuf,
    snapshot: PathBuf,
    expected_control: Vec<u8>,
    git_dir: PathBuf,
    common_dir: PathBuf,
    phase: Option<TrustedGitPhase>,
}

#[derive(Clone, Debug)]
struct TrustedGitPhase {
    deadline: ProviderPhaseDeadline,
    cancellation: CancellationToken,
    authority_dir: PathBuf,
    interruption: Arc<Mutex<Option<TrustedGitInterruption>>>,
}

#[derive(Clone, Debug)]
enum TrustedGitInterruption {
    WorkExpired {
        cleanup: ProviderCleanup,
    },
    Cancelled {
        cleanup: ProviderCleanup,
    },
    LostContainment {
        boundary: GitCommandBoundary,
        authority: Option<PathBuf>,
        detail: String,
    },
}

#[derive(Debug)]
enum TrustedGitPhaseOutcome<T> {
    Completed(T),
    WorkExpired {
        cleanup: ProviderCleanup,
    },
    Cancelled {
        cleanup: ProviderCleanup,
    },
    LostContainment {
        boundary: GitCommandBoundary,
        authority: Option<PathBuf>,
        detail: String,
    },
}

impl TrustedGitPhase {
    fn new(
        state: &PipelineState,
        deadline: ProviderPhaseDeadline,
        cancellation: &CancellationToken,
    ) -> Self {
        Self {
            deadline,
            cancellation: cancellation.clone(),
            authority_dir: state.run_root.join("child-pids"),
            interruption: Arc::new(Mutex::new(None)),
        }
    }

    fn record(&self, interruption: TrustedGitInterruption) -> Result<()> {
        let mut current = self.interruption.lock().map_err(|_| {
            DeadreckonError::InvalidInput(
                "trusted Git phase interruption record was poisoned".to_string(),
            )
        })?;
        if current.is_none() {
            *current = Some(interruption);
        }
        Ok(())
    }

    fn finish<T>(&self, result: Result<T>) -> Result<TrustedGitPhaseOutcome<T>> {
        let interruption = self
            .interruption
            .lock()
            .map_err(|_| {
                DeadreckonError::InvalidInput(
                    "trusted Git phase interruption record was poisoned".to_string(),
                )
            })?
            .take();
        match (result, interruption) {
            (Ok(value), None) => Ok(TrustedGitPhaseOutcome::Completed(value)),
            (Err(error), None) => Err(error),
            (_, Some(TrustedGitInterruption::WorkExpired { cleanup })) => {
                Ok(TrustedGitPhaseOutcome::WorkExpired { cleanup })
            }
            (_, Some(TrustedGitInterruption::Cancelled { cleanup })) => {
                Ok(TrustedGitPhaseOutcome::Cancelled { cleanup })
            }
            (
                _,
                Some(TrustedGitInterruption::LostContainment {
                    boundary,
                    authority,
                    detail,
                }),
            ) => Ok(TrustedGitPhaseOutcome::LostContainment {
                boundary,
                authority,
                detail,
            }),
        }
    }
}

#[cfg(test)]
fn capture_trusted_turn_head(state: &PipelineState, turn: u32) -> Result<()> {
    capture_trusted_turn_head_inner(state, turn, None)
}

fn capture_trusted_turn_head_bounded(
    state: &PipelineState,
    turn: u32,
    deadline: ProviderPhaseDeadline,
    cancellation: &CancellationToken,
) -> Result<TrustedGitPhaseOutcome<()>> {
    let phase = TrustedGitPhase::new(state, deadline, cancellation);
    let result = capture_trusted_turn_head_inner(state, turn, Some(phase.clone()));
    phase.finish(result)
}

fn capture_trusted_turn_head_inner(
    state: &PipelineState,
    turn: u32,
    phase: Option<TrustedGitPhase>,
) -> Result<()> {
    capture_trusted_lifecycle_metadata(state, turn)?;
    let record = read_turn_codebase_record(state)?;
    write_trusted_codebase_record(&state.run_root, &record)?;
    if record.mode != CodebaseMode::Worktree {
        return Ok(());
    }
    let control = capture_trusted_git_control(state, turn, &record, phase)?;
    let head = trusted_git_stdout(&control, &["rev-parse", "HEAD"])?;
    let path = trusted_turn_head_path(state, turn);
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "trusted turn head path has no parent: {}",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent).with_path(parent)?;
    if std::fs::read_to_string(&path).is_ok_and(|existing| !existing.trim().is_empty()) {
        // A process may die after the provider committed but before the turn
        // sanitizer ran. Preserve the original pre-provider boundary across
        // restart instead of blessing the provider's current HEAD.
        return Ok(());
    }
    std::fs::write(&path, format!("{head}\n")).with_path(path)
}

fn capture_trusted_git_control(
    state: &PipelineState,
    turn: u32,
    record: &CodebaseRecord,
    phase: Option<TrustedGitPhase>,
) -> Result<TrustedGitControl> {
    let snapshot = trusted_git_control_snapshot_path(state, turn);
    let captured = snapshot.with_extension("captured");
    let expected_control = match std::fs::symlink_metadata(&snapshot) {
        Ok(metadata) => {
            require_regular_git_control(&snapshot, &metadata, "trusted Git control snapshot")?;
            std::fs::read(&snapshot).with_path(&snapshot)?
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            if captured.exists() {
                return Err(DeadreckonError::InvalidInput(format!(
                    "trusted Git control marker exists without snapshot {}; refusing recovery",
                    snapshot.display()
                )));
            }
            let workspace_control = state.working_dir.join(".git");
            let metadata =
                std::fs::symlink_metadata(&workspace_control).with_path(&workspace_control)?;
            require_regular_git_control(
                &workspace_control,
                &metadata,
                "linked-worktree .git control",
            )?;
            let bytes = std::fs::read(&workspace_control).with_path(&workspace_control)?;
            // Validate the live record before persisting it as control-plane
            // truth. A normal repository `.git` directory or a pointer into an
            // unrelated repository is never accepted for Worktree mode.
            validate_git_control_record(state, record, &bytes)?;
            let parent = snapshot.parent().ok_or_else(|| {
                DeadreckonError::InvalidInput(format!(
                    "trusted Git control path has no parent: {}",
                    snapshot.display()
                ))
            })?;
            std::fs::create_dir_all(parent).with_path(parent)?;
            write_new_file(&snapshot, &bytes)?;
            bytes
        }
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: snapshot,
                source,
            });
        }
    };
    let (git_dir, common_dir) = validate_git_control_record(state, record, &expected_control)?;
    archive_changed_git_control(state, turn, &expected_control)?;
    if !captured.exists() {
        write_new_file(&captured, b"trusted Git control captured\n")?;
    } else {
        let metadata = std::fs::symlink_metadata(&captured).with_path(&captured)?;
        require_regular_git_control(&captured, &metadata, "trusted Git control marker")?;
    }
    let control = TrustedGitControl {
        workspace: state.working_dir.clone(),
        workspace_control: state.working_dir.join(".git"),
        snapshot,
        expected_control,
        git_dir,
        common_dir,
        phase,
    };
    restore_and_verify_git_control(&control)?;
    Ok(control)
}

fn load_trusted_git_control(
    state: &PipelineState,
    turn: u32,
    record: &CodebaseRecord,
    phase: Option<TrustedGitPhase>,
) -> Result<TrustedGitControl> {
    let snapshot = trusted_git_control_snapshot_path(state, turn);
    let captured = snapshot.with_extension("captured");
    let marker_metadata = std::fs::symlink_metadata(&captured).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            DeadreckonError::NotFound(format!("trusted Git control marker {}", captured.display()))
        } else {
            DeadreckonError::Io {
                path: captured.clone(),
                source,
            }
        }
    })?;
    require_regular_git_control(&captured, &marker_metadata, "trusted Git control marker")?;
    let snapshot_metadata = std::fs::symlink_metadata(&snapshot).with_path(&snapshot)?;
    require_regular_git_control(
        &snapshot,
        &snapshot_metadata,
        "trusted Git control snapshot",
    )?;
    let expected_control = std::fs::read(&snapshot).with_path(&snapshot)?;
    let (git_dir, common_dir) = validate_git_control_record(state, record, &expected_control)?;
    archive_changed_git_control(state, turn, &expected_control)?;
    let control = TrustedGitControl {
        workspace: state.working_dir.clone(),
        workspace_control: state.working_dir.join(".git"),
        snapshot,
        expected_control,
        git_dir,
        common_dir,
        phase,
    };
    restore_and_verify_git_control(&control)?;
    Ok(control)
}

fn validate_git_control_record(
    state: &PipelineState,
    record: &CodebaseRecord,
    control: &[u8],
) -> Result<(PathBuf, PathBuf)> {
    let recorded_worktree = record.worktree_path.as_deref().ok_or_else(|| {
        DeadreckonError::InvalidInput(
            "worktree codebase record is missing worktree_path; refusing Git control recovery"
                .to_string(),
        )
    })?;
    let actual_worktree = state
        .working_dir
        .canonicalize()
        .with_path(&state.working_dir)?;
    let recorded_worktree = recorded_worktree
        .canonicalize()
        .with_path(recorded_worktree)?;
    if actual_worktree != recorded_worktree {
        return Err(DeadreckonError::InvalidInput(format!(
            "trusted worktree {} does not match run working directory {}; refusing Git routing",
            recorded_worktree.display(),
            actual_worktree.display()
        )));
    }

    let git_dir = parse_gitdir_control(control, &state.working_dir.join(".git"))?;
    let git_dir = canonical_git_directory(&git_dir, "linked-worktree Git directory")?;
    let common_dir = resolve_common_git_directory(&git_dir, true)?;
    let source_root = record.source_git_root.as_deref().ok_or_else(|| {
        DeadreckonError::InvalidInput(
            "worktree codebase record is missing source_git_root; refusing Git control recovery"
                .to_string(),
        )
    })?;
    let source_common_dir = resolve_source_common_git_directory(source_root)?;
    if common_dir != source_common_dir {
        return Err(DeadreckonError::InvalidInput(format!(
            "linked-worktree Git control resolves to common directory {}, expected trusted source metadata {}; refusing repository redirection",
            common_dir.display(),
            source_common_dir.display()
        )));
    }
    let expected_worktrees = common_dir.join("worktrees");
    if !git_dir.starts_with(&expected_worktrees) || git_dir == expected_worktrees {
        return Err(DeadreckonError::InvalidInput(format!(
            "linked-worktree Git directory {} is outside trusted worktree metadata {}; refusing unexpected control form",
            git_dir.display(),
            expected_worktrees.display()
        )));
    }
    Ok((git_dir, common_dir))
}

fn parse_gitdir_control(bytes: &[u8], control_path: &Path) -> Result<PathBuf> {
    let line = single_control_line(bytes, control_path, "linked-worktree .git control")?;
    let raw = line.strip_prefix(b"gitdir: ").ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "{} is not a linked-worktree gitdir control record",
            control_path.display()
        ))
    })?;
    if raw.is_empty() {
        return Err(DeadreckonError::InvalidInput(format!(
            "{} has an empty gitdir target",
            control_path.display()
        )));
    }
    let raw = std::str::from_utf8(raw).map_err(|_| {
        DeadreckonError::InvalidInput(format!(
            "{} has a non-UTF-8 gitdir target; refusing ambiguous Git routing",
            control_path.display()
        ))
    })?;
    let target = PathBuf::from(raw);
    Ok(if target.is_absolute() {
        target
    } else {
        control_path
            .parent()
            .ok_or_else(|| {
                DeadreckonError::InvalidInput(format!(
                    "Git control path has no parent: {}",
                    control_path.display()
                ))
            })?
            .join(target)
    })
}

fn single_control_line<'a>(bytes: &'a [u8], path: &Path, label: &str) -> Result<&'a [u8]> {
    let without_lf = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let line = without_lf.strip_suffix(b"\r").unwrap_or(without_lf);
    if line.is_empty() || line.contains(&b'\n') || line.contains(&b'\r') || line.contains(&0) {
        return Err(DeadreckonError::InvalidInput(format!(
            "{label} {} has an unexpected multiline or empty form",
            path.display()
        )));
    }
    Ok(line)
}

fn canonical_git_directory(path: &Path, label: &str) -> Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(path).with_path(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(DeadreckonError::InvalidInput(format!(
            "{label} {} must be a real directory, not a symlink or other file type",
            path.display()
        )));
    }
    path.canonicalize().with_path(path)
}

fn resolve_common_git_directory(git_dir: &Path, require_record: bool) -> Result<PathBuf> {
    let record = git_dir.join("commondir");
    let metadata = match std::fs::symlink_metadata(&record) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound && !require_record => {
            return Ok(git_dir.to_path_buf());
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(DeadreckonError::InvalidInput(format!(
                "linked-worktree Git directory {} has no commondir record",
                git_dir.display()
            )));
        }
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: record,
                source,
            });
        }
    };
    require_regular_git_control(&record, &metadata, "Git commondir record")?;
    let bytes = std::fs::read(&record).with_path(&record)?;
    let raw = single_control_line(&bytes, &record, "Git commondir record")?;
    let raw = std::str::from_utf8(raw).map_err(|_| {
        DeadreckonError::InvalidInput(format!(
            "Git commondir record {} is not UTF-8; refusing ambiguous Git routing",
            record.display()
        ))
    })?;
    let common = PathBuf::from(raw);
    let common = if common.is_absolute() {
        common
    } else {
        git_dir.join(common)
    };
    canonical_git_directory(&common, "common Git directory")
}

fn resolve_source_common_git_directory(source_root: &Path) -> Result<PathBuf> {
    let source_control = source_root.join(".git");
    let metadata = std::fs::symlink_metadata(&source_control).with_path(&source_control)?;
    if metadata.file_type().is_symlink() {
        return Err(DeadreckonError::InvalidInput(format!(
            "trusted source Git control {} is a symlink; refusing ambiguous Git routing",
            source_control.display()
        )));
    }
    if metadata.is_dir() {
        return source_control.canonicalize().with_path(source_control);
    }
    if !metadata.is_file() {
        return Err(DeadreckonError::InvalidInput(format!(
            "trusted source Git control {} has an unexpected file type",
            source_control.display()
        )));
    }
    let bytes = std::fs::read(&source_control).with_path(&source_control)?;
    let source_git_dir = parse_gitdir_control(&bytes, &source_control)?;
    let source_git_dir = canonical_git_directory(&source_git_dir, "trusted source Git directory")?;
    resolve_common_git_directory(&source_git_dir, true)
}

fn require_regular_git_control(
    path: &Path,
    metadata: &std::fs::Metadata,
    label: &str,
) -> Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(DeadreckonError::InvalidInput(format!(
            "{label} {} must be a regular file; refusing symlink, directory, or unexpected control form",
            path.display()
        )));
    }
    Ok(())
}

fn archive_changed_git_control(state: &PipelineState, turn: u32, expected: &[u8]) -> Result<()> {
    let control = state.working_dir.join(".git");
    let destination = state
        .run_root
        .join("provider-evidence")
        .join(format!("turn-{turn}"))
        .join("git-control-after-provider");
    if destination.exists() {
        return Ok(());
    }
    let metadata = match std::fs::symlink_metadata(&control) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return write_new_file(
                &destination,
                b"provider removed linked-worktree .git control\n",
            );
        }
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: control,
                source,
            });
        }
    };
    if metadata.is_file() && !metadata.file_type().is_symlink() {
        let actual = std::fs::read(&control).with_path(&control)?;
        if actual == expected {
            return Ok(());
        }
        return write_new_file(&destination, &actual);
    }
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(&control).with_path(&control)?;
        return write_new_file(
            &destination.with_extension("symlink-target"),
            target.to_string_lossy().as_bytes(),
        );
    }
    let kind = if metadata.is_dir() {
        "provider replaced linked-worktree .git control with a directory\n"
    } else {
        "provider replaced linked-worktree .git control with an unexpected file type\n"
    };
    write_new_file(
        &destination.with_extension("unexpected-kind"),
        kind.as_bytes(),
    )
}

fn restore_and_verify_git_control(control: &TrustedGitControl) -> Result<()> {
    let snapshot_metadata =
        std::fs::symlink_metadata(&control.snapshot).with_path(&control.snapshot)?;
    require_regular_git_control(
        &control.snapshot,
        &snapshot_metadata,
        "trusted Git control snapshot",
    )?;
    let snapshot = std::fs::read(&control.snapshot).with_path(&control.snapshot)?;
    if snapshot != control.expected_control {
        return Err(DeadreckonError::InvalidInput(format!(
            "trusted Git control snapshot {} changed after capture",
            control.snapshot.display()
        )));
    }
    match std::fs::symlink_metadata(&control.workspace_control) {
        Ok(metadata) => {
            require_regular_git_control(
                &control.workspace_control,
                &metadata,
                "linked-worktree .git control",
            )?;
            std::fs::remove_file(&control.workspace_control)
                .with_path(&control.workspace_control)?;
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: control.workspace_control.clone(),
                source,
            });
        }
    }
    write_new_file(&control.workspace_control, &control.expected_control)?;
    let restored_metadata = std::fs::symlink_metadata(&control.workspace_control)
        .with_path(&control.workspace_control)?;
    require_regular_git_control(
        &control.workspace_control,
        &restored_metadata,
        "restored linked-worktree .git control",
    )?;
    let restored =
        std::fs::read(&control.workspace_control).with_path(&control.workspace_control)?;
    if restored != control.expected_control {
        return Err(DeadreckonError::InvalidInput(format!(
            "restored linked-worktree Git control {} does not match trusted snapshot",
            control.workspace_control.display()
        )));
    }
    let git_dir = parse_gitdir_control(&restored, &control.workspace_control)?;
    let git_dir = canonical_git_directory(&git_dir, "restored linked-worktree Git directory")?;
    let common_dir = resolve_common_git_directory(&git_dir, true)?;
    if git_dir != control.git_dir || common_dir != control.common_dir {
        return Err(DeadreckonError::InvalidInput(format!(
            "restored linked-worktree Git routing no longer matches trusted metadata; expected {} via {}, found {} via {}",
            control.common_dir.display(),
            control.git_dir.display(),
            common_dir.display(),
            git_dir.display()
        )));
    }
    Ok(())
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "trusted control path has no parent: {}",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent).with_path(parent)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_path(path)?;
    file.write_all(bytes).with_path(path)?;
    file.sync_all().with_path(path)
}

fn capture_trusted_lifecycle_metadata(state: &PipelineState, turn: u32) -> Result<()> {
    let destination = trusted_lifecycle_snapshot_path(state, turn);
    let captured = destination.with_extension("captured");
    if captured.is_file() {
        return Ok(());
    }
    remove_workspace_path(&destination)?;
    let source = state.working_dir.join(".deadreckon");
    if source.is_dir() {
        copy_tree(&source, &destination)?;
    } else {
        std::fs::create_dir_all(&destination).with_path(&destination)?;
    }
    std::fs::write(&captured, b"trusted lifecycle snapshot\n").with_path(captured)
}

#[cfg(test)]
fn commit_worktree_turn(state: &PipelineState, turn: u32, label: &str) -> Result<()> {
    commit_worktree_turn_inner(state, turn, label, true, None)
}

fn commit_worktree_turn_bounded(
    state: &PipelineState,
    turn: u32,
    label: &str,
    deadline: ProviderPhaseDeadline,
    cancellation: &CancellationToken,
) -> Result<TrustedGitPhaseOutcome<()>> {
    let phase = TrustedGitPhase::new(state, deadline, cancellation);
    let result = commit_worktree_turn_inner(state, turn, label, true, Some(phase.clone()));
    phase.finish(result)
}

/// Commit DeadReckon's final generated documents through the same trusted Git
/// boundary as provider edits. The coding sanitizer already restored the
/// pre-provider lifecycle snapshot; retaining it here preserves trusted turn
/// records written by DeadReckon after that point.
#[cfg(test)]
fn commit_finalized_turn(state: &PipelineState, turn: u32) -> Result<()> {
    commit_worktree_turn_inner(state, turn, "finalize_docs", false, None)
}

fn commit_finalized_turn_bounded(
    state: &PipelineState,
    turn: u32,
    deadline: ProviderPhaseDeadline,
    cancellation: &CancellationToken,
) -> Result<TrustedGitPhaseOutcome<()>> {
    let phase = TrustedGitPhase::new(state, deadline, cancellation);
    let result =
        commit_worktree_turn_inner(state, turn, "finalize_docs", false, Some(phase.clone()));
    phase.finish(result)
}

fn commit_worktree_turn_inner(
    state: &PipelineState,
    turn: u32,
    label: &str,
    restore_lifecycle: bool,
    phase: Option<TrustedGitPhase>,
) -> Result<()> {
    if restore_lifecycle {
        restore_lifecycle_metadata(state, turn)?;
    }
    let record = read_turn_codebase_record(state)?;
    if record.mode != CodebaseMode::Worktree {
        return Ok(());
    }
    // Load and restore the pre-provider linked-worktree router before invoking
    // Git. Every Git command below repeats this restoration and also receives
    // an explicit trusted --git-dir, so a provider-created `.git` redirect is
    // evidence, never authority.
    let control = load_trusted_git_control(state, turn, &record, phase)?;
    let base_sha = record.base_sha.as_deref().ok_or_else(|| {
        DeadreckonError::InvalidInput(
            "worktree codebase record is missing base_sha; refusing to commit an unbounded result"
                .to_string(),
        )
    })?;
    refuse_external_git_filters(state, base_sha, &control)?;
    let trusted_head = trusted_head_for_turn(state, turn, base_sha, &control)?;
    // Provider-created commits and index entries are not accepted
    // capabilities. Always rebuild the index from the trusted pre-provider
    // head, while retaining filesystem edits as untrusted input for
    // DeadReckon's own filtered commit. Resetting the whole index also avoids
    // addressing provider-chosen paths through a lossy string conversion.
    // `--no-refresh` is a defense in depth: the default refresh can execute a
    // clean filter while comparing racy worktree timestamps.
    trusted_git_status(
        &control,
        &["reset", "--mixed", "--no-refresh", &trusted_head],
    )?;
    let branch = record.branch_name.as_deref().ok_or_else(|| {
        DeadreckonError::InvalidInput(
            "worktree codebase record is missing branch_name; refusing detached delivery"
                .to_string(),
        )
    })?;
    let branch_ref = format!("refs/heads/{branch}");
    trusted_git_status(&control, &["update-ref", &branch_ref, &trusted_head])?;
    trusted_git_status(&control, &["symbolic-ref", "HEAD", &branch_ref])?;
    sanitize_evidence_only_paths(state, turn, base_sha, &control)?;
    stage_trusted_delivery_paths(state, &control)?;
    refuse_gitlinks(&control)?;
    if trusted_git_quiet(&control, &["diff", "--cached", "--quiet"])? {
        verify_evidence_only_paths_clean(base_sha, &control)?;
        return Ok(());
    }
    let disabled_hooks = state.run_root.join("disabled-git-hooks");
    std::fs::create_dir_all(&disabled_hooks).with_path(&disabled_hooks)?;
    let disabled_hooks = disabled_hooks.to_string_lossy();
    trusted_git_status(
        &control,
        &[
            "-c",
            &format!("core.hooksPath={disabled_hooks}"),
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.email=deadreckon@example.invalid",
            "-c",
            "user.name=deadreckon",
            "commit",
            "-m",
            &format!("turn {turn}: {label}"),
        ],
    )?;
    verify_evidence_only_paths_clean(base_sha, &control)
}

fn stage_trusted_delivery_paths(state: &PipelineState, control: &TrustedGitControl) -> Result<()> {
    let policy = deadreckon_core::require_workspace_capture_policy(state)?;
    let capture = deadreckon_core::capture_workspace_strict(
        &state.working_dir,
        &policy,
        deadreckon_core::CaptureProjection::Deliverable,
        deadreckon_core::CapturePurpose::DeliverableIndex,
    )?;
    capture.require_complete("trusted Git delivery staging")?;
    let mut paths = capture
        .entries
        .into_iter()
        .map(|entry| entry.relative)
        .collect::<BTreeSet<_>>();

    // A deleted tracked path is absent from the filesystem capture. Read it
    // from the trusted index rebuilt above, then apply the same delivery
    // boundary before passing exact literal pathspecs to `git add`.
    let indexed = trusted_git_output(control, &["ls-files", "-z", "--cached", "--"])?;
    if !indexed.status.success() {
        return Err(DeadreckonError::InvalidInput(format!(
            "trusted Git index inventory failed: {}{}",
            String::from_utf8_lossy(&indexed.stdout),
            String::from_utf8_lossy(&indexed.stderr)
        )));
    }
    paths.extend(
        nul_separated_paths(&indexed.stdout)?
            .into_iter()
            .filter(|path| trusted_delivery_path(path)),
    );
    if paths.is_empty() {
        return Ok(());
    }
    let path_vec = paths.into_iter().collect::<Vec<_>>();
    let input = git_path_input(path_vec.iter())?;
    let output = trusted_git_output_with_input(
        control,
        &[
            "--literal-pathspecs",
            "add",
            "-A",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ],
        &input,
    )?;
    if output.status.success() {
        Ok(())
    } else {
        Err(DeadreckonError::InvalidInput(format!(
            "trusted Git delivery staging failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

fn trusted_delivery_path(path: &Path) -> bool {
    is_deliverable_workspace_path(path)
        || (classify_workspace_path(path) == WorkspacePathClass::RuntimeOnly
            && deadreckon_core::runtime_output_root(path).is_some())
}

fn read_turn_codebase_record(state: &PipelineState) -> Result<CodebaseRecord> {
    let paths = paths_for_state(state)?;
    if paths.job_json(&state.run_id).is_file() {
        // A Job is born under the strict durable contract. Missing trusted
        // routing is corruption, not an invitation to bless an agent-visible
        // workspace record during restart.
        read_trusted_codebase_record(&state.run_root)
    } else {
        // Compatibility runs may predate the trusted record. They can migrate
        // once, before provider execution, through the historical workspace
        // copy.
        read_run_codebase_record(&state.run_root, &state.working_dir)
    }
}

fn restore_lifecycle_metadata(state: &PipelineState, turn: u32) -> Result<()> {
    let trusted = trusted_lifecycle_snapshot_path(state, turn);
    let working = state.working_dir.join(".deadreckon");
    if !trusted.exists() {
        return Err(DeadreckonError::NotFound(format!(
            "trusted lifecycle snapshot {}",
            trusted.display()
        )));
    }
    remove_workspace_path(&working)?;
    copy_tree(&trusted, &working)
}

fn trusted_head_for_turn(
    state: &PipelineState,
    turn: u32,
    base_sha: &str,
    control: &TrustedGitControl,
) -> Result<String> {
    let path = trusted_turn_head_path(state, turn);
    let candidate = std::fs::read_to_string(&path)
        .ok()
        .map(|head| head.trim().to_string())
        .filter(|head| !head.is_empty())
        .unwrap_or_else(|| base_sha.to_string());

    // If this is a resumed legacy run whose previous commits ever touched a
    // non-deliverable path, rebuild from the approved base. Aggregate tree
    // cleanliness alone is insufficient because a private blob can survive
    // in an add-then-delete commit pair.
    if !non_deliverable_history_paths(control, base_sha, &candidate)?.is_empty() {
        return Ok(base_sha.to_string());
    }
    Ok(candidate)
}

fn sanitize_evidence_only_paths(
    state: &PipelineState,
    turn: u32,
    base_sha: &str,
    control: &TrustedGitControl,
) -> Result<()> {
    for root in evidence_only_roots() {
        let source = state.working_dir.join(root);
        if source.exists() || std::fs::symlink_metadata(&source).is_ok() {
            archive_evidence_path(state, turn, root, &source)?;
            remove_workspace_path(&source)?;
        }

        let base_entries = trusted_git_stdout(
            control,
            &["ls-tree", "-r", "--name-only", base_sha, "--", root],
        )?;
        if base_entries.is_empty() {
            let indexed = trusted_git_stdout(control, &["ls-files", "--", root])?;
            if !indexed.is_empty() {
                trusted_git_status(control, &["add", "-A", "--", root])?;
            }
        } else {
            trusted_git_status(
                control,
                &[
                    "restore",
                    "--source",
                    base_sha,
                    "--staged",
                    "--worktree",
                    "--",
                    root,
                ],
            )?;
        }
    }
    Ok(())
}

fn archive_evidence_path(
    state: &PipelineState,
    turn: u32,
    root: &str,
    source: &Path,
) -> Result<()> {
    let evidence_destination = state
        .run_root
        .join("provider-evidence")
        .join(format!("turn-{turn}"))
        .join("workspace")
        .join(root);
    copy_evidence_path(source, &evidence_destination, root)?;
    let raw_snapshot_destination = state
        .run_root
        .join("snapshots")
        .join(format!("turn-{turn}-provider-raw"))
        .join(root);
    copy_evidence_path(source, &raw_snapshot_destination, root)
}

fn copy_evidence_path(source: &Path, destination: &Path, root: &str) -> Result<()> {
    let metadata = std::fs::symlink_metadata(source).with_path(source)?;
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(source).with_path(source)?;
        let parent = destination.parent().ok_or_else(|| {
            DeadreckonError::InvalidInput(format!(
                "provider evidence path has no parent: {}",
                destination.display()
            ))
        })?;
        std::fs::create_dir_all(parent).with_path(parent)?;
        let target_record = parent.join(format!("{root}.symlink-target"));
        return std::fs::write(&target_record, target.to_string_lossy().as_bytes())
            .with_path(target_record);
    }
    if metadata.is_dir() {
        return copy_tree(source, destination);
    }
    let parent = destination.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "provider evidence path has no parent: {}",
            destination.display()
        ))
    })?;
    std::fs::create_dir_all(parent).with_path(parent)?;
    std::fs::copy(source, destination)
        .with_path(destination)
        .map(|_| ())
}

fn remove_workspace_path(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path).with_path(path)
    } else {
        std::fs::remove_file(path).with_path(path)
    }
}

fn verify_evidence_only_paths_clean(base_sha: &str, control: &TrustedGitControl) -> Result<()> {
    let head = trusted_git_stdout(control, &["rev-parse", "HEAD"])?;
    let prohibited = non_deliverable_history_paths(control, base_sha, &head)?;
    if !prohibited.is_empty() {
        return Err(DeadreckonError::InvalidInput(format!(
            "non-deliverable paths remain in result history: {}; refusing to seal provider-private or runtime artifacts",
            display_git_paths(&prohibited)
        )));
    }
    let range = format!("{base_sha}..{head}");
    for root in evidence_only_roots() {
        let history = trusted_git_stdout(control, &["log", "--format=%H", &range, "--", root])?;
        let status = trusted_git_stdout(
            control,
            &[
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
                "--",
                root,
            ],
        )?;
        if !trusted_git_quiet(control, &["diff", "--quiet", &range, "--", root])?
            || !history.is_empty()
            || !status.is_empty()
        {
            return Err(DeadreckonError::InvalidInput(format!(
                "evidence-only path {root} remains in the result branch; refusing to seal provider-private artifacts"
            )));
        }
    }
    Ok(())
}

fn refuse_gitlinks(control: &TrustedGitControl) -> Result<()> {
    let output = trusted_git_output(control, &["ls-files", "--stage", "-z", "--"])?;
    if !output.status.success() {
        return Err(DeadreckonError::InvalidInput(format!(
            "git index-kind inventory failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let gitlinks = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let tab = entry.iter().position(|byte| *byte == b'\t')?;
            entry[..tab]
                .starts_with(b"160000 ")
                .then_some(&entry[tab + 1..])
        })
        .map(path_from_git_bytes)
        .collect::<Result<Vec<_>>>()?;
    if gitlinks.is_empty() {
        return Ok(());
    }
    Err(DeadreckonError::InvalidInput(format!(
        "strict result contains unsupported Git submodule entries: {}; stop for review instead of omitting gitlinks from the receipt",
        display_git_paths(&gitlinks)
    )))
}

fn refuse_external_git_filters(
    state: &PipelineState,
    base_sha: &str,
    control: &TrustedGitControl,
) -> Result<()> {
    // This check must precede reset, restore, add, or any other Git command
    // which can convert worktree content. `check-attr` only resolves metadata;
    // it does not execute the configured clean, smudge, or process command.
    let current = deadreckon_core::flight::build_workspace_guard_file_index_for_state(state)?;
    let mut paths = current.files.into_keys().collect::<BTreeSet<_>>();
    let base_tree = trusted_git_output(
        control,
        &["ls-tree", "-r", "--name-only", "-z", base_sha, "--"],
    )?;
    if !base_tree.status.success() {
        return Err(DeadreckonError::InvalidInput(format!(
            "approved-base Git attribute inventory failed: {}{}",
            String::from_utf8_lossy(&base_tree.stdout),
            String::from_utf8_lossy(&base_tree.stderr)
        )));
    }
    paths.extend(nul_separated_paths(&base_tree.stdout)?);
    let input = git_path_input(paths.iter())?;
    if input.is_empty() {
        return Ok(());
    }
    let output =
        trusted_git_output_with_input(control, &["check-attr", "-z", "--stdin", "filter"], &input)?;
    if !output.status.success() {
        return Err(DeadreckonError::InvalidInput(format!(
            "git filter-attribute inventory failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    let fields = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    if fields.len() % 3 != 0 {
        return Err(DeadreckonError::InvalidInput(
            "git filter-attribute inventory returned a malformed record".to_string(),
        ));
    }
    let filtered = fields
        .chunks_exact(3)
        .filter(|record| record[2] != b"unspecified")
        .map(|record| path_from_git_bytes(record[0]))
        .collect::<Result<Vec<_>>>()?;
    if filtered.is_empty() {
        return Ok(());
    }
    Err(DeadreckonError::InvalidInput(format!(
        "strict result applies external Git filter attributes to workspace or approved-base paths: {}; refusing to execute mutable clean, smudge, or process commands",
        display_git_paths(&filtered)
    )))
}

fn non_deliverable_history_paths(
    control: &TrustedGitControl,
    base_sha: &str,
    head: &str,
) -> Result<Vec<PathBuf>> {
    let ancestor = trusted_git_output(control, &["merge-base", "--is-ancestor", base_sha, head])?;
    if !ancestor.status.success() {
        return Err(DeadreckonError::InvalidInput(format!(
            "approved base {base_sha} is not an ancestor of result {head}; refusing unbounded result history"
        )));
    }

    let range = format!("{base_sha}..{head}");
    let revisions = trusted_git_stdout(control, &["rev-list", "--reverse", &range])?;
    let mut paths = BTreeSet::new();
    for revision in revisions
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        // `-m` expands every merge parent. Aggregate `git log --name-only`
        // output can omit paths introduced only through a merge.
        let output = trusted_git_output(
            control,
            &[
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-only",
                "--no-renames",
                "-r",
                "-m",
                "-z",
                revision,
                "--",
            ],
        )?;
        if !output.status.success() {
            return Err(DeadreckonError::InvalidInput(format!(
                "git result-history inventory failed for {revision}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        paths.extend(nul_separated_paths(&output.stdout)?);
    }

    Ok(paths
        .into_iter()
        .filter(|path| !is_deliverable_workspace_path(path))
        .collect())
}

fn nul_separated_paths(output: &[u8]) -> Result<Vec<PathBuf>> {
    output
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
        .map(path_from_git_bytes)
        .collect()
}

#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)] // The non-Unix implementation can reject unrepresentable paths.
fn path_from_git_bytes(raw: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    Ok(PathBuf::from(OsString::from_vec(raw.to_vec())))
}

#[cfg(not(unix))]
fn path_from_git_bytes(raw: &[u8]) -> Result<PathBuf> {
    String::from_utf8(raw.to_vec())
        .map(PathBuf::from)
        .map_err(|_| {
            DeadreckonError::InvalidInput(
                "Git returned a result path that cannot be represented on this platform"
                    .to_string(),
            )
        })
}

fn display_git_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .take(8)
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn git_path_input<'a>(paths: impl Iterator<Item = &'a PathBuf>) -> Result<Vec<u8>> {
    let mut input = Vec::new();
    for path in paths {
        append_git_path_bytes(&mut input, path)?;
        input.push(0);
    }
    Ok(input)
}

#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)] // The non-Unix implementation can reject unrepresentable paths.
fn append_git_path_bytes(output: &mut Vec<u8>, path: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;

    output.extend_from_slice(path.as_os_str().as_bytes());
    Ok(())
}

#[cfg(not(unix))]
fn append_git_path_bytes(output: &mut Vec<u8>, path: &Path) -> Result<()> {
    let path = path.to_str().ok_or_else(|| {
        DeadreckonError::InvalidInput(
            "deliverable path cannot be represented for Git on this platform".to_string(),
        )
    })?;
    output.extend_from_slice(path.as_bytes());
    Ok(())
}

fn trusted_git_output(control: &TrustedGitControl, args: &[&str]) -> Result<std::process::Output> {
    restore_and_verify_git_control(control)?;
    let git_dir = control.git_dir.to_str().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "trusted Git directory {} is not UTF-8; refusing ambiguous command routing",
            control.git_dir.display()
        ))
    })?;
    let workspace = control.workspace.to_str().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "trusted worktree {} is not UTF-8; refusing ambiguous command routing",
            control.workspace.display()
        ))
    })?;
    let git_dir_arg = format!("--git-dir={git_dir}");
    let work_tree_arg = format!("--work-tree={workspace}");
    let mut routed = Vec::with_capacity(args.len() + 2);
    routed.push(git_dir_arg.as_str());
    routed.push(work_tree_arg.as_str());
    routed.extend_from_slice(args);
    let output = if let Some(phase) = &control.phase {
        let cancellation_requested = || phase.cancellation.is_cancelled();
        let deadline = GitCommandDeadline::new(
            phase.deadline.work_expires_at.into_std(),
            phase.deadline.cleanup_budget,
            &cancellation_requested,
        )
        .with_authority_dir(&phase.authority_dir);
        let outcome = run_git_bounded(&control.workspace, &routed, deadline)?;
        finish_bounded_trusted_git(phase, outcome)?
    } else {
        run_git(&control.workspace, &routed)?
    };
    restore_and_verify_git_control(control)?;
    Ok(output)
}

fn trusted_git_output_with_input(
    control: &TrustedGitControl,
    args: &[&str],
    input: &[u8],
) -> Result<std::process::Output> {
    restore_and_verify_git_control(control)?;
    let git_dir = control.git_dir.to_str().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "trusted Git directory {} is not UTF-8; refusing ambiguous command routing",
            control.git_dir.display()
        ))
    })?;
    let workspace = control.workspace.to_str().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "trusted worktree {} is not UTF-8; refusing ambiguous command routing",
            control.workspace.display()
        ))
    })?;
    let git_dir_arg = format!("--git-dir={git_dir}");
    let work_tree_arg = format!("--work-tree={workspace}");
    let mut routed = Vec::with_capacity(args.len() + 2);
    routed.push(git_dir_arg.as_str());
    routed.push(work_tree_arg.as_str());
    routed.extend_from_slice(args);
    let output = if let Some(phase) = &control.phase {
        let cancellation_requested = || phase.cancellation.is_cancelled();
        let deadline = GitCommandDeadline::new(
            phase.deadline.work_expires_at.into_std(),
            phase.deadline.cleanup_budget,
            &cancellation_requested,
        )
        .with_authority_dir(&phase.authority_dir);
        let outcome = run_git_with_input_bounded(&control.workspace, &routed, input, deadline)?;
        finish_bounded_trusted_git(phase, outcome)?
    } else {
        run_git_with_input(&control.workspace, &routed, input)?
    };
    restore_and_verify_git_control(control)?;
    Ok(output)
}

fn finish_bounded_trusted_git(
    phase: &TrustedGitPhase,
    outcome: BoundedGitOutcome,
) -> Result<std::process::Output> {
    let interruption = match outcome {
        BoundedGitOutcome::Completed(output) => return Ok(output),
        BoundedGitOutcome::WorkExpired => TrustedGitInterruption::WorkExpired {
            cleanup: ProviderCleanup::Proven,
        },
        BoundedGitOutcome::Cancelled => TrustedGitInterruption::Cancelled {
            cleanup: ProviderCleanup::Proven,
        },
        BoundedGitOutcome::SupervisionFailed { detail } => {
            return Err(DeadreckonError::InvalidInput(format!(
                "trusted Git subprocess supervision failed after cleanup was proven: {detail}"
            )));
        }
        BoundedGitOutcome::CleanupIncomplete {
            boundary,
            authority,
            detail,
        } => TrustedGitInterruption::LostContainment {
            boundary,
            authority: authority.record_path,
            detail,
        },
    };
    phase.record(interruption)?;
    Err(DeadreckonError::InvalidInput(
        "trusted Git subprocess crossed its controller boundary".to_string(),
    ))
}

fn trusted_git_quiet(control: &TrustedGitControl, args: &[&str]) -> Result<bool> {
    let output = trusted_git_output(control, args)?;
    Ok(output.status.success())
}

fn trusted_git_stdout(control: &TrustedGitControl, args: &[&str]) -> Result<String> {
    let output = trusted_git_output(control, args)?;
    if !output.status.success() {
        return Err(DeadreckonError::InvalidInput(format!(
            "trusted git {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn trusted_git_status(control: &TrustedGitControl, args: &[&str]) -> Result<()> {
    let output = trusted_git_output(control, args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(DeadreckonError::InvalidInput(format!(
            "trusted git {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

#[derive(Debug, Clone)]
struct ResolvedGateToolchain {
    controller: PathBuf,
    evaluator: PathBuf,
    identity: Option<GateEvaluatorIdentity>,
    identity_sha256: Option<String>,
}

#[derive(Debug)]
struct PreparedGateDocker {
    execution: DockerExecution,
    record_path: PathBuf,
}

#[allow(clippy::too_many_arguments)]
fn prepare_gate_docker(
    state: &PipelineState,
    paths: &DeadreckonPaths,
    toolchain: &ResolvedGateToolchain,
    resolved_backend: SandboxBackend,
    purpose: &str,
    launch_id: &str,
    attempt: u32,
    owner_launch_id: Option<String>,
    guarded_process_record: &Path,
) -> Result<Option<PreparedGateDocker>> {
    if resolved_backend != SandboxBackend::Docker {
        return Ok(None);
    }
    let identity = toolchain.identity.as_ref().ok_or_else(|| {
        DeadreckonError::InvalidInput(
            "strict Docker gate is missing immutable evaluator identity".to_string(),
        )
    })?;
    let docker = identity.docker.as_ref().ok_or_else(|| {
        DeadreckonError::InvalidInput(
            "strict Docker gate is missing immutable image identity".to_string(),
        )
    })?;
    if docker.guest_path != Path::new(deadreckon_sandbox::DOCKER_SIDECAR_CONTAINER_PROGRAM)
        || docker.guest_path != Path::new(deadreckon_protocol::DOCKER_GATE_GUEST_PATH)
    {
        return Err(DeadreckonError::InvalidInput(
            "approved Docker evaluator guest path does not match the fixed sandbox boundary"
                .to_string(),
        ));
    }
    let platform = match docker.platform.as_str() {
        "linux/amd64" => DockerPlatform::LinuxAmd64,
        "linux/arm64" => DockerPlatform::LinuxArm64,
        other => {
            return Err(DeadreckonError::InvalidInput(format!(
                "unsupported approved Docker evaluator platform {other}"
            )));
        }
    };
    let image = DockerImage::new(docker.image_id.clone(), platform)
        .map_err(|error| sandbox_error(&error))?;
    let observed = inspect_docker_image(OsStr::new(&docker.image_id))
        .map_err(|error| sandbox_error(&error))?;
    if observed != image {
        return Err(DeadreckonError::InvalidInput(
            "cached Docker image identity changed after Job approval".to_string(),
        ));
    }
    let safe_purpose = purpose
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
        .collect::<String>();
    let launch_slug = launch_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(16)
        .collect::<String>();
    let container_name = format!("deadreckon-{safe_purpose}-{attempt}-{launch_slug}");
    let record_dir = paths.job_dir(&state.run_id).join("docker-executions");
    std::fs::create_dir_all(&record_dir).with_path(&record_dir)?;
    let process_record_dir = guarded_process_record.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "guarded process record has no parent: {}",
            guarded_process_record.display()
        ))
    })?;
    std::fs::create_dir_all(process_record_dir).with_path(process_record_dir)?;
    let cid_file =
        process_record_dir.join(format!("docker-{safe_purpose}-{attempt}-{launch_slug}.cid"));
    let record_path = record_dir.join(format!("{safe_purpose}-{attempt}-{launch_slug}.json"));
    let execution = DockerExecution::new(
        image,
        &toolchain.evaluator,
        container_name,
        cid_file,
        state.run_id.clone(),
        launch_id.to_string(),
        attempt,
        owner_launch_id,
    )
    .map_err(|error| sandbox_error(&error))?;
    write_docker_execution_record(&record_path, &execution)
        .map_err(|error| sandbox_error(&error))?;
    Ok(Some(PreparedGateDocker {
        execution,
        record_path,
    }))
}

fn finish_gate_docker(prepared: Option<&PreparedGateDocker>) -> Result<()> {
    let Some(prepared) = prepared else {
        return Ok(());
    };
    reconcile_docker_execution_record(&prepared.record_path).map_err(|error| sandbox_error(&error))
}

fn resolve_gate_toolchain(
    state: &PipelineState,
    paths: &DeadreckonPaths,
    strict_job: bool,
    resolved_backend: SandboxBackend,
) -> Result<ResolvedGateToolchain> {
    if !strict_job {
        let gate = gate_binary_path()?;
        return Ok(ResolvedGateToolchain {
            controller: gate.clone(),
            evaluator: gate,
            identity: None,
            identity_sha256: None,
        });
    }
    let job = deadreckon_core::load_job(paths, &state.run_id)?;
    let authority_path = paths.job_authority(&state.run_id);
    if deadreckon_core::flight::sha256_file(&authority_path)? != job.authority_sha256 {
        return Err(DeadreckonError::InvalidInput(
            "Job authority changed before deterministic gate launch".to_string(),
        ));
    }
    let authority_raw = std::fs::read(&authority_path).with_path(&authority_path)?;
    let authority: JobAuthority =
        serde_json::from_slice(&authority_raw).map_err(|source| DeadreckonError::Json {
            path: authority_path,
            source,
        })?;
    let identity = job
        .policy
        .execution
        .as_ref()
        .and_then(|execution| execution.gate_evaluator.clone())
        .ok_or_else(|| {
            DeadreckonError::InvalidInput(
                "strict Job is missing its immutable gate evaluator identity".to_string(),
            )
        })?;
    let identity_sha256 = deadreckon_core::gate_evaluator_identity_sha256(&identity)?;
    if authority.gate_evaluator_sha256.as_deref() != Some(identity_sha256.as_str()) {
        return Err(DeadreckonError::InvalidInput(
            "strict Job gate evaluator identity no longer matches approved authority".to_string(),
        ));
    }
    let docker_coherent = if resolved_backend == SandboxBackend::Docker {
        identity.docker.is_some()
    } else {
        identity.docker.is_none()
    };
    if !docker_coherent {
        return Err(DeadreckonError::InvalidInput(format!(
            "approved gate evaluator is incompatible with resolved {resolved_backend} containment"
        )));
    }
    if identity.controller.os != current_gate_os()
        || identity.controller.arch != current_gate_arch()
    {
        return Err(DeadreckonError::InvalidInput(format!(
            "approved gate controller targets {}/{}, but this supervisor is {}/{}",
            identity.controller.os,
            identity.controller.arch,
            current_gate_os(),
            current_gate_arch()
        )));
    }
    let toolchain = ResolvedGateToolchain {
        controller: paths.job_frozen_controller_gate(&state.run_id),
        evaluator: paths.job_frozen_evaluator_gate(&state.run_id),
        identity: Some(identity),
        identity_sha256: Some(identity_sha256),
    };
    revalidate_gate_toolchain(&toolchain)?;
    Ok(toolchain)
}

fn revalidate_gate_toolchain(toolchain: &ResolvedGateToolchain) -> Result<()> {
    let Some(identity) = toolchain.identity.as_ref() else {
        return Ok(());
    };
    if identity.schema_version != deadreckon_protocol::GATE_EVALUATOR_IDENTITY_SCHEMA_VERSION
        || identity.protocol_version != deadreckon_protocol::GATE_EVALUATOR_PROTOCOL_VERSION
    {
        return Err(DeadreckonError::InvalidInput(format!(
            "approved gate evaluator protocol {} is incompatible with this supervisor (requires {}); start a fresh Job with a matching DeadReckon installation",
            identity.protocol_version,
            deadreckon_protocol::GATE_EVALUATOR_PROTOCOL_VERSION
        )));
    }
    validate_frozen_gate(&toolchain.controller, &identity.controller.sha256)?;
    validate_frozen_gate(&toolchain.evaluator, &identity.evaluator.sha256)
}

fn validate_frozen_gate(path: &Path, expected_sha256: &str) -> Result<()> {
    let before = std::fs::symlink_metadata(path).with_path(path)?;
    if !before.file_type().is_file() || before.file_type().is_symlink() {
        return Err(DeadreckonError::InvalidInput(format!(
            "frozen gate artifact is not a regular non-symlink file: {}",
            path.display()
        )));
    }
    let actual = deadreckon_core::flight::sha256_file(path)?;
    let after = std::fs::symlink_metadata(path).with_path(path)?;
    if !stable_gate_artifact_metadata(&before, &after) || actual != expected_sha256 {
        return Err(DeadreckonError::InvalidInput(format!(
            "frozen gate artifact changed after Job approval: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn stable_gate_artifact_metadata(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.file_type().is_file()
        && right.file_type().is_file()
}

#[cfg(not(unix))]
fn stable_gate_artifact_metadata(left: &std::fs::Metadata, right: &std::fs::Metadata) -> bool {
    left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
        && left.file_type().is_file()
        && right.file_type().is_file()
}

fn current_gate_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        "windows" => "windows",
        other => other,
    }
}

fn current_gate_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        other => other,
    }
}

fn gate_binary_path() -> Result<PathBuf> {
    let current = std::env::current_exe().map_err(|source| DeadreckonError::Io {
        path: PathBuf::from("current-exe"),
        source,
    })?;
    let name = format!("dr-gate{}", std::env::consts::EXE_SUFFIX);
    let mut roots = current
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect::<Vec<_>>();
    if current.parent().and_then(Path::file_name) == Some(std::ffi::OsStr::new("deps"))
        && let Some(root) = current.parent().and_then(Path::parent)
    {
        roots.push(root.to_path_buf());
    }
    for root in roots {
        let gate = root.join(&name);
        if gate.exists() {
            return Ok(gate);
        }
    }
    Err(DeadreckonError::NotFound(format!(
        "{name} binary next to {}",
        current.display()
    )))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use deadreckon_providers::{
        ProviderCleanup, ProviderConfigFile, ProviderEntry, ProviderKind, ProviderPhaseDeadline,
        ProviderRouter,
    };
    use deadreckon_sandbox::{SandboxBackend, SandboxSpec, WorkspaceAccess};
    use serde_json::{Value, json};
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    use crate::seam::{
        SeamCommandConfig, SeamKind, SeamOutcome, SeamPhaseOutcome, SeamRunCtx, SeamsConfig,
        read_seams_config,
    };

    use deadreckon_core::events::RunEventBus;
    use deadreckon_core::flight::{
        FlightSessionStatus, list_checkpoint_manifests, read_flight_events, read_flight_manifest,
    };
    use deadreckon_core::gate::{run_acceptance_gate_and_write_marker, validate_acceptance_marker};
    use deadreckon_core::paths::DeadreckonPaths;
    use deadreckon_core::state::{
        PhaseId, PhaseStatus, PipelineState, RunOptions, RunStatus, create_run, spend_summary,
    };
    use deadreckon_core::{
        CodebaseMode, CodebaseRecord, DeadreckonError, TurnDocInput, append_turn_doc,
        implementation_notes_path, snapshot_working,
    };
    use deadreckon_protocol::{
        FlightEventKind, RunEventKind, RunId, SemanticDecision, SemanticJudgment,
    };

    use super::{
        GateLaunchOwner, NarratorConfig, ParentRepairCandidate, ParentRepairCandidateContext,
        ProviderInterruption, RunLoopConfig, RunLoopDocsConfig, RunLoopOutcome, RunWorkBoundary,
        RunWorkClock, RunWorkExpiry, SandboxedPhaseOutcome, SemanticCompletionDisposition,
        TrustedGitPhaseOutcome, append_provider_approval_traces, append_tool_refusal,
        approved_gate_network_access, bash_policy_refusal, begin_verification,
        build_cli_subagent_prompt, build_prompt, capture_trusted_turn_head,
        capture_trusted_turn_head_bounded, changed_files_since_snapshot,
        classify_cli_no_deliverable_changes, commit_finalized_turn, commit_worktree_turn,
        complete_verification, deliverable_changed_files, ensure_sandbox_toml,
        event_sink_must_stop, fail_verification, implementation_notes_ready_or_request_followup,
        is_direct_api_provider_kind, load_or_reconstruct_history,
        load_tool_policy_from_sandbox_toml, load_trusted_git_control,
        non_deliverable_history_paths, persist_parent_repair_candidate, persist_work_boundary,
        policy_seam_refusal, policy_seam_refusal_message, promote_if_ready,
        provider_failure_disposition, provider_output_name, read_turn_codebase_record,
        record_provider_interruption, record_semantic_judge_accounting, refuse_gitlinks,
        revise_verification, run_parent_repair_turn_loop, run_sandboxed_work_phase, run_turn_loop,
        safe_working_path, safe_working_path_with_policy, save_history,
        seal_achieved_semantic_completion, semantic_completion_disposition,
        snapshot_working_bounded, spawn_event_sink_forwarder, wait_for_provider_retry,
        write_workspace_file_no_follow,
    };

    fn load_history_with_work_clock(
        state: &mut PipelineState,
        from_turn: Option<u32>,
    ) -> deadreckon_core::Result<Vec<String>> {
        let work_clock = RunWorkClock::new(state)?;
        load_or_reconstruct_history(state, from_turn, &work_clock)
    }

    #[test]
    fn gate_launch_owner_requires_a_durable_attempt_and_uuid() {
        assert!(GateLaunchOwner::new(0, uuid::Uuid::new_v4().to_string()).is_err());
        assert!(GateLaunchOwner::new(1, "not-a-launch-id").is_err());
        assert!(GateLaunchOwner::new(1, uuid::Uuid::new_v4().to_string()).is_ok());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn strict_macos_gate_uses_controller_resolved_toolchain_paths() {
        let mut env = std::collections::BTreeMap::new();
        let mut read_allowlist = Vec::new();

        super::configure_macos_strict_toolchain(None, &mut env, &mut read_allowlist)
            .expect("controller resolves strict toolchain");

        let clang = PathBuf::from(env.get("CC").expect("controller resolves clang"));
        assert!(clang.is_absolute());
        assert!(clang.is_file());
        assert_ne!(clang, PathBuf::from("/usr/bin/cc"));
        assert_ne!(clang, PathBuf::from("/usr/bin/clang"));
        assert!(read_allowlist.contains(&clang));

        let linker_key = match std::env::consts::ARCH {
            "aarch64" => "CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER",
            "x86_64" => "CARGO_TARGET_X86_64_APPLE_DARWIN_LINKER",
            other => panic!("unexpected macOS architecture {other}"),
        };
        assert_eq!(env.get(linker_key), env.get("CC"));

        let sdk_root = PathBuf::from(env.get("SDKROOT").expect("controller resolves SDK"));
        assert!(sdk_root.is_absolute());
        assert!(sdk_root.is_dir());
        assert!(read_allowlist.contains(&sdk_root));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn strict_macos_gate_redirects_bare_mktemp_into_its_disposable_runtime() {
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let runtime = temp.path().join("runtime");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut env = std::collections::BTreeMap::new();
        let mut read_allowlist = Vec::new();
        let mut write_allowlist = vec![workspace.clone()];
        super::prepare_strict_gate_environment(
            &workspace,
            &runtime,
            SandboxBackend::SandboxExec,
            &mut env,
            &mut read_allowlist,
            &mut write_allowlist,
        )
        .expect("strict gate environment");
        let expected_tmp = runtime
            .join("tmp")
            .canonicalize()
            .expect("canonical gate tmp");
        let profile_dir = temp.path().join("profile");
        let output = super::run_sandbox(deadreckon_sandbox::SandboxSpec {
            backend: SandboxBackend::SandboxExec,
            docker: None,
            cwd: workspace.clone(),
            program: std::ffi::OsString::from("/bin/sh"),
            args: vec![
                std::ffi::OsString::from("-c"),
                std::ffi::OsString::from(
                    "tool=$(command -v mktemp); if printf '#!/bin/sh\\nexit 0\\n' >\"$tool\" 2>\"$EXPECTED_GATE_TMP/attack.err\" || rm -f \"$tool\" 2>\"$EXPECTED_GATE_TMP/attack.err\"; then exit 42; fi; TMPDIR=\"$PWD/hostile-tmp\"; export TMPDIR; mkdir -p \"$TMPDIR\"; created=$(mktemp); case \"$created\" in \"$EXPECTED_GATE_TMP\"/*) ;; *) printf '%s' \"$created\"; exit 41 ;; esac; printf '%s' \"$created\"",
                ),
            ],
            stdin: None,
            env: {
                env.insert(
                    "EXPECTED_GATE_TMP".to_string(),
                    expected_tmp.to_string_lossy().into_owned(),
                );
                env
            },
            allow_network: false,
            pid_file: None,
            cancellation_token: None,
            profile_dir: Some(profile_dir.clone()),
            read_allowlist,
            write_allowlist,
            read_denylist: Vec::new(),
            write_denylist: Vec::new(),
            network_allowlist: Vec::new(),
            workspace_access: deadreckon_sandbox::WorkspaceAccess::Disposable,
            cleanup_process_group: true,
            guarded_launch: None,
        })
        .await
        .expect("contained mktemp check");

        let profile = std::fs::read_to_string(profile_dir.join("profile.sb"))
            .expect("captured sandbox profile");
        assert_eq!(output.status_code, Some(0), "{output:#?}\n{profile}");
        assert!(
            Path::new(&output.stdout).starts_with(expected_tmp),
            "{output:#?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn strict_macos_gate_runs_controller_resolved_python_and_dev_null() {
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let runtime = temp.path().join("runtime");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut env = std::collections::BTreeMap::new();
        let mut read_allowlist = Vec::new();
        let mut write_allowlist = vec![workspace.clone()];
        super::prepare_strict_gate_environment(
            &workspace,
            &runtime,
            SandboxBackend::SandboxExec,
            &mut env,
            &mut read_allowlist,
            &mut write_allowlist,
        )
        .expect("strict gate environment");
        let expected_python = super::xcrun_path(&["--find", "python3"], false)
            .expect("controller resolves Xcode python3");
        let profile_dir = temp.path().join("profile");
        env.insert(
            "EXPECTED_PYTHON".to_string(),
            expected_python.to_string_lossy().into_owned(),
        );

        let output = super::run_sandbox(deadreckon_sandbox::SandboxSpec {
            backend: SandboxBackend::SandboxExec,
            docker: None,
            cwd: workspace.clone(),
            program: std::ffi::OsString::from("/bin/sh"),
            args: vec![
                std::ffi::OsString::from("-c"),
                std::ffi::OsString::from(
                    "test \"$(readlink \"$(command -v python3)\")\" = \"$EXPECTED_PYTHON\"; python3 -c 'print(\"strict-python-ok\")' 2>/dev/null",
                ),
            ],
            stdin: None,
            env,
            allow_network: false,
            pid_file: None,
            cancellation_token: None,
            profile_dir: Some(profile_dir.clone()),
            read_allowlist,
            write_allowlist,
            read_denylist: Vec::new(),
            write_denylist: Vec::new(),
            network_allowlist: Vec::new(),
            workspace_access: deadreckon_sandbox::WorkspaceAccess::Disposable,
            cleanup_process_group: true,
            guarded_launch: None,
        })
        .await
        .expect("contained python check");

        let profile = std::fs::read_to_string(profile_dir.join("profile.sb"))
            .expect("captured sandbox profile");
        assert_eq!(output.status_code, Some(0), "{output:#?}\n{profile}");
        assert_eq!(output.stdout, "strict-python-ok\n", "{output:#?}");
    }

    fn base_run_loop_config() -> RunLoopConfig {
        RunLoopConfig {
            provider: Some("smoke".to_string()),
            max_spend_usd: None,
            max_wall_seconds: None,
            sandbox_backend: SandboxBackend::None,
            no_seams: true,
            max_turns: 1,
            from_turn: None,
            event_sender: None,
            cancellation_token: None,
            work_boundary: None,
            docs: RunLoopDocsConfig {
                home: PathBuf::from("/tmp"),
                config_path: None,
                doc_provider: None,
                doc_provider_source: None,
                doc_subskills: Vec::new(),
                token_budget: 0,
                budget_cap_usd: None,
                doc_skill: "run-narrator".to_string(),
                no_docs: true,
            },
            narrate: None,
        }
    }

    #[test]
    fn every_approval_decision_appends_trace() {
        let temp = TempDir::new().expect("tempdir");
        let (_paths, state) = create_smoke_run(&temp, "trace provider approvals");
        let provider_trace = json!({
            "approvals": [
                {
                    "kind": "commandExecution",
                    "command": "curl https://example.com",
                    "decision": "deny",
                    "reason": "network denied by run capabilities",
                    "capability": "network"
                },
                {
                    "kind": "fileChange",
                    "path": "/workspace/src/lib.rs",
                    "decision": "allow",
                    "reason": "file change stays within writable roots",
                    "capability": "filesystem"
                }
            ]
        });

        append_provider_approval_traces(&state, 1, &provider_trace).expect("append approvals");

        let raw =
            std::fs::read_to_string(state.run_root.join("traces.jsonl")).expect("approval traces");
        let traces = raw
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("trace json"))
            .collect::<Vec<_>>();
        assert_eq!(traces.len(), 2);
        assert!(
            traces
                .iter()
                .all(|trace| trace["event"] == "provider.approval")
        );
        assert_eq!(traces[0]["detail"]["decision"], "deny");
        assert_eq!(traces[1]["detail"]["decision"], "allow");
    }

    #[test]
    fn undelivered_steers_surface_as_attention() {
        let trace = json!({
            "caveats": [{
                "code": "provider.route.degraded",
                "message": "codex app-server died; used cli:codex exec; 2 pending steers remain undelivered"
            }],
            "pending_steers": 2
        });

        let message = super::degraded_caveat_message(&trace, 4).expect("attention message");

        assert!(message.contains("provider route degraded"), "{message}");
        assert!(message.contains("2 pending steers"), "{message}");
    }

    #[test]
    fn run_loop_config_narrate_defaults_none_keeps_existing_constructors() {
        // The additive field is None by default, so prior constructors compile
        // unchanged; the CLI opts in by setting Some(..).
        let off = base_run_loop_config();
        assert!(off.narrate.is_none());

        let defaults = NarratorConfig::default();
        assert!(defaults.foreground);
        assert!(!defaults.headless_append);
        assert_eq!(defaults.lines, 4);
        assert_eq!(defaults.min_gap_seconds, 30);
        assert_eq!(defaults.turn_burst, 8);
        assert_eq!(defaults.quiet_seconds, 45);
        assert_eq!(defaults.max_beats, 200);

        let on = RunLoopConfig {
            narrate: Some(NarratorConfig::default()),
            ..base_run_loop_config()
        };
        assert!(on.narrate.expect("narrate set").foreground);
    }

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

    #[test]
    fn no_changes_after_gate_failure_preserves_cause_without_blaming_provider() {
        let temp = TempDir::new().expect("tempdir");
        let (_paths, mut state) = create_smoke_run(&temp, "repair the failed gate");
        let gate_failure =
            "acceptance failed after turn 1: check greet-exact-output failed".to_string();
        // Durable resume clears failure_reason before starting the next loop,
        // so the prior gate result must also be recoverable from history.
        state.failure_reason = None;

        classify_cli_no_deliverable_changes(&mut state, &[gate_failure.clone()], 2);

        assert_eq!(state.turn, 2);
        assert_eq!(state.failure_reason, Some(gate_failure));
        assert_eq!(state.provider_failure, None);
    }

    #[test]
    fn first_turn_no_change_remains_a_plain_no_progress_failure() {
        let temp = TempDir::new().expect("tempdir");
        let (_paths, mut state) = create_smoke_run(&temp, "make a deliverable change");

        classify_cli_no_deliverable_changes(&mut state, &[], 1);

        assert_eq!(state.turn, 1);
        assert_eq!(
            state.failure_reason.as_deref(),
            Some("cli subagent completed without file changes in the deliverable")
        );
        assert_eq!(state.provider_failure, None);
    }

    #[tokio::test]
    async fn parent_repair_candidate_mode_stops_before_proof_sealing_or_promotion() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "repair the composed parent");
        let original_working_dir = state.working_dir.clone();
        let candidate_path =
            deadreckon_core::parent_repair_candidate_path_for_run_root(&state.run_root);
        let marker_path = deadreckon_core::marker_path_for_run_root(&state.run_root);
        let judgment_path = state.run_root.join(deadreckon_core::SEMANTIC_JUDGMENT_JSON);
        let receipt_path = paths.job_receipt(&state.run_id);
        let library_path = paths.library_dir(&state.scope, &state.run_id);
        let job_id = state.run_id.clone();
        let launch_id = uuid::Uuid::new_v4().to_string();
        let router = ProviderRouter::smoke();
        let mut config = base_run_loop_config();
        config.max_turns = 3;
        config.docs.home = paths.home().to_path_buf();

        let outcome = run_parent_repair_turn_loop(
            &mut state,
            &router,
            config,
            ParentRepairCandidateContext {
                path: candidate_path.clone(),
                job_id,
                round: 2,
                attempt: 3,
                launch_id: launch_id.clone(),
                lease_epoch: 4,
                intent_sha256: "sha256:repair-intent".to_string(),
                manifest_sha256: "sha256:repair-manifest".to_string(),
                feedback: "Address the independent judge's missing requirement.".to_string(),
            },
        )
        .await
        .expect("candidate-only repair loop");

        assert_eq!(outcome, RunLoopOutcome::Done);
        let candidate: ParentRepairCandidate =
            serde_json::from_slice(&std::fs::read(&candidate_path).expect("candidate record"))
                .expect("candidate json");
        let mut result_index =
            deadreckon_core::flight::build_deliverable_file_index_for_state(&state)
                .expect("candidate result index");
        result_index.files.remove(Path::new("manifest.json"));
        assert_eq!(candidate.job_id, state.run_id);
        assert_eq!(candidate.run_id, state.run_id);
        assert_eq!(candidate.round, 2);
        assert_eq!(candidate.attempt, 3);
        assert_eq!(candidate.launch_id, launch_id);
        assert_eq!(candidate.lease_epoch, 4);
        assert_eq!(candidate.intent_sha256, "sha256:repair-intent");
        assert_eq!(candidate.manifest_sha256, "sha256:repair-manifest");
        assert_eq!(candidate.result_tree_sha256, result_index.tree_hash());
        assert_eq!(candidate.turn, state.turn);

        assert!(!marker_path.exists(), "candidate mode wrote a gate marker");
        assert!(
            !judgment_path.exists(),
            "candidate mode wrote a semantic judgment"
        );
        assert!(
            !receipt_path.exists(),
            "candidate mode wrote a completion receipt"
        );
        assert!(
            !library_path.exists(),
            "candidate mode promoted the unverified parent"
        );
        assert_eq!(state.promoted_library_dir, None);
        assert_eq!(state.working_dir, original_working_dir);
    }

    #[test]
    fn parent_repair_candidate_refuses_unfenced_or_malformed_controller_context() {
        let temp = TempDir::new().expect("tempdir");
        let (_paths, state) = create_smoke_run(&temp, "repair the composed parent");
        let candidate_path =
            deadreckon_core::parent_repair_candidate_path_for_run_root(&state.run_root);
        let valid = ParentRepairCandidateContext {
            path: candidate_path.clone(),
            job_id: state.run_id.clone(),
            round: 1,
            attempt: 2,
            launch_id: uuid::Uuid::new_v4().to_string(),
            lease_epoch: 3,
            intent_sha256: "sha256:intent".to_string(),
            manifest_sha256: "sha256:manifest".to_string(),
            feedback: "repair the parent".to_string(),
        };

        for malformed in [
            ParentRepairCandidateContext {
                lease_epoch: 0,
                ..valid.clone()
            },
            ParentRepairCandidateContext {
                launch_id: "not-a-launch-id".to_string(),
                ..valid.clone()
            },
            ParentRepairCandidateContext {
                intent_sha256: String::new(),
                ..valid.clone()
            },
            ParentRepairCandidateContext {
                manifest_sha256: String::new(),
                ..valid
            },
        ] {
            let error = persist_parent_repair_candidate(&state, 1, &malformed)
                .expect_err("malformed controller context must fail closed");
            assert!(
                error
                    .to_string()
                    .contains("does not match the same-ID result run"),
                "{error}"
            );
            assert!(
                !candidate_path.exists(),
                "malformed context wrote a repair candidate"
            );
        }
    }

    fn write_required_semantic_job(
        paths: &DeadreckonPaths,
        state: &PipelineState,
        max_spend_usd: f64,
        max_wall_seconds: u64,
    ) {
        use chrono::Utc;
        use deadreckon_core::write_job;
        use deadreckon_protocol::{
            Job, JobId, JobPolicy, JobSchemaVersion, JobShape, SemanticJudgeMode,
        };

        write_job(
            paths,
            &Job {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: JobId(state.run_id.clone()),
                scope: state.scope.clone(),
                goal: state.goal.clone(),
                shape: JobShape::Single,
                created_at: Utc::now(),
                source_cwd: state.cwd.clone(),
                launch_plan_sha256: "sha256:launch".to_string(),
                authority_sha256: "sha256:authority".to_string(),
                policy: JobPolicy {
                    max_spend_usd,
                    max_wall_seconds,
                    max_attempts: 1,
                    deadline: None,
                    semantic_judge: SemanticJudgeMode::Required,
                    execution: Some(deadreckon_protocol::JobExecutionPolicy::workspace_only(
                        "sandbox-exec",
                    )),
                },
            },
        )
        .expect("job");
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

    #[test]
    fn semantic_judge_spend_and_evidence_do_not_double_add_run_wall_time() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "judge accounting");
        let semantic_run = crate::semantic_judge::SemanticJudgeRun {
            result: crate::semantic_judge::SemanticJudgeResult::Unavailable(
                "malformed semantic output".to_string(),
            ),
            accounting: crate::semantic_judge::SemanticJudgeAccounting {
                provider: "judge-provider".to_string(),
                model: "judge-model".to_string(),
                input_tokens: 12,
                output_tokens: 3,
                cost_usd: 0.25,
                subscription: false,
                wall_time_seconds: 60.0,
                sandbox_backend: Some("sandbox-exec".to_string()),
            },
            budget_exhaustion: None,
        };
        let config = RunLoopConfig {
            max_spend_usd: Some(2.0),
            max_wall_seconds: Some(300.0),
            ..base_run_loop_config()
        };
        state.total_wall_seconds = 7.0;
        let work_clock = RunWorkClock::new(&state).expect("work clock");

        record_semantic_judge_accounting(&mut state, 1, &config, &semantic_run, &work_clock)
            .expect("record accounting");

        assert_eq!(state.total_spend_usd, 0.25);
        assert!(state.total_wall_seconds >= 7.0);
        assert!(
            state.total_wall_seconds < 67.0,
            "semantic provider accounting must not be added to the controller clock"
        );
        let reloaded = deadreckon_core::load_run(&paths, &state.run_id).expect("durable state");
        assert_eq!(reloaded.total_spend_usd, 0.25);
        assert_eq!(reloaded.total_wall_seconds, state.total_wall_seconds);
        let spend = std::fs::read_to_string(state.run_root.join("spend.jsonl")).expect("spend");
        let trace = std::fs::read_to_string(state.run_root.join("traces.jsonl")).expect("trace");
        assert!(spend.contains("\"kind\":\"semantic_judge\""), "{spend}");
        assert!(
            trace.contains("\"event\":\"semantic_judge.unavailable\""),
            "{trace}"
        );
        assert!(trace.contains("\"worker_session\":false"), "{trace}");
        assert!(
            trace.contains("\"workspace_access\":\"read-only\""),
            "{trace}"
        );
    }

    #[tokio::test]
    async fn receipt_sealing_failure_stops_strict_job_needs_review() {
        use chrono::Utc;
        use deadreckon_core::{AcceptanceMarker, AcceptanceProofKind, write_job};
        use deadreckon_protocol::{
            Job, JobId, JobPolicy, JobSchemaVersion, JobShape, SemanticJudgeMode,
        };

        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "strict semantic result");
        write_job(
            &paths,
            &Job {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: JobId(state.run_id.clone()),
                scope: state.scope.clone(),
                goal: state.goal.clone(),
                shape: JobShape::Single,
                created_at: Utc::now(),
                source_cwd: state.cwd.clone(),
                launch_plan_sha256: "sha256:launch".to_string(),
                authority_sha256: "sha256:authority".to_string(),
                policy: JobPolicy {
                    max_spend_usd: 1.0,
                    max_wall_seconds: 60,
                    max_attempts: 1,
                    deadline: None,
                    semantic_judge: SemanticJudgeMode::Required,
                    execution: Some(deadreckon_protocol::JobExecutionPolicy::workspace_only(
                        "sandbox-exec",
                    )),
                },
            },
        )
        .expect("job");
        let authority_path = paths.job_authority(&state.run_id);
        std::fs::create_dir_all(authority_path.parent().expect("authority parent"))
            .expect("authority parent");
        std::fs::write(&authority_path, "{}\n").expect("authority");
        let marker = AcceptanceMarker {
            schema_version: 2,
            run_id: state.run_id.clone(),
            status: "passed".to_string(),
            produced_by: "dr-gate".to_string(),
            issuer: "dr-gate".to_string(),
            proof_kind: AcceptanceProofKind::NativeGate,
            checked_at: Utc::now(),
            working_dir: state.working_dir.clone(),
            contained: true,
            sandbox_backend: "sandbox-exec".to_string(),
            signature: "test".to_string(),
            check_count: 0,
            checks: Vec::new(),
        };
        let judgment = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(state.run_id.clone()),
            run_id: RunId(state.run_id.clone()),
            judged_at: Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-judge".to_string(),
            decision: SemanticDecision::Achieved,
            summary: "the approved goal is achieved".to_string(),
            goal_coverage: Vec::new(),
            missing: Vec::new(),
            input_sha256: "sha256:semantic-input".to_string(),
            spend_usd: 0.0,
        };
        let mut history = Vec::new();
        let authenticated_cutoff = tokio::time::Instant::now();
        let deadline_clock = RunWorkClock::with_boundary(
            &state,
            Some(RunWorkBoundary::new(
                authenticated_cutoff,
                RunWorkExpiry::Deadline,
            )),
        )
        .expect("deadline clock");
        let first_deadline = deadline_clock
            .provider_phase_deadline(Some(60.0))
            .expect("semantic deadline");
        let seal_deadline = deadline_clock
            .provider_phase_deadline(Some(60.0))
            .expect("seal deadline");
        assert_eq!(first_deadline.work_expires_at, authenticated_cutoff);
        assert_eq!(seal_deadline.work_expires_at, authenticated_cutoff);
        let cancellation = tokio_util::sync::CancellationToken::new();
        let expired = seal_achieved_semantic_completion(
            &mut state,
            &paths,
            1,
            &marker,
            &judgment,
            &mut history,
            &deadline_clock,
            seal_deadline,
            &cancellation,
        )
        .expect("expired seal disposition");
        assert_eq!(expired, SemanticCompletionDisposition::BudgetExhausted);
        assert_eq!(
            state.pause_reason.as_deref(),
            Some("calendar deadline reached before completion receipt sealing")
        );
        assert!(!paths.job_receipt(&state.run_id).exists());
        state.pause_reason = None;
        state.failure_reason = None;
        history.clear();

        let work_clock = RunWorkClock::new(&state).expect("work clock");
        let phase_deadline = work_clock
            .provider_phase_deadline(Some(60.0))
            .expect("phase deadline");
        let disposition = seal_achieved_semantic_completion(
            &mut state,
            &paths,
            1,
            &marker,
            &judgment,
            &mut history,
            &work_clock,
            phase_deadline,
            &cancellation,
        )
        .expect("semantic disposition");

        assert_eq!(disposition, SemanticCompletionDisposition::NeedsReview);
        assert!(
            state
                .failure_reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with("NEEDS_REVIEW:"))
        );
        let trace = std::fs::read_to_string(state.run_root.join("traces.jsonl")).expect("trace");
        assert!(trace.contains("\"event\":\"semantic_judge.needs_review\""));
        assert!(
            state
                .failure_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("receipt could not be sealed"))
        );
        assert_eq!(state.status, RunStatus::Failed);
    }

    #[test]
    fn workspace_snapshot_does_not_start_after_the_job_cutoff() {
        let temp = TempDir::new().expect("tempdir");
        let (_paths, state) = create_smoke_run(&temp, "bounded snapshot");
        let deadline = ProviderPhaseDeadline::new(
            tokio::time::Instant::now(),
            std::time::Duration::from_secs(3),
        );

        let error = snapshot_working_bounded(
            &state,
            1,
            deadline,
            &tokio_util::sync::CancellationToken::new(),
        )
        .expect_err("expired Job cutoff must prevent a snapshot");

        assert!(matches!(
            error,
            DeadreckonError::ProcessBoundary {
                kind: deadreckon_core::ProcessBoundaryKind::WorkExpired,
                ..
            }
        ));
        assert!(!state.run_root.join("snapshots/turn-1").exists());
    }

    #[test]
    fn result_promotion_does_not_start_after_cancellation() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "cancelled promotion");
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();
        let deadline = ProviderPhaseDeadline::new(
            tokio::time::Instant::now() + std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(3),
        );

        let error = promote_if_ready(&mut state, deadline, &cancellation)
            .expect_err("cancelled Job must prevent promotion");

        assert!(matches!(
            error,
            DeadreckonError::ProcessBoundary {
                kind: deadreckon_core::ProcessBoundaryKind::Cancelled,
                ..
            }
        ));
        assert!(!paths.library_dir(&state.scope, &state.run_id).exists());
    }

    #[tokio::test]
    async fn semantic_judge_does_not_start_at_the_single_job_spend_cap() {
        use chrono::Utc;
        use deadreckon_core::{AcceptanceMarker, AcceptanceProofKind};

        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "budgeted semantic result");
        write_required_semantic_job(&paths, &state, 1.0, 60);
        state.total_spend_usd = 1.0;
        let marker = AcceptanceMarker {
            schema_version: 2,
            run_id: state.run_id.clone(),
            status: "passed".to_string(),
            produced_by: "dr-gate".to_string(),
            issuer: "dr-gate".to_string(),
            proof_kind: AcceptanceProofKind::NativeGate,
            checked_at: Utc::now(),
            working_dir: state.working_dir.clone(),
            contained: true,
            sandbox_backend: "sandbox-exec".to_string(),
            signature: "test".to_string(),
            check_count: 0,
            checks: Vec::new(),
        };
        let work_clock = RunWorkClock::new(&state).expect("work clock");
        let phase_deadline = work_clock
            .provider_phase_deadline(Some(60.0))
            .expect("phase deadline");

        let disposition = semantic_completion_disposition(
            &mut state,
            &ProviderRouter::smoke(),
            &base_run_loop_config(),
            1,
            &marker,
            &mut Vec::new(),
            &tokio_util::sync::CancellationToken::new(),
            &work_clock,
            phase_deadline,
        )
        .await
        .expect("semantic disposition");

        assert_eq!(disposition, SemanticCompletionDisposition::BudgetExhausted);
        assert_eq!(
            state.pause_reason.as_deref(),
            Some("spend cap reached before semantic judge")
        );
        assert!(
            !state
                .run_root
                .join(deadreckon_core::SEMANTIC_JUDGMENT_JSON)
                .exists()
        );
        let traces =
            std::fs::read_to_string(state.run_root.join("traces.jsonl")).unwrap_or_default();
        assert!(!traces.contains("semantic_judge."), "{traces}");
    }

    #[tokio::test]
    async fn semantic_judge_does_not_start_at_the_single_job_wall_cap() {
        use chrono::Utc;
        use deadreckon_core::{AcceptanceMarker, AcceptanceProofKind};

        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "wall-budgeted semantic result");
        write_required_semantic_job(&paths, &state, 10.0, 60);
        state.total_wall_seconds = 60.0;
        let marker = AcceptanceMarker {
            schema_version: 2,
            run_id: state.run_id.clone(),
            status: "passed".to_string(),
            produced_by: "dr-gate".to_string(),
            issuer: "dr-gate".to_string(),
            proof_kind: AcceptanceProofKind::NativeGate,
            checked_at: Utc::now(),
            working_dir: state.working_dir.clone(),
            contained: true,
            sandbox_backend: "sandbox-exec".to_string(),
            signature: "test".to_string(),
            check_count: 0,
            checks: Vec::new(),
        };
        let work_clock = RunWorkClock::new(&state).expect("work clock");
        let phase_deadline = work_clock
            .provider_phase_deadline(Some(60.0))
            .expect("phase deadline");

        let disposition = semantic_completion_disposition(
            &mut state,
            &ProviderRouter::smoke(),
            &base_run_loop_config(),
            1,
            &marker,
            &mut Vec::new(),
            &tokio_util::sync::CancellationToken::new(),
            &work_clock,
            phase_deadline,
        )
        .await
        .expect("semantic disposition");

        assert_eq!(disposition, SemanticCompletionDisposition::BudgetExhausted);
        assert_eq!(
            state.pause_reason.as_deref(),
            Some("wall-clock cap reached before semantic judge")
        );
        assert!(
            !state
                .run_root
                .join(deadreckon_core::SEMANTIC_JUDGMENT_JSON)
                .exists()
        );
        let traces =
            std::fs::read_to_string(state.run_root.join("traces.jsonl")).unwrap_or_default();
        assert!(!traces.contains("semantic_judge."), "{traces}");
    }

    #[tokio::test]
    async fn semantic_judge_retains_the_authenticated_calendar_deadline() {
        use chrono::Utc;
        use deadreckon_core::{AcceptanceMarker, AcceptanceProofKind};

        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "deadline-bound semantic result");
        write_required_semantic_job(&paths, &state, 10.0, 600);
        let marker = AcceptanceMarker {
            schema_version: 2,
            run_id: state.run_id.clone(),
            status: "passed".to_string(),
            produced_by: "dr-gate".to_string(),
            issuer: "dr-gate".to_string(),
            proof_kind: AcceptanceProofKind::NativeGate,
            checked_at: Utc::now(),
            working_dir: state.working_dir.clone(),
            contained: true,
            sandbox_backend: "sandbox-exec".to_string(),
            signature: "test".to_string(),
            check_count: 0,
            checks: Vec::new(),
        };
        let authenticated_cutoff = tokio::time::Instant::now();
        let work_clock = RunWorkClock::with_boundary(
            &state,
            Some(RunWorkBoundary::new(
                authenticated_cutoff,
                RunWorkExpiry::Deadline,
            )),
        )
        .expect("deadline clock");
        let phase_deadline = work_clock
            .provider_phase_deadline(Some(600.0))
            .expect("phase deadline");

        let disposition = semantic_completion_disposition(
            &mut state,
            &ProviderRouter::smoke(),
            &base_run_loop_config(),
            1,
            &marker,
            &mut Vec::new(),
            &tokio_util::sync::CancellationToken::new(),
            &work_clock,
            phase_deadline,
        )
        .await
        .expect("semantic disposition");

        assert_eq!(phase_deadline.work_expires_at, authenticated_cutoff);
        assert_eq!(disposition, SemanticCompletionDisposition::BudgetExhausted);
        assert_eq!(
            state.pause_reason.as_deref(),
            Some("calendar deadline reached before semantic judge")
        );
        let traces =
            std::fs::read_to_string(state.run_root.join("traces.jsonl")).unwrap_or_default();
        assert!(!traces.contains("semantic_judge."), "{traces}");
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
                work_boundary: None,
                narrate: None,
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
                work_boundary: None,
                narrate: None,
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
            ProviderPhaseDeadline::from_now(Duration::from_secs(1), Duration::from_millis(100)),
            &CancellationToken::new(),
        )
        .await
        .expect("policy seam dispatch");
        assert!(matches!(seam_refusal, SeamPhaseOutcome::Completed(None)));

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

    #[test]
    fn detached_event_sink_stops_after_lost_containment() {
        assert!(event_sink_must_stop(&SeamOutcome::LostContainment(
            "retained process authority".to_string()
        )));
        assert!(!event_sink_must_stop(&SeamOutcome::Skipped(
            "ordinary observer failure".to_string()
        )));
    }

    #[tokio::test]
    async fn event_sink_shutdown_cancels_active_dispatch_and_joins_boundedly() {
        let temp = TempDir::new().expect("tempdir");
        let run_root = temp.path().join("run");
        let working_dir = temp.path().join("work");
        std::fs::create_dir_all(run_root.join("gate")).expect("gate");
        std::fs::create_dir_all(run_root.join("proofs")).expect("proofs");
        std::fs::create_dir_all(&working_dir).expect("work");
        let started_marker = working_dir.join("sink-started");
        let seams = SeamsConfig::with_command(
            SeamKind::EventSink,
            SeamCommandConfig {
                command: vec![
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    format!(
                        "cat >/dev/null; touch {}; sleep 60",
                        sh_quote(&started_marker)
                    ),
                ],
                timeout_ms: 30_000,
            },
        )
        .expect("event sink seam");
        let ctx = SeamRunCtx {
            run_root: run_root.clone(),
            working_dir,
            sandbox_backend: SandboxBackend::None,
        };
        let (sender, _) = tokio::sync::broadcast::channel(8);
        let cancellation = CancellationToken::new();
        let forwarder = spawn_event_sink_forwarder(
            seams,
            ctx,
            &sender,
            ProviderPhaseDeadline::from_now(Duration::from_secs(30), Duration::from_secs(2)),
            &cancellation,
        );
        sender
            .send(deadreckon_protocol::RunEvent {
                timestamp: chrono::Utc::now(),
                run_id: "event-sink-test".to_string(),
                event: RunEventKind::TurnStarted { turn: 1 },
            })
            .expect("event sent");
        let wait_deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while !started_marker.exists() && tokio::time::Instant::now() < wait_deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(started_marker.exists(), "event sink did not start");

        let shutdown_started = std::time::Instant::now();
        forwarder.shutdown(Duration::from_secs(2)).await;
        assert!(
            shutdown_started.elapsed() < Duration::from_secs(3),
            "event-sink join exceeded its cleanup budget: {:?}",
            shutdown_started.elapsed()
        );
        let remaining = std::fs::read_dir(run_root.join("child-pids"))
            .expect("child-pids")
            .collect::<std::io::Result<Vec<_>>>()
            .expect("authority entries");
        assert!(
            remaining.is_empty(),
            "event sink retained process authority after joined shutdown: {remaining:?}"
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
                work_boundary: None,
                narrate: None,
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
                .ends_with("Return exactly one JSON object with action bash, write_file, reshape (propose splitting the goal into 2-6 independent pieces: {\"action\":\"reshape\",\"tool_call_id\":\"...\",\"pieces\":[{\"goal\":\"...\",\"done_hint\":\"...\"}]} - recorded for the operator, never executed by you), or done.")
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
                    work_boundary: None,
                    narrate: None,
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
                    work_boundary: None,
                    narrate: None,
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
                work_boundary: None,
                narrate: None,
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
                work_boundary: None,
                narrate: None,
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
    fn truncated_history_json_resumes_via_trace_reconstruction() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "truncated history".to_string(),
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
"#,
        )
        .expect("trace");
        std::fs::write(state.run_root.join("history.json"), "[\"half an entr")
            .expect("truncated history");

        let history = load_history_with_work_clock(&mut state, None).expect("history");
        assert_eq!(history.len(), 1);
        assert!(history[0].contains("tool-1"));
    }

    #[test]
    fn garbage_history_json_resumes_via_trace_reconstruction_and_resaves() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "garbage history".to_string(),
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
            r#"{"timestamp":"2026-05-11T00:00:00Z","run_id":"r","turn":1,"event":"tool.bash","latency_ms":1,"detail":{"tool_call_id":"tool-9"}}
"#,
        )
        .expect("trace");
        let history_file = state.run_root.join("history.json");
        std::fs::write(&history_file, "not json at all \u{0}\u{1}").expect("garbage history");

        let history = load_history_with_work_clock(&mut state, None).expect("history");
        assert_eq!(history.len(), 1);
        let resaved = std::fs::read_to_string(&history_file).expect("resaved");
        let parsed: Vec<String> =
            serde_json::from_str(&resaved).expect("history.json is valid again");
        assert_eq!(parsed, history);
    }

    #[test]
    fn history_save_is_atomic_tempfile_rename() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "atomic history".to_string(),
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
        let history_file = state.run_root.join("history.json");
        save_history(&state, &["first".to_string()]).expect("first save");
        #[cfg(unix)]
        let before_inode = {
            use std::os::unix::fs::MetadataExt;
            std::fs::metadata(&history_file).expect("meta").ino()
        };

        save_history(&state, &["first".to_string(), "second".to_string()]).expect("second save");

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let after_inode = std::fs::metadata(&history_file).expect("meta").ino();
            assert_ne!(
                before_inode, after_inode,
                "save_history must replace via tempfile rename, not write in place"
            );
        }
        let parsed: Vec<String> =
            serde_json::from_str(&std::fs::read_to_string(&history_file).expect("read"))
                .expect("valid json");
        assert_eq!(parsed.len(), 2);
        let stray_temps = std::fs::read_dir(&state.run_root)
            .expect("run root")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(".tmp"))
            .count();
        assert_eq!(stray_temps, 0, "no leftover temp files");
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

        let history = load_history_with_work_clock(&mut state, None).expect("history");
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

        let history = load_history_with_work_clock(&mut state, None).expect("history");

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

        let history = load_history_with_work_clock(&mut state, Some(2)).expect("history");
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

        let history = load_history_with_work_clock(&mut state, Some(1)).expect("history");
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
    fn write_file_refuses_every_non_deliverable_path_class() {
        let root = PathBuf::from("/tmp/deadreckon-safe-root");
        for path in [
            ".specstory/history/session.md",
            "nested/.specstory/history/session.md",
            ".deadreckon/codebase.json",
            "target/debug/output",
            "web/node_modules/package/index.js",
        ] {
            let err = safe_working_path(&root, Path::new(path)).expect_err("reserved path refused");
            assert!(
                err.to_string().contains("not part of the deliverable"),
                "{path}: {err}"
            );
        }
        assert!(
            safe_working_path(&root, Path::new("docs/RUN-AS-BUILT.md"))
                .expect("deliverable documentation")
                .ends_with("docs/RUN-AS-BUILT.md")
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_file_rejects_symlink_root_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let outside = temp.path().join("outside");
        let workspace_link = temp.path().join("workspace-link");
        std::fs::create_dir(&outside).expect("outside directory");
        symlink(&outside, &workspace_link).expect("workspace symlink");

        let error = write_workspace_file_no_follow(
            &workspace_link,
            Path::new("nested/result.txt"),
            b"escaped",
        )
        .expect_err("symlink root refused");

        assert!(error.to_string().contains("symlink"), "{error}");
        assert!(!outside.join("nested/result.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn write_file_rejects_symlink_ancestor_without_touching_external_files() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside");
        std::fs::create_dir(&workspace).expect("workspace directory");
        std::fs::create_dir(&outside).expect("outside directory");
        std::fs::write(outside.join("sentinel.txt"), "keep").expect("sentinel");
        symlink(&outside, workspace.join("escape")).expect("ancestor symlink");

        let error =
            write_workspace_file_no_follow(&workspace, Path::new("escape/result.txt"), b"escaped")
                .expect_err("symlink ancestor refused");

        assert!(error.to_string().contains("symlink"), "{error}");
        assert!(!outside.join("result.txt").exists());
        assert_eq!(
            std::fs::read_to_string(outside.join("sentinel.txt")).expect("sentinel"),
            "keep"
        );
    }

    #[cfg(unix)]
    #[test]
    fn write_file_rejects_symlink_leaf_without_overwriting_its_target() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let outside = temp.path().join("outside.txt");
        std::fs::create_dir(&workspace).expect("workspace directory");
        std::fs::write(&outside, "keep").expect("outside file");
        let leaf = workspace.join("result.txt");
        symlink(&outside, &leaf).expect("leaf symlink");

        let error =
            write_workspace_file_no_follow(&workspace, Path::new("result.txt"), b"overwritten")
                .expect_err("symlink leaf refused");

        assert!(error.to_string().contains("symlink"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&outside).expect("outside file"),
            "keep"
        );
        assert!(
            std::fs::symlink_metadata(&leaf)
                .expect("leaf metadata")
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn write_file_creates_nested_directories_and_overwrites_regular_files() {
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace directory");
        let relative = Path::new("src/generated/result.txt");

        let target =
            write_workspace_file_no_follow(&workspace, relative, b"first").expect("initial write");
        assert_eq!(target, workspace.join(relative));
        assert_eq!(
            std::fs::read_to_string(&target).expect("initial content"),
            "first"
        );

        write_workspace_file_no_follow(&workspace, relative, b"second").expect("regular overwrite");
        assert_eq!(
            std::fs::read_to_string(&target).expect("updated content"),
            "second"
        );
    }

    #[test]
    fn strict_job_never_migrates_workspace_codebase_routing() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let operator = temp.path().join("operator");
        std::fs::create_dir_all(&operator).expect("operator directory");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "strict routing".to_string(),
                cwd: operator,
                sandbox: "none".to_string(),
                provider: Some("codex".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: Some("strict-job".to_string()),
                codebase: Some(CodebaseRecord::fresh()),
            },
        )
        .expect("run");
        std::fs::create_dir_all(paths.job_dir(&state.run_id)).expect("job directory");
        std::fs::write(paths.job_json(&state.run_id), "{}\n").expect("job marker");
        std::fs::remove_file(
            state
                .run_root
                .join(deadreckon_core::TRUSTED_CODEBASE_RECORD),
        )
        .expect("remove trusted record");
        std::fs::write(
            state.working_dir.join(".deadreckon/codebase.json"),
            serde_json::to_vec_pretty(&CodebaseRecord::fresh()).expect("workspace record"),
        )
        .expect("tampered workspace record");

        let error = capture_trusted_turn_head(&state, 1)
            .expect_err("strict Job must fail when trusted routing is absent");

        assert!(error.to_string().contains("codebase.json"), "{error}");
        assert!(
            !state
                .run_root
                .join(deadreckon_core::TRUSTED_CODEBASE_RECORD)
                .exists(),
            "workspace routing must not be promoted to trusted Job authority"
        );
    }

    #[test]
    fn turn_commit_archives_private_artifacts_and_rewrites_provider_commits() {
        let temp = TempDir::new().expect("tempdir");
        let source_repository = temp.path().join("source-repository");
        let repository = temp.path().join("repository");
        std::fs::create_dir_all(source_repository.join("src")).expect("src");
        std::fs::create_dir_all(source_repository.join(".specstory/history")).expect("specstory");
        test_git(&source_repository, &["init", "-q"]);
        test_git(
            &source_repository,
            &["config", "user.email", "fixture@example.invalid"],
        );
        test_git(&source_repository, &["config", "user.name", "fixture"]);
        std::fs::write(
            source_repository.join("src/lib.rs"),
            "pub fn value() -> u8 { 1 }\n",
        )
        .expect("source");
        std::fs::write(source_repository.join(".gitignore"), ".agent-cache/\n")
            .expect("original ignore policy");
        std::fs::write(
            source_repository.join(".specstory/history/base.md"),
            "operator-owned base\n",
        )
        .expect("base evidence");
        std::fs::write(
            source_repository.join(".git/info/exclude"),
            ".deadreckon/\ntarget/\nnode_modules/\n",
        )
        .expect("git exclude");
        test_git(&source_repository, &["add", "-A"]);
        test_git(&source_repository, &["commit", "-q", "-m", "base"]);
        let base_sha = test_git(&source_repository, &["rev-parse", "HEAD"]);
        test_git(
            &source_repository,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "deadreckon-result",
                repository.to_str().expect("UTF-8 worktree"),
                &base_sha,
            ],
        );

        let mut codebase = CodebaseRecord::fresh();
        codebase.mode = CodebaseMode::Worktree;
        codebase.source_path = Some(source_repository.clone());
        codebase.source_git_root = Some(source_repository);
        codebase.branch_name = Some("deadreckon-result".to_string());
        codebase.base_ref = Some("master".to_string());
        codebase.base_sha = Some(base_sha.clone());
        codebase.worktree_path = Some(repository.clone());
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "keep provider evidence out of the result".to_string(),
                cwd: repository.clone(),
                sandbox: "none".to_string(),
                provider: Some("codex".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: Some(codebase),
            },
        )
        .expect("run");

        capture_trusted_turn_head(&state, 1).expect("trusted first-turn head");
        snapshot_working(&state, 0).expect("raw before snapshot");
        std::fs::write(
            repository.join("src/lib.rs"),
            "pub fn value() -> u8 { 2 }\n",
        )
        .expect("source change");
        std::fs::write(
            repository.join(".specstory/history/base.md"),
            "provider modified private evidence\n",
        )
        .expect("modified private evidence");
        std::fs::write(
            repository.join(".specstory/history/untracked.md"),
            "provider-private untracked evidence\n",
        )
        .expect("untracked private evidence");
        std::fs::create_dir_all(repository.join(".build/debug")).expect("SwiftPM output");
        std::fs::write(
            repository.join(".build/debug/Cloudwing"),
            "rebuildable binary",
        )
        .expect("SwiftPM artifact");

        let raw_changed = changed_files_since_snapshot(&state, 0).expect("raw changes");
        let deliverable =
            deliverable_changed_files(&state, &raw_changed).expect("deliverable changes");
        assert!(
            raw_changed
                .iter()
                .any(|path| path.ends_with(".specstory/history/untracked.md"))
        );
        assert!(
            raw_changed
                .iter()
                .all(|path| !path.ends_with(".build/debug/Cloudwing")),
            "disposable build output must be pruned before changed-file and provenance scanning"
        );
        assert_eq!(
            deliverable
                .iter()
                .filter_map(|path| path.strip_prefix(&repository).ok())
                .collect::<Vec<_>>(),
            vec![Path::new("src/lib.rs")]
        );
        super::append_provenance_for_files(&state, 1, "cli-turn-1", "codex", raw_changed)
            .expect("raw provenance");
        std::fs::write(repository.join(".gitignore"), "").expect("provider ignore rewrite");
        std::fs::write(
            repository.join("src/added.rs"),
            "pub const ADDED: u8 = 1;\n",
        )
        .expect("untracked source deliverable");
        std::fs::create_dir_all(repository.join(".agent-cache")).expect("ignored cache");
        std::fs::write(repository.join(".agent-cache/provider.bin"), "generated\n")
            .expect("ignored generated file");
        snapshot_working(&state, 1).expect("raw after snapshot");
        assert_eq!(
            std::fs::read_to_string(
                state
                    .run_root
                    .join("snapshots/turn-1/.specstory/history/untracked.md")
            )
            .expect("private evidence in raw snapshot"),
            "provider-private untracked evidence\n"
        );
        assert!(
            std::fs::read_to_string(state.run_root.join("provenance.jsonl"))
                .expect("provenance")
                .contains(".specstory")
        );
        assert!(
            !state.run_root.join("snapshots/turn-1/.build").exists(),
            "disposable build output must never enter turn snapshots"
        );

        commit_worktree_turn(&state, 1, "cli_subagent").expect("sanitized first turn");
        let first_turn_paths = test_git(
            &repository,
            &["diff", "--name-only", &format!("{base_sha}..HEAD")],
        );
        assert!(first_turn_paths.lines().any(|path| path == ".gitignore"));
        assert!(first_turn_paths.lines().any(|path| path == "src/added.rs"));
        assert!(
            first_turn_paths
                .lines()
                .all(|path| !path.starts_with(".agent-cache/")),
            "rewriting .gitignore must not admit generated files hidden by the frozen policy"
        );
        assert_eq!(
            std::fs::read_to_string(
                state
                    .run_root
                    .join("provider-evidence/turn-1/workspace/.specstory/history/untracked.md")
            )
            .expect("archived untracked evidence"),
            "provider-private untracked evidence\n"
        );
        assert_eq!(
            std::fs::read_to_string(
                state
                    .run_root
                    .join("snapshots/turn-1-provider-raw/.specstory/history/untracked.md")
            )
            .expect("durable raw provider snapshot"),
            "provider-private untracked evidence\n"
        );
        assert_eq!(
            std::fs::read_to_string(repository.join(".specstory/history/base.md"))
                .expect("base restored"),
            "operator-owned base\n"
        );
        assert!(!repository.join(".specstory/history/untracked.md").exists());
        assert!(
            test_git(
                &repository,
                &["diff", "--name-only", &format!("{base_sha}..HEAD")]
            )
            .lines()
            .all(|path| !path.starts_with(".specstory"))
        );

        capture_trusted_turn_head(&state, 2).expect("trusted second-turn head");
        let trusted_codebase =
            std::fs::read(repository.join(".deadreckon/codebase.json")).expect("trusted codebase");
        std::fs::write(
            repository.join("src/second.rs"),
            "pub const SECOND: u8 = 2;\n",
        )
        .expect("second source");
        std::fs::write(
            repository.join(".specstory/history/base.md"),
            "provider committed private modification\n",
        )
        .expect("committed private modification");
        std::fs::write(
            repository.join(".specstory/history/committed.md"),
            "provider-private committed evidence\n",
        )
        .expect("committed private evidence");
        std::fs::write(repository.join(".deadreckon/provider-forced.json"), "{}\n")
            .expect("forced lifecycle metadata");
        std::fs::write(
            repository.join(".deadreckon/codebase.json"),
            "{ \"tampered\": true }\n",
        )
        .expect("tampered lifecycle authority");
        std::fs::create_dir_all(repository.join("target/debug")).expect("target");
        std::fs::write(repository.join("target/debug/provider.out"), "runtime\n")
            .expect("forced runtime output");
        std::fs::create_dir_all(repository.join("web/node_modules/pkg")).expect("node modules");
        std::fs::write(
            repository.join("web/node_modules/pkg/provider.js"),
            "runtime\n",
        )
        .expect("forced dependency output");
        test_git(&repository, &["add", "-A"]);
        test_git(
            &repository,
            &[
                "add",
                "-f",
                "--",
                ".deadreckon/codebase.json",
                ".deadreckon/provider-forced.json",
                "target/debug/provider.out",
                "web/node_modules/pkg/provider.js",
            ],
        );
        test_git(&repository, &["commit", "-q", "-m", "provider commit"]);
        test_git(&repository, &["switch", "--detach", "-q"]);
        assert!(
            !test_git(
                &repository,
                &[
                    "log",
                    "--format=%H",
                    &format!("{base_sha}..HEAD"),
                    "--",
                    ".specstory"
                ]
            )
            .is_empty(),
            "fixture must prove the provider committed private evidence"
        );
        snapshot_working(&state, 2).expect("raw provider-committed snapshot");
        capture_trusted_turn_head(&state, 2)
            .expect("restart must preserve the original pre-provider head");
        snapshot_working(&state, 1)
            .expect("restart overwrites the ordinary pre-turn snapshot with current state");

        commit_worktree_turn(&state, 2, "cli_subagent").expect("rewrite provider commit");
        assert_eq!(
            std::fs::read(repository.join(".deadreckon/codebase.json"))
                .expect("restored lifecycle authority"),
            trusted_codebase
        );
        assert_eq!(
            std::fs::read_to_string(
                state
                    .run_root
                    .join("provider-evidence/turn-2/workspace/.specstory/history/committed.md")
            )
            .expect("archived committed evidence"),
            "provider-private committed evidence\n"
        );
        assert!(
            test_git(
                &repository,
                &[
                    "log",
                    "--format=%H",
                    &format!("{base_sha}..HEAD"),
                    "--",
                    ".specstory"
                ]
            )
            .is_empty(),
            "no delivered result commit may retain a private evidence blob"
        );
        for path in [
            ".deadreckon/codebase.json",
            ".deadreckon/provider-forced.json",
            "target/debug/provider.out",
            "web/node_modules/pkg/provider.js",
        ] {
            assert!(
                test_git(
                    &repository,
                    &[
                        "log",
                        "--format=%H",
                        &format!("{base_sha}..HEAD"),
                        "--",
                        path
                    ]
                )
                .is_empty(),
                "no delivered result commit may retain {path}"
            );
        }
        let result_paths = test_git(
            &repository,
            &["diff", "--name-only", &format!("{base_sha}..HEAD")],
        );
        assert!(result_paths.lines().any(|path| path == "src/lib.rs"));
        assert!(result_paths.lines().any(|path| path == "src/second.rs"));
        assert!(
            result_paths
                .lines()
                .all(|path| !path.starts_with(".specstory"))
        );
        let subjects = test_git(
            &repository,
            &["log", "--format=%s", &format!("{base_sha}..HEAD")],
        );
        assert_eq!(
            test_git(&repository, &["symbolic-ref", "--short", "HEAD"]),
            "deadreckon-result"
        );
        assert!(subjects.contains("turn 1: cli_subagent"));
        assert!(subjects.contains("turn 2: cli_subagent"));
        assert!(!subjects.contains("provider commit"));

        capture_trusted_turn_head(&state, 3).expect("trusted third-turn head");
        let sentinel = temp.path().join("clean-filter-ran");
        test_git(
            &repository,
            &[
                "config",
                "filter.evil.clean",
                &format!("sh -c 'touch {}; cat'", sentinel.display()),
            ],
        );
        std::fs::write(repository.join(".gitattributes"), "src/*.rs filter=evil\n")
            .expect("provider filter attributes");
        std::fs::write(
            repository.join("src/third.rs"),
            "pub const THIRD: u8 = 3;\n",
        )
        .expect("third source");

        let error = commit_worktree_turn(&state, 3, "cli_subagent")
            .expect_err("provider-selected filter must be refused before staging");
        assert!(error.to_string().contains("external Git filter"), "{error}");
        assert!(
            !sentinel.exists(),
            "sanitisation must not execute the configured clean command"
        );

        let token = CancellationToken::new();
        let expired = capture_trusted_turn_head_bounded(
            &state,
            4,
            ProviderPhaseDeadline::new(tokio::time::Instant::now(), Duration::from_millis(100)),
            &token,
        )
        .expect("typed trusted Git work boundary");
        assert!(matches!(
            expired,
            TrustedGitPhaseOutcome::WorkExpired {
                cleanup: ProviderCleanup::Proven
            }
        ));

        token.cancel();
        let cancelled = capture_trusted_turn_head_bounded(
            &state,
            4,
            ProviderPhaseDeadline::from_now(Duration::from_secs(1), Duration::from_millis(100)),
            &token,
        )
        .expect("typed trusted Git cancellation boundary");
        assert!(matches!(
            cancelled,
            TrustedGitPhaseOutcome::Cancelled {
                cleanup: ProviderCleanup::Proven
            }
        ));
    }

    #[test]
    fn turn_commit_restores_git_control_before_sanitizing_after_restart() {
        let temp = TempDir::new().expect("tempdir");
        let source = temp.path().join("source");
        let worktree = temp.path().join("worktree");
        let decoy = temp.path().join("operator-decoy");
        for repository in [&source, &decoy] {
            std::fs::create_dir_all(repository.join("src")).expect("source directory");
            test_git(repository, &["init", "-q"]);
            test_git(
                repository,
                &["config", "user.email", "fixture@example.invalid"],
            );
            test_git(repository, &["config", "user.name", "fixture"]);
        }
        std::fs::write(source.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n").expect("source");
        test_git(&source, &["add", "-A"]);
        test_git(&source, &["commit", "-q", "-m", "source base"]);
        let base_sha = test_git(&source, &["rev-parse", "HEAD"]);
        test_git(
            &source,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "deadreckon-result",
                worktree.to_str().expect("UTF-8 worktree"),
                &base_sha,
            ],
        );
        std::fs::write(decoy.join("src/operator.txt"), "operator state\n").expect("decoy source");
        test_git(&decoy, &["add", "-A"]);
        test_git(&decoy, &["commit", "-q", "-m", "operator base"]);
        let decoy_head = test_git(&decoy, &["rev-parse", "HEAD"]);
        let decoy_refs = test_git(&decoy, &["show-ref"]);
        let decoy_index = std::fs::read(decoy.join(".git/index")).expect("decoy index");
        let decoy_config = std::fs::read(decoy.join(".git/config")).expect("decoy config");

        let mut codebase = CodebaseRecord::fresh();
        codebase.mode = CodebaseMode::Worktree;
        codebase.source_path = Some(source.clone());
        codebase.source_git_root = Some(source);
        codebase.branch_name = Some("deadreckon-result".to_string());
        codebase.base_ref = Some("master".to_string());
        codebase.base_sha = Some(base_sha.clone());
        codebase.worktree_path = Some(worktree.clone());
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "never follow provider-controlled Git routing".to_string(),
                cwd: worktree.clone(),
                sandbox: "none".to_string(),
                provider: Some("codex".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: Some(codebase),
            },
        )
        .expect("run");

        capture_trusted_turn_head(&state, 1).expect("capture trusted Git control");
        let trusted_control = std::fs::read(worktree.join(".git")).expect("trusted control");
        std::fs::write(worktree.join("src/lib.rs"), "pub fn value() -> u8 { 2 }\n")
            .expect("provider source edit");
        let redirect = format!("gitdir: {}\n", decoy.join(".git").display());
        std::fs::write(worktree.join(".git"), &redirect).expect("provider redirect");

        // Model a process restart: turn setup is called again after the
        // provider changed `.git`. It must preserve the original snapshot,
        // archive the redirect, and restore trusted routing before reading
        // HEAD.
        capture_trusted_turn_head(&state, 1).expect("restart recovery");
        assert_eq!(
            std::fs::read(
                state
                    .run_root
                    .join("provider-evidence/turn-1/git-control-after-provider")
            )
            .expect("redirect evidence"),
            redirect.as_bytes()
        );
        assert_eq!(
            std::fs::read(worktree.join(".git")).expect("restored after restart"),
            trusted_control
        );

        // A second redirect immediately before sanitisation proves each Git
        // command routes explicitly through the trusted snapshot.
        std::fs::write(worktree.join(".git"), &redirect).expect("second provider redirect");
        commit_worktree_turn(&state, 1, "cli_subagent").expect("trusted sanitisation");

        assert_eq!(test_git(&decoy, &["rev-parse", "HEAD"]), decoy_head);
        assert_eq!(test_git(&decoy, &["show-ref"]), decoy_refs);
        assert_eq!(
            std::fs::read(decoy.join(".git/index")).expect("unchanged decoy index"),
            decoy_index
        );
        assert_eq!(
            std::fs::read(decoy.join(".git/config")).expect("unchanged decoy config"),
            decoy_config
        );
        assert_eq!(
            std::fs::read_to_string(decoy.join("src/operator.txt")).expect("decoy worktree"),
            "operator state\n"
        );
        assert_eq!(
            std::fs::read(worktree.join(".git")).expect("final trusted control"),
            trusted_control
        );
        assert_eq!(
            test_git(&worktree, &["symbolic-ref", "--short", "HEAD"]),
            "deadreckon-result"
        );
        let delivered_paths = test_git(
            &worktree,
            &["diff", "--name-only", &format!("{base_sha}..HEAD")],
        );
        assert!(delivered_paths.lines().any(|path| path == "src/lib.rs"));

        // DeadReckon's generated documentation is a later write boundary than
        // the coding provider. It must be committed through the same trusted
        // router, even if `.git` is redirected again between those phases.
        std::fs::create_dir_all(worktree.join("docs")).expect("docs directory");
        std::fs::write(
            worktree.join("docs/RUN-NARRATIVE.md"),
            "# Trusted final docs\n",
        )
        .expect("final docs");
        std::fs::write(worktree.join(".git"), &redirect).expect("docs-phase redirect");
        commit_finalized_turn(&state, 1).expect("trusted final docs sanitisation");
        assert_eq!(test_git(&decoy, &["rev-parse", "HEAD"]), decoy_head);
        assert_eq!(test_git(&decoy, &["show-ref"]), decoy_refs);
        assert_eq!(
            test_git(&worktree, &["show", "HEAD:docs/RUN-NARRATIVE.md",],),
            "# Trusted final docs"
        );

        capture_trusted_turn_head(&state, 2).expect("second-turn control snapshot");
        std::fs::remove_file(worktree.join(".git")).expect("remove regular control");
        std::fs::create_dir(worktree.join(".git")).expect("replace control with directory");
        let directory_error = commit_worktree_turn(&state, 2, "cli_subagent")
            .expect_err("directory Git control must fail closed");
        assert!(
            directory_error
                .to_string()
                .contains("must be a regular file"),
            "{directory_error}"
        );

        #[cfg(unix)]
        {
            std::fs::remove_dir(worktree.join(".git")).expect("remove directory control");
            std::os::unix::fs::symlink(decoy.join(".git"), worktree.join(".git"))
                .expect("replace control with symlink");
            let symlink_error = commit_worktree_turn(&state, 2, "cli_subagent")
                .expect_err("symlink Git control must fail closed");
            assert!(
                symlink_error.to_string().contains("must be a regular file"),
                "{symlink_error}"
            );
        }
        assert_eq!(
            test_git(&decoy, &["show-ref"]),
            decoy_refs,
            "refused control forms must never route sanitisation into the decoy"
        );
    }

    #[test]
    fn result_history_inventory_detects_merge_only_private_paths() {
        let temp = TempDir::new().expect("tempdir");
        let source = temp.path().join("source");
        let worktree = temp.path().join("worktree");
        std::fs::create_dir_all(source.join("src")).expect("source directory");
        test_git(&source, &["init", "-q", "-b", "main"]);
        test_git(
            &source,
            &["config", "user.email", "fixture@example.invalid"],
        );
        test_git(&source, &["config", "user.name", "fixture"]);
        std::fs::write(source.join("src/base.txt"), "base\n").expect("base");
        test_git(&source, &["add", "-A"]);
        test_git(&source, &["commit", "-q", "-m", "base"]);
        let base_sha = test_git(&source, &["rev-parse", "HEAD"]);

        test_git(&source, &["switch", "-q", "-c", "provider-side"]);
        std::fs::write(source.join("src/side.txt"), "side\n").expect("side");
        test_git(&source, &["add", "src/side.txt"]);
        test_git(&source, &["commit", "-q", "-m", "side"]);
        test_git(&source, &["switch", "-q", "main"]);
        test_git(
            &source,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "deadreckon-result",
                worktree.to_str().expect("UTF-8 worktree"),
                &base_sha,
            ],
        );

        let mut codebase = CodebaseRecord::fresh();
        codebase.mode = CodebaseMode::Worktree;
        codebase.source_path = Some(source.clone());
        codebase.source_git_root = Some(source);
        codebase.branch_name = Some("deadreckon-result".to_string());
        codebase.base_ref = Some("main".to_string());
        codebase.base_sha = Some(base_sha.clone());
        codebase.worktree_path = Some(worktree.clone());
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "detect private paths created by a merge".to_string(),
                cwd: worktree.clone(),
                sandbox: "none".to_string(),
                provider: Some("codex".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: Some(codebase),
            },
        )
        .expect("run");
        capture_trusted_turn_head(&state, 1).expect("trusted Git control");

        std::fs::write(worktree.join("src/main.txt"), "main\n").expect("main");
        test_git(&worktree, &["add", "src/main.txt"]);
        test_git(&worktree, &["commit", "-q", "-m", "main"]);
        test_git(
            &worktree,
            &["merge", "-q", "--no-ff", "--no-commit", "provider-side"],
        );
        std::fs::create_dir_all(worktree.join(".specstory")).expect("private directory");
        let private_path = PathBuf::from(".specstory/merge-only.txt");
        std::fs::write(
            worktree.join(&private_path),
            "merge-only private evidence\n",
        )
        .expect("merge-only private evidence");
        test_git(&worktree, &["add", "-f", "--", ".specstory/merge-only.txt"]);
        test_git(&worktree, &["commit", "-q", "-m", "provider merge"]);
        let head = test_git(&worktree, &["rev-parse", "HEAD"]);

        let record = read_turn_codebase_record(&state).expect("trusted routing");
        let control = load_trusted_git_control(&state, 1, &record, None)
            .expect("trusted Git control snapshot");
        let prohibited =
            non_deliverable_history_paths(&control, &base_sha, &head).expect("history inventory");

        assert_eq!(prohibited, vec![private_path]);

        let nested = worktree.join("vendor/nested-repository");
        std::fs::create_dir_all(&nested).expect("nested repository");
        test_git(&nested, &["init", "-q", "-b", "main"]);
        test_git(
            &nested,
            &["config", "user.email", "fixture@example.invalid"],
        );
        test_git(&nested, &["config", "user.name", "fixture"]);
        std::fs::write(nested.join("README.md"), "nested\n").expect("nested file");
        test_git(&nested, &["add", "README.md"]);
        test_git(&nested, &["commit", "-q", "-m", "nested"]);
        test_git(&worktree, &["add", "vendor/nested-repository"]);
        let error = refuse_gitlinks(&control).expect_err("gitlinks must fail before commit");
        assert!(error.to_string().contains("unsupported Git submodule"));
        assert!(error.to_string().contains("vendor/nested-repository"));
    }

    #[cfg(unix)]
    #[test]
    fn git_path_inventory_preserves_non_utf8_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let raw = b".specstory/private-\xff\0src/lib.rs\0";
        let paths = super::nul_separated_paths(raw).expect("raw Git paths");

        assert_eq!(paths[0].as_os_str().as_bytes(), b".specstory/private-\xff");
        assert_eq!(
            paths[0],
            PathBuf::from(std::ffi::OsString::from_vec(
                b".specstory/private-\xff".to_vec()
            ))
        );
        assert_eq!(paths[1], PathBuf::from("src/lib.rs"));
    }

    fn test_git(cwd: &Path, args: &[&str]) -> String {
        let output = deadreckon_core::git::run_git(cwd, args).expect("run git");
        assert!(
            output.status.success(),
            "git {args:?} failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
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
    fn strict_job_refuses_execution_policy_drift() {
        use chrono::Utc;
        use deadreckon_core::write_job;
        use deadreckon_protocol::{
            AuthorityAcceptedBy, Job, JobAuthority, JobExecutionPolicy, JobGateNetworkAccess,
            JobId, JobPolicy, JobSchemaVersion, JobShape, RunId, SemanticJudgeMode,
        };

        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        std::fs::create_dir_all(&source).expect("source");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "bind effective execution policy".to_string(),
                cwd: source.clone(),
                sandbox: "sandbox-exec".to_string(),
                provider: Some("codex".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(60.0),
                run_id: Some("strict-policy".to_string()),
                codebase: Some(CodebaseRecord::fresh()),
            },
        )
        .expect("run");
        let mut execution = JobExecutionPolicy::workspace_only("sandbox-exec");
        execution.gate.network = JobGateNetworkAccess::Loopback;
        execution
            .tools
            .get_mut("bash")
            .expect("bash policy")
            .network_allowlist = vec![
            "127.0.0.1".to_string(),
            "localhost".to_string(),
            "::1".to_string(),
        ];
        let policy = JobPolicy {
            max_spend_usd: 1.0,
            max_wall_seconds: 60,
            max_attempts: 1,
            deadline: None,
            semantic_judge: SemanticJudgeMode::Required,
            execution: Some(execution),
        };
        let policy_hash = deadreckon_core::flight::sha256_text(
            &serde_json::to_string(&policy).expect("policy JSON"),
        );
        let authority = JobAuthority {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(state.run_id.clone()),
            run_id: RunId(state.run_id.clone()),
            approved_at: Utc::now(),
            accepted_by: AuthorityAcceptedBy::Operator,
            goal_sha256: "sha256:goal".to_string(),
            contract_sha256: "sha256:contract".to_string(),
            effective_policy_sha256: policy_hash,
            launch_plan_sha256: "sha256:launch".to_string(),
            source_tree_sha256: "sha256:source".to_string(),
            source_revision: None,
            sandbox_requested: "sandbox-exec".to_string(),
            semantic_judge_mode: SemanticJudgeMode::Required,
            gate_evaluator_sha256: None,
        };
        let authority_path = paths.job_authority(&state.run_id);
        std::fs::create_dir_all(authority_path.parent().expect("authority parent"))
            .expect("authority directory");
        std::fs::write(
            &authority_path,
            serde_json::to_vec_pretty(&authority).expect("authority JSON"),
        )
        .expect("authority");
        let authority_sha256 =
            deadreckon_core::flight::sha256_file(&authority_path).expect("authority digest");
        write_job(
            &paths,
            &Job {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: JobId(state.run_id.clone()),
                scope: state.scope.clone(),
                goal: state.goal.clone(),
                shape: JobShape::Single,
                created_at: Utc::now(),
                source_cwd: source,
                launch_plan_sha256: "sha256:launch".to_string(),
                authority_sha256,
                policy,
            },
        )
        .expect("job");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "capabilities:\n  network: loopback\nchecks:\n  - kind: shell\n    command: \"curl http://127.0.0.1:4173/health\"\n",
        )
        .expect("acceptance contract");
        assert_eq!(
            approved_gate_network_access(&state, true).expect("approved gate network"),
            JobGateNetworkAccess::Loopback
        );
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "capabilities:\n  network: full\nchecks:\n  - kind: shell\n    command: \"curl https://example.com/health\"\n",
        )
        .expect("changed acceptance contract");
        let mismatch = approved_gate_network_access(&state, true)
            .expect_err("contract/policy network drift must fail closed");
        assert!(
            mismatch.to_string().contains("does not match"),
            "{mismatch}"
        );
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "capabilities:\n  network: loopback\nchecks:\n  - kind: shell\n    command: \"curl http://127.0.0.1:4173/health\"\n",
        )
        .expect("restore acceptance contract");

        ensure_sandbox_toml(&state).expect("approved sandbox policy");
        let sandbox_path = state.run_root.join("sandbox.toml");
        let mut sandbox: super::SandboxToml =
            toml::from_str(&std::fs::read_to_string(&sandbox_path).expect("sandbox policy"))
                .expect("sandbox TOML");
        sandbox
            .tools
            .get_mut("bash")
            .expect("bash policy")
            .network
            .push("*".to_string());
        std::fs::write(
            &sandbox_path,
            toml::to_string_pretty(&sandbox).expect("tampered sandbox TOML"),
        )
        .expect("tamper sandbox");

        let error = ensure_sandbox_toml(&state).expect_err("policy drift must fail closed");
        assert!(
            error
                .to_string()
                .contains("immutable approved Job execution policy"),
            "{error}"
        );
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

    #[test]
    fn run_work_clock_preserves_durable_baseline_and_never_decreases() {
        let temp = TempDir::new().expect("tempdir");
        let (_paths, mut state) = create_smoke_run(&temp, "monotonic work clock");
        state.total_wall_seconds = 5.0;
        let work_clock = RunWorkClock::new(&state).expect("work clock");

        work_clock.sync(&mut state);
        assert!(state.total_wall_seconds >= 5.0);

        state.total_wall_seconds = 7.0;
        work_clock.sync(&mut state);
        assert!(
            state.total_wall_seconds >= 7.0,
            "clock synchronization must never move durable time backward"
        );
    }

    #[test]
    fn run_work_clock_returns_zero_instead_of_extending_an_expired_cap() {
        let temp = TempDir::new().expect("tempdir");
        let (_paths, mut state) = create_smoke_run(&temp, "expired work clock");
        state.total_wall_seconds = 5.0;
        let work_clock = RunWorkClock::new(&state).expect("work clock");

        assert_eq!(
            work_clock.remaining(Some(5.0)).expect("remaining"),
            Some(Duration::ZERO)
        );
        assert!(work_clock.remaining(Some(f64::NAN)).is_err());
    }

    #[test]
    fn provider_retries_share_one_absolute_work_deadline() {
        let temp = TempDir::new().expect("tempdir");
        let (_paths, mut state) = create_smoke_run(&temp, "absolute provider deadline");
        state.total_wall_seconds = 5.0;
        let work_clock = RunWorkClock::new(&state).expect("work clock");

        let first = work_clock
            .provider_phase_deadline(Some(30.0))
            .expect("first deadline");
        std::thread::sleep(Duration::from_millis(2));
        let retry = work_clock
            .provider_phase_deadline(Some(30.0))
            .expect("retry deadline");

        assert_eq!(first.work_expires_at, retry.work_expires_at);
        assert_eq!(first.cleanup_budget, Duration::from_secs(30));
        assert_eq!(retry.cleanup_budget, Duration::from_secs(30));
    }

    #[test]
    fn authenticated_resume_ignores_relative_cap_against_durable_baseline() {
        let temp = TempDir::new().expect("tempdir");
        let (_paths, mut state) = create_smoke_run(&temp, "authenticated resume clock");
        state.total_wall_seconds = 120.0;
        let cutoff = tokio::time::Instant::now() + Duration::from_secs(30);
        let work_clock = RunWorkClock::with_boundary(
            &state,
            Some(RunWorkBoundary::new(cutoff, RunWorkExpiry::Deadline)),
        )
        .expect("work clock");

        let remaining = work_clock
            .remaining_seconds(Some(30.0))
            .expect("remaining")
            .expect("bounded");
        let cap = work_clock
            .wall_time_cap_seconds(Some(30.0))
            .expect("accounting cap");

        assert!(remaining > 29.0, "relative cap was reapplied: {remaining}");
        assert!(cap > 149.0, "durable baseline was discarded: {cap}");
        assert_eq!(
            work_clock
                .provider_phase_deadline(Some(30.0))
                .expect("phase deadline")
                .work_expires_at,
            cutoff
        );
    }

    #[tokio::test]
    async fn provider_retry_backoff_never_runs_past_the_absolute_work_deadline() {
        let token = tokio_util::sync::CancellationToken::new();
        let started = tokio::time::Instant::now();

        wait_for_provider_retry(
            started + Duration::from_millis(20),
            &token,
            Duration::from_secs(2),
        )
        .await;

        assert!(
            started.elapsed() < Duration::from_secs(1),
            "retry backoff reopened work after the absolute cutoff"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn sandboxed_tool_uses_work_deadline_then_proves_cleanup() {
        let temp = TempDir::new().expect("tempdir");
        let authority = temp.path().join("tool.pid");
        let token = tokio_util::sync::CancellationToken::new();
        let started = tokio::time::Instant::now();
        let outcome = run_sandboxed_work_phase(
            SandboxSpec {
                backend: SandboxBackend::None,
                docker: None,
                cwd: temp.path().to_path_buf(),
                program: OsString::from("sh"),
                args: vec![OsString::from("-c"), OsString::from("sleep 30")],
                stdin: None,
                env: BTreeMap::new(),
                allow_network: false,
                pid_file: Some(authority.clone()),
                cancellation_token: None,
                profile_dir: None,
                read_allowlist: Vec::new(),
                write_allowlist: Vec::new(),
                read_denylist: Vec::new(),
                write_denylist: Vec::new(),
                network_allowlist: Vec::new(),
                workspace_access: WorkspaceAccess::ReadWrite,
                cleanup_process_group: true,
                guarded_launch: None,
            },
            started + Duration::from_millis(50),
            &token,
        )
        .await;

        assert!(matches!(
            outcome,
            SandboxedPhaseOutcome::WorkExpired {
                cleanup: ProviderCleanup::Proven
            }
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(std::fs::symlink_metadata(authority).is_err());
    }

    #[test]
    fn proven_provider_expiry_pauses_and_proven_cancellation_kills() {
        let temp = TempDir::new().expect("tempdir");
        let (_paths, mut expired) = create_smoke_run(&temp, "proven provider expiry");
        let expired_clock = RunWorkClock::new(&expired).expect("expired clock");

        let outcome = record_provider_interruption(
            &mut expired,
            1,
            ProviderInterruption::WorkExpired,
            &deadreckon_providers::ProviderCleanup::Proven,
            &expired_clock,
        )
        .expect("record expiry");
        assert_eq!(outcome, RunLoopOutcome::PausedAtCap);
        assert_eq!(
            expired.pause_reason.as_deref(),
            Some("wall-clock cap reached mid-turn")
        );

        let other = TempDir::new().expect("other tempdir");
        let (_paths, mut cancelled) = create_smoke_run(&other, "proven provider cancellation");
        let cancelled_clock = RunWorkClock::new(&cancelled).expect("cancelled clock");
        let outcome = record_provider_interruption(
            &mut cancelled,
            1,
            ProviderInterruption::Cancelled,
            &deadreckon_providers::ProviderCleanup::NotApplicable,
            &cancelled_clock,
        )
        .expect("record cancellation");
        assert_eq!(outcome, RunLoopOutcome::Killed);
        assert_eq!(cancelled.status, RunStatus::Killed);
    }

    #[test]
    fn retained_provider_authority_fails_closed_as_lost_containment() {
        let temp = TempDir::new().expect("tempdir");
        let (_paths, mut state) = create_smoke_run(&temp, "retained provider authority");
        let work_clock = RunWorkClock::new(&state).expect("work clock");
        let authority = temp.path().join("provider.pid");

        let outcome = record_provider_interruption(
            &mut state,
            1,
            ProviderInterruption::WorkExpired,
            &deadreckon_providers::ProviderCleanup::RetainedAuthority {
                path: authority.clone(),
                detail: "cleanup proof deadline elapsed".to_string(),
            },
            &work_clock,
        )
        .expect("record lost containment");

        assert_eq!(outcome, RunLoopOutcome::Failed);
        assert_eq!(state.status, RunStatus::Failed);
        assert_eq!(
            state.provider_failure,
            Some(deadreckon_core::ProviderFailureDisposition::Fatal)
        );
        let reason = state.failure_reason.as_deref().expect("failure reason");
        assert!(reason.starts_with("LOST_CONTAINMENT:"), "{reason}");
        assert!(
            reason.contains(authority.to_string_lossy().as_ref()),
            "{reason}"
        );
        assert_eq!(
            state
                .phases
                .iter()
                .find(|phase| phase.id == PhaseId(40))
                .map(|phase| phase.status),
            Some(PhaseStatus::Failed)
        );
    }

    #[test]
    fn verification_phase_executes_revises_fails_and_completes_explicitly() {
        let temp = TempDir::new().expect("tempdir");
        let (_paths, mut state) = create_smoke_run(&temp, "verification transitions");
        let work_clock = RunWorkClock::new(&state).expect("work clock");
        let phase_status = |state: &PipelineState, id| {
            state
                .phases
                .iter()
                .find(|phase| phase.id == PhaseId(id))
                .map(|phase| phase.status)
        };

        begin_verification(&mut state, &work_clock).expect("begin verification");
        assert_eq!(phase_status(&state, 40), Some(PhaseStatus::Completed));
        assert_eq!(phase_status(&state, 50), Some(PhaseStatus::Executing));
        assert_eq!(state.current_phase_id, PhaseId(50));

        revise_verification(&mut state, &work_clock).expect("revise verification");
        assert_eq!(phase_status(&state, 50), Some(PhaseStatus::Pending));
        assert_eq!(phase_status(&state, 40), Some(PhaseStatus::Executing));
        assert_eq!(state.current_phase_id, PhaseId(40));

        begin_verification(&mut state, &work_clock).expect("restart verification");
        fail_verification(&mut state, &work_clock).expect("fail verification");
        assert_eq!(phase_status(&state, 50), Some(PhaseStatus::Failed));
        assert_eq!(state.status, RunStatus::Failed);

        begin_verification(&mut state, &work_clock).expect("retry verification");
        complete_verification(&mut state, &work_clock).expect("complete verification");
        assert_eq!(phase_status(&state, 50), Some(PhaseStatus::Completed));
        assert_eq!(state.current_phase_id, PhaseId(50));
    }

    #[test]
    fn failed_work_boundary_persists_clock_before_propagating_the_error() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "failed durable boundary");
        state.total_wall_seconds = 5.0;
        let work_clock = RunWorkClock::new(&state).expect("work clock");
        work_clock.save(&mut state).expect("save baseline");
        let before_failure = state.total_wall_seconds;
        std::thread::sleep(Duration::from_millis(2));

        let phase_result: deadreckon_core::Result<()> = Err(
            deadreckon_core::DeadreckonError::InvalidInput("injected phase failure".to_string()),
        );
        let error = persist_work_boundary(&mut state, &work_clock, phase_result)
            .expect_err("phase failure must propagate");

        assert!(error.to_string().contains("injected phase failure"));
        let reloaded = deadreckon_core::load_run(&paths, &state.run_id).expect("durable state");
        assert!(
            reloaded.total_wall_seconds > before_failure,
            "failed local work must still advance the durable run clock"
        );
    }

    #[tokio::test]
    async fn wall_cap_pauses_runs_without_provider_reported_wall_time() {
        // The smoke provider reports no wall time (like direct-API routes);
        // the loop must accrue measured elapsed and honor the cap anyway.
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut state) = create_smoke_run(&temp, "wall capped smoke");
        state.max_wall_seconds = Some(0.0001);
        let router = ProviderRouter::smoke();

        let outcome = run_turn_loop(
            &mut state,
            &router,
            RunLoopConfig {
                provider: Some("smoke".to_string()),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(0.0001),
                sandbox_backend: SandboxBackend::None,
                no_seams: true,
                max_turns: 3,
                from_turn: None,
                event_sender: None,
                cancellation_token: None,
                work_boundary: None,
                narrate: None,
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

        assert_eq!(outcome, RunLoopOutcome::PausedAtCap);
        assert!(
            state
                .pause_reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with("wall-clock cap reached")),
            "unexpected pause reason: {:?}",
            state.pause_reason
        );
        assert!(
            state.total_wall_seconds > 0.0,
            "elapsed wall time must accrue even when the provider reports none"
        );
    }

    // --- Semaphore P9: spend ledger carries real CLI tokens ----------------

    fn write_exec(path: &Path, body: &str) {
        std::fs::write(path, body).expect("write fake bin");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path).expect("meta").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(path, perms).expect("chmod");
        }
    }

    fn cli_route_config(name: &str, kind: ProviderKind, binary: &Path) -> ProviderConfigFile {
        ProviderConfigFile {
            default_provider: None,
            fallback: Some(vec![name.to_string()]),
            providers: [(
                name.to_string(),
                ProviderEntry {
                    kind: Some(kind),
                    api_key: None,
                    api_key_env: None,
                    base_url: None,
                    model: Some(name.to_string()),
                    input_cost_per_million: Some(0.0),
                    output_cost_per_million: Some(0.0),
                    binary: Some(binary.display().to_string()),
                    extra_args: Vec::new(),
                },
            )]
            .into_iter()
            .collect(),
        }
    }

    async fn spend_after_cli_turn(name: &str, kind: ProviderKind, binary: &Path) -> u64 {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = temp.path().join("cwd");
        std::fs::create_dir_all(&cwd).expect("cwd");
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "spend".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: Some(name.to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        let router = ProviderRouter::from_config(cli_route_config(name, kind, binary), None)
            .expect("router");
        let mut config = base_run_loop_config();
        config.provider = Some(name.to_string());
        config.max_spend_usd = Some(1.0);
        config.docs.home = paths.home().to_path_buf();
        run_turn_loop(&mut state, &router, config)
            .await
            .expect("loop");
        spend_summary(&state).expect("spend").input_tokens
    }

    #[tokio::test]
    async fn spend_ledger_records_cli_tokens_per_turn_both_providers() {
        let temp = TempDir::new().expect("tempdir");

        // Fake codex: capability-capable; emits usage 111 and a Done action.
        let codex = temp.path().join("fake-codex");
        write_exec(
            &codex,
            "#!/bin/sh\n\
for a in \"$@\"; do [ \"$a\" = \"--help\" ] && { printf -- '--json\\n-o, --output-last-message <FILE>\\nresume\\n'; exit 0; }; done\n\
prev=\"\"; out=\"\"\n\
for a in \"$@\"; do [ \"$prev\" = \"-o\" ] && out=\"$a\"; prev=\"$a\"; done\n\
printf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"t\"}' '{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":111,\"cached_input_tokens\":0,\"output_tokens\":9}}'\n\
[ -n \"$out\" ] && printf '%s' '{\"action\":\"done\",\"summary\":\"ok\"}' > \"$out\"\n\
exit 0\n",
        );
        let codex_tokens =
            spend_after_cli_turn("cli:codex-test", ProviderKind::CliCodex, &codex).await;
        assert_eq!(
            codex_tokens, 111,
            "codex spend ledger carries real input tokens"
        );

        // Fake claude: capability-capable; emits usage 55 and a Done result.
        let claude = temp.path().join("fake-claude");
        write_exec(
            &claude,
            "#!/bin/sh\n\
for a in \"$@\"; do [ \"$a\" = \"--help\" ] && { printf -- '--output-format stream-json\\n-r, --resume\\n'; exit 0; }; done\n\
printf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s\"}' '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"{\\\"action\\\":\\\"done\\\"}\",\"session_id\":\"s\",\"usage\":{\"input_tokens\":55,\"output_tokens\":7}}'\n\
exit 0\n",
        );
        let claude_tokens =
            spend_after_cli_turn("cli:claude-test", ProviderKind::CliClaudeCode, &claude).await;
        assert_eq!(
            claude_tokens, 55,
            "claude spend ledger carries real input tokens"
        );
    }

    #[test]
    fn degraded_contract_caveat_reaches_attention_surface() {
        use super::degraded_caveat_message;
        let trace = serde_json::json!({
            "caveats": [
                {"code": "provider.contract.degraded", "message": "fell back to raw stdout"}
            ]
        });
        let message = degraded_caveat_message(&trace, 3).expect("degraded surfaces a notice");
        assert!(message.contains("degraded"));
        assert!(message.contains("turn 3"));
        assert!(message.contains("raw stdout"));
        // A clean contract raises nothing on the attention channel.
        assert!(degraded_caveat_message(&serde_json::json!({}), 1).is_none());
        assert!(
            degraded_caveat_message(
                &serde_json::json!({"caveats": [{"code": "provider.session.reset"}]}),
                1
            )
            .is_none()
        );
    }

    #[test]
    fn provider_retryability_is_preserved_as_typed_run_state_evidence() {
        let transient = deadreckon_providers::ProviderError::Http {
            provider: "test".to_string(),
            detail: "opaque transient".to_string(),
            retryable: true,
        };
        let fatal = deadreckon_providers::ProviderError::MissingCredential("test".to_string());

        assert_eq!(
            provider_failure_disposition(&transient),
            deadreckon_core::ProviderFailureDisposition::Retryable
        );
        assert_eq!(
            provider_failure_disposition(&fatal),
            deadreckon_core::ProviderFailureDisposition::Fatal
        );
    }

    #[test]
    fn frozen_gate_validation_rejects_content_and_symlink_tamper() {
        let temp = TempDir::new().expect("tempdir");
        let gate = temp.path().join("dr-gate");
        std::fs::write(&gate, b"approved gate bytes").expect("gate fixture");
        let expected = deadreckon_core::flight::sha256_file(&gate).expect("approved gate digest");

        super::validate_frozen_gate(&gate, &expected).expect("approved gate validates");

        std::fs::write(&gate, b"tampered gate bytes").expect("tamper gate");
        let error = super::validate_frozen_gate(&gate, &expected)
            .expect_err("content tamper must be rejected");
        assert!(
            error
                .to_string()
                .contains("frozen gate artifact changed after Job approval"),
            "{error}"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let target = temp.path().join("replacement");
            std::fs::write(&target, b"approved gate bytes").expect("replacement");
            std::fs::remove_file(&gate).expect("remove gate");
            symlink(&target, &gate).expect("symlink gate");
            let error = super::validate_frozen_gate(&gate, &expected)
                .expect_err("symlink substitution must be rejected");
            assert!(
                error.to_string().contains("regular non-symlink file"),
                "{error}"
            );
        }
    }
}
