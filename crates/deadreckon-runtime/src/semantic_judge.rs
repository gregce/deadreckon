//! Independent, read-only assessment of whether a checked result means the
//! approved goal was achieved.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::Utc;
use deadreckon_core::{
    AcceptanceMarker, DeadreckonError, PipelineState, Result, acceptance_spec_path_for_run_root,
    diff_working_trees, implementation_notes_path, snapshot_diff,
};
use deadreckon_protocol::{
    GoalCoverage, GoalCoverageStatus, JobId, JobSchemaVersion, RunId, SemanticDecision,
    SemanticJudgment,
};
use deadreckon_providers::{
    ProviderCleanup, ProviderError, ProviderKind, ProviderPhaseDeadline, ProviderPhaseOutcome,
    ProviderRequest, ProviderResponse, ProviderRouter, WorkspaceAccess, complete_provider_phase,
};
use deadreckon_sandbox::SandboxBackend;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tempfile::{NamedTempFile, TempDir};
use tokio_util::sync::CancellationToken;

pub const SEMANTIC_JUDGMENT_PATH: &str = "proofs/semantic-judgment.json";
const MAX_CONTRACT_BYTES: usize = 64 * 1024;
const MAX_DIFF_BYTES: usize = 256 * 1024;
const MAX_NOTES_BYTES: usize = 64 * 1024;
const MAX_SUMMARY_CHARS: usize = 4_000;
const MAX_FINDINGS: usize = 64;
const SEMANTIC_CLEANUP_BUDGET: Duration = Duration::from_secs(30);
const SEMANTIC_UNBOUNDED_WORK_WINDOW: Duration = Duration::from_secs(100 * 365 * 24 * 60 * 60);

const EVIDENCE_GOAL: &str = "approved-goal";
const EVIDENCE_CONTRACT: &str = "approved-contract";
const EVIDENCE_DIFF: &str = "source-diff";
const EVIDENCE_GATE: &str = "deterministic-gate";
const EVIDENCE_AUTHORITY: &str = "authority";
const EVIDENCE_NOTES: &str = "implementation-notes";
const EVIDENCE_RESULT_PROJECTION: &str = "result-projection";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticEvidencePack {
    pub schema_version: u32,
    pub job_id: String,
    pub run_id: String,
    pub goal: EvidenceItem,
    pub contract: EvidenceItem,
    pub source_diff: EvidenceItem,
    pub deterministic_gate: EvidenceItem,
    pub authority: EvidenceItem,
    pub implementation_notes: EvidenceItem,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_projection: Option<EvidenceItem>,
    pub changed_files: Vec<String>,
    pub caveats: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub id: String,
    pub content: Value,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SemanticJudgeResult {
    Achieved(SemanticJudgment),
    Revise(SemanticJudgment),
    NeedsReview(SemanticJudgment),
    Unavailable(String),
    LostContainment(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticJudgeAccounting {
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub subscription: bool,
    pub wall_time_seconds: f64,
    pub sandbox_backend: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticJudgeRun {
    pub result: SemanticJudgeResult,
    pub accounting: SemanticJudgeAccounting,
    pub budget_exhaustion: Option<SemanticBudgetExhaustion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticBudgetExhaustion {
    Spend,
    Wall,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SemanticJudgeBudget {
    pub remaining_spend_usd: Option<f64>,
    pub remaining_wall_seconds: Option<f64>,
}

impl SemanticJudgeResult {
    pub fn judgment(&self) -> Option<&SemanticJudgment> {
        match self {
            Self::Achieved(judgment) | Self::Revise(judgment) | Self::NeedsReview(judgment) => {
                Some(judgment)
            }
            Self::Unavailable(_) | Self::LostContainment(_) => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SemanticModelResponse {
    decision: SemanticDecision,
    summary: String,
    goal_coverage: Vec<GoalCoverage>,
    #[serde(rename = "blocking_missing")]
    missing: Vec<String>,
}

pub fn semantic_judgment_path(run_root: &Path) -> PathBuf {
    run_root.join(SEMANTIC_JUDGMENT_PATH)
}

pub fn build_semantic_evidence(
    state: &PipelineState,
    marker: &AcceptanceMarker,
) -> Result<SemanticEvidencePack> {
    build_semantic_evidence_with_baseline(state, marker, None)
}

/// Build semantic evidence for a composed result whose approved baseline is
/// not represented by this run's turn snapshots.
///
/// A durable graph has many child snapshots and one merged result. Comparing
/// the operator-approved source tree directly with that result gives the
/// independent judge the same concrete changed-file evidence a leaf run gets
/// from turn snapshots.
pub fn build_semantic_evidence_against_source(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    approved_source: &Path,
) -> Result<SemanticEvidencePack> {
    build_semantic_evidence_with_baseline(state, marker, Some(approved_source))
}

pub fn validate_semantic_judgment_input(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    judgment: &SemanticJudgment,
) -> Result<()> {
    validate_semantic_judgment_input_with_baseline(state, marker, judgment, None)
}

pub fn validate_semantic_judgment_input_against_source(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    approved_source: &Path,
    judgment: &SemanticJudgment,
) -> Result<()> {
    validate_semantic_judgment_input_with_baseline(state, marker, judgment, Some(approved_source))
}

fn validate_semantic_judgment_input_with_baseline(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    judgment: &SemanticJudgment,
    approved_source: Option<&Path>,
) -> Result<()> {
    let evidence = build_semantic_evidence_with_baseline(state, marker, approved_source)?;
    let input = serde_json::to_string(&evidence).map_err(json_error("semantic evidence"))?;
    let expected = deadreckon_core::flight::sha256_text(&input);
    if judgment.input_sha256 != expected {
        return Err(DeadreckonError::InvalidInput(
            "semantic judgment does not bind the current result, deterministic marker and approved evidence"
                .to_string(),
        ));
    }
    Ok(())
}

fn build_semantic_evidence_with_baseline(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    approved_source: Option<&Path>,
) -> Result<SemanticEvidencePack> {
    let mut caveats = Vec::new();
    let contract_path = acceptance_spec_path_for_run_root(&state.run_root);
    let (contract, contract_truncated) =
        read_bounded_text(&contract_path, MAX_CONTRACT_BYTES, &mut caveats);
    let (notes, notes_truncated) = read_bounded_text(
        &implementation_notes_path(&state.working_dir),
        MAX_NOTES_BYTES,
        &mut caveats,
    );
    let home = state
        .run_root
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| {
            DeadreckonError::InvalidInput(format!(
                "cannot infer DeadReckon home from run root {}",
                state.run_root.display()
            ))
        })?;
    let authority_path =
        deadreckon_core::DeadreckonPaths::from_home(home).job_authority(&state.run_id);
    let authority_bytes = fs::read(&authority_path).map_err(|source| DeadreckonError::Io {
        path: authority_path.clone(),
        source,
    })?;
    let authority =
        serde_json::from_slice(&authority_bytes).map_err(|source| DeadreckonError::Json {
            path: authority_path,
            source,
        })?;
    let diff = if deadreckon_core::result_projection_exists(state) {
        let baseline = approved_source
            .map(Path::to_path_buf)
            .unwrap_or_else(|| state.run_root.join("snapshots").join("turn-0"));
        projected_diff(state, &baseline)
    } else {
        approved_source.map_or_else(
            || snapshot_diff(state, 0, state.turn),
            |source| diff_working_trees(source, &state.working_dir),
        )
    };
    let (changed_files, diff_text) = match diff {
        Ok(summary) => {
            let changed_files = summary
                .files
                .iter()
                .map(|file| file.path.to_string_lossy().to_string())
                .collect();
            let text = summary
                .files
                .iter()
                .filter_map(|file| file.unified_diff.as_deref())
                .collect::<Vec<_>>()
                .join("\n");
            (changed_files, text)
        }
        Err(error) => {
            caveats.push(format!("source diff unavailable: {error}"));
            (Vec::new(), String::new())
        }
    };
    let (diff_text, diff_truncated) = truncate_text(&diff_text, MAX_DIFF_BYTES);
    let result_projection = if deadreckon_core::result_projection_exists(state) {
        Some(EvidenceItem {
            id: EVIDENCE_RESULT_PROJECTION.to_string(),
            content: serde_json::to_value(deadreckon_core::load_result_projection(state)?.manifest)
                .map_err(json_error("result projection manifest"))?,
            truncated: false,
        })
    } else {
        None
    };

    Ok(SemanticEvidencePack {
        schema_version: 1,
        job_id: state.run_id.clone(),
        run_id: state.run_id.clone(),
        goal: EvidenceItem {
            id: EVIDENCE_GOAL.to_string(),
            content: Value::String(state.goal.clone()),
            truncated: false,
        },
        contract: EvidenceItem {
            id: EVIDENCE_CONTRACT.to_string(),
            content: Value::String(contract),
            truncated: contract_truncated,
        },
        source_diff: EvidenceItem {
            id: EVIDENCE_DIFF.to_string(),
            content: Value::String(diff_text),
            truncated: diff_truncated,
        },
        deterministic_gate: EvidenceItem {
            id: EVIDENCE_GATE.to_string(),
            content: serde_json::to_value(marker).map_err(json_error("deterministic marker"))?,
            truncated: false,
        },
        authority: EvidenceItem {
            id: EVIDENCE_AUTHORITY.to_string(),
            content: authority,
            truncated: false,
        },
        implementation_notes: EvidenceItem {
            id: EVIDENCE_NOTES.to_string(),
            content: Value::String(notes),
            truncated: notes_truncated,
        },
        result_projection,
        changed_files,
        caveats,
    })
}

/// Compare only paths selected by the controller's persisted result policy.
///
/// Turn snapshots intentionally retain recoverable worker residue. Once a
/// result projection is sealed, that broader recovery plane must not leak
/// late-ignored output into the independent semantic evidence plane.
fn projected_diff(state: &PipelineState, baseline: &Path) -> Result<deadreckon_core::DiffSummary> {
    let before = deadreckon_core::result_projection_index_at(state, baseline)?;
    let after = deadreckon_core::result_projection_index_at(state, &state.working_dir)?;
    let selected = before
        .files
        .keys()
        .chain(after.files.keys())
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let mut diff = diff_working_trees(baseline, &state.working_dir)?;
    diff.files.retain(|file| selected.contains(&file.path));
    Ok(diff)
}

/// Invoke a fresh provider request. CLI routes receive an empty, temporary
/// workspace under an enforceable read-only sandbox; direct APIs receive only
/// prompt bytes and no cwd. No worker session is ever supplied.
pub async fn run_semantic_judge(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    router: &ProviderRouter,
    sandbox_backend: SandboxBackend,
) -> Result<SemanticJudgeRun> {
    run_semantic_judge_with_baseline(
        state,
        marker,
        router,
        sandbox_backend,
        None,
        SemanticJudgeBudget::default(),
        None,
        None,
    )
    .await
}

pub async fn run_semantic_judge_with_budget(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    router: &ProviderRouter,
    sandbox_backend: SandboxBackend,
    budget: SemanticJudgeBudget,
    cancellation_token: Option<&CancellationToken>,
) -> Result<SemanticJudgeRun> {
    run_semantic_judge_with_baseline(
        state,
        marker,
        router,
        sandbox_backend,
        None,
        budget,
        None,
        cancellation_token,
    )
    .await
}

/// Run an ordinary leaf-result semantic judge under the exact outer Job
/// deadline while retaining the frozen turn-snapshot evidence baseline.
///
/// `budget` still supplies spend accounting; `phase_deadline` is authoritative
/// for work and is never rebuilt from a relative remaining-wall value.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_semantic_judge_with_deadline_and_cancellation(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    router: &ProviderRouter,
    sandbox_backend: SandboxBackend,
    budget: SemanticJudgeBudget,
    phase_deadline: ProviderPhaseDeadline,
    cancellation_token: Option<&CancellationToken>,
) -> Result<SemanticJudgeRun> {
    run_semantic_judge_with_baseline(
        state,
        marker,
        router,
        sandbox_backend,
        None,
        budget,
        Some(phase_deadline),
        cancellation_token,
    )
    .await
}

/// Run the fresh, read-only semantic judge for a composed result against the
/// source tree frozen by the parent job authority.
pub async fn run_semantic_judge_against_source(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    router: &ProviderRouter,
    sandbox_backend: SandboxBackend,
    approved_source: &Path,
) -> Result<SemanticJudgeRun> {
    run_semantic_judge_with_baseline(
        state,
        marker,
        router,
        sandbox_backend,
        Some(approved_source),
        SemanticJudgeBudget::default(),
        None,
        None,
    )
    .await
}

pub async fn run_semantic_judge_against_source_with_budget(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    router: &ProviderRouter,
    sandbox_backend: SandboxBackend,
    approved_source: &Path,
    budget: SemanticJudgeBudget,
) -> Result<SemanticJudgeRun> {
    run_semantic_judge_against_source_with_budget_and_cancellation(
        state,
        marker,
        router,
        sandbox_backend,
        approved_source,
        budget,
        None,
    )
    .await
}

pub async fn run_semantic_judge_against_source_with_budget_and_cancellation(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    router: &ProviderRouter,
    sandbox_backend: SandboxBackend,
    approved_source: &Path,
    budget: SemanticJudgeBudget,
    cancellation_token: Option<&CancellationToken>,
) -> Result<SemanticJudgeRun> {
    run_semantic_judge_with_baseline(
        state,
        marker,
        router,
        sandbox_backend,
        Some(approved_source),
        budget,
        None,
        cancellation_token,
    )
    .await
}

/// Run a composed-result semantic judge under the exact outer Job deadline.
///
/// `budget` still supplies spend accounting; `phase_deadline` is authoritative
/// for work and is never rebuilt from a relative remaining-wall value.
#[allow(clippy::too_many_arguments)]
pub async fn run_semantic_judge_against_source_with_deadline_and_cancellation(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    router: &ProviderRouter,
    sandbox_backend: SandboxBackend,
    approved_source: &Path,
    budget: SemanticJudgeBudget,
    phase_deadline: ProviderPhaseDeadline,
    cancellation_token: Option<&CancellationToken>,
) -> Result<SemanticJudgeRun> {
    run_semantic_judge_with_baseline(
        state,
        marker,
        router,
        sandbox_backend,
        Some(approved_source),
        budget,
        Some(phase_deadline),
        cancellation_token,
    )
    .await
}

// Keep each independent judge input visible at this trust boundary; hiding
// them in a loosely related options bag makes review and call-site auditing
// harder.
#[allow(clippy::too_many_arguments)]
async fn run_semantic_judge_with_baseline(
    state: &PipelineState,
    marker: &AcceptanceMarker,
    router: &ProviderRouter,
    sandbox_backend: SandboxBackend,
    approved_source: Option<&Path>,
    budget: SemanticJudgeBudget,
    exact_phase_deadline: Option<ProviderPhaseDeadline>,
    external_cancellation: Option<&CancellationToken>,
) -> Result<SemanticJudgeRun> {
    let started = Instant::now();
    let selected = router.selected_route_info();
    let phase_deadline = match exact_phase_deadline
        .map(Ok)
        .unwrap_or_else(|| semantic_phase_deadline(started, budget.remaining_wall_seconds))
    {
        Ok(deadline) => deadline,
        Err(reason) => {
            return Ok(unavailable_run(
                format!("strict semantic judge unavailable: {reason}"),
                accounting_without_response(
                    selected.as_ref(),
                    sandbox_backend,
                    started.elapsed().as_secs_f64(),
                ),
            ));
        }
    };
    if tokio::time::Instant::now() >= phase_deadline.work_expires_at {
        return Ok(budget_exhausted_run(
            "strict semantic judge has no remaining wall-time budget".to_string(),
            SemanticBudgetExhaustion::Wall,
            accounting_without_response(
                selected.as_ref(),
                sandbox_backend,
                started.elapsed().as_secs_f64(),
            ),
        ));
    }
    if selected
        .as_ref()
        .is_some_and(|route| route.kind == ProviderKind::ScriptedSmoke)
    {
        return Ok(unavailable_run(
            "strict semantic judge unavailable: the scripted smoke provider is a transport fixture, not an independent semantic assessor"
                .to_string(),
            accounting_without_response(
                selected.as_ref(),
                sandbox_backend,
                started.elapsed().as_secs_f64(),
            ),
        ));
    }
    let evidence = match build_semantic_evidence_with_baseline(state, marker, approved_source) {
        Ok(evidence) => evidence,
        Err(error) => {
            return Ok(unavailable_run(
                format!(
                    "strict semantic judge unavailable: trusted evidence could not be assembled: {error}"
                ),
                accounting_without_response(
                    selected.as_ref(),
                    sandbox_backend,
                    started.elapsed().as_secs_f64(),
                ),
            ));
        }
    };
    let input = serde_json::to_string(&evidence).map_err(json_error("semantic evidence"))?;
    let input_sha256 = deadreckon_core::flight::sha256_text(&input);
    let is_cli = selected
        .as_ref()
        .is_some_and(|route| provider_kind_is_cli(&route.kind));
    if is_cli && sandbox_backend == SandboxBackend::None {
        return Ok(unavailable_run(
            "strict semantic judge unavailable: CLI provider has no enforceable read-only sandbox"
                .to_string(),
            accounting_without_response(
                selected.as_ref(),
                sandbox_backend,
                started.elapsed().as_secs_f64(),
            ),
        ));
    }

    let before = semantic_guard_identity(state)?;
    let judge_workspace =
        is_cli
            .then(TempDir::new)
            .transpose()
            .map_err(|source| DeadreckonError::Io {
                path: state.run_root.join("semantic-judge-workspace"),
                source,
            })?;
    let process_authority = router
        .routes()
        .iter()
        .any(|route| provider_kind_uses_process(&route.kind()))
        .then(|| semantic_judge_process_authority(state))
        .transpose()?;
    let mut request = semantic_provider_request(
        &input,
        judge_workspace.as_ref().map(TempDir::path),
        sandbox_backend,
        process_authority.as_deref(),
    );
    request.cancellation_token = external_cancellation.cloned();
    let completion = complete_provider_phase(router, &mut request, phase_deadline).await;
    let accounting = match &completion {
        ProviderPhaseOutcome::Completed(result) => match result {
            Ok(response) => {
                accounting_from_response(response, sandbox_backend, started.elapsed().as_secs_f64())
            }
            Err(_) => accounting_without_response(
                selected.as_ref(),
                sandbox_backend,
                started.elapsed().as_secs_f64(),
            ),
        },
        ProviderPhaseOutcome::WorkExpired { .. } | ProviderPhaseOutcome::Cancelled { .. } => {
            accounting_without_response(
                selected.as_ref(),
                sandbox_backend,
                started.elapsed().as_secs_f64(),
            )
        }
    };
    if let Some(reason) = semantic_cleanup_failure(&completion) {
        return Ok(lost_containment_run(reason, accounting));
    }
    let after = match semantic_guard_identity(state) {
        Ok(after) => after,
        Err(error) => {
            return Ok(unavailable_run(
                format!(
                    "semantic judge changed or obscured the result workspace; judgment refused: {error}"
                ),
                accounting,
            ));
        }
    };
    if before != after {
        return Ok(unavailable_run(
            "semantic judge changed the result or trusted workspace state; judgment refused"
                .to_string(),
            accounting,
        ));
    }

    let response = match completion {
        ProviderPhaseOutcome::Completed(result) => match result {
            Ok(response) => response,
            Err(error) => {
                return Ok(unavailable_run(
                    format!("strict semantic judge unavailable: {error}"),
                    accounting,
                ));
            }
        },
        ProviderPhaseOutcome::WorkExpired { .. } => {
            return Ok(budget_exhausted_run(
                "strict semantic judge exceeded the remaining wall-time budget".to_string(),
                SemanticBudgetExhaustion::Wall,
                accounting,
            ));
        }
        ProviderPhaseOutcome::Cancelled { .. } => {
            return Ok(unavailable_run(
                "strict semantic judge cancelled by the durable run controller".to_string(),
                accounting,
            ));
        }
    };
    if let Some((dimension, reason)) = semantic_budget_overrun(&accounting, budget) {
        return Ok(budget_exhausted_run(reason, dimension, accounting));
    }

    let result = classify_semantic_response(&state.run_id, &state.run_id, &input_sha256, response);
    Ok(SemanticJudgeRun {
        result,
        accounting,
        budget_exhaustion: None,
    })
}

fn semantic_phase_deadline(
    started: Instant,
    remaining_wall_seconds: Option<f64>,
) -> std::result::Result<ProviderPhaseDeadline, String> {
    let work_budget = match remaining_wall_seconds {
        Some(seconds) if seconds.is_finite() && seconds >= 0.0 => Duration::from_secs_f64(seconds),
        Some(seconds) => {
            return Err(format!(
                "remaining wall-time budget must be finite and non-negative, got {seconds}"
            ));
        }
        None => SEMANTIC_UNBOUNDED_WORK_WINDOW,
    };
    let work_expires_at = started.checked_add(work_budget).ok_or_else(|| {
        "remaining wall-time budget exceeds the monotonic clock range".to_string()
    })?;
    Ok(ProviderPhaseDeadline::new(
        tokio::time::Instant::from_std(work_expires_at),
        SEMANTIC_CLEANUP_BUDGET,
    ))
}

fn semantic_judge_process_authority(state: &PipelineState) -> Result<PathBuf> {
    let directory = state.run_root.join("child-pids");
    fs::create_dir_all(&directory).map_err(|source| DeadreckonError::Io {
        path: directory.clone(),
        source,
    })?;
    Ok(directory.join(format!(
        "semantic-judge-{}.pid",
        uuid::Uuid::new_v4().simple()
    )))
}

fn semantic_guard_identity(state: &PipelineState) -> Result<(String, String)> {
    Ok((
        deadreckon_core::flight::build_workspace_guard_file_index_for_state(state)?.tree_hash(),
        workspace_git_control_identity(&state.working_dir)?,
    ))
}

#[cfg(test)]
fn semantic_guard_identity_with_policy(
    working_dir: &Path,
    policy: &deadreckon_core::WorkspaceCapturePolicy,
) -> Result<(String, String)> {
    Ok((
        deadreckon_core::flight::build_workspace_guard_file_index_with_policy(working_dir, policy)?
            .tree_hash(),
        workspace_git_control_identity(working_dir)?,
    ))
}

fn workspace_git_control_identity(working_dir: &Path) -> Result<String> {
    let git = working_dir.join(".git");
    let metadata = match fs::symlink_metadata(&git) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok("missing".to_string());
        }
        Err(source) => {
            return Err(DeadreckonError::Io { path: git, source });
        }
    };
    if metadata.file_type().is_file() {
        return deadreckon_core::flight::sha256_file(&git).map(|hash| format!("file:{hash}"));
    }
    if metadata.file_type().is_dir() {
        let hash = deadreckon_core::flight::build_workspace_guard_file_index(&git)?.tree_hash();
        return Ok(format!("directory:{hash}"));
    }
    Err(DeadreckonError::InvalidInput(format!(
        "semantic judge refuses unsupported Git control entry {}",
        git.display()
    )))
}

fn semantic_cleanup_failure(
    completion: &ProviderPhaseOutcome<deadreckon_providers::Result<ProviderResponse>>,
) -> Option<String> {
    let cleanup = match completion {
        ProviderPhaseOutcome::WorkExpired { cleanup }
        | ProviderPhaseOutcome::Cancelled { cleanup } => cleanup,
        ProviderPhaseOutcome::Completed(Err(ProviderError::CleanupIncomplete {
            provider,
            authority,
            detail,
        })) => {
            return Some(format!(
                "strict semantic judge provider cleanup was not proven for {provider}: {detail}; process authority: {authority:?}"
            ));
        }
        ProviderPhaseOutcome::Completed(_) => return None,
    };
    match cleanup {
        ProviderCleanup::Proven | ProviderCleanup::NotApplicable => None,
        ProviderCleanup::RetainedAuthority { path, detail } => Some(format!(
            "strict semantic judge cleanup was not proven within the bounded {:.1}s safety window: {detail}; process authority remains at {}",
            SEMANTIC_CLEANUP_BUDGET.as_secs_f64(),
            path.display()
        )),
    }
}

fn provider_kind_uses_process(kind: &ProviderKind) -> bool {
    matches!(kind, ProviderKind::CliClaudeCode | ProviderKind::CliCodex)
        || matches!(kind, ProviderKind::Generic(_))
}

fn unavailable_run(reason: String, accounting: SemanticJudgeAccounting) -> SemanticJudgeRun {
    SemanticJudgeRun {
        result: SemanticJudgeResult::Unavailable(reason),
        accounting,
        budget_exhaustion: None,
    }
}

fn lost_containment_run(reason: String, accounting: SemanticJudgeAccounting) -> SemanticJudgeRun {
    SemanticJudgeRun {
        result: SemanticJudgeResult::LostContainment(reason),
        accounting,
        budget_exhaustion: None,
    }
}

fn budget_exhausted_run(
    reason: String,
    dimension: SemanticBudgetExhaustion,
    accounting: SemanticJudgeAccounting,
) -> SemanticJudgeRun {
    SemanticJudgeRun {
        result: SemanticJudgeResult::Unavailable(reason),
        accounting,
        budget_exhaustion: Some(dimension),
    }
}

fn accounting_without_response(
    selected: Option<&deadreckon_providers::ProviderRouteInfo>,
    sandbox_backend: SandboxBackend,
    wall_time_seconds: f64,
) -> SemanticJudgeAccounting {
    SemanticJudgeAccounting {
        provider: selected
            .map(|route| route.name.clone())
            .unwrap_or_else(|| "unavailable".to_string()),
        model: selected
            .map(|route| route.model.clone())
            .unwrap_or_else(|| "unavailable".to_string()),
        input_tokens: 0,
        output_tokens: 0,
        cost_usd: 0.0,
        subscription: selected.is_some_and(|route| provider_kind_is_cli(&route.kind)),
        wall_time_seconds,
        sandbox_backend: Some(sandbox_backend.to_string()),
    }
}

fn accounting_from_response(
    response: &ProviderResponse,
    requested_backend: SandboxBackend,
    measured_wall_seconds: f64,
) -> SemanticJudgeAccounting {
    SemanticJudgeAccounting {
        provider: response.spend.provider.clone(),
        model: response.spend.model.clone(),
        input_tokens: response.spend.input_tokens,
        output_tokens: response.spend.output_tokens,
        cost_usd: response.spend.cost_usd,
        subscription: response.spend.subscription,
        wall_time_seconds: response
            .spend
            .wall_time_seconds
            .unwrap_or(0.0)
            .max(measured_wall_seconds),
        sandbox_backend: response
            .trace
            .get("sandbox_backend")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| Some(requested_backend.to_string())),
    }
}

fn semantic_budget_overrun(
    accounting: &SemanticJudgeAccounting,
    budget: SemanticJudgeBudget,
) -> Option<(SemanticBudgetExhaustion, String)> {
    if budget
        .remaining_spend_usd
        .is_some_and(|remaining| accounting.cost_usd > remaining)
    {
        return Some((
            SemanticBudgetExhaustion::Spend,
            format!(
                "strict semantic judge exceeded the remaining spend budget (${:.6} used)",
                accounting.cost_usd
            ),
        ));
    }
    if budget
        .remaining_wall_seconds
        .is_some_and(|remaining| accounting.wall_time_seconds > remaining)
    {
        return Some((
            SemanticBudgetExhaustion::Wall,
            format!(
                "strict semantic judge exceeded the remaining wall-time budget ({:.3}s used)",
                accounting.wall_time_seconds
            ),
        ));
    }
    None
}

fn provider_kind_is_cli(kind: &ProviderKind) -> bool {
    match kind {
        ProviderKind::CliClaudeCode | ProviderKind::CliCodex => true,
        ProviderKind::Generic(id) => id.starts_with("cli:") || id.starts_with("cli-"),
        ProviderKind::Anthropic
        | ProviderKind::OpenAi
        | ProviderKind::OpenAiCompatible
        | ProviderKind::ScriptedSmoke => false,
    }
}

fn semantic_provider_request(
    evidence_json: &str,
    judge_workspace: Option<&Path>,
    sandbox_backend: SandboxBackend,
    process_authority: Option<&Path>,
) -> ProviderRequest {
    ProviderRequest {
        prompt: format!(
            "You are an independent completion judge. Assess meaning only; deterministic checks \
             have already passed and you may not override them. Use only the evidence packet. \
             Return JSON matching the supplied schema. `blocking_missing` means only an omission \
             that prevents the approved goal from being achieved. If `decision` is `achieved`, \
             `blocking_missing` MUST be empty, every `goal_coverage.status` MUST be `met`, and \
             every coverage item MUST cite evidence. Put cosmetic issues and other non-blocking \
             observations in `summary`, never in `blocking_missing`. If any blocking omission \
             exists, use `revise` when another implementation turn could fix it or `uncertain` \
             when the supplied evidence cannot decide it. Every evidence citation must be one of \
             approved-goal, approved-contract, source-diff, deterministic-gate, authority, \
             implementation-notes, result-projection.\n\nEVIDENCE PACK:\n{evidence_json}"
        ),
        max_output_tokens: 2_048,
        cwd: judge_workspace.map(Path::to_path_buf),
        output_path: None,
        sandbox_backend: Some(sandbox_backend),
        workspace_access: WorkspaceAccess::ReadOnly,
        pid_file: process_authority.map(Path::to_path_buf),
        cancellation_token: None,
        session_dir: None,
        output_schema: Some(semantic_output_schema()),
        capability_posture: None,
    }
}

fn classify_semantic_response(
    job_id: &str,
    run_id: &str,
    input_sha256: &str,
    response: ProviderResponse,
) -> SemanticJudgeResult {
    let judgment = match parse_semantic_response(job_id, run_id, input_sha256, response) {
        Ok(judgment) => judgment,
        Err(error) => {
            return SemanticJudgeResult::Unavailable(format!(
                "strict semantic judge unavailable: response was malformed or invalid: {error}"
            ));
        }
    };
    match judgment.decision {
        SemanticDecision::Achieved => SemanticJudgeResult::Achieved(judgment),
        SemanticDecision::Revise => SemanticJudgeResult::Revise(judgment),
        SemanticDecision::Uncertain => SemanticJudgeResult::NeedsReview(judgment),
    }
}

fn parse_semantic_response(
    job_id: &str,
    run_id: &str,
    input_sha256: &str,
    response: ProviderResponse,
) -> Result<SemanticJudgment> {
    let content = strip_json_fence(response.content.trim());
    let mut model: SemanticModelResponse = serde_json::from_str(content).map_err(|source| {
        DeadreckonError::InvalidInput(format!("semantic judge returned malformed JSON: {source}"))
    })?;
    bound_model_response(&mut model)?;
    validate_evidence_references(&model.goal_coverage)?;
    validate_achieved_response(&model)?;
    Ok(SemanticJudgment {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: JobId(job_id.to_string()),
        run_id: RunId(run_id.to_string()),
        judged_at: Utc::now(),
        provider: response.provider,
        model: response.model,
        decision: model.decision,
        summary: model.summary,
        goal_coverage: model.goal_coverage,
        missing: model.missing,
        input_sha256: input_sha256.to_string(),
        spend_usd: response.spend.cost_usd,
    })
}

pub fn persist_semantic_judgment(run_root: &Path, judgment: &SemanticJudgment) -> Result<()> {
    let path = semantic_judgment_path(run_root);
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "semantic proof path has no parent: {}",
            path.display()
        ))
    })?;
    fs::create_dir_all(parent).map_err(|source| DeadreckonError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    let mut temp = NamedTempFile::new_in(parent).map_err(|source| DeadreckonError::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    serde_json::to_writer_pretty(&mut temp, judgment).map_err(|source| DeadreckonError::Json {
        path: path.clone(),
        source,
    })?;
    temp.write_all(b"\n")
        .map_err(|source| DeadreckonError::Io {
            path: path.clone(),
            source,
        })?;
    temp.as_file_mut()
        .sync_all()
        .map_err(|source| DeadreckonError::Io {
            path: path.clone(),
            source,
        })?;
    temp.persist(&path).map_err(|error| DeadreckonError::Io {
        path,
        source: error.error,
    })?;
    Ok(())
}

fn read_bounded_text(path: &Path, limit: usize, caveats: &mut Vec<String>) -> (String, bool) {
    if let Ok(bytes) = fs::read(path) {
        let text = String::from_utf8_lossy(&bytes);
        truncate_text(&text, limit)
    } else {
        caveats.push(format!("evidence absent: {}", path.display()));
        (String::new(), false)
    }
}

fn truncate_text(text: &str, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text.to_string(), false);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (
        format!(
            "{}\n[… {} bytes omitted by semantic evidence bound]",
            &text[..end],
            text.len() - end
        ),
        true,
    )
}

fn bound_model_response(model: &mut SemanticModelResponse) -> Result<()> {
    if model.summary.chars().count() > MAX_SUMMARY_CHARS {
        return Err(DeadreckonError::InvalidInput(
            "semantic judge summary exceeded the 4000-character bound".to_string(),
        ));
    }
    if model.goal_coverage.len() > MAX_FINDINGS || model.missing.len() > MAX_FINDINGS {
        return Err(DeadreckonError::InvalidInput(
            "semantic judge returned too many findings".to_string(),
        ));
    }
    Ok(())
}

fn validate_evidence_references(coverage: &[GoalCoverage]) -> Result<()> {
    let allowed = [
        EVIDENCE_GOAL,
        EVIDENCE_CONTRACT,
        EVIDENCE_DIFF,
        EVIDENCE_GATE,
        EVIDENCE_AUTHORITY,
        EVIDENCE_NOTES,
        EVIDENCE_RESULT_PROJECTION,
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    for item in coverage {
        for reference in &item.evidence {
            if !allowed.contains(reference.as_str()) {
                return Err(DeadreckonError::InvalidInput(format!(
                    "semantic judge cited unknown evidence id {reference}"
                )));
            }
        }
    }
    Ok(())
}

fn validate_achieved_response(model: &SemanticModelResponse) -> Result<()> {
    if model.decision != SemanticDecision::Achieved {
        return Ok(());
    }
    if model.goal_coverage.is_empty() {
        return Err(DeadreckonError::InvalidInput(
            "semantic judge claimed achieved without goal coverage".to_string(),
        ));
    }
    if !model.missing.is_empty() {
        return Err(DeadreckonError::InvalidInput(
            "semantic judge claimed achieved while reporting missing claims".to_string(),
        ));
    }
    for item in &model.goal_coverage {
        if item.status != GoalCoverageStatus::Met {
            return Err(DeadreckonError::InvalidInput(format!(
                "semantic judge claimed achieved with non-met goal coverage: {}",
                item.claim
            )));
        }
        if item.evidence.is_empty() {
            return Err(DeadreckonError::InvalidInput(format!(
                "semantic judge claimed achieved without evidence for goal coverage: {}",
                item.claim
            )));
        }
    }
    Ok(())
}

fn strip_json_fence(content: &str) -> &str {
    content
        .strip_prefix("```json")
        .or_else(|| content.strip_prefix("```"))
        .and_then(|content| content.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(content)
}

fn semantic_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["decision", "summary", "goal_coverage", "blocking_missing"],
        "properties": {
            "decision": {
                "type": "string",
                "enum": ["achieved", "revise", "uncertain"],
                "description": "Use achieved only when every approved claim is met and blocking_missing is empty."
            },
            "summary": {
                "type": "string",
                "maxLength": MAX_SUMMARY_CHARS,
                "description": "Evidence-backed assessment. Non-blocking or cosmetic observations belong here."
            },
            "goal_coverage": {
                "type": "array",
                "maxItems": MAX_FINDINGS,
                "description": "Coverage of approved goal and contract claims. Achieved requires every item to be met with evidence.",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["claim", "status", "evidence"],
                    "properties": {
                        "claim": { "type": "string" },
                        "status": { "type": "string", "enum": ["met", "missing", "unclear"] },
                        "evidence": {
                            "type": "array",
                            "items": {
                                "type": "string",
                                "enum": [
                                    EVIDENCE_GOAL,
                                    EVIDENCE_CONTRACT,
                                    EVIDENCE_DIFF,
                                    EVIDENCE_GATE,
                                    EVIDENCE_AUTHORITY,
                                    EVIDENCE_NOTES,
                                    EVIDENCE_RESULT_PROJECTION
                                ]
                            }
                        }
                    }
                }
            },
            "blocking_missing": {
                "type": "array",
                "maxItems": MAX_FINDINGS,
                "description": "Only omissions that prevent goal achievement. Must be empty when decision is achieved.",
                "items": { "type": "string" }
            }
        }
    })
}

fn json_error(label: &'static str) -> impl FnOnce(serde_json::Error) -> DeadreckonError {
    move |source| DeadreckonError::Json {
        path: PathBuf::from(label),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use deadreckon_providers::{
        ProviderCleanup, ProviderConfigFile, ProviderEntry, ProviderPhaseDeadline,
        ProviderPhaseOutcome, ProviderRequest, ProviderResponse, ProviderRouter, ProviderUsage,
        SpendEstimate, WorkspaceAccess, complete_provider_phase,
    };
    use deadreckon_sandbox::SandboxBackend;

    use super::{
        EVIDENCE_CONTRACT, EVIDENCE_DIFF, EVIDENCE_GATE, EVIDENCE_RESULT_PROJECTION,
        SEMANTIC_CLEANUP_BUDGET, SemanticBudgetExhaustion, SemanticDecision,
        SemanticJudgeAccounting, SemanticJudgeBudget, SemanticJudgeResult,
        accounting_from_response, build_semantic_evidence, build_semantic_evidence_against_source,
        classify_semantic_response, provider_kind_is_cli, semantic_budget_overrun,
        semantic_cleanup_failure, semantic_guard_identity_with_policy, semantic_output_schema,
        semantic_phase_deadline, semantic_provider_request, strip_json_fence,
        validate_evidence_references, validate_semantic_judgment_input,
        validate_semantic_judgment_input_against_source,
    };
    use deadreckon_protocol::{
        GoalCoverage, GoalCoverageStatus, JobId, JobSchemaVersion, RunId, SemanticJudgment,
    };
    use deadreckon_providers::ProviderKind;

    fn projected_semantic_fixture() -> (
        tempfile::TempDir,
        deadreckon_core::PipelineState,
        deadreckon_core::AcceptanceMarker,
    ) {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = deadreckon_core::DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let mut state = deadreckon_core::create_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: "ship the selected source without runtime residue".to_string(),
                cwd: source,
                sandbox: "sandbox-exec".to_string(),
                provider: Some("independent-test-judge".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(60.0),
                run_id: Some("projected-semantic-fixture".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        fs::write(state.working_dir.join("baseline.txt"), "before\n").expect("baseline");
        deadreckon_core::snapshot_working(&state, 0).expect("turn zero");
        fs::write(state.working_dir.join("baseline.txt"), "after\n").expect("result");
        fs::write(state.working_dir.join(".gitignore"), "/.runtime-z91/\n").expect("ignore");
        fs::create_dir_all(state.working_dir.join(".runtime-z91")).expect("runtime");
        fs::write(state.working_dir.join(".runtime-z91/lock"), "residue\n").expect("residue");
        state.turn = 1;
        deadreckon_core::snapshot_working(&state, 1).expect("turn one");
        deadreckon_core::seal_result_projection(&state).expect("projection");
        fs::write(
            deadreckon_core::acceptance_spec_path_for_run_root(&state.run_root),
            "name: projected\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/baseline.txt\"\n",
        )
        .expect("contract");
        let authority_path = paths.job_authority(&state.run_id);
        fs::create_dir_all(authority_path.parent().expect("authority parent"))
            .expect("authority parent");
        fs::write(&authority_path, "{}\n").expect("authority");
        let marker = deadreckon_core::AcceptanceMarker {
            schema_version: 2,
            run_id: state.run_id.clone(),
            status: "pass".to_string(),
            produced_by: "dr-gate".to_string(),
            issuer: "dr-gate".to_string(),
            proof_kind: deadreckon_core::AcceptanceProofKind::NativeGate,
            checked_at: chrono::Utc::now(),
            working_dir: deadreckon_core::result_projection_evaluation_path(&state),
            contained: true,
            sandbox_backend: "sandbox-exec".to_string(),
            signature: "test".to_string(),
            check_count: 1,
            checks: Vec::new(),
        };
        (temp, state, marker)
    }

    #[test]
    fn semantic_judge_has_no_worker_session_or_write_capability() {
        let process_authority = std::path::Path::new("/tmp/semantic-judge.pid");
        let request = semantic_provider_request(
            "{}",
            Some(std::path::Path::new("/tmp/empty-judge-workspace")),
            SandboxBackend::SandboxExec,
            Some(process_authority),
        );
        assert_eq!(
            request.workspace_access,
            deadreckon_providers::WorkspaceAccess::ReadOnly
        );
        assert!(request.session_dir.is_none());
        assert_eq!(request.pid_file.as_deref(), Some(process_authority));
        assert_eq!(
            request.cwd.as_deref(),
            Some(std::path::Path::new("/tmp/empty-judge-workspace"))
        );
        assert!(request.prompt.contains("`blocking_missing` MUST be empty"));
        assert!(
            request
                .prompt
                .contains("cosmetic issues and other non-blocking observations")
        );
    }

    #[test]
    fn semantic_guard_covers_lifecycle_evidence_and_git_control() {
        let temp = tempfile::tempdir().expect("tempdir");
        let working = temp.path();
        fs::create_dir_all(working.join(".deadreckon")).expect("lifecycle");
        fs::create_dir_all(working.join(".specstory")).expect("evidence");
        fs::write(working.join("result.txt"), "result\n").expect("result");
        fs::write(working.join(".deadreckon/codebase.json"), "{}\n").expect("codebase");
        fs::write(working.join(".specstory/trace.jsonl"), "{}\n").expect("trace");
        let policy = deadreckon_core::freeze_workspace_capture_policy(working).expect("policy");
        fs::write(working.join(".git"), "gitdir: /trusted/one\n").expect("git control");

        let before = semantic_guard_identity_with_policy(working, &policy).expect("before");
        fs::write(
            working.join(".deadreckon/codebase.json"),
            "{\"tampered\":true}\n",
        )
        .expect("tamper lifecycle");
        let lifecycle =
            semantic_guard_identity_with_policy(working, &policy).expect("lifecycle identity");
        assert_ne!(before, lifecycle);

        fs::write(
            working.join(".specstory/trace.jsonl"),
            "{\"tampered\":true}\n",
        )
        .expect("tamper evidence");
        let evidence =
            semantic_guard_identity_with_policy(working, &policy).expect("evidence identity");
        assert_ne!(lifecycle, evidence);

        fs::write(working.join(".git"), "gitdir: /operator/decoy\n").expect("tamper Git control");
        let git = semantic_guard_identity_with_policy(working, &policy).expect("Git identity");
        assert_ne!(evidence, git);
    }

    #[test]
    fn semantic_judge_receives_goal_contract_diff_and_gate_evidence() {
        let request = semantic_provider_request(
            "{\"goal\":\"x\"}",
            None,
            SandboxBackend::Docker,
            Some(std::path::Path::new("/tmp/semantic-judge.pid")),
        );
        for evidence in [
            EVIDENCE_CONTRACT,
            EVIDENCE_DIFF,
            EVIDENCE_GATE,
            EVIDENCE_RESULT_PROJECTION,
        ] {
            assert!(request.prompt.contains(evidence));
        }
        assert!(request.output_schema.is_some());
    }

    #[test]
    fn semantic_judge_reads_candidate_and_projection_omissions() {
        let (_temp, state, marker) = projected_semantic_fixture();
        let mut semantic_state = state.clone();
        semantic_state.working_dir = deadreckon_core::result_projection_candidate_path(&state);
        let evidence = build_semantic_evidence(&semantic_state, &marker).expect("evidence");
        let projection = evidence.result_projection.expect("projection evidence");
        let encoded = serde_json::to_string(&projection.content).expect("projection json");
        assert!(encoded.contains(".runtime-z91"));
        assert!(
            evidence
                .changed_files
                .iter()
                .all(|path| !path.contains(".runtime-z91"))
        );
    }

    #[test]
    fn semantic_input_hash_changes_with_h_or_p() {
        let (_temp, state, marker) = projected_semantic_fixture();
        let mut semantic_state = state.clone();
        semantic_state.working_dir = deadreckon_core::result_projection_candidate_path(&state);
        let before = deadreckon_core::flight::sha256_text(
            &serde_json::to_string(
                &build_semantic_evidence(&semantic_state, &marker).expect("before evidence"),
            )
            .expect("before json"),
        );
        fs::write(state.working_dir.join("baseline.txt"), "new candidate\n").expect("change");
        deadreckon_core::seal_result_projection(&state).expect("reseal");
        let after = deadreckon_core::flight::sha256_text(
            &serde_json::to_string(
                &build_semantic_evidence(&semantic_state, &marker).expect("after evidence"),
            )
            .expect("after json"),
        );
        assert_ne!(before, after);
    }

    #[test]
    fn live_worker_residue_is_absent_from_semantic_evidence() {
        let (_temp, state, marker) = projected_semantic_fixture();
        let mut semantic_state = state.clone();
        semantic_state.working_dir = deadreckon_core::result_projection_candidate_path(&state);
        let evidence = build_semantic_evidence(&semantic_state, &marker).expect("evidence");
        assert!(
            evidence
                .changed_files
                .iter()
                .all(|path| !path.contains(".runtime-z91"))
        );
        assert!(
            !evidence
                .source_diff
                .content
                .as_str()
                .unwrap_or_default()
                .contains("residue")
        );
    }

    #[test]
    fn semantic_judge_mutation_guard_covers_candidate() {
        let (_temp, state, _marker) = projected_semantic_fixture();
        let candidate = deadreckon_core::result_projection_candidate_path(&state);
        let policy = deadreckon_core::load_result_projection(&state)
            .expect("projection")
            .policy;
        let before = semantic_guard_identity_with_policy(&candidate, &policy).expect("before");
        fs::write(candidate.join("baseline.txt"), "tampered\n").expect("candidate mutation");
        let after = semantic_guard_identity_with_policy(&candidate, &policy).expect("after");
        assert_ne!(before, after);

        fs::write(
            state.working_dir.join(".runtime-z91/lock"),
            "more residue\n",
        )
        .expect("live residue");
        let after_live = semantic_guard_identity_with_policy(&candidate, &policy).expect("live");
        assert_eq!(after, after_live);
    }

    #[test]
    fn semantic_input_freshness_rejects_a_stale_parent_judgment() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = deadreckon_core::DeadreckonPaths::from_home(temp.path().join("home"));
        let approved_source = temp.path().join("approved-source");
        fs::create_dir_all(&approved_source).expect("approved source");
        fs::write(approved_source.join("result.txt"), "approved baseline\n").expect("baseline");
        let state = deadreckon_core::create_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: "repair the composed parent".to_string(),
                cwd: approved_source.clone(),
                sandbox: "sandbox-exec".to_string(),
                provider: Some("independent-test-judge".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(60.0),
                run_id: Some("semantic-parent-freshness".to_string()),
                codebase: None,
            },
        )
        .expect("parent run");
        fs::write(state.working_dir.join("result.txt"), "candidate A\n").expect("candidate A");
        fs::write(
            deadreckon_core::acceptance_spec_path_for_run_root(&state.run_root),
            "name: parent\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/result.txt\"\n",
        )
        .expect("contract");
        let authority_path = paths.job_authority(&state.run_id);
        fs::create_dir_all(authority_path.parent().expect("authority parent"))
            .expect("authority parent");
        fs::write(
            &authority_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "job_id": state.run_id,
                "authority": "test"
            }))
            .expect("authority json"),
        )
        .expect("authority");
        let marker_a = deadreckon_core::write_acceptance_marker(
            &state.run_root,
            state.run_id.clone(),
            state.working_dir.clone(),
            1,
        )
        .expect("candidate A marker");
        let input_a = deadreckon_core::flight::sha256_text(
            &serde_json::to_string(
                &build_semantic_evidence_against_source(&state, &marker_a, &approved_source)
                    .expect("candidate A evidence"),
            )
            .expect("candidate A evidence json"),
        );
        let mut judgment = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(state.run_id.clone()),
            run_id: RunId(state.run_id.clone()),
            judged_at: chrono::Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Achieved,
            summary: "candidate satisfies the goal".to_string(),
            goal_coverage: vec![GoalCoverage {
                claim: "repair the composed parent".to_string(),
                status: GoalCoverageStatus::Met,
                evidence: vec!["source-diff".to_string(), "deterministic-gate".to_string()],
            }],
            missing: Vec::new(),
            input_sha256: input_a,
            spend_usd: 0.0,
        };
        validate_semantic_judgment_input_against_source(
            &state,
            &marker_a,
            &approved_source,
            &judgment,
        )
        .expect("candidate A judgment is fresh");

        fs::write(state.working_dir.join("result.txt"), "candidate B\n").expect("candidate B");
        let marker_b = deadreckon_core::write_acceptance_marker(
            &state.run_root,
            state.run_id.clone(),
            state.working_dir.clone(),
            1,
        )
        .expect("candidate B marker");
        let stale = validate_semantic_judgment_input_against_source(
            &state,
            &marker_b,
            &approved_source,
            &judgment,
        )
        .expect_err("candidate A judgment must not validate candidate B");
        assert!(
            stale
                .to_string()
                .contains("does not bind the current result"),
            "{stale}"
        );

        let input_b = deadreckon_core::flight::sha256_text(
            &serde_json::to_string(
                &build_semantic_evidence_against_source(&state, &marker_b, &approved_source)
                    .expect("candidate B evidence"),
            )
            .expect("candidate B evidence json"),
        );
        assert_ne!(judgment.input_sha256, input_b);
        judgment.input_sha256 = input_b;
        validate_semantic_judgment_input_against_source(
            &state,
            &marker_b,
            &approved_source,
            &judgment,
        )
        .expect("fresh candidate B judgment validates");
    }

    #[test]
    fn leaf_semantic_binding_uses_frozen_turn_snapshots_not_the_live_source_tree() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = deadreckon_core::DeadreckonPaths::from_home(temp.path().join("home"));
        let result = temp.path().join("result");
        fs::create_dir_all(&result).expect("result");
        fs::write(result.join("purpose.sh"), "unfinished\n").expect("initial result");
        fs::write(
            result.join("implementation-notes.html"),
            "<p>initial notes</p>\n",
        )
        .expect("initial notes");
        let mut state = deadreckon_core::create_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: "finish the leaf result".to_string(),
                cwd: result.clone(),
                sandbox: "sandbox-exec".to_string(),
                provider: Some("independent-test-judge".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(60.0),
                run_id: Some("leaf-snapshot-binding".to_string()),
                codebase: None,
            },
        )
        .expect("leaf run");
        deadreckon_core::snapshot_working(&state, 0).expect("turn zero snapshot");

        let approved_source = temp.path().join("approved-source");
        fs::create_dir_all(&approved_source).expect("approved source");
        fs::write(approved_source.join("purpose.sh"), "unfinished\n").expect("source baseline");
        state.cwd = approved_source.clone();
        fs::write(state.working_dir.join("purpose.sh"), "finished\n").expect("finished result");
        fs::write(
            state.working_dir.join("implementation-notes.html"),
            "<p>finished notes</p>\n",
        )
        .expect("finished notes");
        state.turn = 1;
        deadreckon_core::snapshot_working(&state, 1).expect("turn one snapshot");
        fs::write(
            deadreckon_core::acceptance_spec_path_for_run_root(&state.run_root),
            "name: leaf\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/purpose.sh\"\n",
        )
        .expect("contract");
        let authority_path = paths.job_authority(&state.run_id);
        fs::create_dir_all(authority_path.parent().expect("authority parent"))
            .expect("authority parent");
        fs::write(
            &authority_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "job_id": state.run_id,
                "authority": "test"
            }))
            .expect("authority json"),
        )
        .expect("authority");
        let marker = deadreckon_core::AcceptanceMarker {
            schema_version: 2,
            run_id: state.run_id.clone(),
            status: "pass".to_string(),
            produced_by: "dr-gate".to_string(),
            issuer: "dr-gate".to_string(),
            proof_kind: deadreckon_core::AcceptanceProofKind::NativeGate,
            checked_at: chrono::Utc::now(),
            working_dir: state.working_dir.clone(),
            contained: true,
            sandbox_backend: "sandbox-exec".to_string(),
            signature: "test".to_string(),
            check_count: 1,
            checks: Vec::new(),
        };
        let snapshot_input = deadreckon_core::flight::sha256_text(
            &serde_json::to_string(
                &build_semantic_evidence(&state, &marker).expect("snapshot evidence"),
            )
            .expect("snapshot evidence json"),
        );
        let source_input = deadreckon_core::flight::sha256_text(
            &serde_json::to_string(
                &build_semantic_evidence_against_source(&state, &marker, &approved_source)
                    .expect("source evidence"),
            )
            .expect("source evidence json"),
        );
        assert_ne!(
            snapshot_input, source_input,
            "initialized leaf artifacts make source and turn baselines observably different"
        );
        let judgment = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(state.run_id.clone()),
            run_id: RunId(state.run_id.clone()),
            judged_at: chrono::Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Achieved,
            summary: "the frozen leaf result is complete".to_string(),
            goal_coverage: vec![GoalCoverage {
                claim: "finish the leaf result".to_string(),
                status: GoalCoverageStatus::Met,
                evidence: vec!["source-diff".to_string(), "deterministic-gate".to_string()],
            }],
            missing: Vec::new(),
            input_sha256: snapshot_input,
            spend_usd: 0.0,
        };
        validate_semantic_judgment_input(&state, &marker, &judgment)
            .expect("leaf seal reuses the frozen snapshot evidence");
        let mismatch = validate_semantic_judgment_input_against_source(
            &state,
            &marker,
            &approved_source,
            &judgment,
        )
        .expect_err("a leaf judgment must not silently switch to source-tree evidence");
        assert!(
            mismatch
                .to_string()
                .contains("does not bind the current result"),
            "{mismatch}"
        );
    }

    #[test]
    fn semantic_schema_has_three_decisions() {
        let schema = semantic_output_schema();
        assert_eq!(
            schema["properties"]["decision"]["enum"],
            serde_json::json!(["achieved", "revise", "uncertain"])
        );
        assert_eq!(
            schema["required"],
            serde_json::json!(["decision", "summary", "goal_coverage", "blocking_missing"])
        );
        assert!(schema["properties"].get("missing").is_none());
        assert_eq!(
            schema["properties"]["goal_coverage"]["items"]["properties"]["evidence"]["items"]["enum"],
            serde_json::json!([
                "approved-goal",
                "approved-contract",
                "source-diff",
                "deterministic-gate",
                "authority",
                "implementation-notes",
                "result-projection"
            ])
        );
        deadreckon_providers::validate_openai_strict_output_schema("semantic-test", &schema)
            .expect("semantic schema must remain valid for strict Codex/OpenAI output");
    }

    #[test]
    fn semantic_budget_refuses_only_actual_overruns() {
        let accounting = SemanticJudgeAccounting {
            provider: "judge".to_string(),
            model: "model".to_string(),
            input_tokens: 1,
            output_tokens: 1,
            cost_usd: 0.25,
            subscription: false,
            wall_time_seconds: 2.0,
            sandbox_backend: Some("docker".to_string()),
        };
        assert!(
            semantic_budget_overrun(
                &accounting,
                SemanticJudgeBudget {
                    remaining_spend_usd: Some(0.25),
                    remaining_wall_seconds: Some(2.0),
                },
            )
            .is_none(),
            "using exactly the remaining cap stays within policy"
        );
        assert!(
            semantic_budget_overrun(
                &accounting,
                SemanticJudgeBudget {
                    remaining_spend_usd: Some(0.24),
                    remaining_wall_seconds: Some(3.0),
                },
            )
            .is_some_and(|(dimension, reason)| {
                dimension == SemanticBudgetExhaustion::Spend && reason.contains("spend")
            })
        );
        assert!(
            semantic_budget_overrun(
                &accounting,
                SemanticJudgeBudget {
                    remaining_spend_usd: Some(0.30),
                    remaining_wall_seconds: Some(1.9),
                },
            )
            .is_some_and(|(dimension, reason)| {
                dimension == SemanticBudgetExhaustion::Wall && reason.contains("wall-time")
            })
        );
    }

    #[test]
    fn semantic_accounting_uses_measured_wall_when_provider_undercounts() {
        let response = ProviderResponse {
            provider: "judge".to_string(),
            model: "model".to_string(),
            content: "{}".to_string(),
            usage: ProviderUsage {
                input_tokens: 1,
                output_tokens: 1,
            },
            spend: SpendEstimate {
                provider: "judge".to_string(),
                model: "model".to_string(),
                input_tokens: 1,
                output_tokens: 1,
                cost_usd: 0.1,
                subscription: false,
                wall_time_seconds: Some(0.1),
            },
            trace: serde_json::Value::Null,
        };

        let accounting = accounting_from_response(&response, SandboxBackend::Docker, 2.5);
        assert_eq!(accounting.wall_time_seconds, 2.5);
    }

    #[test]
    fn semantic_deadline_is_anchored_before_preflight_with_separate_cleanup() {
        let started = std::time::Instant::now();
        let deadline = semantic_phase_deadline(started, Some(2.5)).expect("semantic deadline");

        assert_eq!(
            deadline.work_expires_at.into_std(),
            started + Duration::from_secs_f64(2.5)
        );
        assert_eq!(deadline.cleanup_budget, SEMANTIC_CLEANUP_BUDGET);
    }

    #[test]
    fn cancelled_semantic_phase_with_proven_cleanup_is_not_lost_containment() {
        let cancelled =
            ProviderPhaseOutcome::<deadreckon_providers::Result<ProviderResponse>>::Cancelled {
                cleanup: ProviderCleanup::Proven,
            };
        assert!(semantic_cleanup_failure(&cancelled).is_none());
    }

    #[test]
    fn completed_provider_cleanup_error_is_lost_containment() {
        let authority = std::path::PathBuf::from("/tmp/semantic-judge-retained.pid");
        let completion = ProviderPhaseOutcome::Completed(Err(
            deadreckon_providers::ProviderError::CleanupIncomplete {
                provider: "judge".to_string(),
                authority: Some(authority.clone()),
                detail: "provider returned before cleanup proof".to_string(),
            },
        ));

        let reason = semantic_cleanup_failure(&completion)
            .expect("completed cleanup failure must remain fail closed");
        assert!(reason.contains("judge"), "{reason}");
        assert!(
            reason.contains("provider returned before cleanup proof"),
            "{reason}"
        );
        assert!(
            reason.contains(&authority.display().to_string()),
            "{reason}"
        );
    }

    #[test]
    fn expired_semantic_call_retains_unproven_process_authority() {
        let temp = tempfile::tempdir().expect("tempdir");
        let process_authority = temp.path().join("semantic-judge.pid");
        fs::write(&process_authority, b"retained authority\n").expect("process authority");
        let outcome =
            ProviderPhaseOutcome::<deadreckon_providers::Result<ProviderResponse>>::WorkExpired {
                cleanup: ProviderCleanup::RetainedAuthority {
                    path: process_authority.clone(),
                    detail: "provider cleanup did not resolve within 30.0s".to_string(),
                },
            };

        assert!(matches!(
            outcome,
            ProviderPhaseOutcome::WorkExpired {
                cleanup: ProviderCleanup::RetainedAuthority { .. }
            }
        ));
        let reason =
            semantic_cleanup_failure(&outcome).expect("unresolved cleanup must fail closed");
        assert!(reason.contains("cleanup was not proven"), "{reason}");
        assert!(reason.contains(&process_authority.display().to_string()));
        assert!(
            process_authority.exists(),
            "unproven process authority must remain available for recovery"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn semantic_cancellation_reaps_adversarial_background_descendant() {
        use std::os::unix::fs::PermissionsExt as _;

        struct DescendantGuard(std::path::PathBuf);
        impl Drop for DescendantGuard {
            fn drop(&mut self) {
                let Ok(raw) = fs::read_to_string(&self.0) else {
                    return;
                };
                let Ok(pid) = raw.trim().parse::<u32>() else {
                    return;
                };
                if deadreckon_core::pid_is_alive(pid) {
                    let _ = deadreckon_core::terminate_pid(pid, true);
                }
            }
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let binary = temp.path().join("adversarial-judge");
        let descendant_path = temp.path().join("descendant.pid");
        let _descendant_guard = DescendantGuard(descendant_path.clone());
        fs::write(
            &binary,
            format!(
                "#!/bin/sh\nif [ \"${{1:-}}\" = \"--help\" ]; then printf '%s\\n' 'Usage: adversarial-judge'; exit 0; fi\n(trap '' TERM; sleep 60) &\ndescendant=$!\nprintf '%s\\n' \"$descendant\" > '{}'\ntrap '' TERM\nwait\n",
                descendant_path.display()
            ),
        )
        .expect("adversarial judge");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
            .expect("adversarial judge permissions");
        let router = ProviderRouter::from_config(
            ProviderConfigFile {
                default_provider: None,
                fallback: Some(vec!["cli:claude-code".to_string()]),
                providers: [(
                    "cli:claude-code".to_string(),
                    ProviderEntry {
                        kind: Some(ProviderKind::CliClaudeCode),
                        api_key: None,
                        api_key_env: None,
                        base_url: None,
                        model: Some("adversarial-judge".to_string()),
                        input_cost_per_million: Some(0.0),
                        output_cost_per_million: Some(0.0),
                        binary: Some(binary.display().to_string()),
                        extra_args: Vec::new(),
                    },
                )]
                .into_iter()
                .collect(),
            },
            None,
        )
        .expect("judge router");
        let process_authority = temp.path().join("semantic-judge.pid");
        let external_cancellation = tokio_util::sync::CancellationToken::new();
        let mut request = ProviderRequest {
            prompt: "judge evidence".to_string(),
            max_output_tokens: 64,
            cwd: Some(temp.path().to_path_buf()),
            output_path: None,
            sandbox_backend: None,
            workspace_access: WorkspaceAccess::ReadWrite,
            pid_file: Some(process_authority.clone()),
            cancellation_token: Some(external_cancellation.clone()),
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        };
        let cancel_after_descendant_starts = async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            while !descendant_path.exists() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert!(
                descendant_path.exists(),
                "adversarial descendant did not start before cancellation"
            );
            external_cancellation.cancel();
        };
        let (outcome, ()) = tokio::time::timeout(Duration::from_secs(6), async {
            tokio::join!(
                complete_provider_phase(
                    &router,
                    &mut request,
                    ProviderPhaseDeadline::new(
                        tokio::time::Instant::now() + Duration::from_secs(5),
                        Duration::from_secs(3),
                    ),
                ),
                cancel_after_descendant_starts,
            )
        })
        .await
        .expect("semantic cancellation and cleanup must remain bounded");

        assert!(matches!(
            outcome,
            ProviderPhaseOutcome::Cancelled {
                cleanup: ProviderCleanup::Proven
            }
        ));
        assert!(
            semantic_cleanup_failure(&outcome).is_none(),
            "completed descendant reconciliation must prove semantic cleanup"
        );
        assert!(
            !process_authority.exists(),
            "proven semantic cleanup must remove process authority"
        );
        let descendant = fs::read_to_string(&descendant_path)
            .expect("descendant pid")
            .trim()
            .parse::<u32>()
            .expect("numeric descendant pid");
        let exit_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while deadreckon_core::pid_is_alive(descendant) && std::time::Instant::now() < exit_deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let survived = deadreckon_core::pid_is_alive(descendant);
        if survived {
            let _ = deadreckon_core::terminate_pid(descendant, true);
        }
        assert!(!survived, "background semantic descendant survived cleanup");
    }

    #[test]
    fn semantic_evidence_rejects_unknown_citations() {
        let coverage = vec![GoalCoverage {
            claim: "done".to_string(),
            status: GoalCoverageStatus::Met,
            evidence: vec!["worker-says-done".to_string()],
        }];
        assert!(validate_evidence_references(&coverage).is_err());
    }

    #[test]
    fn semantic_json_fences_are_bounded_to_one_payload() {
        assert_eq!(
            strip_json_fence("```json\n{\"decision\":\"uncertain\"}\n```"),
            "{\"decision\":\"uncertain\"}"
        );
    }

    #[test]
    fn semantic_response_fixture_uses_supervisor_provider_facts() {
        let response = ProviderResponse {
            provider: "judge-provider".to_string(),
            model: "judge-model".to_string(),
            content: "{}".to_string(),
            usage: ProviderUsage {
                input_tokens: 1,
                output_tokens: 1,
            },
            spend: SpendEstimate {
                provider: "judge-provider".to_string(),
                model: "judge-model".to_string(),
                input_tokens: 1,
                output_tokens: 1,
                cost_usd: 0.01,
                subscription: false,
                wall_time_seconds: None,
            },
            trace: serde_json::Value::Null,
        };
        assert_eq!(response.provider, "judge-provider");
        assert_eq!(SemanticDecision::Uncertain, SemanticDecision::Uncertain);
    }

    #[test]
    fn malformed_semantic_response_is_unavailable_not_execution_error() {
        let result = classify_semantic_response(
            "job-1",
            "run-1",
            "sha256:input",
            ProviderResponse {
                provider: "smoke".to_string(),
                model: "scripted".to_string(),
                content: r#"{"action":"bash","command":"echo nope"}"#.to_string(),
                usage: ProviderUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                },
                spend: SpendEstimate {
                    provider: "smoke".to_string(),
                    model: "scripted".to_string(),
                    input_tokens: 1,
                    output_tokens: 1,
                    cost_usd: 0.0,
                    subscription: false,
                    wall_time_seconds: Some(0.01),
                },
                trace: serde_json::Value::Null,
            },
        );

        let SemanticJudgeResult::Unavailable(reason) = result else {
            panic!("malformed output must be unavailable")
        };
        assert!(reason.contains("malformed or invalid"));
    }

    #[test]
    fn semantic_response_maps_achieved_revise_and_uncertain() {
        fn response(decision: &str) -> ProviderResponse {
            let goal_coverage = if decision == "achieved" {
                r#"[{"claim":"approved goal","status":"met","evidence":["approved-goal"]}]"#
            } else {
                "[]"
            };
            ProviderResponse {
                provider: "judge".to_string(),
                model: "judge-model".to_string(),
                content: format!(
                    r#"{{"decision":"{decision}","summary":"bounded","goal_coverage":{goal_coverage},"blocking_missing":[]}}"#
                ),
                usage: ProviderUsage {
                    input_tokens: 2,
                    output_tokens: 1,
                },
                spend: SpendEstimate {
                    provider: "judge".to_string(),
                    model: "judge-model".to_string(),
                    input_tokens: 2,
                    output_tokens: 1,
                    cost_usd: 0.01,
                    subscription: false,
                    wall_time_seconds: Some(0.1),
                },
                trace: serde_json::Value::Null,
            }
        }

        assert!(matches!(
            classify_semantic_response("job", "run", "sha256:input", response("achieved")),
            SemanticJudgeResult::Achieved(_)
        ));
        assert!(matches!(
            classify_semantic_response("job", "run", "sha256:input", response("revise")),
            SemanticJudgeResult::Revise(_)
        ));
        assert!(matches!(
            classify_semantic_response("job", "run", "sha256:input", response("uncertain")),
            SemanticJudgeResult::NeedsReview(_)
        ));
    }

    #[test]
    fn achieved_semantic_response_requires_evidence_backed_complete_coverage() {
        fn response(goal_coverage: &str, missing: &str) -> ProviderResponse {
            ProviderResponse {
                provider: "judge".to_string(),
                model: "judge-model".to_string(),
                content: format!(
                    r#"{{"decision":"achieved","summary":"bounded","goal_coverage":{goal_coverage},"blocking_missing":{missing}}}"#
                ),
                usage: ProviderUsage {
                    input_tokens: 2,
                    output_tokens: 1,
                },
                spend: SpendEstimate {
                    provider: "judge".to_string(),
                    model: "judge-model".to_string(),
                    input_tokens: 2,
                    output_tokens: 1,
                    cost_usd: 0.01,
                    subscription: false,
                    wall_time_seconds: Some(0.1),
                },
                trace: serde_json::Value::Null,
            }
        }

        for (coverage, missing) in [
            ("[]", "[]"),
            (
                r#"[{"claim":"goal","status":"missing","evidence":["approved-goal"]}]"#,
                "[]",
            ),
            (
                r#"[{"claim":"goal","status":"unclear","evidence":["approved-goal"]}]"#,
                "[]",
            ),
            (r#"[{"claim":"goal","status":"met","evidence":[]}]"#, "[]"),
            (
                r#"[{"claim":"goal","status":"met","evidence":["worker-says-done"]}]"#,
                "[]",
            ),
            (
                r#"[{"claim":"goal","status":"met","evidence":["approved-goal"]}]"#,
                r#"["still missing"]"#,
            ),
        ] {
            assert!(matches!(
                classify_semantic_response(
                    "job",
                    "run",
                    "sha256:input",
                    response(coverage, missing)
                ),
                SemanticJudgeResult::Unavailable(_)
            ));
        }
    }

    #[test]
    fn revise_and_uncertain_keep_permitting_incomplete_semantic_evidence() {
        fn response(decision: &str) -> ProviderResponse {
            ProviderResponse {
                provider: "judge".to_string(),
                model: "judge-model".to_string(),
                content: format!(
                    r#"{{"decision":"{decision}","summary":"bounded","goal_coverage":[{{"claim":"goal","status":"unclear","evidence":[]}}],"blocking_missing":["goal"]}}"#
                ),
                usage: ProviderUsage {
                    input_tokens: 2,
                    output_tokens: 1,
                },
                spend: SpendEstimate {
                    provider: "judge".to_string(),
                    model: "judge-model".to_string(),
                    input_tokens: 2,
                    output_tokens: 1,
                    cost_usd: 0.01,
                    subscription: false,
                    wall_time_seconds: Some(0.1),
                },
                trace: serde_json::Value::Null,
            }
        }

        assert!(matches!(
            classify_semantic_response("job", "run", "sha256:input", response("revise")),
            SemanticJudgeResult::Revise(_)
        ));
        assert!(matches!(
            classify_semantic_response("job", "run", "sha256:input", response("uncertain")),
            SemanticJudgeResult::NeedsReview(_)
        ));
    }

    #[test]
    fn generic_cli_routes_are_treated_as_cli_not_direct_apis() {
        assert!(provider_kind_is_cli(&ProviderKind::Generic(
            "cli:codex-server".to_string()
        )));
        assert!(provider_kind_is_cli(&ProviderKind::Generic(
            "cli:pi".to_string()
        )));
        assert!(!provider_kind_is_cli(&ProviderKind::OpenAi));
    }

    #[tokio::test]
    async fn scripted_smoke_cannot_act_as_the_independent_semantic_judge() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = deadreckon_core::DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        std::fs::create_dir_all(&source).expect("source");
        let state = deadreckon_core::create_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: "exercise semantic refusal".to_string(),
                cwd: source,
                sandbox: "sandbox-exec".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "test".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(30.0),
                run_id: Some("smoke-judge-refusal".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        let marker = deadreckon_core::AcceptanceMarker {
            schema_version: 2,
            run_id: state.run_id.clone(),
            status: "pass".to_string(),
            produced_by: "dr-gate".to_string(),
            issuer: "dr-gate".to_string(),
            proof_kind: deadreckon_core::AcceptanceProofKind::NativeGate,
            checked_at: chrono::Utc::now(),
            working_dir: state.working_dir.clone(),
            contained: true,
            sandbox_backend: "sandbox-exec".to_string(),
            signature: "unused-before-fixture-refusal".to_string(),
            check_count: 1,
            checks: Vec::new(),
        };

        let run = super::run_semantic_judge(
            &state,
            &marker,
            &deadreckon_providers::ProviderRouter::smoke(),
            SandboxBackend::SandboxExec,
        )
        .await
        .expect("semantic result");

        let SemanticJudgeResult::Unavailable(reason) = run.result else {
            panic!("scripted smoke must never return a trusted semantic decision")
        };
        assert!(
            reason.contains("not an independent semantic assessor"),
            "{reason}"
        );
        assert_eq!(run.accounting.provider, "smoke");
        assert_eq!(run.accounting.model, "local-scripted-smoke");
    }
}
