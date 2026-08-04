//! Trusted drivers that place existing orchestration engines under one Job.
//!
//! The driver specification is embedded in the signed launch plan. The
//! mutable sidecar records which existing Plan or Campaign artifact belongs to
//! the parent Job; it is navigation evidence, never completion authority.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use deadreckon_protocol::{
    CompletionReceipt, JobEventKind, JobId, JobShape, SemanticDecision, SpendRecord, StopReason,
    TraceRecord,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::super::*;

const DRIVER_SIGNAL: &str = "watchkeeper_driver";
const DRIVER_STATE_FILE: &str = "driver.json";
const PLAN_PLANNER_ACCOUNTING_FILE: &str = "root-planner-accounting.json";
const PARENT_REPAIR_INTENT_FILE: &str = "parent-repair.json";
const PARENT_REPAIR_ARCHIVE_DIR: &str = "parent-repairs";
const DELEGATION_JOB_ENV: &str = "DEADRECKON_DELEGATION_JOB";
const DELEGATION_ID_ENV: &str = "DEADRECKON_DELEGATION_ID";
const MAX_DELEGATION_TOKEN_BYTES: u64 = 512;
const MAX_DELEGATION_RECORD_BYTES: u64 = 128 * 1024;
const CAMPAIGN_SUB_LAUNCH_PROTOCOL: &str = "delegated_campaign_sub_v1";
const CAMPAIGN_SUB_LAUNCH_DIR: &str = "campaign-sub-launches";
const CAMPAIGN_SUB_RELEASE_ACK_DIR: &str = "release-acks";
const MERGE_REPAIR_AUTHORITY_DIR: &str = "merge-repair-authorities";
const ORDERED_CANDIDATE_MANIFEST: &str = "ordered-candidate.json";
const ORDERED_CANDIDATE_DIR: &str = "ordered-candidate";
const ORDERED_CANDIDATE_BRANCH: &str = "deadreckon-ordered-candidate";
const DURABLE_CHAIN_ADAPTER_SIGNAL: &str = "watchkeeper_chain_adapter";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrderedCandidateManifest {
    schema_version: u32,
    job_id: String,
    source_tree_sha256: String,
    workspace: PathBuf,
    branch: String,
    initial_revision: String,
    prepared_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DriverKind {
    Review,
    FullPlan,
    Campaign,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct DriverSpec {
    pub(crate) kind: DriverKind,
    pub(crate) child_count: Option<u8>,
    pub(crate) apply: deadreckon_core::plan::ApplyWhen,
    pub(crate) planner_provider: Option<String>,
    pub(crate) child_provider: Option<String>,
    pub(crate) child_provider_overrides: Vec<String>,
    pub(crate) coder_provider: Option<String>,
    pub(crate) reviewer_provider: Option<String>,
    #[serde(default)]
    pub(crate) planner_model: Option<String>,
    #[serde(default)]
    pub(crate) child_model: Option<String>,
    #[serde(default)]
    pub(crate) child_model_overrides: Vec<String>,
    #[serde(default)]
    pub(crate) coder_model: Option<String>,
    #[serde(default)]
    pub(crate) reviewer_model: Option<String>,
    /// Pre-execution-team jobs stored one model for every role. New jobs keep
    /// this only as a backward-compatible fallback.
    #[serde(default)]
    pub(crate) model: Option<String>,
    pub(crate) source_init_git: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DriverState {
    pub(crate) schema_version: u32,
    pub(crate) job_id: JobId,
    pub(crate) kind: DriverKind,
    pub(crate) artifact_kind: String,
    pub(crate) artifact_id: String,
    pub(crate) recorded_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct DriverContext {
    job_id: String,
    acceptance_path: PathBuf,
    authority: commands::supervisor::GuardedDriverAuthority,
    root_artifact: bool,
    durable_chain: Option<deadreckon_core::chain::DurableChainAdapterManifest>,
}

static DRIVER_CONTEXT: OnceLock<DriverContext> = OnceLock::new();
static DELEGATED_PLAN_CHILD: OnceLock<deadreckon_core::RunOwnership> = OnceLock::new();
static DELEGATED_CHILD_RUN_ID: OnceLock<String> = OnceLock::new();
static DELEGATED_PLAN_AUTHORITY: OnceLock<commands::supervisor::GuardedDriverAuthority> =
    OnceLock::new();
static DELEGATED_REPAIR_AUTHORITY: OnceLock<commands::supervisor::GuardedDriverAuthority> =
    OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum DelegatedAction {
    PlanChild {
        plan_id: String,
        task_id: String,
        task_index: u32,
        task_attempt: u32,
        run_id: String,
    },
    PlanFork {
        plan_id: String,
    },
    PlanMerge {
        plan_id: String,
    },
    CampaignSub {
        campaign_id: String,
        sub_id: String,
        plan_id: String,
    },
    MergeRepair {
        root_artifact_id: String,
        repair_id: String,
        repair_round: u32,
        run_id: String,
        proof_dir: PathBuf,
        repair_request_sha256: String,
        repair_plan_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DelegatedInvocation {
    schema_version: u32,
    capability_id: String,
    job_id: String,
    authority: commands::supervisor::GuardedDriverAuthority,
    action: DelegatedAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    immutable_plan_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    immutable_campaign_sha256: Option<String>,
    argv_sha256: String,
    cwd: PathBuf,
    scope_root: PathBuf,
    token_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    campaign_sub_launch_id: Option<String>,
    issued_at: chrono::DateTime<Utc>,
}

pub(crate) struct PreparedDelegation {
    capability_id: String,
    job_id: String,
    token: String,
    campaign_sub: Option<PreparedCampaignSubLaunch>,
}

impl PreparedDelegation {
    pub(crate) fn capability_id(&self) -> &str {
        &self.capability_id
    }
}

#[derive(Debug, Clone)]
struct PreparedCampaignSubLaunch {
    campaign_id: String,
    sub_id: String,
    plan_id: String,
    launch_id: String,
}

/// Protected process authority for one durable Campaign sub-plan launch.
///
/// `released` records receipt of the private token. `linked` is the execution
/// boundary: the child cannot return to CLI dispatch until its one-time
/// delegation is consumed and this flag is durable. An unlinked launch is
/// therefore provably nonexecuted and may be replaced after its exact process
/// identity has exited. A linked launch may only be adopted or recovered from
/// its durable Plan artifacts; it is never launched a second time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CampaignSubLaunchAuthority {
    schema_version: u32,
    launch_protocol: String,
    parent_job_id: String,
    campaign_id: String,
    sub_id: String,
    plan_id: String,
    attempt: u32,
    lease_epoch: u64,
    outer_launch_id: String,
    launch_id: String,
    capability_id: String,
    release_token_sha256: String,
    #[serde(with = "strict_supervised_process_record")]
    process: deadreckon_core::SupervisedProcessRecord,
    released: bool,
    linked: bool,
    adopted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adopted_by_attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adopted_by_lease_epoch: Option<u64>,
    prepared_at: chrono::DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    released_at: Option<chrono::DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    linked_at: Option<chrono::DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    adopted_at: Option<chrono::DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CampaignSubReleaseAck {
    schema_version: u32,
    launch_protocol: String,
    parent_job_id: String,
    campaign_id: String,
    sub_id: String,
    plan_id: String,
    attempt: u32,
    lease_epoch: u64,
    launch_id: String,
    capability_id: String,
    release_token_sha256: String,
    pid: u32,
    process_group: Option<u32>,
    boot_id: String,
    process_start_identity: String,
    acknowledged_at: chrono::DateTime<Utc>,
}

mod strict_supervised_process_record {
    use deadreckon_core::{SupervisedProcess, SupervisedProcessPhase, SupervisedProcessRecord};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StrictRecord {
        schema_version: u32,
        pid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pgid: Option<u32>,
        launch_id: String,
        attempt: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        owner_launch_id: Option<String>,
        release_token_sha256: String,
        boot_id: String,
        process_start_identity: String,
        phase: SupervisedProcessPhase,
    }

    pub(super) fn serialize<S>(
        record: &SupervisedProcessRecord,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        StrictRecord {
            schema_version: record.schema_version,
            pid: record.process.pid,
            pgid: record.process.pgid,
            launch_id: record.launch_id.clone(),
            attempt: record.attempt,
            owner_launch_id: record.owner_launch_id.clone(),
            release_token_sha256: record.release_token_sha256.clone(),
            boot_id: record.boot_id.clone(),
            process_start_identity: record.process_start_identity.clone(),
            phase: record.phase,
        }
        .serialize(serializer)
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<SupervisedProcessRecord, D::Error>
    where
        D: Deserializer<'de>,
    {
        let record = StrictRecord::deserialize(deserializer)?;
        Ok(SupervisedProcessRecord {
            schema_version: record.schema_version,
            process: SupervisedProcess {
                pid: record.pid,
                pgid: record.pgid,
            },
            launch_id: record.launch_id,
            attempt: record.attempt,
            owner_launch_id: record.owner_launch_id,
            release_token_sha256: record.release_token_sha256,
            boot_id: record.boot_id,
            process_start_identity: record.process_start_identity,
            phase: record.phase,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CampaignSubRecoveryDisposition {
    RelaunchNonexecuted,
    AdoptLinked,
    RecoverLinkedArtifacts,
}

pub(crate) enum CampaignSubProcessPoll {
    Running,
    Exited { success: Option<bool> },
}

pub(crate) struct CampaignSubProcess {
    launch: CampaignSubLaunchAuthority,
    child: Option<Child>,
    prepared: Option<PreparedDelegation>,
}

pub(crate) enum CampaignSubLaunchRecovery {
    Relaunch,
    Adopted(Box<CampaignSubProcess>),
    RecoverLinkedArtifacts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OwnedPlanRecord {
    schema_version: u32,
    job_id: String,
    root_plan_id: String,
    plan_id: String,
    parent_plan_id: Option<String>,
    immutable_definition_sha256: String,
    recorded_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OwnedCampaignRecord {
    schema_version: u32,
    job_id: String,
    immutable_definition_sha256: String,
    recorded_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedPlanOwner {
    pub(crate) job: deadreckon_protocol::Job,
    pub(crate) root_plan_id: String,
    pub(crate) lineage: Vec<String>,
}

#[derive(Debug)]
pub(crate) enum ParentCompletion {
    Verified(Box<CompletionReceipt>),
    ReviseRequested {
        reason: String,
        round: u32,
        intent_path: PathBuf,
        intent_sha256: String,
        judgment_path: PathBuf,
        judgment_sha256: String,
    },
    RepairPending {
        reason: String,
        round: u32,
        stop_reason: StopReason,
    },
    RepairFailed {
        reason: String,
        stop_reason: StopReason,
    },
    Failed {
        reason: String,
        stop_reason: StopReason,
    },
    Cancelled {
        reason: String,
    },
    NeedsReview {
        reason: String,
        decision: Option<SemanticDecision>,
        stop_reason: StopReason,
    },
    BudgetExhausted {
        reason: String,
        stop_reason: StopReason,
    },
    GateFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParentRepairIntent {
    schema_version: u32,
    job_id: String,
    shape: JobShape,
    round: u32,
    merged_run_id: String,
    merged_tree_sha256: String,
    pre_repair_tree_sha256: String,
    revise_marker_sha256: String,
    revise_judgment_sha256: String,
    revise_input_sha256: String,
    requested_after_attempt: u32,
    requested_after_launch_id: String,
    requested_after_lease_epoch: u64,
    provider: Option<String>,
    model: Option<String>,
    feedback: String,
    previous_round_sha256: Option<String>,
    requested_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParentRepairAttemptManifest {
    schema_version: u32,
    job_id: String,
    shape: JobShape,
    round: u32,
    merged_run_id: String,
    merged_tree_sha256: String,
    pre_repair_tree_sha256: String,
    intent_sha256: String,
    attempt: u32,
    launch_id: String,
    lease_epoch: u64,
    attempt_baseline_tree_sha256: String,
    started_at: chrono::DateTime<Utc>,
}

/// Mirrors the Single Job cancellation token while parent verification runs
/// inside the supervisor after the conductor process has exited.
struct ParentCompletionCancellation {
    marker_path: PathBuf,
    token: CancellationToken,
    stop: Option<std::sync::mpsc::Sender<()>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

const PARENT_COMPLETION_CLEANUP_BUDGET: std::time::Duration = std::time::Duration::from_secs(30);

fn parent_completion_phase_deadline(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
) -> Result<ProviderPhaseDeadline> {
    let now = Utc::now();
    let work_remaining = commands::supervisor::remaining_job_work_duration(paths, job, now)?;
    let supervisor_cutoff =
        match std::env::var(commands::supervisor::TRUSTED_SUPERVISOR_WORK_CUTOFF_ENV) {
            Ok(value) => Some(value.parse::<chrono::DateTime<Utc>>().map_err(|error| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "trusted supervisor work cutoff is invalid: {error}"
                )))
            })?),
            Err(std::env::VarError::NotPresent) => None,
            Err(error) => {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "trusted supervisor work cutoff could not be read: {error}"
                ))));
            }
        };
    let remaining =
        parent_completion_remaining_at(work_remaining, job.policy.deadline, supervisor_cutoff, now);
    Ok(ProviderPhaseDeadline::new(
        tokio::time::Instant::now() + remaining,
        PARENT_COMPLETION_CLEANUP_BUDGET,
    ))
}

fn parent_completion_remaining_at(
    work_remaining: std::time::Duration,
    job_deadline: Option<chrono::DateTime<Utc>>,
    supervisor_cutoff: Option<chrono::DateTime<Utc>>,
    now: chrono::DateTime<Utc>,
) -> std::time::Duration {
    let calendar_cutoff = match (job_deadline, supervisor_cutoff) {
        (Some(job), Some(supervisor)) => Some(job.min(supervisor)),
        (Some(job), None) => Some(job),
        (None, Some(supervisor)) => Some(supervisor),
        (None, None) => None,
    };
    let calendar_remaining = calendar_cutoff.map(|cutoff| {
        cutoff
            .signed_duration_since(now)
            .to_std()
            .unwrap_or(std::time::Duration::ZERO)
    });
    calendar_remaining.map_or(work_remaining, |calendar| calendar.min(work_remaining))
}

enum ParentGateSettlement {
    Completed(deadreckon_core::Result<()>),
    Terminal(ParentCompletion),
}

fn settle_parent_gate_phase(
    parent: &mut deadreckon_core::PipelineState,
    context: &str,
    outcome: deadreckon_runtime::DeterministicGatePhaseOutcome,
) -> Result<ParentGateSettlement> {
    match outcome {
        deadreckon_runtime::DeterministicGatePhaseOutcome::Completed { result, cleanup } => {
            match cleanup {
                ProviderCleanup::Proven | ProviderCleanup::NotApplicable => {
                    Ok(ParentGateSettlement::Completed(result))
                }
                ProviderCleanup::RetainedAuthority { path, detail } => {
                    let terminal = parent_failed(
                        parent,
                        &format!(
                            "LOST_CONTAINMENT: {context} retained process authority at {}: {detail}",
                            path.display()
                        ),
                        StopReason::LostContainment,
                    )?;
                    Ok(ParentGateSettlement::Terminal(terminal))
                }
            }
        }
        deadreckon_runtime::DeterministicGatePhaseOutcome::WorkExpired { cleanup } => {
            let terminal = match cleanup {
                ProviderCleanup::Proven | ProviderCleanup::NotApplicable => {
                    parent_budget_exhausted(
                        parent,
                        StopReason::WallCap,
                        &format!("approved Job work cutoff reached during {context}"),
                    )?
                }
                ProviderCleanup::RetainedAuthority { path, detail } => parent_failed(
                    parent,
                    &format!(
                        "LOST_CONTAINMENT: {context} exceeded the Job work cutoff and retained process authority at {}: {detail}",
                        path.display()
                    ),
                    StopReason::LostContainment,
                )?,
            };
            Ok(ParentGateSettlement::Terminal(terminal))
        }
        deadreckon_runtime::DeterministicGatePhaseOutcome::Cancelled { cleanup } => {
            let terminal = match cleanup {
                ProviderCleanup::Proven | ProviderCleanup::NotApplicable => {
                    parent_cancelled(parent, &format!("operator cancelled during {context}"))?
                }
                ProviderCleanup::RetainedAuthority { path, detail } => parent_failed(
                    parent,
                    &format!(
                        "LOST_CONTAINMENT: {context} was cancelled and retained process authority at {}: {detail}",
                        path.display()
                    ),
                    StopReason::LostContainment,
                )?,
            };
            Ok(ParentGateSettlement::Terminal(terminal))
        }
    }
}

impl ParentCompletionCancellation {
    fn start(parent: &deadreckon_core::PipelineState) -> Result<Self> {
        let marker_path = deadreckon_core::cancel_marker_path(parent);
        let token = CancellationToken::new();
        if marker_path.is_file() {
            token.cancel();
        }
        let (stop, stopped) = std::sync::mpsc::channel();
        let watched_path = marker_path.clone();
        let watched_token = token.clone();
        let handle = std::thread::Builder::new()
            .name(format!(
                "dr-parent-cancel-{}",
                &parent.run_id[..parent.run_id.len().min(8)]
            ))
            .spawn(move || {
                loop {
                    match stopped.recv_timeout(std::time::Duration::from_millis(25)) {
                        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if watched_path.is_file() {
                                watched_token.cancel();
                                break;
                            }
                        }
                    }
                }
            })?;
        Ok(Self {
            marker_path,
            token,
            stop: Some(stop),
            handle: Some(handle),
        })
    }

    fn token(&self) -> &CancellationToken {
        &self.token
    }

    fn requested(&self) -> bool {
        self.token.is_cancelled() || self.marker_path.is_file()
    }
}

impl Drop for ParentCompletionCancellation {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PendingDriverRecovery {
    Unchanged,
    Recovered,
    BudgetExhausted {
        stop_reason: StopReason,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RootPlannerBudgetExhaustion {
    pub(crate) dimension: deadreckon_core::plan::BudgetDimension,
    pub(crate) stop_reason: StopReason,
    pub(crate) reason: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ParentExecutionUsage {
    spend_usd: f64,
    wall_seconds: f64,
}

pub(crate) const fn semantic_decision_stop_reason(
    decision: Option<SemanticDecision>,
) -> Option<StopReason> {
    match decision {
        Some(SemanticDecision::Revise) => Some(StopReason::SemanticRevise),
        Some(SemanticDecision::Uncertain) => Some(StopReason::SemanticUncertain),
        Some(SemanticDecision::Achieved) | None => None,
    }
}

pub(crate) fn embed_driver_spec(
    plan: &mut commands::course::LaunchPlan,
    driver: &DriverSpec,
) -> Result<()> {
    let mut driver = driver.clone();
    if driver.kind == DriverKind::Campaign {
        // The legacy Campaign adapter has no ordered candidate workspace.
        // Provider-drawn nested graphs now remain Plans, whose PerNode mode is
        // isolated below instead of being silently rewritten.
        driver.apply = deadreckon_core::plan::ApplyWhen::AtEnd;
    }
    let mut signals = plan.signals.as_object().cloned().unwrap_or_default();
    signals.insert(DRIVER_SIGNAL.to_string(), serde_json::to_value(&driver)?);
    plan.signals = serde_json::Value::Object(signals);
    Ok(())
}

pub(crate) fn driver_spec(plan: &commands::course::LaunchPlan) -> Result<DriverSpec> {
    let value = plan.signals.get(DRIVER_SIGNAL).ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "advanced job launch plan is missing its immutable driver specification".to_string(),
        ))
    })?;
    serde_json::from_value(value.clone()).map_err(CliError::from)
}

pub(crate) fn driver_state_path(paths: &DeadreckonPaths, job_id: &str) -> PathBuf {
    paths.job_dir(job_id).join(DRIVER_STATE_FILE)
}

fn ordered_candidate_manifest_path(paths: &DeadreckonPaths, job_id: &str) -> PathBuf {
    paths.job_dir(job_id).join(ORDERED_CANDIDATE_MANIFEST)
}

fn ordered_candidate_workspace(paths: &DeadreckonPaths, job_id: &str) -> PathBuf {
    paths
        .job_dir(job_id)
        .join(ORDERED_CANDIDATE_DIR)
        .join("workspace")
}

fn load_ordered_candidate_manifest(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<OrderedCandidateManifest> {
    let path = ordered_candidate_manifest_path(paths, job_id);
    serde_json::from_slice(&fs::read(&path)?).map_err(|source| {
        CliError::Core(DeadreckonError::Json {
            path: path.clone(),
            source,
        })
    })
}

fn validate_ordered_candidate_manifest(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    authority: &deadreckon_protocol::JobAuthority,
    manifest: &OrderedCandidateManifest,
) -> Result<PathBuf> {
    let expected = ordered_candidate_workspace(paths, job.job_id.as_ref());
    if manifest.schema_version != 1
        || manifest.job_id != job.job_id.as_ref()
        || manifest.source_tree_sha256 != authority.source_tree_sha256
        || manifest.workspace != expected
        || manifest.branch != ORDERED_CANDIDATE_BRANCH
        || manifest.initial_revision.trim().is_empty()
        || !manifest.workspace.is_dir()
        || !manifest.workspace.join(".git").exists()
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "ordered candidate workspace for Job {} changed or is incomplete",
            job.job_id
        ))));
    }
    let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))?;
    let durable_manifest = history.events().iter().any(|event| {
        event.kind == JobEventKind::WorkspacePrepared
            && event
                .detail
                .get("workspace_kind")
                .and_then(serde_json::Value::as_str)
                == Some("ordered_candidate")
            && event
                .detail
                .get("workspace")
                .and_then(serde_json::Value::as_str)
                == manifest.workspace.to_str()
            && event
                .detail
                .get("branch")
                .and_then(serde_json::Value::as_str)
                == Some(manifest.branch.as_str())
            && event
                .detail
                .get("initial_revision")
                .and_then(serde_json::Value::as_str)
                == Some(manifest.initial_revision.as_str())
            && event
                .detail
                .get("source_tree_sha256")
                .and_then(serde_json::Value::as_str)
                == Some(manifest.source_tree_sha256.as_str())
    });
    if !durable_manifest {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "ordered candidate workspace for Job {} is not backed by Job history",
            job.job_id
        ))));
    }
    let branch = git_stdout(&manifest.workspace, &["symbolic-ref", "--short", "HEAD"])?;
    if branch != manifest.branch {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "ordered candidate workspace for Job {} changed branch from {} to {branch}",
            job.job_id, manifest.branch
        ))));
    }
    git_status(
        &manifest.workspace,
        &[
            "merge-base",
            "--is-ancestor",
            &manifest.initial_revision,
            "HEAD",
        ],
    )
    .map_err(|_| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "ordered candidate workspace for Job {} no longer descends from its approved baseline",
            job.job_id
        )))
    })?;
    let dirty = git_stdout(&manifest.workspace, &["status", "--porcelain"])?;
    if !dirty.trim().is_empty() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "ordered candidate workspace for Job {} has an incomplete landing",
            job.job_id
        ))));
    }
    let head = git_stdout(&manifest.workspace, &["rev-parse", "HEAD^{commit}"])?;
    let application_events = deadreckon_core::plan::read_ordered_candidate_application_events(
        paths,
        job.job_id.as_ref(),
    )?;
    if application_events
        .iter()
        .any(|event| event.plan_id != job.job_id.as_ref())
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "ordered candidate workspace for Job {} has application evidence for another Plan",
            job.job_id
        ))));
    }
    let application_fold = deadreckon_core::plan::fold_ordered_candidate_application_events(
        &application_events,
        &manifest.initial_revision,
    )?;
    if head != application_fold.expected_head_revision {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "ordered candidate workspace for Job {} has clean unledgered HEAD {head}; expected {}",
            job.job_id, application_fold.expected_head_revision
        ))));
    }
    Ok(manifest.workspace.clone())
}

fn persist_ordered_candidate_manifest(
    paths: &DeadreckonPaths,
    token: &deadreckon_core::LeaseToken,
    manifest: &OrderedCandidateManifest,
) -> Result<()> {
    deadreckon_core::replace_fenced_job_json_and_append_event(
        paths,
        token,
        Utc::now(),
        &ordered_candidate_manifest_path(paths, token.job_id.as_ref()),
        deadreckon_core::FencedJobJsonEvent {
            kind: JobEventKind::WorkspacePrepared,
            causation_id: format!("ordered-candidate:{}:{}", token.job_id, token.epoch),
            detail: json!({
                "workspace_kind": "ordered_candidate",
                "workspace": manifest.workspace,
                "branch": manifest.branch,
                "initial_revision": manifest.initial_revision,
                "source_tree_sha256": manifest.source_tree_sha256,
            }),
        },
        manifest,
    )?;
    Ok(())
}

fn prepare_ordered_candidate(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    authority: &deadreckon_protocol::JobAuthority,
    token: &deadreckon_core::LeaseToken,
) -> Result<PathBuf> {
    if token.job_id != job.job_id || authority.job_id != job.job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "ordered candidate authority does not match its Job".to_string(),
        )));
    }
    let manifest_path = ordered_candidate_manifest_path(paths, job.job_id.as_ref());
    if manifest_path.is_file() {
        let manifest = load_ordered_candidate_manifest(paths, job.job_id.as_ref())?;
        super::plan::reconcile_prepared_ordered_candidate_application(
            paths,
            token,
            &manifest.initial_revision,
            &manifest.workspace,
        )?;
        return validate_ordered_candidate_manifest(paths, job, authority, &manifest);
    }
    let workspace = ordered_candidate_workspace(paths, job.job_id.as_ref());
    let candidate_root = workspace.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "ordered candidate path has no protected parent".to_string(),
        ))
    })?;
    fs::create_dir_all(candidate_root)?;
    let staging = candidate_root.join(format!("workspace-preparing-{}", token.epoch));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }

    let initial_revision = if workspace.is_dir() {
        let candidate_tree =
            deadreckon_core::flight::build_deliverable_file_index(&workspace)?.tree_hash();
        let branch = git_stdout(&workspace, &["symbolic-ref", "--short", "HEAD"])?;
        let dirty = git_stdout(&workspace, &["status", "--porcelain"])?;
        if candidate_tree != authority.source_tree_sha256
            || branch != ORDERED_CANDIDATE_BRANCH
            || !dirty.trim().is_empty()
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "uncommitted ordered candidate workspace for Job {} cannot be recovered safely",
                job.job_id
            ))));
        }
        git_stdout(&workspace, &["rev-parse", "HEAD"])?
    } else {
        let current_source_tree =
            deadreckon_core::flight::build_deliverable_file_index(&job.source_cwd)?.tree_hash();
        if current_source_tree != authority.source_tree_sha256 {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "ordered candidate refused because Job {} source changed after approval",
                job.job_id
            ))));
        }
        deadreckon_core::copy_deliverable_tree(&job.source_cwd, &staging)?;
        let staged_tree =
            deadreckon_core::flight::build_deliverable_file_index(&staging)?.tree_hash();
        if staged_tree != authority.source_tree_sha256 {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "ordered candidate copy does not match the approved source tree".to_string(),
            )));
        }
        git_status(&staging, &["init", "--quiet"])?;
        git_status(
            &staging,
            &["checkout", "--quiet", "-b", ORDERED_CANDIDATE_BRANCH],
        )?;
        git_status(&staging, &["add", "--all"])?;
        git_status(
            &staging,
            &[
                "-c",
                "user.name=DeadReckon",
                "-c",
                "user.email=deadreckon@localhost",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "DeadReckon approved ordered candidate",
            ],
        )?;
        let revision = git_stdout(&staging, &["rev-parse", "HEAD"])?;
        fs::rename(&staging, &workspace)?;
        revision
    };

    let manifest = OrderedCandidateManifest {
        schema_version: 1,
        job_id: job.job_id.as_ref().to_string(),
        source_tree_sha256: authority.source_tree_sha256.clone(),
        workspace: workspace.clone(),
        branch: ORDERED_CANDIDATE_BRANCH.to_string(),
        initial_revision,
        prepared_at: Utc::now(),
    };
    persist_ordered_candidate_manifest(paths, token, &manifest)?;
    validate_ordered_candidate_manifest(paths, job, authority, &manifest)
}

fn expected_plan_parent_cwd(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    authority: &deadreckon_protocol::JobAuthority,
    driver: &DriverSpec,
) -> Result<PathBuf> {
    if driver.apply != deadreckon_core::plan::ApplyWhen::PerNode {
        return Ok(job.source_cwd.clone());
    }
    let manifest = load_ordered_candidate_manifest(paths, job.job_id.as_ref())?;
    validate_ordered_candidate_manifest(paths, job, authority, &manifest)
}

fn parent_repair_intent_path(paths: &DeadreckonPaths, job_id: &str) -> PathBuf {
    paths.job_dir(job_id).join(PARENT_REPAIR_INTENT_FILE)
}

fn parent_repair_round_dir(state: &deadreckon_core::PipelineState, round: u32) -> PathBuf {
    state
        .run_root
        .join("proofs")
        .join(PARENT_REPAIR_ARCHIVE_DIR)
        .join(format!("round-{round}"))
}

fn load_parent_repair_intent(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<Option<ParentRepairIntent>> {
    let path = parent_repair_intent_path(paths, job_id);
    let raw = match fs::read(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let intent: ParentRepairIntent = serde_json::from_slice(&raw).map_err(|source| {
        CliError::Core(DeadreckonError::Json {
            path: path.clone(),
            source,
        })
    })?;
    if intent.schema_version != 1
        || intent.job_id != job_id
        || intent.round == 0
        || !matches!(intent.shape, JobShape::Graph | JobShape::LegacyCampaign)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "parent repair intent for Job {job_id} is malformed or mismatched"
        ))));
    }
    Ok(Some(intent))
}

pub(crate) fn parent_repair_is_pending(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
) -> bool {
    let Ok(Some(intent)) = load_parent_repair_intent(paths, job.job_id.as_ref()) else {
        return false;
    };
    if intent.shape != job.shape {
        return false;
    }
    let Ok(parent) = deadreckon_core::load_run(paths, job.job_id.as_ref()) else {
        return false;
    };
    let candidate_path =
        deadreckon_core::parent_repair_candidate_path_for_run_root(&parent.run_root);
    if candidate_path.is_file() {
        return false;
    }
    matches!(
        parent.status,
        deadreckon_core::RunStatus::Pending
            | deadreckon_core::RunStatus::Planned
            | deadreckon_core::RunStatus::Executing
    ) || (parent.status == deadreckon_core::RunStatus::Failed
        && parent.provider_failure == Some(deadreckon_core::ProviderFailureDisposition::Retryable))
}

pub(crate) fn parent_repair_candidate_is_ready(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
) -> bool {
    let Ok(Some(intent)) = load_parent_repair_intent(paths, job.job_id.as_ref()) else {
        return false;
    };
    let Ok(parent) = deadreckon_core::load_run(paths, job.job_id.as_ref()) else {
        return false;
    };
    let path = deadreckon_core::parent_repair_candidate_path_for_run_root(&parent.run_root);
    let Ok(raw) = fs::read(&path) else {
        return false;
    };
    let Ok(candidate) = serde_json::from_slice::<deadreckon_runtime::ParentRepairCandidate>(&raw)
    else {
        return false;
    };
    candidate.job_id == job.job_id.as_ref()
        && candidate.run_id == job.job_id.as_ref()
        && candidate.round == intent.round
        && candidate.intent_sha256
            == deadreckon_core::flight::sha256_file(&parent_repair_intent_path(
                paths,
                job.job_id.as_ref(),
            ))
            .unwrap_or_default()
}

pub(crate) fn parent_repair_needs_projection(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
) -> bool {
    let Ok(Some(intent)) = load_parent_repair_intent(paths, job.job_id.as_ref()) else {
        return false;
    };
    let Ok(history) = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
    else {
        return false;
    };
    let Ok(intent_sha256) = deadreckon_core::flight::sha256_file(&parent_repair_intent_path(
        paths,
        job.job_id.as_ref(),
    )) else {
        return false;
    };
    !history.events().iter().any(|event| {
        event.kind == deadreckon_protocol::JobEventKind::SemanticJudgeRevise
            && event
                .detail
                .get("round")
                .and_then(serde_json::Value::as_u64)
                == Some(u64::from(intent.round))
            && event
                .detail
                .get("intent_sha256")
                .and_then(serde_json::Value::as_str)
                == Some(intent_sha256.as_str())
            && event
                .detail
                .get("judgment_sha256")
                .and_then(serde_json::Value::as_str)
                == Some(intent.revise_judgment_sha256.as_str())
    })
}

#[cfg(test)]
pub(crate) fn record_plan_planner_accounting(
    paths: &DeadreckonPaths,
    plan_id: &str,
    accounting: Option<&commands::plan::PlannerAccounting>,
) -> Result<()> {
    let record = root_planner_accounting(accounting);
    record_plan_planner_accounting_snapshot(paths, plan_id, &record)
}

pub(crate) fn root_planner_accounting(
    accounting: Option<&commands::plan::PlannerAccounting>,
) -> deadreckon_core::plan::RootPlannerAccounting {
    deadreckon_core::plan::RootPlannerAccounting {
        schema_version: 1,
        planner_invoked: accounting.is_some(),
        provider: accounting.map(|value| value.spend.provider.clone()),
        model: accounting.map(|value| value.spend.model.clone()),
        input_tokens: accounting.map_or(0, |value| value.spend.input_tokens),
        output_tokens: accounting.map_or(0, |value| value.spend.output_tokens),
        cost_usd: accounting.map_or(0.0, |value| value.spend.cost_usd),
        subscription: accounting.is_some_and(|value| value.spend.subscription),
        wall_seconds: accounting.map_or(0.0, |value| value.wall_seconds),
        recorded_at: Utc::now(),
    }
}

pub(crate) fn record_plan_planner_accounting_snapshot(
    paths: &DeadreckonPaths,
    plan_id: &str,
    record: &deadreckon_core::plan::RootPlannerAccounting,
) -> Result<()> {
    super::job::write_json_synced(
        &paths.plan_dir(plan_id).join(PLAN_PLANNER_ACCOUNTING_FILE),
        record,
    )
}

pub(crate) fn load_driver_state(paths: &DeadreckonPaths, job_id: &str) -> Result<DriverState> {
    let path = driver_state_path(paths, job_id);
    serde_json::from_slice(&fs::read(&path)?).map_err(|source| {
        CliError::Core(DeadreckonError::Json {
            path: path.clone(),
            source,
        })
    })
}

pub(crate) fn current_parent_job_id() -> Option<&'static str> {
    DRIVER_CONTEXT.get().map(|context| context.job_id.as_str())
}

fn durable_chain_adapter_from_launch(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<Option<deadreckon_core::chain::DurableChainAdapterManifest>> {
    let job = deadreckon_core::load_job(paths, job_id)?;
    let launch_path = paths.job_launch_plan(job_id);
    if deadreckon_core::flight::sha256_file(&launch_path)? != job.launch_plan_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable chain launch plan changed after approval".to_string(),
        )));
    }
    let authority_path = paths.job_authority(job_id);
    if deadreckon_core::flight::sha256_file(&authority_path)? != job.authority_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable chain authority changed after approval".to_string(),
        )));
    }
    let launch = commands::course::load_launch_plan(&launch_path)?;
    let Some(value) = launch.signals.get(DURABLE_CHAIN_ADAPTER_SIGNAL) else {
        return Ok(None);
    };
    let adapter: deadreckon_core::chain::DurableChainAdapterManifest =
        serde_json::from_value(value.clone())?;
    adapter.verify()?;
    if adapter.branch_policy != deadreckon_core::plan::BranchPolicy::Stack
        || adapter.apply_mode != deadreckon_core::plan::ApplyMode::Auto
        || adapter.apply_strategy != deadreckon_core::plan::ApplyStrategy::Squash
        || adapter.on_fail != deadreckon_core::plan::OnFail::Stop
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable chain adapter contains a policy the Graph executor cannot preserve"
                .to_string(),
        )));
    }
    let authority: deadreckon_protocol::JobAuthority =
        serde_json::from_slice(&fs::read(&authority_path)?)?;
    if authority.job_id.as_ref() != job_id
        || authority.source_revision.as_deref() != Some(adapter.source_base_sha.as_str())
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable chain source base is not bound to the approved Job authority".to_string(),
        )));
    }
    Ok(Some(adapter))
}

fn current_durable_chain_adapter(
    plan: &deadreckon_core::plan::Plan,
) -> Result<Option<&'static deadreckon_core::chain::DurableChainAdapterManifest>> {
    let Some(context) = DRIVER_CONTEXT.get() else {
        return Ok(None);
    };
    let Some(adapter) = context.durable_chain.as_ref() else {
        return Ok(None);
    };
    if plan.owner_job_id.as_deref() != Some(context.job_id.as_str())
        || plan.plan_id != context.job_id
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable chain hooks may execute only for the owning root Job Plan".to_string(),
        )));
    }
    if plan.apply != deadreckon_core::plan::ApplyWhen::PerNode
        || plan.branch_policy != adapter.branch_policy
        || plan.apply_strategy != adapter.apply_strategy
        || plan.on_fail != adapter.on_fail
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable chain Plan changed an approved execution policy".to_string(),
        )));
    }
    Ok(Some(adapter))
}

fn durable_chain_hook_for(
    adapter: &deadreckon_core::chain::DurableChainAdapterManifest,
    name: deadreckon_core::chain::ChainHookName,
) -> Option<&deadreckon_core::chain::FrozenChainHook> {
    adapter.hooks.iter().find(|hook| hook.name == name)
}

fn validate_durable_chain_hook_history(
    events: &[deadreckon_core::chain::DurableChainHookEvent],
    job_id: &str,
    adapter: &deadreckon_core::chain::DurableChainAdapterManifest,
) -> Result<()> {
    use deadreckon_core::chain::DurableChainHookEventKind;

    let mut seen = std::collections::BTreeMap::<String, (bool, bool)>::new();
    for event in events {
        event.verify()?;
        let hook = durable_chain_hook_for(adapter, event.hook.name).ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "durable chain hook history names unapproved hook {}",
                event.hook.name.as_str()
            )))
        })?;
        if event.schema_version != 1
            || event.job_id.as_ref() != job_id
            || event.source_chain_id != adapter.source_chain_id
            || event.attempt == 0
            || (event.hook.name == deadreckon_core::chain::ChainHookName::OnChainEnd)
                != event.step_index.is_none()
            || &event.hook != hook
            || event.invocation_id
                != deadreckon_core::chain::durable_chain_hook_invocation_id(
                    job_id,
                    &adapter.source_chain_id,
                    hook,
                    event.step_index,
                    event.attempt,
                    &event.payload_sha256,
                )
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "durable chain hook history is not bound to its approved invocation".to_string(),
            )));
        }
        let state = seen.entry(event.invocation_id.clone()).or_default();
        match event.kind {
            DurableChainHookEventKind::Started if !state.0 && !state.1 => state.0 = true,
            DurableChainHookEventKind::Completed if state.0 && !state.1 => {
                if event.exit_code.is_none() {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(
                        "completed durable chain hook evidence has no exit code".to_string(),
                    )));
                }
                state.1 = true;
            }
            _ => {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "durable chain hook invocation {} has duplicate or out-of-order evidence",
                    event.invocation_id
                ))));
            }
        }
    }
    Ok(())
}

fn validate_durable_chain_completion_evidence(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    authority: &deadreckon_protocol::JobAuthority,
    plan: &deadreckon_core::plan::Plan,
) -> Result<()> {
    let Some(adapter) = durable_chain_adapter_from_launch(paths, job.job_id.as_ref())? else {
        return Ok(());
    };
    if plan.plan_id != job.job_id.as_ref()
        || plan.owner_job_id.as_deref() != Some(job.job_id.as_ref())
        || plan.apply != deadreckon_core::plan::ApplyWhen::PerNode
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable chain completion lost its approved ordered Plan identity".to_string(),
        )));
    }
    let manifest = load_ordered_candidate_manifest(paths, job.job_id.as_ref())?;
    validate_ordered_candidate_manifest(paths, job, authority, &manifest)?;

    use deadreckon_core::chain::DurableChainHookEventKind;
    let events = deadreckon_core::chain::read_durable_chain_hook_events(paths, &job.job_id)?;
    validate_durable_chain_hook_history(&events, job.job_id.as_ref(), &adapter)?;
    let mut completion = std::collections::BTreeMap::<&str, (bool, bool)>::new();
    for event in &events {
        let state = completion.entry(event.invocation_id.as_str()).or_default();
        match event.kind {
            DurableChainHookEventKind::Started => state.0 = true,
            DurableChainHookEventKind::Completed => state.1 = true,
        }
    }
    if completion.values().any(|state| !state.0 || !state.1) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable chain completion has a hook invocation with unknown external outcome"
                .to_string(),
        )));
    }
    Ok(())
}

fn write_durable_hook_payload(
    stdin: &mut dyn std::io::Write,
    canonical_payload: &[u8],
) -> Result<()> {
    match stdin.write_all(canonical_payload) {
        Ok(()) => match stdin.write_all(b"\n") {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
            Err(error) => Err(error.into()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[allow(clippy::too_many_arguments)]
fn invoke_frozen_durable_chain_hook(
    paths: &DeadreckonPaths,
    token: &deadreckon_core::LeaseToken,
    adapter: &deadreckon_core::chain::DurableChainAdapterManifest,
    hook: &deadreckon_core::chain::FrozenChainHook,
    step_index: Option<u32>,
    attempt: u32,
    cwd: &Path,
    payload: &serde_json::Value,
) -> Result<i32> {
    use deadreckon_core::chain::{DurableChainHookEvent, DurableChainHookEventKind};

    adapter.verify()?;
    if attempt == 0 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable chain hook invocation has invalid fenced identity".to_string(),
        )));
    }
    let requested = DurableChainHookEvent::started(
        token.job_id.clone(),
        adapter.source_chain_id.clone(),
        hook.clone(),
        step_index,
        attempt,
        Utc::now(),
        payload,
    )?;
    let invocation_id = requested.invocation_id.clone();
    let events = deadreckon_core::chain::read_durable_chain_hook_events(paths, &token.job_id)?;
    validate_durable_chain_hook_history(&events, token.job_id.as_ref(), adapter)?;
    if let Some(completed) =
        deadreckon_core::chain::reusable_durable_chain_hook_completion(&events, &requested)?
    {
        return completed.exit_code.ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "completed durable chain hook evidence has no exit code".to_string(),
            ))
        });
    }
    let mut started = None;
    for event in events
        .into_iter()
        .filter(|event| event.invocation_id == invocation_id)
    {
        match event.kind {
            DurableChainHookEventKind::Started => started = Some(event),
            DurableChainHookEventKind::Completed => {}
        }
    }
    if started.is_some() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "chain hook {} started before the worker was interrupted; its external outcome is unknown",
                hook.name.as_str()
            ),
            "inspect the Job hook evidence and explicitly replace or terminate the Job",
        )));
    }

    let approved_hook_path = deadreckon_core::chain::materialize_fenced_approved_chain_hook(
        paths,
        token,
        hook,
        Utc::now(),
    )?;
    if fs::read(&approved_hook_path)? != hook.approved_bytes {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "protected chain hook {} changed before sandbox materialization",
            hook.name.as_str()
        ))));
    }
    // The Job directory is read-denied inside the hook sandbox, so execute a
    // second exact copy from a controller-owned temporary directory. This
    // keeps the launch-bound bytes while withholding every other Job proof and
    // the real DEADRECKON_HOME from workspace/user hook code.
    let hook_sandbox = tempfile::TempDir::new()?;
    let hook_program_dir = hook_sandbox.path().join("program");
    let hook_home = hook_sandbox.path().join("home");
    fs::create_dir_all(&hook_program_dir)?;
    fs::create_dir_all(&hook_home)?;
    let sandboxed_hook_path = hook_program_dir.join("hook");
    fs::write(&sandboxed_hook_path, &hook.approved_bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&sandboxed_hook_path, fs::Permissions::from_mode(0o500))?;
    }
    let mut hook_env = BTreeMap::new();
    hook_env.insert(
        "DEADRECKON_JOB_ID".to_string(),
        token.job_id.as_ref().to_string(),
    );
    hook_env.insert(
        "DEADRECKON_CHAIN_ID".to_string(),
        adapter.source_chain_id.clone(),
    );
    hook_env.insert(
        "DEADRECKON_STEP_INDEX".to_string(),
        step_index.map_or_else(|| "-".to_string(), |index| index.to_string()),
    );
    hook_env.insert(
        "DEADRECKON_HOME".to_string(),
        hook_home.display().to_string(),
    );
    hook_env.insert("HOME".to_string(), hook_home.display().to_string());
    let sandbox_backend =
        current_driver_sandbox_backend(paths)?.unwrap_or(deadreckon_sandbox::SandboxBackend::Auto);
    let sandbox_spec = deadreckon_sandbox::SandboxSpec {
        backend: sandbox_backend,
        docker: None,
        cwd: cwd.to_path_buf(),
        program: sandboxed_hook_path.into_os_string(),
        args: Vec::new(),
        stdin: None,
        env: hook_env,
        allow_network: true,
        pid_file: None,
        cancellation_token: None,
        profile_dir: None,
        read_allowlist: vec![cwd.to_path_buf(), hook_program_dir],
        write_allowlist: vec![cwd.to_path_buf(), hook_home],
        read_denylist: vec![paths.jobs_dir(), paths.home().join("gate-keys")],
        write_denylist: vec![paths.jobs_dir(), paths.home().join("gate-keys")],
        network_allowlist: vec!["*".to_string()],
        workspace_access: deadreckon_sandbox::WorkspaceAccess::ReadWrite,
        cleanup_process_group: false,
        guarded_launch: None,
    };
    let sandbox_command = deadreckon_sandbox::build_command(&sandbox_spec)?;
    if sandbox_command.backend == deadreckon_sandbox::SandboxBackend::None && !cfg!(test) {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "chain hook {} cannot execute without a sandbox that denies controller authority",
                hook.name.as_str()
            ),
            "select sandbox-exec, bwrap, or docker for this durable Job",
        )));
    }
    let started = requested;
    deadreckon_core::chain::append_fenced_durable_chain_hook_event(paths, token, &started)?;

    let mut command = Command::new(&sandbox_command.program);
    command
        .args(&sandbox_command.args)
        .current_dir(&sandbox_command.cwd)
        .envs(&sandbox_command.env)
        .env_remove(deadreckon_core::GATE_KEY_ENV)
        .env_remove(deadreckon_core::GATE_CONTAINED_ENV)
        .env_remove(deadreckon_core::GATE_SANDBOX_BACKEND_ENV)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let completed = DurableChainHookEvent::completed(
                &started,
                Utc::now(),
                -1,
                "",
                format!("hook could not start: {error}"),
            )?;
            deadreckon_core::chain::append_fenced_durable_chain_hook_event(
                paths, token, &completed,
            )?;
            return Ok(-1);
        }
    };
    if let Some(stdin) = child.stdin.as_mut() {
        write_durable_hook_payload(stdin, &started.payload_bytes)?;
    }
    let output = child.wait_with_output()?;
    let exit_code = output.status.code().unwrap_or(-2);
    let completed = DurableChainHookEvent::completed(
        &started,
        Utc::now(),
        exit_code,
        truncate_text(&String::from_utf8_lossy(&output.stdout), 4096),
        truncate_text(&String::from_utf8_lossy(&output.stderr), 4096),
    )?;
    deadreckon_core::chain::append_fenced_durable_chain_hook_event(paths, token, &completed)?;
    Ok(exit_code)
}

fn bind_durable_chain_hook_payload(
    adapter: &deadreckon_core::chain::DurableChainAdapterManifest,
    job_id: &str,
    mut payload: serde_json::Value,
) -> Result<serde_json::Value> {
    let object = payload.as_object_mut().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "durable chain hook payload must be a JSON object".to_string(),
        ))
    })?;
    object.insert(
        "chain_id".to_string(),
        serde_json::Value::String(adapter.source_chain_id.clone()),
    );
    object.insert(
        "job_id".to_string(),
        serde_json::Value::String(job_id.to_string()),
    );
    Ok(payload)
}

pub(crate) fn invoke_current_durable_chain_hook(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
    name: deadreckon_core::chain::ChainHookName,
    step_index: Option<u32>,
    attempt: u32,
    payload: serde_json::Value,
) -> Result<Option<i32>> {
    let Some(adapter) = current_durable_chain_adapter(plan)? else {
        return Ok(None);
    };
    let Some(hook) = durable_chain_hook_for(adapter, name) else {
        return Ok(None);
    };
    let token = current_driver_lease_token(paths, &plan.plan_id)?;
    let payload = bind_durable_chain_hook_payload(adapter, &plan.plan_id, payload)?;
    let cwd = plan.parent_cwd.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "durable chain Plan has no isolated execution workspace".to_string(),
        ))
    })?;
    invoke_frozen_durable_chain_hook(
        paths, &token, adapter, hook, step_index, attempt, cwd, &payload,
    )
    .map(Some)
}

pub(crate) fn current_durable_chain_has_hook(
    plan: &deadreckon_core::plan::Plan,
    name: deadreckon_core::chain::ChainHookName,
) -> Result<bool> {
    Ok(current_durable_chain_adapter(plan)?
        .and_then(|adapter| durable_chain_hook_for(adapter, name))
        .is_some())
}

pub(crate) fn completed_current_durable_chain_hook_invocation_ids(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
    step_index: u32,
    attempt: u32,
) -> Result<Vec<String>> {
    use deadreckon_core::chain::{ChainHookName, DurableChainHookEventKind};

    let Some(adapter) = current_durable_chain_adapter(plan)? else {
        return Ok(Vec::new());
    };
    let events = deadreckon_core::chain::read_durable_chain_hook_events(
        paths,
        &JobId(plan.plan_id.clone()),
    )?;
    validate_durable_chain_hook_history(&events, &plan.plan_id, adapter)?;
    let mut completed = Vec::new();
    for name in [
        ChainHookName::PreStep,
        ChainHookName::PostStep,
        ChainHookName::OnPromote,
    ] {
        if durable_chain_hook_for(adapter, name).is_none() {
            continue;
        }
        let matches = events
            .iter()
            .filter(|event| {
                event.hook.name == name
                    && event.step_index == Some(step_index)
                    && event.attempt == attempt
                    && event.kind == DurableChainHookEventKind::Completed
            })
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "durable chain hook {} has no unique completed invocation for step {step_index} attempt {attempt}",
                name.as_str()
            ))));
        }
        completed.push(matches[0].invocation_id.clone());
    }
    Ok(completed)
}

pub(crate) fn durable_chain_end_hook_pending(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
) -> Result<bool> {
    use deadreckon_core::chain::ChainHookName;

    let Some(adapter) = current_durable_chain_adapter(plan)? else {
        return Ok(false);
    };
    let Some(hook) = durable_chain_hook_for(adapter, ChainHookName::OnChainEnd) else {
        return Ok(false);
    };
    let events = deadreckon_core::chain::read_durable_chain_hook_events(
        paths,
        &JobId(plan.plan_id.clone()),
    )?;
    validate_durable_chain_hook_history(&events, &plan.plan_id, adapter)?;
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == deadreckon_core::plan::PlanTaskStatus::Completed)
        .count();
    let skipped = plan
        .tasks
        .iter()
        .filter(|task| task.status == deadreckon_core::plan::PlanTaskStatus::Skipped)
        .count();
    let payload = bind_durable_chain_hook_payload(
        adapter,
        &plan.plan_id,
        json!({
            "status": "completed",
            "steps_completed": completed,
            "steps_skipped": skipped,
            "total_spend_usd": plan.attempts_spend_usd(),
        }),
    )?;
    let requested = deadreckon_core::chain::DurableChainHookEvent::started(
        JobId(plan.plan_id.clone()),
        adapter.source_chain_id.clone(),
        hook.clone(),
        None,
        1,
        Utc::now(),
        &payload,
    )?;
    Ok(
        deadreckon_core::chain::reusable_durable_chain_hook_completion(&events, &requested)?
            .is_none(),
    )
}

pub(crate) fn current_driver_sandbox_backend(
    paths: &DeadreckonPaths,
) -> Result<Option<deadreckon_sandbox::SandboxBackend>> {
    let Some(context) = DRIVER_CONTEXT.get() else {
        return Ok(None);
    };
    let authority_path = paths.job_authority(&context.job_id);
    let authority: deadreckon_protocol::JobAuthority =
        serde_json::from_slice(&fs::read(&authority_path)?).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: authority_path,
                source,
            })
        })?;
    if authority.job_id.as_ref() != context.job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "trusted driver authority changed Job identity from {} to {}",
            context.job_id, authority.job_id
        ))));
    }
    Ok(Some(authority.sandbox_requested.parse()?))
}

pub(crate) fn current_acceptance_path() -> Option<&'static Path> {
    DRIVER_CONTEXT
        .get()
        .map(|context| context.acceptance_path.as_path())
}

pub(crate) fn current_driver_owns_root_artifact() -> bool {
    DRIVER_CONTEXT
        .get()
        .is_some_and(|context| context.root_artifact)
}

pub(crate) fn current_plan_mutation_token(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
) -> Result<Option<deadreckon_core::LeaseToken>> {
    let Some(owner_job_id) = plan.owner_job_id.as_deref() else {
        return Ok(None);
    };
    if cfg!(test) && DRIVER_CONTEXT.get().is_none() {
        return Ok(None);
    }
    current_driver_lease_token(paths, owner_job_id).map(Some)
}

pub(crate) fn current_ordered_candidate_initial_revision(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
) -> Result<Option<String>> {
    let Some(owner_job_id) = plan.owner_job_id.as_deref() else {
        return Ok(None);
    };
    let manifest = load_ordered_candidate_manifest(paths, owner_job_id)?;
    if manifest.job_id != owner_job_id
        || plan.parent_cwd.as_deref() != Some(manifest.workspace.as_path())
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Plan {} is not bound to its Job ordered candidate workspace",
            plan.plan_id
        ))));
    }
    Ok(Some(manifest.initial_revision))
}

fn current_driver_lease_token(
    paths: &DeadreckonPaths,
    owner_job_id: &str,
) -> Result<deadreckon_core::LeaseToken> {
    let context = DRIVER_CONTEXT.get().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "Job {owner_job_id} cannot mutate outside its fenced driver"
        )))
    })?;
    if context.job_id != owner_job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "requested Job {owner_job_id} does not match active Job {}",
            context.job_id
        ))));
    }
    guarded_authority_lease_token(paths, &context.authority)
}

pub(crate) fn delegated_plan_child_authorized() -> bool {
    DELEGATED_PLAN_CHILD.get().is_some()
}

pub(crate) fn delegated_plan_child_ownership() -> Option<deadreckon_core::RunOwnership> {
    DELEGATED_PLAN_CHILD.get().cloned()
}

pub(crate) fn delegated_plan_child_run_id() -> Option<String> {
    DELEGATED_CHILD_RUN_ID.get().cloned()
}

pub(crate) fn plan_task_run_id(
    job_id: &str,
    plan_id: &str,
    task_id: &str,
    task_attempt: u32,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"deadreckon-plan-task-run-v1\0");
    for value in [job_id, plan_id, task_id, &task_attempt.to_string()] {
        hasher.update((value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    }
    format!("{:x}", hasher.finalize())[..32].to_string()
}

pub(crate) fn link_delegated_owned_run(state: &deadreckon_core::PipelineState) -> Result<()> {
    if let Some(deadreckon_core::RunOwnership {
        job_id,
        artifact:
            deadreckon_core::RunOwnershipArtifact::PlanTask {
                plan_id,
                task_id,
                task_index,
                task_attempt,
            },
        ..
    }) = state.ownership.as_ref()
    {
        let authority = DELEGATED_PLAN_AUTHORITY.get().ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "Plan child Run has no consumed fenced driver authority".to_string(),
            ))
        })?;
        if authority.job_id != *job_id {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "Plan child Run cannot link across its immutable Job authority".to_string(),
            )));
        }
        let paths = DeadreckonPaths::discover();
        let token = guarded_authority_lease_token(&paths, authority)?;
        return link_plan_task_run_fenced(
            &paths,
            &token,
            state,
            plan_id,
            task_id,
            *task_index,
            *task_attempt,
        );
    }

    let Some(deadreckon_core::RunOwnership {
        job_id,
        artifact:
            deadreckon_core::RunOwnershipArtifact::MergeRepair {
                root_artifact_id,
                repair_id,
                repair_round,
                run_id,
                proof_dir,
                repair_request_sha256,
                repair_plan_sha256,
            },
        ..
    }) = state.ownership.as_ref()
    else {
        return Ok(());
    };
    let path = proof_dir.join("repair-run.json");
    let delegated_authority = DELEGATED_REPAIR_AUTHORITY.get();
    let paths = DeadreckonPaths::discover();
    let mut record: serde_json::Value = if let Some(authority) = delegated_authority {
        let protected_path = merge_repair_authority_path(&paths, &authority.job_id, repair_id);
        let raw =
            read_bounded_regular_control_file(&protected_path, "protected merge-repair authority")?
                .ok_or_else(|| {
                    CliError::Core(DeadreckonError::InvalidInput(
                        "merge repair Run has no protected launch authority".to_string(),
                    ))
                })?;
        serde_json::from_slice(&raw).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: protected_path,
                source,
            })
        })?
    } else if cfg!(test) {
        serde_json::from_slice(&fs::read(&path)?).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: path.clone(),
                source,
            })
        })?
    } else {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "merge repair Run has no consumed fenced driver authority".to_string(),
        )));
    };
    let object = record.as_object_mut().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "merge repair launch authority is not an object".to_string(),
        ))
    })?;
    let existing_run_id = object.get("run_id").and_then(serde_json::Value::as_str);
    if job_id != root_artifact_id
        || object
            .get("root_artifact_id")
            .and_then(serde_json::Value::as_str)
            != Some(root_artifact_id.as_str())
        || object.get("repair_id").and_then(serde_json::Value::as_str) != Some(repair_id.as_str())
        || object
            .get("repair_round")
            .and_then(serde_json::Value::as_u64)
            != Some(u64::from(*repair_round))
        || object
            .get("repair_request_sha256")
            .and_then(serde_json::Value::as_str)
            != Some(repair_request_sha256.as_str())
        || object
            .get("repair_plan_sha256")
            .and_then(serde_json::Value::as_str)
            != Some(repair_plan_sha256.as_str())
        || existing_run_id.is_some_and(|existing| existing != state.run_id)
        || state.run_id != *run_id
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "merge repair Run cannot link across its immutable launch authority".to_string(),
        )));
    }
    object.insert(
        "run_id".to_string(),
        serde_json::Value::String(state.run_id.clone()),
    );
    object.insert(
        "status".to_string(),
        serde_json::Value::String("child_linked".to_string()),
    );
    object.insert("linked_at".to_string(), serde_json::to_value(Utc::now())?);
    if let Some(authority) = delegated_authority {
        replace_merge_repair_authority_fenced(&paths, authority, repair_id, "run_linked", &record)?;
    }
    commands::job::replace_json_synced(&path, &record)
}

fn link_plan_task_run_fenced(
    paths: &DeadreckonPaths,
    token: &deadreckon_core::LeaseToken,
    state: &deadreckon_core::PipelineState,
    plan_id: &str,
    task_id: &str,
    task_index: u32,
    task_attempt: u32,
) -> Result<()> {
    let Some(deadreckon_core::RunOwnership {
        job_id,
        artifact:
            deadreckon_core::RunOwnershipArtifact::PlanTask {
                plan_id: owned_plan_id,
                task_id: owned_task_id,
                task_index: owned_task_index,
                task_attempt: owned_task_attempt,
            },
        ..
    }) = state.ownership.as_ref()
    else {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "only an owned Plan task Run can be linked to Job history".to_string(),
        )));
    };
    if token.job_id.as_ref() != job_id
        || owned_plan_id != plan_id
        || owned_task_id != task_id
        || *owned_task_index != task_index
        || *owned_task_attempt != task_attempt
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Plan child Run cannot link across its immutable task authority".to_string(),
        )));
    }

    let history = deadreckon_core::read_job_history(&paths.job_events(job_id))?;
    let prepared = history.events().iter().any(|event| {
        plan_task_event_matches(
            event,
            JobEventKind::ChildLaunchPrepared,
            plan_id,
            task_id,
            task_index,
            task_attempt,
        ) && event
            .detail
            .get("run_id")
            .and_then(serde_json::Value::as_str)
            == Some(state.run_id.as_str())
    });
    if !prepared {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Plan task {plan_id}/{task_id} attempt {task_attempt} has no fenced launch record for Run {}",
            state.run_id
        ))));
    }
    let existing = history.events().iter().find(|event| {
        plan_task_event_matches(
            event,
            JobEventKind::ChildLinked,
            plan_id,
            task_id,
            task_index,
            task_attempt,
        )
    });
    if let Some(existing) = existing {
        if existing
            .detail
            .get("run_id")
            .and_then(serde_json::Value::as_str)
            == Some(state.run_id.as_str())
        {
            return Ok(());
        }
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Plan task {plan_id}/{task_id} attempt {task_attempt} is already linked to another Run"
        ))));
    }

    commands::supervisor::append_control_event(
        paths,
        token,
        JobEventKind::ChildLinked,
        format!("plan-task:{plan_id}:{task_id}:{task_attempt}:run-linked"),
        json!({
            "relationship": "plan_task",
            "run_id": state.run_id,
            "plan_id": plan_id,
            "task_id": task_id,
            "task_index": task_index,
            "task_attempt": task_attempt,
        }),
    )?;
    Ok(())
}

fn plan_task_event_matches(
    event: &deadreckon_protocol::JobEvent,
    kind: JobEventKind,
    plan_id: &str,
    task_id: &str,
    task_index: u32,
    task_attempt: u32,
) -> bool {
    event.kind == kind
        && event
            .detail
            .get("relationship")
            .and_then(serde_json::Value::as_str)
            == Some("plan_task")
        && event
            .detail
            .get("plan_id")
            .and_then(serde_json::Value::as_str)
            == Some(plan_id)
        && event
            .detail
            .get("task_id")
            .and_then(serde_json::Value::as_str)
            == Some(task_id)
        && event
            .detail
            .get("task_index")
            .and_then(serde_json::Value::as_u64)
            == Some(u64::from(task_index))
        && event
            .detail
            .get("task_attempt")
            .and_then(serde_json::Value::as_u64)
            == Some(u64::from(task_attempt))
}

fn prepare_plan_task_run_fenced(
    paths: &DeadreckonPaths,
    token: &deadreckon_core::LeaseToken,
    plan_id: &str,
    task_id: &str,
    task_index: u32,
    task_attempt: u32,
    run_id: &str,
) -> Result<()> {
    let expected = plan_task_run_id(token.job_id.as_ref(), plan_id, task_id, task_attempt);
    if run_id != expected {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Plan task {plan_id}/{task_id} attempt {task_attempt} received a non-deterministic Run identity"
        ))));
    }
    let history = deadreckon_core::read_job_history(&paths.job_events(token.job_id.as_ref()))?;
    if let Some(existing) = history.events().iter().find(|event| {
        plan_task_event_matches(
            event,
            JobEventKind::ChildLaunchPrepared,
            plan_id,
            task_id,
            task_index,
            task_attempt,
        )
    }) {
        if existing
            .detail
            .get("run_id")
            .and_then(serde_json::Value::as_str)
            == Some(run_id)
        {
            return Ok(());
        }
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Plan task {plan_id}/{task_id} attempt {task_attempt} changed its prepared Run identity"
        ))));
    }
    commands::supervisor::append_control_event(
        paths,
        token,
        JobEventKind::ChildLaunchPrepared,
        format!("plan-task:{plan_id}:{task_id}:{task_attempt}:launch-prepared"),
        json!({
            "relationship": "plan_task",
            "run_id": run_id,
            "plan_id": plan_id,
            "task_id": task_id,
            "task_index": task_index,
            "task_attempt": task_attempt,
        }),
    )?;
    Ok(())
}

fn reconcile_plan_task_run_links(
    paths: &DeadreckonPaths,
    token: &deadreckon_core::LeaseToken,
    plan: &deadreckon_core::plan::Plan,
) -> Result<()> {
    let history = deadreckon_core::read_job_history(&paths.job_events(token.job_id.as_ref()))?;
    let prepared = history
        .events()
        .iter()
        .filter(|event| {
            event.kind == JobEventKind::ChildLaunchPrepared
                && event
                    .detail
                    .get("relationship")
                    .and_then(serde_json::Value::as_str)
                    == Some("plan_task")
        })
        .filter_map(|event| {
            Some((
                event.detail.get("plan_id")?.as_str()?.to_string(),
                event.detail.get("task_id")?.as_str()?.to_string(),
                u32::try_from(event.detail.get("task_index")?.as_u64()?).ok()?,
                u32::try_from(event.detail.get("task_attempt")?.as_u64()?).ok()?,
                event.detail.get("run_id")?.as_str()?.to_string(),
            ))
        })
        .collect::<Vec<_>>();
    for (plan_id, task_id, task_index, task_attempt, run_id) in prepared {
        if plan_id != plan.plan_id && plan.owner_job_id.as_deref() != Some(token.job_id.as_ref()) {
            continue;
        }
        let Ok(state) = deadreckon_core::load_run(paths, &run_id) else {
            continue;
        };
        link_plan_task_run_fenced(
            paths,
            token,
            &state,
            &plan_id,
            &task_id,
            task_index,
            task_attempt,
        )?;
        let mut linked_plan = deadreckon_core::load_plan(paths, &plan_id)?;
        if linked_plan.owner_job_id.as_deref() != Some(token.job_id.as_ref()) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "prepared Plan task Run {run_id} changed its owning Job"
            ))));
        }
        let task = linked_plan
            .tasks
            .get_mut(task_index as usize)
            .filter(|task| task.task_id == task_id)
            .ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "prepared Plan task {plan_id}/{task_id} disappeared before recovery"
                )))
            })?;
        if task
            .child_run_id
            .as_deref()
            .is_some_and(|existing| existing != run_id)
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "prepared Plan task {plan_id}/{task_id} already points to another Run"
            ))));
        }
        if task.child_run_id.is_none() {
            task.child_run_id = Some(run_id.clone());
            deadreckon_core::save_owned_plan_fenced(paths, token, &linked_plan)?;
            let already_discovered = deadreckon_core::read_plan_events(paths, &plan_id)?
                .iter()
                .any(|event| {
                    matches!(
                        &event.event,
                        deadreckon_core::PlanEventKind::TaskRunDiscovered {
                            task_id: discovered_task,
                            task_index: discovered_index,
                            run_id: Some(discovered_run),
                            ..
                        } if discovered_task == &task_id
                            && *discovered_index == task_index as usize
                            && discovered_run == &run_id
                    )
                });
            if !already_discovered {
                deadreckon_core::append_owned_plan_event_fenced(
                    paths,
                    token,
                    &plan_id,
                    deadreckon_core::PlanEventKind::TaskRunDiscovered {
                        task_id: task_id.clone(),
                        task_index: task_index as usize,
                        run_id: Some(run_id.clone()),
                        pid: None,
                    },
                )?;
            }
        }
    }
    Ok(())
}

fn install_driver_context(
    paths: &DeadreckonPaths,
    authority: commands::supervisor::GuardedDriverAuthority,
    root_artifact: bool,
) -> Result<()> {
    let job_id = authority.job_id.clone();
    let durable_chain = durable_chain_adapter_from_launch(paths, &job_id)?;
    DRIVER_CONTEXT
        .set(DriverContext {
            acceptance_path: commands::job::job_acceptance_path(paths, &job_id),
            job_id,
            authority,
            root_artifact,
            durable_chain,
        })
        .map_err(|_| {
            CliError::Core(DeadreckonError::InvalidInput(
                "this process is already driving another durable job".to_string(),
            ))
        })
}

pub(crate) fn resolve_plan_owner(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
) -> Result<Option<ResolvedPlanOwner>> {
    let owner_job_id = plan.owner_job_id.clone().or_else(|| {
        paths
            .job_json(&plan.plan_id)
            .is_file()
            .then(|| plan.plan_id.clone())
    });
    let Some(owner_job_id) = owner_job_id else {
        return Ok(None);
    };
    let job = deadreckon_core::load_job(paths, &owner_job_id)?;
    if job.job_id.as_ref() != owner_job_id
        || !matches!(job.shape, JobShape::Graph | JobShape::LegacyCampaign)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Plan {} names incompatible durable Job {} as its owner",
            plan.plan_id, owner_job_id
        ))));
    }

    let mut lineage = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = plan.clone();
    loop {
        if !seen.insert(current.plan_id.clone()) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan ownership lineage contains a cycle at {}",
                current.plan_id
            ))));
        }
        lineage.push(current.plan_id.clone());
        if lineage.len() as u32 > deadreckon_core::plan::MAX_SUBPLAN_DEPTH {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan ownership lineage exceeds nesting cap {}",
                deadreckon_core::plan::MAX_SUBPLAN_DEPTH
            ))));
        }
        let compatible_owner = current.owner_job_id.as_deref() == Some(owner_job_id.as_str())
            || (current.owner_job_id.is_none() && current.plan_id == owner_job_id);
        if !compatible_owner {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan {} does not retain durable Job owner {}",
                current.plan_id, owner_job_id
            ))));
        }
        let Some(parent_id) = current.parent_plan_id.as_deref() else {
            break;
        };
        let parent = deadreckon_core::load_plan(paths, parent_id)?;
        let reverse_links = parent
            .tasks
            .iter()
            .filter(|task| task.subplan.as_deref() == Some(current.plan_id.as_str()))
            .count();
        if reverse_links != 1 {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan {} has no unique parent-task link from {}",
                current.plan_id, parent.plan_id
            ))));
        }
        current = parent;
    }
    let root_plan_id = current.plan_id;
    if job.shape == JobShape::Graph && root_plan_id != owner_job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Graph Job {} does not own root Plan {}",
            owner_job_id, root_plan_id
        ))));
    }
    Ok(Some(ResolvedPlanOwner {
        job,
        root_plan_id,
        lineage,
    }))
}

fn immutable_plan_sha256(plan: &deadreckon_core::plan::Plan) -> Result<String> {
    let tasks = plan
        .tasks
        .iter()
        .map(|task| {
            json!({
                "index": task.index,
                "task_id": task.task_id,
                "subject": task.subject,
                "goal": task.goal,
                "active_form": task.active_form,
                "provider": task.provider,
                "role": task.role,
                "depends_on": task.depends_on,
                "subplan": task.subplan,
                "worker_spec": task.worker_spec,
            })
        })
        .collect::<Vec<_>>();
    let definition = json!({
        "schema_version": plan.schema_version,
        "plan_id": plan.plan_id,
        "owner_job_id": plan.owner_job_id,
        "parent_plan_id": plan.parent_plan_id,
        "root_goal": plan.root_goal,
        "mode": plan.mode,
        "n": plan.n,
        "providers": plan.providers,
        "capability_preview": plan.capability_preview,
        "parent_scope": plan.parent_scope,
        "parent_cwd": plan.parent_cwd,
        "acceptance_path": plan.acceptance_path,
        "apply": plan.apply,
        "branch_policy": plan.branch_policy,
        "apply_strategy": plan.apply_strategy,
        "apply_allowlist": plan.apply_allowlist,
        "on_fail": plan.on_fail,
        "max_attempts": plan.max_attempts,
        "circuit_breaker_threshold": plan.circuit_breaker_threshold,
        "tasks": tasks,
    });
    Ok(deadreckon_core::flight::sha256_text(
        &serde_json::to_string(&definition)?,
    ))
}

fn immutable_campaign_sha256(campaign: &deadreckon_core::campaign::Campaign) -> Result<String> {
    let sub_goals = campaign
        .sub_goals
        .iter()
        .map(|sub| {
            json!({
                "sub_id": sub.sub_id,
                "goal": sub.goal,
                "task_key": sub.task_key,
            })
        })
        .collect::<Vec<_>>();
    let definition = json!({
        "schema_version": campaign.schema_version,
        "campaign_id": campaign.campaign_id,
        "root_goal": campaign.root_goal,
        "n": campaign.n,
        "depth": campaign.depth,
        "providers": campaign.providers,
        "tree_budget_usd": campaign.tree_budget_usd,
        "tree_wall_seconds": campaign.tree_wall_seconds,
        "sub_goals": sub_goals,
    });
    Ok(deadreckon_core::flight::sha256_text(
        &serde_json::to_string(&definition)?,
    ))
}

fn owned_campaign_record_path(paths: &DeadreckonPaths, job_id: &str) -> PathBuf {
    paths.job_dir(job_id).join("owned-campaign.json")
}

pub(crate) fn write_owned_campaign_record(
    paths: &DeadreckonPaths,
    job_id: &str,
    campaign: &deadreckon_core::campaign::Campaign,
) -> Result<()> {
    if campaign.campaign_id != job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Campaign {} does not retain durable Job identity {job_id}",
            campaign.campaign_id
        ))));
    }
    let record = OwnedCampaignRecord {
        schema_version: 1,
        job_id: job_id.to_string(),
        immutable_definition_sha256: immutable_campaign_sha256(campaign)?,
        recorded_at: Utc::now(),
    };
    let path = owned_campaign_record_path(paths, job_id);
    if path.is_file() {
        let existing: OwnedCampaignRecord = serde_json::from_slice(&fs::read(&path)?)?;
        if existing.schema_version != 1
            || existing.job_id != record.job_id
            || existing.immutable_definition_sha256 != record.immutable_definition_sha256
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "Campaign no longer matches its protected Job-owned definition".to_string(),
            )));
        }
        return Ok(());
    }
    commands::job::write_json_synced(&path, &record)
}

pub(crate) fn record_owned_campaign(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
) -> Result<()> {
    let Some(context) = DRIVER_CONTEXT.get() else {
        return Ok(());
    };
    if context.job_id != campaign.campaign_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Campaign {} does not retain current durable Job identity {}",
            campaign.campaign_id, context.job_id
        ))));
    }
    let job = deadreckon_core::load_job(paths, &context.job_id)?;
    if job.shape != JobShape::LegacyCampaign {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Campaign {} belongs to incompatible durable Job shape {:?}",
            campaign.campaign_id, job.shape
        ))));
    }
    if !commands::supervisor::guarded_driver_authority_is_live(paths, &context.authority)? {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign definition cannot be frozen without current fenced authority".to_string(),
        )));
    }
    write_owned_campaign_record(paths, &context.job_id, campaign)
}

pub(crate) fn validate_owned_campaign(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    job_id: &str,
) -> Result<()> {
    if campaign.campaign_id != job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign changed its durable Job identity".to_string(),
        )));
    }
    let path = owned_campaign_record_path(paths, job_id);
    let record: OwnedCampaignRecord =
        serde_json::from_slice(&fs::read(&path).map_err(|source| {
            CliError::Core(DeadreckonError::Io {
                path: path.clone(),
                source,
            })
        })?)?;
    if record.schema_version != 1
        || record.job_id != job_id
        || record.immutable_definition_sha256 != immutable_campaign_sha256(campaign)?
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign no longer matches its protected Job-owned definition".to_string(),
        )));
    }
    Ok(())
}

fn owned_plan_record_path(paths: &DeadreckonPaths, job_id: &str, plan_id: &str) -> PathBuf {
    paths
        .job_dir(job_id)
        .join("owned-plans")
        .join(format!("{plan_id}.json"))
}

fn record_owned_plan(
    paths: &DeadreckonPaths,
    root_plan_id: &str,
    plan: &deadreckon_core::plan::Plan,
) -> Result<()> {
    let Some(job_id) = plan.owner_job_id.as_deref() else {
        return Ok(());
    };
    let record = OwnedPlanRecord {
        schema_version: 1,
        job_id: job_id.to_string(),
        root_plan_id: root_plan_id.to_string(),
        plan_id: plan.plan_id.clone(),
        parent_plan_id: plan.parent_plan_id.clone(),
        immutable_definition_sha256: immutable_plan_sha256(plan)?,
        recorded_at: Utc::now(),
    };
    let path = owned_plan_record_path(paths, job_id, &plan.plan_id);
    if path.is_file() {
        let existing: OwnedPlanRecord = serde_json::from_slice(&fs::read(&path)?)?;
        if existing.job_id != record.job_id
            || existing.root_plan_id != record.root_plan_id
            || existing.plan_id != record.plan_id
            || existing.parent_plan_id != record.parent_plan_id
            || existing.immutable_definition_sha256 != record.immutable_definition_sha256
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "protected ownership definition changed for Plan {}",
                plan.plan_id
            ))));
        }
        return Ok(());
    }
    commands::job::write_json_synced(&path, &record)
}

pub(crate) fn record_owned_plan_tree(
    paths: &DeadreckonPaths,
    root: &deadreckon_core::plan::Plan,
) -> Result<()> {
    if root.owner_job_id.is_none() {
        return Ok(());
    }
    let root_plan_id = root.plan_id.clone();
    let mut pending = vec![root.clone()];
    let mut seen = BTreeSet::new();
    while let Some(plan) = pending.pop() {
        if !seen.insert(plan.plan_id.clone()) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "owned Plan tree contains a cycle at {}",
                plan.plan_id
            ))));
        }
        if plan.owner_job_id != root.owner_job_id {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "subplan {} does not retain root Job owner",
                plan.plan_id
            ))));
        }
        let owner = resolve_plan_owner(paths, &plan)?.ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan {} lost its durable owner while freezing its definition",
                plan.plan_id
            )))
        })?;
        if owner.root_plan_id != root_plan_id {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan {} resolves to root {}, not approved root {root_plan_id}",
                plan.plan_id, owner.root_plan_id
            ))));
        }
        record_owned_plan(paths, &root_plan_id, &plan)?;
        for task in &plan.tasks {
            let Some(subplan_id) = task.subplan.as_deref() else {
                continue;
            };
            let child = deadreckon_core::load_plan(paths, subplan_id)?;
            if child.parent_plan_id.as_deref() != Some(plan.plan_id.as_str()) {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "subplan {subplan_id} does not point back to parent {}",
                    plan.plan_id
                ))));
            }
            pending.push(child);
        }
    }
    Ok(())
}

fn validate_owned_plan_if_present(
    paths: &DeadreckonPaths,
    plan_id: &str,
    job_id: &str,
    root_plan_id: &str,
) -> Result<()> {
    if !paths.plan_json(plan_id).is_file() {
        return Ok(());
    }
    let plan = deadreckon_core::load_plan(paths, plan_id)?;
    let path = owned_plan_record_path(paths, job_id, plan_id);
    let record: OwnedPlanRecord = serde_json::from_slice(&fs::read(&path).map_err(|source| {
        CliError::Core(DeadreckonError::Io {
            path: path.clone(),
            source,
        })
    })?)?;
    if record.schema_version != 1
        || record.job_id != job_id
        || record.root_plan_id != root_plan_id
        || record.plan_id != plan_id
        || record.parent_plan_id != plan.parent_plan_id
        || record.immutable_definition_sha256 != immutable_plan_sha256(&plan)?
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Plan {plan_id} no longer matches its protected Job-owned definition"
        ))));
    }
    Ok(())
}

fn validate_owned_plan_lineage(paths: &DeadreckonPaths, owner: &ResolvedPlanOwner) -> Result<()> {
    for plan_id in &owner.lineage {
        validate_owned_plan_if_present(
            paths,
            plan_id,
            owner.job.job_id.as_ref(),
            &owner.root_plan_id,
        )?;
    }
    Ok(())
}

fn delegation_pending_path(paths: &DeadreckonPaths, job_id: &str, capability_id: &str) -> PathBuf {
    paths
        .job_dir(job_id)
        .join("delegations")
        .join("pending")
        .join(format!("{capability_id}.json"))
}

fn delegation_consumed_path(paths: &DeadreckonPaths, job_id: &str, capability_id: &str) -> PathBuf {
    paths
        .job_dir(job_id)
        .join("delegations")
        .join("consumed")
        .join(format!("{capability_id}.json"))
}

fn merge_repair_authority_path(paths: &DeadreckonPaths, job_id: &str, repair_id: &str) -> PathBuf {
    let key = repair_id.strip_prefix("sha256:").unwrap_or(repair_id);
    paths
        .job_dir(job_id)
        .join(MERGE_REPAIR_AUTHORITY_DIR)
        .join(format!("{key}.json"))
}

fn replace_merge_repair_authority_fenced(
    paths: &DeadreckonPaths,
    authority: &commands::supervisor::GuardedDriverAuthority,
    repair_id: &str,
    transition: &str,
    value: &serde_json::Value,
) -> Result<()> {
    let token = guarded_authority_lease_token(paths, authority)?;
    let path = merge_repair_authority_path(paths, &authority.job_id, repair_id);
    deadreckon_core::replace_fenced_job_json_and_append_event(
        paths,
        &token,
        Utc::now(),
        &path,
        deadreckon_core::FencedJobJsonEvent {
            kind: JobEventKind::RepairChildAuthorityChanged,
            causation_id: format!("merge-repair-authority:{repair_id}:{transition}"),
            detail: json!({
                "repair_id": repair_id,
                "transition": transition,
                "run_id": value.get("run_id"),
                "authority": value,
            }),
        },
        value,
    )?;
    Ok(())
}

pub(crate) fn mark_merge_repair_trusted_fenced(
    paths: &DeadreckonPaths,
    repair_id: &str,
    proof_path: &Path,
) -> Result<()> {
    let context = DRIVER_CONTEXT.get().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "only a current Job driver can trust merge-repair evidence".to_string(),
        ))
    })?;
    if !commands::supervisor::guarded_driver_authority_is_live(paths, &context.authority)? {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "merge-repair evidence cannot become trusted without the current fenced lease"
                .to_string(),
        )));
    }
    let mut proof: serde_json::Value =
        serde_json::from_slice(&fs::read(proof_path)?).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: proof_path.to_path_buf(),
                source,
            })
        })?;
    let authority_path = merge_repair_authority_path(paths, &context.job_id, repair_id);
    let authority: serde_json::Value = serde_json::from_slice(&fs::read(&authority_path)?)
        .map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: authority_path.clone(),
                source,
            })
        })?;
    for field in [
        "schema_version",
        "plan_id",
        "root_artifact_id",
        "repair_id",
        "repair_round",
        "repair_request_sha256",
        "repair_plan_sha256",
        "capability_id",
        "run_id",
        "sandbox_requested",
        "process",
        "adoption_window_seconds",
        "adoption_deadline_at",
    ] {
        if authority.get(field) != proof.get(field) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "trusted merge-repair proof changed its fenced {field} authority"
            ))));
        }
    }
    if proof.get("trusted").and_then(serde_json::Value::as_bool) != Some(true)
        || proof
            .get("acceptance_marker_sha256")
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty)
        || proof
            .get("result_tree_sha256")
            .and_then(serde_json::Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "merge-repair proof is not independently validated as trusted".to_string(),
        )));
    }
    if authority.get("status").and_then(serde_json::Value::as_str) == Some("trusted") {
        for field in ["trusted", "acceptance_marker_sha256", "result_tree_sha256"] {
            if authority.get(field) != proof.get(field) {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "trusted merge-repair authority changed its {field} evidence"
                ))));
            }
        }
        return Ok(());
    }
    let object = proof.as_object_mut().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "trusted merge-repair proof is not an object".to_string(),
        ))
    })?;
    object.insert(
        "status".to_string(),
        serde_json::Value::String("trusted".to_string()),
    );
    object.insert("trusted_at".to_string(), serde_json::to_value(Utc::now())?);
    replace_merge_repair_authority_fenced(paths, &context.authority, repair_id, "trusted", &proof)
}

pub(crate) fn restore_merge_repair_projection_if_needed(
    paths: &DeadreckonPaths,
    repair_id: &str,
    projection_path: &Path,
) -> Result<()> {
    let context = DRIVER_CONTEXT.get().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "only a current Job driver can restore merge repair authority".to_string(),
        ))
    })?;
    if !commands::supervisor::guarded_driver_authority_is_live(paths, &context.authority)? {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "merge repair authority cannot be restored without the current fenced lease"
                .to_string(),
        )));
    }
    let authority_path = merge_repair_authority_path(paths, &context.job_id, repair_id);
    let raw = match read_bounded_regular_control_file(
        &authority_path,
        "protected merge-repair authority",
    )? {
        Some(raw) => raw,
        None if projection_path.exists() => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "merge repair projection exists without protected Job authority".to_string(),
            )));
        }
        None => return Ok(()),
    };
    let authority: serde_json::Value = serde_json::from_slice(&raw).map_err(|source| {
        CliError::Core(DeadreckonError::Json {
            path: authority_path.clone(),
            source,
        })
    })?;
    let history = deadreckon_core::read_job_history(&paths.job_events(&context.job_id))?;
    let committed = latest_merge_repair_authority_matches(history.events(), repair_id, &authority);
    if !committed {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "merge repair projection has no committed fenced authority event".to_string(),
        )));
    }
    if let Some(projection_raw) =
        read_bounded_regular_control_file(projection_path, "merge-repair projection")?
    {
        let projection: serde_json::Value =
            serde_json::from_slice(&projection_raw).map_err(|source| {
                CliError::Core(DeadreckonError::Json {
                    path: projection_path.to_path_buf(),
                    source,
                })
            })?;
        if projection != authority {
            commands::job::replace_json_synced(projection_path, &authority)?;
        }
        return Ok(());
    }
    commands::job::write_json_synced(projection_path, &authority)
}

fn latest_merge_repair_authority_matches(
    events: &[deadreckon_protocol::JobEvent],
    repair_id: &str,
    authority: &serde_json::Value,
) -> bool {
    events
        .iter()
        .rev()
        .find(|event| {
            event.kind == JobEventKind::RepairChildAuthorityChanged
                && event
                    .detail
                    .get("repair_id")
                    .and_then(serde_json::Value::as_str)
                    == Some(repair_id)
        })
        .is_some_and(|event| event.detail.get("authority") == Some(authority))
}

fn campaign_sub_launch_key(sub_id: &str, plan_id: &str) -> String {
    deadreckon_core::flight::sha256_text(&format!("{sub_id}\0{plan_id}"))
        .trim_start_matches("sha256:")
        .to_string()
}

fn campaign_sub_launch_path(
    paths: &DeadreckonPaths,
    job_id: &str,
    sub_id: &str,
    plan_id: &str,
) -> PathBuf {
    paths
        .job_dir(job_id)
        .join(CAMPAIGN_SUB_LAUNCH_DIR)
        .join(format!("{}.json", campaign_sub_launch_key(sub_id, plan_id)))
}

fn campaign_sub_release_ack_path(
    paths: &DeadreckonPaths,
    job_id: &str,
    launch_id: &str,
) -> PathBuf {
    paths
        .job_dir(job_id)
        .join(CAMPAIGN_SUB_LAUNCH_DIR)
        .join(CAMPAIGN_SUB_RELEASE_ACK_DIR)
        .join(format!("{launch_id}.json"))
}

fn read_bounded_regular_control_file(path: &Path, label: &str) -> Result<Option<Vec<u8>>> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(CliError::Core(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            }));
        }
    };
    if before.file_type().is_symlink() || !before.file_type().is_file() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "{label} must be a regular non-symlink file"
        ))));
    }
    if before.len() > MAX_DELEGATION_RECORD_BYTES {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "{label} exceeded its bounded size"
        ))));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(|source| {
        CliError::Core(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        })
    })?;
    let opened = file.metadata().map_err(|source| {
        CliError::Core(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        })
    })?;
    if !same_control_file_identity(&before, &opened) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "{label} changed identity while it was opened"
        ))));
    }
    let mut raw = Vec::new();
    (&mut file)
        .take(MAX_DELEGATION_RECORD_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|source| {
            CliError::Core(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            })
        })?;
    if raw.len() as u64 > MAX_DELEGATION_RECORD_BYTES {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "{label} exceeded its bounded size"
        ))));
    }
    let after = file.metadata().map_err(|source| {
        CliError::Core(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        })
    })?;
    let post_path = fs::symlink_metadata(path).map_err(|source| {
        CliError::Core(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        })
    })?;
    if !same_control_file_identity(&opened, &after)
        || !same_control_file_identity(&after, &post_path)
        || raw.len() as u64 != after.len()
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "{label} changed while it was being read"
        ))));
    }
    Ok(Some(raw))
}

#[cfg(unix)]
fn same_control_file_identity(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    before.file_type().is_file()
        && after.file_type().is_file()
        && before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
}

#[cfg(not(unix))]
fn same_control_file_identity(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.file_type().is_file()
        && after.file_type().is_file()
        && before.len() == after.len()
        && before.modified().ok() == after.modified().ok()
        && before.created().ok() == after.created().ok()
}

fn load_campaign_sub_launch(
    paths: &DeadreckonPaths,
    job_id: &str,
    sub_id: &str,
    plan_id: &str,
) -> Result<Option<CampaignSubLaunchAuthority>> {
    let path = campaign_sub_launch_path(paths, job_id, sub_id, plan_id);
    let Some(raw) = read_bounded_regular_control_file(&path, "Campaign sub-process authority")?
    else {
        return Ok(None);
    };
    serde_json::from_slice(&raw)
        .map(Some)
        .map_err(|source| CliError::Core(DeadreckonError::Json { path, source }))
}

#[cfg(test)]
fn write_campaign_sub_launch(
    paths: &DeadreckonPaths,
    launch: &CampaignSubLaunchAuthority,
) -> Result<()> {
    validate_campaign_sub_launch_identity(launch)?;
    commands::job::replace_json_synced(
        &campaign_sub_launch_path(
            paths,
            &launch.parent_job_id,
            &launch.sub_id,
            &launch.plan_id,
        ),
        launch,
    )
}

fn guarded_authority_lease_token(
    paths: &DeadreckonPaths,
    authority: &commands::supervisor::GuardedDriverAuthority,
) -> Result<deadreckon_core::LeaseToken> {
    if !commands::supervisor::guarded_driver_authority_is_live(paths, authority)? {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process transition requires the current fenced parent Job".to_string(),
        )));
    }
    let lease = deadreckon_core::load_job_lease(paths, &JobId(authority.job_id.clone()))?;
    if lease.epoch != authority.lease_epoch {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process transition crossed parent lease epochs".to_string(),
        )));
    }
    Ok(deadreckon_core::LeaseToken::from(&lease))
}

fn campaign_sub_authority_sha256(launch: &CampaignSubLaunchAuthority) -> Result<String> {
    Ok(deadreckon_core::flight::sha256_text(
        &serde_json::to_string(launch)?,
    ))
}

fn write_campaign_sub_launch_fenced(
    paths: &DeadreckonPaths,
    launch: &CampaignSubLaunchAuthority,
    token: &deadreckon_core::LeaseToken,
    transition: &str,
) -> Result<()> {
    validate_campaign_sub_launch_identity(launch)?;
    if token.job_id.as_ref() != launch.parent_job_id
        || (transition == "adopted" && launch.adopted_by_lease_epoch != Some(token.epoch))
        || (transition != "adopted" && launch.lease_epoch != token.epoch)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process transition does not match its fenced lease".to_string(),
        )));
    }
    let path = campaign_sub_launch_path(
        paths,
        &launch.parent_job_id,
        &launch.sub_id,
        &launch.plan_id,
    );
    let mut detail = campaign_sub_launch_detail(launch);
    let object = detail.as_object_mut().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process event detail is not an object".to_string(),
        ))
    })?;
    object.insert(
        "transition".to_string(),
        serde_json::Value::String(transition.to_string()),
    );
    object.insert(
        "authority_sha256".to_string(),
        serde_json::Value::String(campaign_sub_authority_sha256(launch)?),
    );
    object.insert("authority".to_string(), serde_json::to_value(launch)?);
    deadreckon_core::replace_fenced_job_json_and_append_event(
        paths,
        token,
        Utc::now(),
        &path,
        deadreckon_core::FencedJobJsonEvent {
            kind: JobEventKind::CampaignSubAuthorityChanged,
            causation_id: format!(
                "campaign-sub-authority:{}:{}:{transition}",
                launch.sub_id, launch.launch_id
            ),
            detail,
        },
        launch,
    )?;
    Ok(())
}

fn campaign_sub_transition_is_durable(
    paths: &DeadreckonPaths,
    launch: &CampaignSubLaunchAuthority,
    transition: &str,
) -> Result<bool> {
    let history = deadreckon_core::read_job_history(&paths.job_events(&launch.parent_job_id))?;
    for event in history.events() {
        if event.kind != JobEventKind::CampaignSubAuthorityChanged
            || event
                .detail
                .get("transition")
                .and_then(serde_json::Value::as_str)
                != Some(transition)
        {
            continue;
        }
        let Some(authority_value) = event.detail.get("authority").cloned() else {
            continue;
        };
        let Ok(authority) = serde_json::from_value::<CampaignSubLaunchAuthority>(authority_value)
        else {
            continue;
        };
        let authority_sha256 = campaign_sub_authority_sha256(&authority)?;
        if validate_campaign_sub_launch_identity(&authority).is_err()
            || !campaign_sub_authority_is_successor(launch, &authority)
            || event
                .detail
                .get("authority_sha256")
                .and_then(serde_json::Value::as_str)
                != Some(authority_sha256.as_str())
            || !campaign_sub_authority_has_transition(&authority, transition)
        {
            continue;
        }
        return Ok(true);
    }
    Ok(false)
}

fn campaign_sub_authority_is_successor(
    current: &CampaignSubLaunchAuthority,
    prior: &CampaignSubLaunchAuthority,
) -> bool {
    current.schema_version == prior.schema_version
        && current.launch_protocol == prior.launch_protocol
        && current.parent_job_id == prior.parent_job_id
        && current.campaign_id == prior.campaign_id
        && current.sub_id == prior.sub_id
        && current.plan_id == prior.plan_id
        && current.attempt == prior.attempt
        && current.lease_epoch == prior.lease_epoch
        && current.outer_launch_id == prior.outer_launch_id
        && current.launch_id == prior.launch_id
        && current.capability_id == prior.capability_id
        && current.release_token_sha256 == prior.release_token_sha256
        && current.process == prior.process
        && current.prepared_at == prior.prepared_at
        && (!prior.released || current.released)
        && (!prior.linked || current.linked)
        && (!prior.adopted || current.adopted)
        && prior
            .released_at
            .is_none_or(|timestamp| current.released_at == Some(timestamp))
        && prior
            .linked_at
            .is_none_or(|timestamp| current.linked_at == Some(timestamp))
        && prior
            .adopted_at
            .is_none_or(|timestamp| current.adopted_at == Some(timestamp))
        && prior
            .adopted_by_attempt
            .is_none_or(|attempt| current.adopted_by_attempt == Some(attempt))
        && prior
            .adopted_by_lease_epoch
            .is_none_or(|epoch| current.adopted_by_lease_epoch == Some(epoch))
}

fn campaign_sub_authority_has_transition(
    authority: &CampaignSubLaunchAuthority,
    transition: &str,
) -> bool {
    match transition {
        "prepared" => !authority.released && !authority.linked && !authority.adopted,
        "released" => authority.released && !authority.linked && !authority.adopted,
        "linked" => authority.released && authority.linked && !authority.adopted,
        "adopted" => authority.released && authority.linked && authority.adopted,
        _ => false,
    }
}

fn remove_campaign_sub_launch_projection_if_matches(
    paths: &DeadreckonPaths,
    expected: &CampaignSubLaunchAuthority,
) -> Result<()> {
    let path = campaign_sub_launch_path(
        paths,
        &expected.parent_job_id,
        &expected.sub_id,
        &expected.plan_id,
    );
    let Some(current) = load_campaign_sub_launch(
        paths,
        &expected.parent_job_id,
        &expected.sub_id,
        &expected.plan_id,
    )?
    else {
        return Ok(());
    };
    if current != *expected {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "refusing to remove a Campaign sub-process authority that advanced or was replaced"
                .to_string(),
        )));
    }
    fs::remove_file(&path).map_err(|source| {
        CliError::Core(DeadreckonError::Io {
            path: path.clone(),
            source,
        })
    })?;
    if let Some(parent) = path.parent() {
        sync_delegation_directory(parent)?;
    }
    Ok(())
}

fn load_campaign_sub_release_ack(
    paths: &DeadreckonPaths,
    launch: &CampaignSubLaunchAuthority,
) -> Result<Option<CampaignSubReleaseAck>> {
    let path = campaign_sub_release_ack_path(paths, &launch.parent_job_id, &launch.launch_id);
    let Some(raw) =
        read_bounded_regular_control_file(&path, "Campaign sub-process release acknowledgement")?
    else {
        return Ok(None);
    };
    serde_json::from_slice(&raw)
        .map(Some)
        .map_err(|source| CliError::Core(DeadreckonError::Json { path, source }))
}

fn write_campaign_sub_release_ack(
    paths: &DeadreckonPaths,
    ack: &CampaignSubReleaseAck,
) -> Result<()> {
    commands::job::write_json_synced(
        &campaign_sub_release_ack_path(paths, &ack.parent_job_id, &ack.launch_id),
        ack,
    )
}

fn sync_delegation_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

fn invocation_argv_sha256<I, S>(argv: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut hasher = Sha256::new();
    for argument in argv {
        let argument = argument.as_ref();
        #[cfg(unix)]
        let bytes = {
            use std::os::unix::ffi::OsStrExt;
            argument.as_bytes().to_vec()
        };
        #[cfg(windows)]
        let bytes = {
            use std::os::windows::ffi::OsStrExt;
            argument
                .encode_wide()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        };
        #[cfg(not(any(unix, windows)))]
        let bytes = argument.to_string_lossy().as_bytes().to_vec();
        let length = u64::try_from(bytes.len()).map_err(|_| {
            CliError::Core(DeadreckonError::InvalidInput(
                "delegated invocation argument is too large to hash".to_string(),
            ))
        })?;
        hasher.update(length.to_le_bytes());
        hasher.update(bytes);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn canonical_invocation_path(path: &Path, label: &str) -> Result<PathBuf> {
    fs::canonicalize(path).map_err(|source| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "delegated {label} path {} is unavailable: {source}",
            path.display()
        )))
    })
}

pub(crate) fn prepare_delegated_invocation<S: AsRef<OsStr>>(
    paths: &DeadreckonPaths,
    action: DelegatedAction,
    argv: &[S],
    cwd: &Path,
    scope_root: &Path,
    plan: Option<&deadreckon_core::plan::Plan>,
) -> Result<PreparedDelegation> {
    let context = DRIVER_CONTEXT.get().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "only an authenticated Job driver can delegate executable work".to_string(),
        ))
    })?;
    if !commands::supervisor::guarded_driver_authority_is_live(paths, &context.authority)? {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "the Job driver cannot delegate work without a current fenced lease".to_string(),
        )));
    }
    let immutable_plan_sha256 = if let Some(plan) = plan {
        let owner = resolve_plan_owner(paths, plan)?.ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan {} has no durable Job owner",
                plan.plan_id
            )))
        })?;
        if owner.job.job_id.as_ref() != context.job_id {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan {} belongs to Job {}, not current Job {}",
                plan.plan_id, owner.job.job_id, context.job_id
            ))));
        }
        validate_owned_plan_lineage(paths, &owner)?;
        Some(immutable_plan_sha256(plan)?)
    } else {
        None
    };
    let immutable_campaign_sha256 = match &action {
        DelegatedAction::CampaignSub { campaign_id, .. } => {
            if campaign_id != &context.job_id {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "Campaign delegation changed its durable Job identity".to_string(),
                )));
            }
            let campaign = deadreckon_core::campaign::read_campaign(&paths.plan_dir(campaign_id))?;
            validate_owned_campaign(paths, &campaign, &context.job_id)?;
            Some(immutable_campaign_sha256(&campaign)?)
        }
        DelegatedAction::PlanChild { .. }
        | DelegatedAction::PlanFork { .. }
        | DelegatedAction::PlanMerge { .. }
        | DelegatedAction::MergeRepair { .. } => None,
    };
    let prepared_campaign_sub = match &action {
        DelegatedAction::CampaignSub {
            campaign_id,
            sub_id,
            plan_id,
        } => Some(PreparedCampaignSubLaunch {
            campaign_id: campaign_id.clone(),
            sub_id: sub_id.clone(),
            plan_id: plan_id.clone(),
            launch_id: Uuid::new_v4().to_string(),
        }),
        DelegatedAction::PlanChild { .. }
        | DelegatedAction::PlanFork { .. }
        | DelegatedAction::PlanMerge { .. }
        | DelegatedAction::MergeRepair { .. } => None,
    };
    let capability_id = Uuid::new_v4().to_string();
    let token = format!("{capability_id}:{}", Uuid::new_v4());
    let record = DelegatedInvocation {
        schema_version: 1,
        capability_id: capability_id.clone(),
        job_id: context.job_id.clone(),
        authority: context.authority.clone(),
        action,
        immutable_plan_sha256,
        immutable_campaign_sha256,
        argv_sha256: invocation_argv_sha256(argv)?,
        cwd: canonical_invocation_path(cwd, "working directory")?,
        scope_root: canonical_invocation_path(scope_root, "scope root")?,
        token_sha256: deadreckon_core::flight::sha256_text(&token),
        campaign_sub_launch_id: prepared_campaign_sub
            .as_ref()
            .map(|launch| launch.launch_id.clone()),
        issued_at: Utc::now(),
    };
    if let DelegatedAction::PlanChild {
        plan_id,
        task_id,
        task_index,
        task_attempt,
        run_id,
    } = &record.action
    {
        let lease_token = guarded_authority_lease_token(paths, &context.authority)?;
        prepare_plan_task_run_fenced(
            paths,
            &lease_token,
            plan_id,
            task_id,
            *task_index,
            *task_attempt,
            run_id,
        )?;
    }
    commands::job::write_json_synced(
        &delegation_pending_path(paths, &context.job_id, &capability_id),
        &record,
    )?;
    Ok(PreparedDelegation {
        capability_id,
        job_id: context.job_id.clone(),
        token,
        campaign_sub: prepared_campaign_sub,
    })
}

pub(crate) fn apply_delegation(command: &mut Command, prepared: &PreparedDelegation) {
    commands::supervisor::remove_guarded_driver_metadata(command);
    command
        .env(DELEGATION_JOB_ENV, &prepared.job_id)
        .env(DELEGATION_ID_ENV, &prepared.capability_id)
        .stdin(Stdio::piped());
}

pub(crate) fn release_delegation(child: &mut Child, prepared: &PreparedDelegation) -> Result<()> {
    let mut input = child.stdin.take().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "delegated child did not expose its private capability pipe".to_string(),
        ))
    })?;
    input.write_all(prepared.token.as_bytes())?;
    input.write_all(b"\n")?;
    input.flush()?;
    drop(input);
    Ok(())
}

pub(crate) fn revoke_pending_delegation(
    paths: &DeadreckonPaths,
    prepared: &PreparedDelegation,
) -> Result<()> {
    let pending = delegation_pending_path(paths, &prepared.job_id, &prepared.capability_id);
    match fs::remove_file(&pending) {
        Ok(()) => {
            if let Some(parent) = pending.parent() {
                sync_delegation_directory(parent)?;
            }
            Ok(())
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CliError::Core(DeadreckonError::Io {
            path: pending,
            source,
        })),
    }
}

pub(crate) fn spawn_delegated(
    paths: &DeadreckonPaths,
    command: &mut Command,
    prepared: &PreparedDelegation,
) -> Result<Child> {
    apply_delegation(command, prepared);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(source) => {
            revoke_pending_delegation(paths, prepared)?;
            return Err(source.into());
        }
    };
    if let Err(error) = release_delegation(&mut child, prepared) {
        let _ = child.kill();
        let _ = child.wait();
        if let Err(revoke_error) = revoke_pending_delegation(paths, prepared) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "delegated child release failed ({error}); capability revocation also failed ({revoke_error})"
            ))));
        }
        return Err(error);
    }
    Ok(child)
}

pub(crate) fn spawn_merge_repair_delegated(
    paths: &DeadreckonPaths,
    mut command: Command,
    prepared: &PreparedDelegation,
    authority_path: &Path,
    mut authority: serde_json::Value,
) -> Result<Child> {
    let context = DRIVER_CONTEXT.get().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "durable merge repair launch requires the current parent Job driver".to_string(),
        ))
    })?;
    if context.job_id != prepared.job_id
        || !commands::supervisor::guarded_driver_authority_is_live(paths, &context.authority)?
    {
        revoke_pending_delegation(paths, prepared)?;
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable merge repair launch lost its fenced parent authority".to_string(),
        )));
    }
    let token = match guarded_authority_lease_token(paths, &context.authority) {
        Ok(token) => token,
        Err(error) => {
            revoke_pending_delegation(paths, prepared)?;
            return Err(error);
        }
    };
    apply_delegation(&mut command, prepared);
    let (mut child, terminator) = match deadreckon_core::spawn_grouped(command) {
        Ok(spawned) => spawned,
        Err(source) => {
            revoke_pending_delegation(paths, prepared)?;
            return Err(source.into());
        }
    };
    let mut process = match deadreckon_core::SupervisedProcessRecord::prepared(
        deadreckon_core::SupervisedProcess {
            pid: child.id(),
            pgid: None,
        },
        prepared.capability_id.clone(),
        context.authority.attempt,
        Some(context.authority.launch_id.clone()),
        deadreckon_core::flight::sha256_text(&prepared.token),
    ) {
        Ok(process) => process,
        Err(source) => {
            let _ = terminator.terminate(std::time::Duration::from_secs(2));
            revoke_pending_delegation(paths, prepared)?;
            return Err(source.into());
        }
    };
    #[cfg(unix)]
    {
        process.process.pgid = Some(child.id());
    }
    process.phase = deadreckon_core::SupervisedProcessPhase::Running;
    let object = authority.as_object_mut().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "merge repair launch authority is not an object".to_string(),
        ))
    })?;
    object.insert("process".to_string(), serde_json::to_value(&process)?);
    object.insert(
        "status".to_string(),
        serde_json::Value::String("process_prepared".to_string()),
    );
    object.insert(
        "process_prepared_at".to_string(),
        serde_json::to_value(Utc::now())?,
    );
    let repair_id = authority
        .get("repair_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "merge repair authority is missing its repair identity".to_string(),
            ))
        })?;
    let protected_path = merge_repair_authority_path(paths, &context.job_id, repair_id);
    let creation = deadreckon_core::create_fenced_job_json_and_append_event(
        paths,
        &token,
        Utc::now(),
        &protected_path,
        deadreckon_core::FencedJobJsonEvent {
            kind: JobEventKind::RepairChildAuthorityChanged,
            causation_id: format!("merge-repair-authority:{repair_id}:process_prepared"),
            detail: json!({
                "repair_id": repair_id,
                "transition": "process_prepared",
                "run_id": authority.get("run_id"),
                "authority": &authority,
            }),
        },
        &authority,
    );
    let durable = match creation {
        Ok(deadreckon_core::CreateFencedJobJsonDisposition::Created(_)) => {
            commands::job::write_json_synced(authority_path, &authority)
        }
        Ok(deadreckon_core::CreateFencedJobJsonDisposition::AlreadyExists) => {
            Err(CliError::Core(DeadreckonError::InvalidInput(
                "merge repair launch authority already exists; refusing a duplicate child"
                    .to_string(),
            )))
        }
        Err(error) => Err(error.into()),
    };
    if let Err(error) = durable {
        let _ = terminator.terminate(std::time::Duration::from_secs(2));
        revoke_pending_delegation(paths, prepared)?;
        return Err(error);
    }
    if let Err(error) = release_delegation(&mut child, prepared) {
        let termination = terminator.terminate(std::time::Duration::from_secs(2));
        let revocation = revoke_pending_delegation(paths, prepared);
        if matches!(termination, deadreckon_core::TerminationOutcome::Failed(_)) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "merge repair capability release failed ({error}); its exact process group could not be stopped ({termination:?}); capability revocation: {}",
                revocation
                    .as_ref()
                    .map(|()| "ok".to_string())
                    .unwrap_or_else(|revoke_error| revoke_error.to_string())
            ))));
        }
        revocation?;
        return Err(error);
    }
    Ok(child)
}

fn validate_campaign_sub_launch_identity(launch: &CampaignSubLaunchAuthority) -> Result<()> {
    let process = &launch.process;
    if launch.schema_version != 1
        || launch.launch_protocol != CAMPAIGN_SUB_LAUNCH_PROTOCOL
        || launch.parent_job_id != launch.campaign_id
        || launch.attempt == 0
        || launch.lease_epoch == 0
        || Uuid::parse_str(&launch.outer_launch_id).is_err()
        || Uuid::parse_str(&launch.launch_id).is_err()
        || Uuid::parse_str(&launch.capability_id).is_err()
        || launch.release_token_sha256.trim().is_empty()
        || process.launch_id != launch.launch_id
        || process.attempt != launch.attempt
        || process.owner_launch_id.as_deref() != Some(launch.outer_launch_id.as_str())
        || process.release_token_sha256 != launch.release_token_sha256
        || process.schema_version != deadreckon_core::SUPERVISED_PROCESS_RECORD_SCHEMA_VERSION
        || process.process.pid == 0
        || process.boot_id.trim().is_empty()
        || process.process_start_identity.trim().is_empty()
        || process.phase != deadreckon_core::SupervisedProcessPhase::Running
        || launch.adopted && !launch.linked
        || launch.linked && !launch.released
        || launch.released_at.is_some() != launch.released
        || launch.linked_at.is_some() != launch.linked
        || launch.adopted_at.is_some() != launch.adopted
        || launch.adopted_by_attempt.is_some() != launch.adopted
        || launch.adopted_by_lease_epoch.is_some() != launch.adopted
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process authority is malformed or crosses launch identities".to_string(),
        )));
    }
    #[cfg(unix)]
    if process.process.pgid != Some(process.process.pid) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process authority has a mismatched process group".to_string(),
        )));
    }
    #[cfg(not(unix))]
    if process.process.pgid.is_some() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process authority unexpectedly names a process group".to_string(),
        )));
    }
    Ok(())
}

fn campaign_sub_ack_matches(
    launch: &CampaignSubLaunchAuthority,
    ack: &CampaignSubReleaseAck,
) -> bool {
    ack.schema_version == 1
        && ack.launch_protocol == CAMPAIGN_SUB_LAUNCH_PROTOCOL
        && ack.parent_job_id == launch.parent_job_id
        && ack.campaign_id == launch.campaign_id
        && ack.sub_id == launch.sub_id
        && ack.plan_id == launch.plan_id
        && ack.attempt == launch.attempt
        && ack.lease_epoch == launch.lease_epoch
        && ack.launch_id == launch.launch_id
        && ack.capability_id == launch.capability_id
        && ack.release_token_sha256 == launch.release_token_sha256
        && ack.pid == launch.process.process.pid
        && ack.process_group == launch.process.process.pgid
        && ack.boot_id == launch.process.boot_id
        && ack.process_start_identity == launch.process.process_start_identity
}

fn delegated_record_matches_campaign_launch(
    record: &DelegatedInvocation,
    launch: &CampaignSubLaunchAuthority,
) -> bool {
    record.schema_version == 1
        && record.capability_id == launch.capability_id
        && record.job_id == launch.parent_job_id
        && record.authority.job_id == launch.parent_job_id
        && record.authority.attempt == launch.attempt
        && record.authority.launch_id == launch.outer_launch_id
        && record.authority.lease_epoch == launch.lease_epoch
        && record.token_sha256 == launch.release_token_sha256
        && record.campaign_sub_launch_id.as_deref() == Some(launch.launch_id.as_str())
        && matches!(
            &record.action,
            DelegatedAction::CampaignSub {
                campaign_id,
                sub_id,
                plan_id,
            } if campaign_id == &launch.campaign_id
                && sub_id == &launch.sub_id
                && plan_id == &launch.plan_id
        )
}

fn read_matching_delegated_record(
    path: &Path,
    launch: &CampaignSubLaunchAuthority,
) -> Result<Option<DelegatedInvocation>> {
    let Some(raw) =
        read_bounded_regular_control_file(path, "Campaign sub-process capability record")?
    else {
        return Ok(None);
    };
    let record: DelegatedInvocation = serde_json::from_slice(&raw).map_err(|source| {
        CliError::Core(DeadreckonError::Json {
            path: path.to_path_buf(),
            source,
        })
    })?;
    if !delegated_record_matches_campaign_launch(&record, launch) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process capability conflicts with its launch authority".to_string(),
        )));
    }
    Ok(Some(record))
}

fn classify_campaign_sub_recovery(
    launch: &CampaignSubLaunchAuthority,
    identity: deadreckon_core::SupervisedProcessIdentity,
    ack_present: bool,
    pending_present: bool,
    consumed_present: bool,
    linked_transition_durable: bool,
) -> Result<CampaignSubRecoveryDisposition> {
    validate_campaign_sub_launch_identity(launch)?;
    if launch.released && !ack_present {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process says it was released without a matching acknowledgement"
                .to_string(),
        )));
    }
    if linked_transition_durable && !launch.linked {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process durable link conflicts with its authority projection".to_string(),
        )));
    }
    if launch.linked
        && linked_transition_durable
        && (!ack_present || !consumed_present || pending_present)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process linked state is not backed by its release and one-time capability"
                .to_string(),
        )));
    }
    if !pending_present && !consumed_present {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process lost both its pending and consumed capability evidence"
                .to_string(),
        )));
    }
    if matches!(
        identity,
        deadreckon_core::SupervisedProcessIdentity::Reused
            | deadreckon_core::SupervisedProcessIdentity::Unverifiable
    ) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process identity is conflicting or unverifiable".to_string(),
        )));
    }
    if !launch.linked || !linked_transition_durable {
        return Ok(CampaignSubRecoveryDisposition::RelaunchNonexecuted);
    }
    match identity {
        deadreckon_core::SupervisedProcessIdentity::Current => {
            Ok(CampaignSubRecoveryDisposition::AdoptLinked)
        }
        deadreckon_core::SupervisedProcessIdentity::Exited
        | deadreckon_core::SupervisedProcessIdentity::DifferentBoot => {
            Ok(CampaignSubRecoveryDisposition::RecoverLinkedArtifacts)
        }
        deadreckon_core::SupervisedProcessIdentity::Reused
        | deadreckon_core::SupervisedProcessIdentity::Unverifiable => unreachable!(),
    }
}

fn campaign_sub_launch_evidence(
    paths: &DeadreckonPaths,
    launch: &CampaignSubLaunchAuthority,
) -> Result<(
    deadreckon_core::SupervisedProcessIdentity,
    bool,
    bool,
    bool,
    bool,
)> {
    validate_campaign_sub_launch_identity(launch)?;
    let ack = load_campaign_sub_release_ack(paths, launch)?;
    if ack
        .as_ref()
        .is_some_and(|ack| !campaign_sub_ack_matches(launch, ack))
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process release acknowledgement conflicts with its launch authority"
                .to_string(),
        )));
    }
    let pending = read_matching_delegated_record(
        &delegation_pending_path(paths, &launch.parent_job_id, &launch.capability_id),
        launch,
    )?
    .is_some();
    let consumed = read_matching_delegated_record(
        &delegation_consumed_path(paths, &launch.parent_job_id, &launch.capability_id),
        launch,
    )?
    .is_some();
    Ok((
        launch.process.identity(),
        ack.is_some(),
        pending,
        consumed,
        campaign_sub_transition_is_durable(paths, launch, "linked")?,
    ))
}

fn recover_linked_campaign_sub_artifacts(
    paths: &DeadreckonPaths,
    launch: &CampaignSubLaunchAuthority,
    identity: deadreckon_core::SupervisedProcessIdentity,
) -> Result<CampaignSubLaunchRecovery> {
    match identity {
        deadreckon_core::SupervisedProcessIdentity::Exited => {
            terminate_campaign_sub_process(
                launch,
                std::time::Duration::from_secs(2),
                "linked residual",
            )?;
        }
        deadreckon_core::SupervisedProcessIdentity::DifferentBoot => {}
        deadreckon_core::SupervisedProcessIdentity::Current
        | deadreckon_core::SupervisedProcessIdentity::Reused
        | deadreckon_core::SupervisedProcessIdentity::Unverifiable => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "linked Campaign artifact recovery requires an exited process or changed boot"
                    .to_string(),
            )));
        }
    }
    // The durable link authorizes dispatch, but the reserved Plan is the
    // first durable artifact proving dispatch actually started. A crash in
    // the narrow link-before-dispatch window is therefore safe to relaunch.
    if paths.plan_json(&launch.plan_id).is_file() {
        Ok(CampaignSubLaunchRecovery::RecoverLinkedArtifacts)
    } else {
        append_campaign_sub_launch_event(
            paths,
            launch,
            "sub_process_empty_dispatch_relaunch_safe",
        )?;
        Ok(CampaignSubLaunchRecovery::Relaunch)
    }
}

fn campaign_sub_launch_detail(launch: &CampaignSubLaunchAuthority) -> serde_json::Value {
    json!({
        "parent_job_id": launch.parent_job_id,
        "sub_id": launch.sub_id,
        "plan_id": launch.plan_id,
        "attempt": launch.attempt,
        "lease_epoch": launch.lease_epoch,
        "outer_launch_id": launch.outer_launch_id,
        "launch_id": launch.launch_id,
        "release_token_sha256": launch.release_token_sha256,
        "pid": launch.process.process.pid,
        "process_group": launch.process.process.pgid,
        "boot_id": launch.process.boot_id,
        "process_start_identity": launch.process.process_start_identity,
        "released": launch.released,
        "linked": launch.linked,
        "adopted": launch.adopted,
        "adopted_by_attempt": launch.adopted_by_attempt,
        "adopted_by_lease_epoch": launch.adopted_by_lease_epoch,
        "adopted_at": launch.adopted_at,
    })
}

fn append_campaign_sub_launch_event(
    paths: &DeadreckonPaths,
    launch: &CampaignSubLaunchAuthority,
    kind: &str,
) -> Result<()> {
    deadreckon_core::campaign::append_campaign_event(
        &paths.plan_dir(&launch.campaign_id),
        kind,
        campaign_sub_launch_detail(launch),
    )?;
    Ok(())
}

fn campaign_sub_launch_test_failpoint_once(
    paths: &DeadreckonPaths,
    launch: &CampaignSubLaunchAuthority,
    name: &str,
) -> Result<()> {
    if std::env::var("DEADRECKON_TEST_CAMPAIGN_FAILPOINTS").as_deref() != Ok("1")
        || std::env::var("DEADRECKON_TEST_CAMPAIGN_FAILPOINT").as_deref() != Ok(name)
    {
        return Ok(());
    }
    let marker = paths
        .job_dir(&launch.parent_job_id)
        .join(CAMPAIGN_SUB_LAUNCH_DIR)
        .join(format!(".test-failpoint-{name}"));
    let Some(parent) = marker.parent() else {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process failpoint marker has no parent".to_string(),
        )));
    };
    fs::create_dir_all(parent)?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
    {
        Ok(file) => {
            file.sync_all()?;
            #[cfg(unix)]
            fs::File::open(parent)?.sync_all()?;
            std::process::exit(86);
        }
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(source) => Err(CliError::Core(DeadreckonError::Io {
            path: marker,
            source,
        })),
    }
}

fn record_campaign_sub_dispatch_test_side_effect(
    launch: &CampaignSubLaunchAuthority,
) -> Result<()> {
    let Some(directory) =
        std::env::var_os("DEADRECKON_TEST_CAMPAIGN_DISPATCH_DIR").filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let directory = PathBuf::from(directory);
    fs::create_dir_all(&directory)?;
    let marker = directory.join(format!(
        "{}.dispatch",
        campaign_sub_launch_key(&launch.sub_id, &launch.plan_id)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::AlreadyExists {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Campaign sub {} attempted its observable dispatch side effect more than once",
                    launch.sub_id
                )))
            } else {
                CliError::Core(DeadreckonError::Io {
                    path: marker.clone(),
                    source,
                })
            }
        })?;
    file.write_all(launch.launch_id.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    #[cfg(unix)]
    fs::File::open(&directory)?.sync_all()?;
    Ok(())
}

fn terminate_campaign_sub_process(
    launch: &CampaignSubLaunchAuthority,
    grace: std::time::Duration,
    label: &str,
) -> Result<()> {
    use deadreckon_core::ChildTerminator as _;

    match launch.process.identity() {
        deadreckon_core::SupervisedProcessIdentity::DifferentBoot => return Ok(()),
        deadreckon_core::SupervisedProcessIdentity::Current => {}
        deadreckon_core::SupervisedProcessIdentity::Exited => {
            #[cfg(not(unix))]
            return Ok(());
        }
        deadreckon_core::SupervisedProcessIdentity::Reused
        | deadreckon_core::SupervisedProcessIdentity::Unverifiable => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "refusing to signal a Campaign sub-process with conflicting or unknown identity"
                    .to_string(),
            )));
        }
    }
    let outcome = {
        #[cfg(unix)]
        {
            let pgid = launch.process.process.pgid.ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "Campaign sub-process has no recoverable process group".to_string(),
                ))
            })?;
            let pgid = i32::try_from(pgid).map_err(|_| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "Campaign sub-process group identity is invalid".to_string(),
                ))
            })?;
            deadreckon_core::ProcessGroupTerminator::new(pgid).terminate(grace)
        }
        #[cfg(not(unix))]
        {
            deadreckon_core::RawPidTerminator::new(launch.process.process.pid).terminate(grace)
        }
    };
    if matches!(outcome, deadreckon_core::TerminationOutcome::Failed(_)) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "could not stop {label} Campaign sub-process: {outcome:?}"
        ))));
    }
    Ok(())
}

pub(crate) struct ValidatedCampaignSubInventory {
    launches: Vec<CampaignSubLaunchAuthority>,
}

pub(crate) fn validate_campaign_sub_process_inventory_for_job(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<ValidatedCampaignSubInventory> {
    let directory = paths.job_dir(job_id).join(CAMPAIGN_SUB_LAUNCH_DIR);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ValidatedCampaignSubInventory {
                launches: Vec::new(),
            });
        }
        Err(source) => {
            return Err(CliError::Core(DeadreckonError::Io {
                path: directory,
                source,
            }));
        }
    };
    let mut authority_paths = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    authority_paths.sort();
    let mut launches = Vec::new();
    for path in authority_paths {
        if path.extension() != Some(OsStr::new("json")) {
            continue;
        }
        let Some(raw) = read_bounded_regular_control_file(&path, "Campaign sub-process authority")?
        else {
            continue;
        };
        let launch: CampaignSubLaunchAuthority =
            serde_json::from_slice(&raw).map_err(|source| {
                CliError::Core(DeadreckonError::Json {
                    path: path.clone(),
                    source,
                })
            })?;
        validate_campaign_sub_launch_identity(&launch)?;
        if launch.parent_job_id != job_id || launch.campaign_id != job_id {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Campaign sub-process authority {} changed its parent Job",
                path.display()
            ))));
        }
        if matches!(
            launch.process.identity(),
            deadreckon_core::SupervisedProcessIdentity::Reused
                | deadreckon_core::SupervisedProcessIdentity::Unverifiable
        ) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "refusing Campaign cancellation because authority {} has a conflicting or unverifiable process identity",
                path.display()
            ))));
        }
        launches.push(launch);
    }
    Ok(ValidatedCampaignSubInventory { launches })
}

pub(crate) fn terminate_validated_campaign_sub_processes(
    paths: &DeadreckonPaths,
    inventory: ValidatedCampaignSubInventory,
    grace: std::time::Duration,
) -> Result<()> {
    for launch in inventory.launches {
        terminate_campaign_sub_process(&launch, grace, "cancelled")?;
        let pending = delegation_pending_path(paths, &launch.parent_job_id, &launch.capability_id);
        match fs::remove_file(&pending) {
            Ok(()) => {
                if let Some(parent) = pending.parent() {
                    sync_delegation_directory(parent)?;
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(CliError::Core(DeadreckonError::Io {
                    path: pending,
                    source,
                }));
            }
        }
    }
    Ok(())
}

pub(crate) fn reconcile_campaign_sub_processes_for_job(
    paths: &DeadreckonPaths,
    job_id: &str,
    grace: std::time::Duration,
) -> Result<()> {
    let inventory = validate_campaign_sub_process_inventory_for_job(paths, job_id)?;
    terminate_validated_campaign_sub_processes(paths, inventory, grace)
}

#[derive(Debug, Clone)]
struct MergeRepairProcessAuthority {
    path: PathBuf,
    raw: Vec<u8>,
    job_id: String,
    repair_id: String,
    capability_id: String,
    process: deadreckon_core::SupervisedProcessRecord,
}

#[derive(Debug)]
pub(crate) struct ValidatedMergeRepairProcessInventory {
    authorities: Vec<MergeRepairProcessAuthority>,
}

/// Validate every separately-grouped merge-repair process before signalling
/// any of them. The authority projection alone is not enough: it must be the
/// exact value committed by the Job event history, name the current Job and
/// outer launch, and match its one-time delegated capability.
pub(crate) fn validate_merge_repair_process_inventory_for_job(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<ValidatedMergeRepairProcessInventory> {
    let directory = paths.job_dir(job_id).join(MERGE_REPAIR_AUTHORITY_DIR);
    let metadata = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ValidatedMergeRepairProcessInventory {
                authorities: Vec::new(),
            });
        }
        Err(source) => {
            return Err(CliError::Core(DeadreckonError::Io {
                path: directory,
                source,
            }));
        }
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "merge-repair authorities for Job {job_id} are not a trusted directory"
        ))));
    }
    let mut authority_paths = fs::read_dir(&directory)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    authority_paths.sort();
    let history = deadreckon_core::read_job_history(&paths.job_events(job_id))?;
    let mut authorities = Vec::new();
    for path in authority_paths {
        if path.extension() != Some(OsStr::new("json")) {
            continue;
        }
        let Some(raw) = read_bounded_regular_control_file(&path, "merge-repair process authority")?
        else {
            continue;
        };
        let authority: serde_json::Value = serde_json::from_slice(&raw).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: path.clone(),
                source,
            })
        })?;
        let object = authority.as_object().ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "merge-repair authority {} is not an object",
                path.display()
            )))
        })?;
        let repair_id = required_merge_repair_string(object, "repair_id", &path)?;
        let capability_id = required_merge_repair_string(object, "capability_id", &path)?;
        let run_id = required_merge_repair_string(object, "run_id", &path)?;
        let schema_version = object
            .get("schema_version")
            .and_then(serde_json::Value::as_u64);
        let valid_adoption_deadline = schema_version != Some(3)
            || object
                .get("adoption_deadline_at")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|value| DateTime::parse_from_rfc3339(value).is_ok());
        let valid_adoption_window = schema_version != Some(3)
            || object
                .get("adoption_window_seconds")
                .and_then(serde_json::Value::as_f64)
                .is_some_and(|value| value.is_finite() && value > 0.0);
        if !matches!(schema_version, Some(2 | 3))
            || !valid_adoption_deadline
            || !valid_adoption_window
            || object
                .get("root_artifact_id")
                .and_then(serde_json::Value::as_str)
                != Some(job_id)
            || merge_repair_authority_path(paths, job_id, &repair_id) != path
            || Uuid::parse_str(&capability_id).is_err()
            || Uuid::parse_str(&run_id).is_err()
            || !matches!(
                object.get("status").and_then(serde_json::Value::as_str),
                Some("process_prepared" | "child_linked" | "trusted")
            )
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "merge-repair authority {} changed its exact Job, repair, capability, run, or lifecycle identity",
                path.display()
            ))));
        }
        let process: deadreckon_core::SupervisedProcessRecord =
            serde_json::from_value(object.get("process").cloned().ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "merge-repair authority {} has no supervised process",
                    path.display()
                )))
            })?)?;
        validate_merge_repair_process_identity(&path, &capability_id, &process)?;

        let (authority_index, authority_event) = history
            .events()
            .iter()
            .enumerate()
            .rev()
            .find(|(_, event)| {
                event.kind == JobEventKind::RepairChildAuthorityChanged
                    && event
                        .detail
                        .get("repair_id")
                        .and_then(serde_json::Value::as_str)
                        == Some(repair_id.as_str())
            })
            .ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "merge-repair authority {} has no committed Job event",
                    path.display()
                )))
            })?;
        if authority_event.detail.get("authority") != Some(&authority) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "merge-repair authority {} differs from its latest committed Job event",
                path.display()
            ))));
        }
        let owner_launch_id = process.owner_launch_id.as_deref().ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "merge-repair authority {} has no outer launch identity",
                path.display()
            )))
        })?;
        let outer_launch_committed = history.events()[..=authority_index].iter().any(|event| {
            event.kind == JobEventKind::ChildLinked
                && event
                    .detail
                    .get("attempt")
                    .and_then(serde_json::Value::as_u64)
                    == Some(u64::from(process.attempt))
                && event
                    .detail
                    .get("launch_id")
                    .and_then(serde_json::Value::as_str)
                    == Some(owner_launch_id)
                && event
                    .detail
                    .get("root_id")
                    .and_then(serde_json::Value::as_str)
                    == Some(job_id)
        });
        if !outer_launch_committed {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "merge-repair authority {} is not bound to its exact outer Job launch",
                path.display()
            ))));
        }
        validate_merge_repair_delegation(
            paths,
            job_id,
            &repair_id,
            &run_id,
            &capability_id,
            authority_event.lease_epoch,
            &process,
        )?;
        if matches!(
            process.identity(),
            deadreckon_core::SupervisedProcessIdentity::Reused
                | deadreckon_core::SupervisedProcessIdentity::Unverifiable
        ) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "refusing Job cleanup because merge-repair authority {} has a conflicting or unverifiable process identity",
                path.display()
            ))));
        }
        authorities.push(MergeRepairProcessAuthority {
            path,
            raw,
            job_id: job_id.to_string(),
            repair_id,
            capability_id,
            process,
        });
    }
    Ok(ValidatedMergeRepairProcessInventory { authorities })
}

fn required_merge_repair_string(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    path: &Path,
) -> Result<String> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "merge-repair authority {} has no valid {field}",
                path.display()
            )))
        })
}

fn validate_merge_repair_process_identity(
    path: &Path,
    capability_id: &str,
    process: &deadreckon_core::SupervisedProcessRecord,
) -> Result<()> {
    let process_group_valid = {
        #[cfg(unix)]
        {
            process.process.pgid == Some(process.process.pid)
        }
        #[cfg(not(unix))]
        {
            process.process.pgid.is_none()
        }
    };
    if process.schema_version != deadreckon_core::SUPERVISED_PROCESS_RECORD_SCHEMA_VERSION
        || process.process.pid == 0
        || process.attempt == 0
        || process.launch_id != capability_id
        || process
            .owner_launch_id
            .as_deref()
            .is_none_or(|launch_id| Uuid::parse_str(launch_id).is_err())
        || process.release_token_sha256.trim().is_empty()
        || process.boot_id.trim().is_empty()
        || process.process_start_identity.trim().is_empty()
        || process.phase != deadreckon_core::SupervisedProcessPhase::Running
        || !process_group_valid
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "merge-repair authority {} has an invalid exact process or launch identity",
            path.display()
        ))));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_merge_repair_delegation(
    paths: &DeadreckonPaths,
    job_id: &str,
    repair_id: &str,
    run_id: &str,
    capability_id: &str,
    authority_lease_epoch: u64,
    process: &deadreckon_core::SupervisedProcessRecord,
) -> Result<()> {
    let pending = delegation_pending_path(paths, job_id, capability_id);
    let consumed = delegation_consumed_path(paths, job_id, capability_id);
    let pending_raw =
        read_bounded_regular_control_file(&pending, "merge-repair pending delegated capability")?;
    let consumed_raw =
        read_bounded_regular_control_file(&consumed, "merge-repair consumed delegated capability")?;
    if pending_raw.is_some() && consumed_raw.is_some() && pending_raw != consumed_raw {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "merge-repair capability {capability_id} has conflicting pending and consumed authority"
        ))));
    }
    let raw = consumed_raw.or(pending_raw).ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "merge-repair capability {capability_id} has no protected delegation record"
        )))
    })?;
    let record: DelegatedInvocation = serde_json::from_slice(&raw)?;
    let action_matches = matches!(
        &record.action,
        DelegatedAction::MergeRepair {
            root_artifact_id,
            repair_id: delegated_repair_id,
            run_id: delegated_run_id,
            ..
        } if root_artifact_id == job_id
            && delegated_repair_id == repair_id
            && delegated_run_id == run_id
    );
    if record.schema_version != 1
        || record.job_id != job_id
        || record.capability_id != capability_id
        || record.authority.job_id != job_id
        || record.authority.attempt != process.attempt
        || record.authority.launch_id.as_str() != process.owner_launch_id.as_deref().unwrap_or("")
        || record.authority.lease_epoch != authority_lease_epoch
        || record.token_sha256 != process.release_token_sha256
        || record.campaign_sub_launch_id.is_some()
        || !action_matches
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "merge-repair capability {capability_id} does not match its exact Job, launch, process, and repair authority"
        ))));
    }
    Ok(())
}

pub(crate) fn terminate_validated_merge_repair_processes(
    paths: &DeadreckonPaths,
    inventory: ValidatedMergeRepairProcessInventory,
    grace: std::time::Duration,
) -> Result<()> {
    for authority in inventory.authorities {
        let current =
            read_bounded_regular_control_file(&authority.path, "merge-repair process authority")?
                .ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "merge-repair authority {} disappeared before cleanup",
                    authority.path.display()
                )))
            })?;
        if current != authority.raw {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "merge-repair authority {} changed before cleanup",
                authority.path.display()
            ))));
        }
        match authority.process.identity() {
            deadreckon_core::SupervisedProcessIdentity::Current
            | deadreckon_core::SupervisedProcessIdentity::Exited => {
                let outcome =
                    commands::job::terminate_supervised_process(authority.process.process, grace);
                if let deadreckon_core::TerminationOutcome::Failed(reason) = outcome {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                        "could not stop merge-repair process {} for {}: {reason}",
                        authority.process.process.pid, authority.repair_id
                    ))));
                }
            }
            deadreckon_core::SupervisedProcessIdentity::DifferentBoot => {}
            deadreckon_core::SupervisedProcessIdentity::Reused
            | deadreckon_core::SupervisedProcessIdentity::Unverifiable => {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "merge-repair process identity {} became conflicting before cleanup",
                    authority.process.process.pid
                ))));
            }
        }
        let pending = delegation_pending_path(paths, &authority.job_id, &authority.capability_id);
        match fs::remove_file(&pending) {
            Ok(()) => {
                if let Some(parent) = pending.parent() {
                    sync_delegation_directory(parent)?;
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(CliError::Core(DeadreckonError::Io {
                    path: pending,
                    source,
                }));
            }
        }
    }
    Ok(())
}

pub(crate) fn reconcile_merge_repair_processes_for_job(
    paths: &DeadreckonPaths,
    job_id: &str,
    grace: std::time::Duration,
) -> Result<()> {
    let inventory = validate_merge_repair_process_inventory_for_job(paths, job_id)?;
    terminate_validated_merge_repair_processes(paths, inventory, grace)
}

pub(crate) fn campaign_sub_launch_process_is_live(
    paths: &DeadreckonPaths,
    campaign_id: &str,
    sub_id: &str,
    plan_id: &str,
) -> Result<bool> {
    let Some(launch) = load_campaign_sub_launch(paths, campaign_id, sub_id, plan_id)? else {
        return Ok(false);
    };
    if launch.parent_job_id != campaign_id
        || launch.campaign_id != campaign_id
        || launch.sub_id != sub_id
        || launch.plan_id != plan_id
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process authority changed its parent or reserved Plan identity"
                .to_string(),
        )));
    }
    let (identity, ack, pending, consumed, linked_transition_durable) =
        campaign_sub_launch_evidence(paths, &launch)?;
    if matches!(
        identity,
        deadreckon_core::SupervisedProcessIdentity::Reused
            | deadreckon_core::SupervisedProcessIdentity::Unverifiable
    ) && paths.plan_json(plan_id).is_file()
    {
        // Once the exact reserved Plan exists, its protected ownership and
        // lifecycle are better recovery evidence than a numeric PID that may
        // have been reused. Validate every non-process invariant, then let
        // Plan recovery take precedence.
        let disposition = classify_campaign_sub_recovery(
            &launch,
            deadreckon_core::SupervisedProcessIdentity::DifferentBoot,
            ack,
            pending,
            consumed,
            linked_transition_durable,
        )?;
        if disposition == CampaignSubRecoveryDisposition::RecoverLinkedArtifacts {
            return Ok(false);
        }
    }
    classify_campaign_sub_recovery(
        &launch,
        identity,
        ack,
        pending,
        consumed,
        linked_transition_durable,
    )?;
    Ok(identity == deadreckon_core::SupervisedProcessIdentity::Current)
}

pub(crate) fn recover_campaign_sub_launch(
    paths: &DeadreckonPaths,
    campaign_id: &str,
    sub_id: &str,
    plan_id: &str,
) -> Result<CampaignSubLaunchRecovery> {
    let Some(mut launch) = load_campaign_sub_launch(paths, campaign_id, sub_id, plan_id)? else {
        return Ok(CampaignSubLaunchRecovery::Relaunch);
    };
    if launch.parent_job_id != campaign_id
        || launch.campaign_id != campaign_id
        || launch.sub_id != sub_id
        || launch.plan_id != plan_id
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process authority changed its parent or reserved Plan identity"
                .to_string(),
        )));
    }
    let (identity, ack, pending, consumed, linked_transition_durable) =
        campaign_sub_launch_evidence(paths, &launch)?;
    match classify_campaign_sub_recovery(
        &launch,
        identity,
        ack,
        pending,
        consumed,
        linked_transition_durable,
    )? {
        CampaignSubRecoveryDisposition::RelaunchNonexecuted => {
            if paths.plan_json(plan_id).is_file() {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "unlinked Campaign sub-process has a reserved Plan artifact; refusing to assume it never executed"
                        .to_string(),
                )));
            }
            if identity == deadreckon_core::SupervisedProcessIdentity::Current {
                terminate_campaign_sub_process(
                    &launch,
                    std::time::Duration::from_secs(2),
                    "nonexecuted",
                )?;
                if launch.process.identity() == deadreckon_core::SupervisedProcessIdentity::Current
                {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(
                        "nonexecuted Campaign sub-process remained alive after bounded termination"
                            .to_string(),
                    )));
                }
                // The child and recovering driver can race at the link
                // boundary. Re-read after the exact process is dead. If the
                // child durably linked before termination, it may have begun
                // work and must not be launched again.
                let refreshed = load_campaign_sub_launch(paths, campaign_id, sub_id, plan_id)?
                    .ok_or_else(|| {
                        CliError::Core(DeadreckonError::InvalidInput(
                            "Campaign sub-process authority disappeared during recovery"
                                .to_string(),
                        ))
                    })?;
                if refreshed.launch_id != launch.launch_id {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(
                        "Campaign sub-process launch identity changed during recovery".to_string(),
                    )));
                }
                let (identity, ack, pending, consumed, linked_transition_durable) =
                    campaign_sub_launch_evidence(paths, &refreshed)?;
                match classify_campaign_sub_recovery(
                    &refreshed,
                    identity,
                    ack,
                    pending,
                    consumed,
                    linked_transition_durable,
                )? {
                    CampaignSubRecoveryDisposition::RecoverLinkedArtifacts => {
                        return recover_linked_campaign_sub_artifacts(paths, &refreshed, identity);
                    }
                    CampaignSubRecoveryDisposition::AdoptLinked => {
                        return Err(CliError::Core(DeadreckonError::InvalidInput(
                            "Campaign sub-process remained live after recovery terminated its exact process group"
                                .to_string(),
                        )));
                    }
                    CampaignSubRecoveryDisposition::RelaunchNonexecuted => {
                        launch = refreshed;
                    }
                }
            }
            let pending_path =
                delegation_pending_path(paths, &launch.parent_job_id, &launch.capability_id);
            match fs::remove_file(&pending_path) {
                Ok(()) => {
                    if let Some(parent) = pending_path.parent() {
                        sync_delegation_directory(parent)?;
                    }
                }
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
                Err(source) => {
                    return Err(CliError::Core(DeadreckonError::Io {
                        path: pending_path,
                        source,
                    }));
                }
            }
            append_campaign_sub_launch_event(paths, &launch, "sub_process_relaunch_safe")?;
            Ok(CampaignSubLaunchRecovery::Relaunch)
        }
        CampaignSubRecoveryDisposition::AdoptLinked => {
            let context = DRIVER_CONTEXT.get().ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "only a current Job driver can adopt a Campaign sub-process".to_string(),
                ))
            })?;
            if context.job_id != campaign_id {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "Campaign sub-process adoption requires the current fenced parent Job"
                        .to_string(),
                )));
            }
            let token = guarded_authority_lease_token(paths, &context.authority)?;
            launch.adopted = true;
            launch.adopted_by_attempt = Some(context.authority.attempt);
            launch.adopted_by_lease_epoch = Some(context.authority.lease_epoch);
            launch.adopted_at = Some(Utc::now());
            write_campaign_sub_launch_fenced(paths, &launch, &token, "adopted")?;
            append_campaign_sub_launch_event(paths, &launch, "sub_process_adopted")?;
            Ok(CampaignSubLaunchRecovery::Adopted(Box::new(
                CampaignSubProcess {
                    launch,
                    child: None,
                    prepared: None,
                },
            )))
        }
        CampaignSubRecoveryDisposition::RecoverLinkedArtifacts => {
            recover_linked_campaign_sub_artifacts(paths, &launch, identity)
        }
    }
}

pub(crate) fn spawn_campaign_sub_delegated(
    paths: &DeadreckonPaths,
    mut command: Command,
    prepared: PreparedDelegation,
) -> Result<CampaignSubProcess> {
    let Some(campaign) = prepared.campaign_sub.clone() else {
        revoke_pending_delegation(paths, &prepared)?;
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process launch requires a Campaign delegation".to_string(),
        )));
    };
    let Some(context) = DRIVER_CONTEXT.get() else {
        revoke_pending_delegation(paths, &prepared)?;
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "only an authenticated Job driver can launch a Campaign sub-process".to_string(),
        )));
    };
    if context.job_id != campaign.campaign_id {
        revoke_pending_delegation(paths, &prepared)?;
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Campaign sub-process launch requires the current fenced parent Job".to_string(),
        )));
    }
    let token = match guarded_authority_lease_token(paths, &context.authority) {
        Ok(token) => token,
        Err(error) => {
            revoke_pending_delegation(paths, &prepared)?;
            return Err(error);
        }
    };
    apply_delegation(&mut command, &prepared);
    let (mut child, terminator) = match deadreckon_core::spawn_grouped(command) {
        Ok(spawned) => spawned,
        Err(source) => {
            revoke_pending_delegation(paths, &prepared)?;
            return Err(source.into());
        }
    };
    let process = deadreckon_core::SupervisedProcess {
        pid: child.id(),
        pgid: None,
    };
    let mut supervised = match deadreckon_core::SupervisedProcessRecord::prepared(
        process,
        campaign.launch_id.clone(),
        context.authority.attempt,
        Some(context.authority.launch_id.clone()),
        deadreckon_core::flight::sha256_text(&prepared.token),
    ) {
        Ok(supervised) => supervised,
        Err(source) => {
            let _ = terminator.terminate(std::time::Duration::from_secs(2));
            revoke_pending_delegation(paths, &prepared)?;
            return Err(source.into());
        }
    };
    #[cfg(unix)]
    {
        supervised.process.pgid = Some(child.id());
    }
    supervised.phase = deadreckon_core::SupervisedProcessPhase::Running;
    let launch = CampaignSubLaunchAuthority {
        schema_version: 1,
        launch_protocol: CAMPAIGN_SUB_LAUNCH_PROTOCOL.to_string(),
        parent_job_id: context.job_id.clone(),
        campaign_id: campaign.campaign_id,
        sub_id: campaign.sub_id,
        plan_id: campaign.plan_id,
        attempt: context.authority.attempt,
        lease_epoch: context.authority.lease_epoch,
        outer_launch_id: context.authority.launch_id.clone(),
        launch_id: campaign.launch_id,
        capability_id: prepared.capability_id.clone(),
        release_token_sha256: deadreckon_core::flight::sha256_text(&prepared.token),
        process: supervised,
        released: false,
        linked: false,
        adopted: false,
        adopted_by_attempt: None,
        adopted_by_lease_epoch: None,
        prepared_at: Utc::now(),
        released_at: None,
        linked_at: None,
        adopted_at: None,
    };
    if let Err(error) = write_campaign_sub_launch_fenced(paths, &launch, &token, "prepared")
        .and_then(|()| {
            append_campaign_sub_launch_event(paths, &launch, "sub_process_launch_prepared")
        })
    {
        let _ = terminator.terminate(std::time::Duration::from_secs(2));
        if !campaign_sub_transition_is_durable(paths, &launch, "prepared").unwrap_or(false) {
            let _ = remove_campaign_sub_launch_projection_if_matches(paths, &launch);
            let _ = revoke_pending_delegation(paths, &prepared);
        }
        return Err(error);
    }
    campaign_sub_launch_test_failpoint_once(
        paths,
        &launch,
        "after_sub_process_prepare_before_release",
    )?;
    if let Err(error) = release_delegation(&mut child, &prepared) {
        let termination = terminator.terminate(std::time::Duration::from_secs(2));
        if matches!(termination, deadreckon_core::TerminationOutcome::Failed(_)) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Campaign sub-process release failed ({error}) and its exact process group could not be stopped: {termination:?}"
            ))));
        }
        // If the protected authority never advanced, the child cannot have
        // crossed dispatch. Remove both pieces of abandoned pre-authority so
        // the next attempt does not inherit an orphaned capability.
        if remove_campaign_sub_launch_projection_if_matches(paths, &launch).is_ok() {
            revoke_pending_delegation(paths, &prepared)?;
        }
        return Err(error);
    }
    Ok(CampaignSubProcess {
        launch,
        child: Some(child),
        prepared: Some(prepared),
    })
}

impl CampaignSubProcess {
    pub(crate) fn try_wait(&mut self) -> Result<CampaignSubProcessPoll> {
        if let Some(child) = self.child.as_mut() {
            let Some(status) = child.try_wait()? else {
                return Ok(CampaignSubProcessPoll::Running);
            };
            terminate_campaign_sub_process(
                &self.launch,
                std::time::Duration::from_secs(2),
                "exited residual",
            )?;
            return Ok(CampaignSubProcessPoll::Exited {
                success: Some(status.success()),
            });
        }
        match self.launch.process.identity() {
            deadreckon_core::SupervisedProcessIdentity::Current => {
                Ok(CampaignSubProcessPoll::Running)
            }
            deadreckon_core::SupervisedProcessIdentity::Exited
            | deadreckon_core::SupervisedProcessIdentity::DifferentBoot => {
                terminate_campaign_sub_process(
                    &self.launch,
                    std::time::Duration::from_secs(2),
                    "adopted residual",
                )?;
                Ok(CampaignSubProcessPoll::Exited { success: None })
            }
            deadreckon_core::SupervisedProcessIdentity::Reused
            | deadreckon_core::SupervisedProcessIdentity::Unverifiable => {
                Err(CliError::Core(DeadreckonError::InvalidInput(
                    "adopted Campaign sub-process identity became conflicting or unverifiable"
                        .to_string(),
                )))
            }
        }
    }

    pub(crate) fn revoke_pending(&self, paths: &DeadreckonPaths) -> Result<()> {
        if let Some(prepared) = self.prepared.as_ref() {
            let current = load_campaign_sub_launch(
                paths,
                &self.launch.parent_job_id,
                &self.launch.sub_id,
                &self.launch.plan_id,
            )?;
            if current.as_ref().is_some_and(|launch| launch.linked) {
                revoke_pending_delegation(paths, prepared)?;
            }
        }
        Ok(())
    }
}

fn validate_delegated_action(paths: &DeadreckonPaths, record: &DelegatedInvocation) -> Result<()> {
    match &record.action {
        DelegatedAction::PlanChild {
            plan_id,
            task_id,
            task_index,
            task_attempt,
            run_id,
        } => {
            let plan = deadreckon_core::load_plan(paths, plan_id)?;
            let owner = resolve_plan_owner(paths, &plan)?.ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "delegated child Plan is no longer Job-owned".to_string(),
                ))
            })?;
            if owner.job.job_id.as_ref() != record.job_id {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated child Plan changed Job owner".to_string(),
                )));
            }
            validate_owned_plan_lineage(paths, &owner)?;
            if record.immutable_plan_sha256.as_deref()
                != Some(immutable_plan_sha256(&plan)?.as_str())
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated child Plan definition changed after approval".to_string(),
                )));
            }
            let task = plan.tasks.get(*task_index as usize).ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "delegated child task index is absent".to_string(),
                ))
            })?;
            if task.task_id != *task_id
                || task.status != deadreckon_core::PlanTaskStatus::Running
                || task.attempts.len() as u32 + 1 != *task_attempt
                || *run_id != plan_task_run_id(&record.job_id, plan_id, task_id, *task_attempt)
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated child does not match the exact running task attempt".to_string(),
                )));
            }
            let expected_scope = canonical_invocation_path(
                &paths.plan_dir(plan_id).join("launch").join(task_id),
                "task launch scope",
            )?;
            if expected_scope != record.scope_root {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated child scope does not match its exact Plan task".to_string(),
                )));
            }
        }
        DelegatedAction::PlanFork { plan_id } | DelegatedAction::PlanMerge { plan_id } => {
            let plan = deadreckon_core::load_plan(paths, plan_id)?;
            let owner = resolve_plan_owner(paths, &plan)?.ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "delegated Plan operation is no longer Job-owned".to_string(),
                ))
            })?;
            if owner.job.job_id.as_ref() != record.job_id {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated Plan operation changed Job owner".to_string(),
                )));
            }
            validate_owned_plan_lineage(paths, &owner)?;
            if record.immutable_plan_sha256.as_deref()
                != Some(immutable_plan_sha256(&plan)?.as_str())
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated Plan definition changed after approval".to_string(),
                )));
            }
        }
        DelegatedAction::CampaignSub {
            campaign_id,
            sub_id,
            plan_id,
        } => {
            if campaign_id != &record.job_id {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated campaign does not match its durable Job".to_string(),
                )));
            }
            let campaign = deadreckon_core::campaign::read_campaign(&paths.plan_dir(campaign_id))?;
            validate_owned_campaign(paths, &campaign, &record.job_id)?;
            if record.immutable_campaign_sha256.as_deref()
                != Some(immutable_campaign_sha256(&campaign)?.as_str())
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated Campaign definition changed after approval".to_string(),
                )));
            }
            if !campaign.sub_goals.iter().any(|sub| {
                sub.sub_id == *sub_id && sub.sub_plan_id.as_deref() == Some(plan_id.as_str())
            }) {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated campaign sub-goal or reserved Plan is absent from the approved schedule"
                        .to_string(),
                )));
            }
            if std::env::var(deadreckon_core::campaign::ENV_SUB_PLAN_ID).as_deref()
                != Ok(plan_id.as_str())
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated Campaign child did not receive its exact reserved Plan identity"
                        .to_string(),
                )));
            }
        }
        DelegatedAction::MergeRepair {
            root_artifact_id,
            repair_id,
            repair_round,
            run_id,
            proof_dir,
            repair_request_sha256,
            repair_plan_sha256,
        } => {
            if root_artifact_id != &record.job_id
                || repair_id.trim().is_empty()
                || *repair_round == 0
                || Uuid::parse_str(run_id).is_err()
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated merge repair changed its durable parent identity".to_string(),
                )));
            }
            let canonical_proof = canonical_invocation_path(proof_dir, "repair proof")?;
            if canonical_proof != *proof_dir
                || canonical_invocation_path(&proof_dir.join("repair-child"), "repair child scope")?
                    != record.scope_root
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated merge repair changed its protected proof scope".to_string(),
                )));
            }
            if deadreckon_core::flight::sha256_file(&proof_dir.join("repair-request.json"))?
                != *repair_request_sha256
                || deadreckon_core::flight::sha256_file(&proof_dir.join("repair-plan.json"))?
                    != *repair_plan_sha256
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "delegated merge repair request or plan changed after authorization"
                        .to_string(),
                )));
            }
        }
    }
    Ok(())
}

fn claim_delegation_record(pending: &Path, consumed: &Path, raw: &[u8]) -> Result<()> {
    let pending_parent = pending.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "delegation pending path has no parent".to_string(),
        ))
    })?;
    let consumed_parent = consumed.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "delegation consumed path has no parent".to_string(),
        ))
    })?;
    let current = read_bounded_regular_control_file(
        pending,
        "delegated invocation pending capability record",
    )?
    .ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation pending capability disappeared before claim".to_string(),
        ))
    })?;
    if current != raw {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation pending capability changed before claim".to_string(),
        )));
    }
    fs::create_dir_all(consumed_parent)?;
    let mut tombstone = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(consumed)
    {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "delegated invocation capability was already consumed".to_string(),
            )));
        }
        Err(source) => {
            return Err(CliError::Core(DeadreckonError::Io {
                path: consumed.to_path_buf(),
                source,
            }));
        }
    };
    tombstone.write_all(raw)?;
    tombstone.sync_all()?;
    sync_delegation_directory(consumed_parent)?;
    fs::remove_file(pending)?;
    sync_delegation_directory(pending_parent)?;
    Ok(())
}

fn release_campaign_sub_invocation(
    paths: &DeadreckonPaths,
    record: &DelegatedInvocation,
    token: &deadreckon_core::LeaseToken,
) -> Result<Option<CampaignSubLaunchAuthority>> {
    let DelegatedAction::CampaignSub {
        campaign_id,
        sub_id,
        plan_id,
    } = &record.action
    else {
        return Ok(None);
    };
    let launch_id = record.campaign_sub_launch_id.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "durable Campaign delegation is missing its process launch identity".to_string(),
        ))
    })?;
    let mut launch =
        load_campaign_sub_launch(paths, campaign_id, sub_id, plan_id)?.ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "durable Campaign delegation has no protected process authority".to_string(),
            ))
        })?;
    if launch.launch_id != launch_id
        || !delegated_record_matches_campaign_launch(record, &launch)
        || launch.released
        || launch.linked
        || launch.adopted
        || launch.process.process.pid != std::process::id()
        || launch.process.identity() != deadreckon_core::SupervisedProcessIdentity::Current
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable Campaign delegation does not match its exact prepared process".to_string(),
        )));
    }
    let ack = CampaignSubReleaseAck {
        schema_version: 1,
        launch_protocol: CAMPAIGN_SUB_LAUNCH_PROTOCOL.to_string(),
        parent_job_id: launch.parent_job_id.clone(),
        campaign_id: launch.campaign_id.clone(),
        sub_id: launch.sub_id.clone(),
        plan_id: launch.plan_id.clone(),
        attempt: launch.attempt,
        lease_epoch: launch.lease_epoch,
        launch_id: launch.launch_id.clone(),
        capability_id: launch.capability_id.clone(),
        release_token_sha256: launch.release_token_sha256.clone(),
        pid: launch.process.process.pid,
        process_group: launch.process.process.pgid,
        boot_id: launch.process.boot_id.clone(),
        process_start_identity: launch.process.process_start_identity.clone(),
        acknowledged_at: Utc::now(),
    };
    if let Some(existing) = load_campaign_sub_release_ack(paths, &launch)?
        && existing != ack
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable Campaign process release conflicts with an existing acknowledgement"
                .to_string(),
        )));
    }
    write_campaign_sub_release_ack(paths, &ack)?;
    launch.released = true;
    launch.released_at = Some(Utc::now());
    write_campaign_sub_launch_fenced(paths, &launch, token, "released")?;
    append_campaign_sub_launch_event(paths, &launch, "sub_process_released")?;
    campaign_sub_launch_test_failpoint_once(
        paths,
        &launch,
        "after_sub_process_release_before_link",
    )?;
    Ok(Some(launch))
}

fn link_campaign_sub_invocation(
    paths: &DeadreckonPaths,
    mut launch: CampaignSubLaunchAuthority,
    token: &deadreckon_core::LeaseToken,
) -> Result<()> {
    if !launch.released || launch.linked {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable Campaign process crossed an invalid execution boundary".to_string(),
        )));
    }
    launch.linked = true;
    launch.linked_at = Some(Utc::now());
    write_campaign_sub_launch_fenced(paths, &launch, token, "linked")?;
    append_campaign_sub_launch_event(paths, &launch, "sub_process_linked")?;
    campaign_sub_launch_test_failpoint_once(
        paths,
        &launch,
        "after_sub_process_link_before_dispatch",
    )?;
    record_campaign_sub_dispatch_test_side_effect(&launch)
}

pub(crate) fn authorize_delegated_invocation_if_present() -> Result<bool> {
    let Some(job_id) = std::env::var_os(DELEGATION_JOB_ENV).filter(|value| !value.is_empty())
    else {
        return Ok(false);
    };
    let job_id = job_id.to_str().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "delegation Job identity is not UTF-8".to_string(),
        ))
    })?;
    let capability_id = std::env::var(DELEGATION_ID_ENV).map_err(|_| {
        CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation is missing its capability identity".to_string(),
        ))
    })?;
    Uuid::parse_str(&capability_id).map_err(|_| {
        CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation capability identity is malformed".to_string(),
        ))
    })?;
    let paths = DeadreckonPaths::discover();
    let pending = delegation_pending_path(&paths, job_id, &capability_id);
    let raw = read_bounded_regular_control_file(
        &pending,
        "delegated invocation protected capability record",
    )?
    .ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation has no pending protected capability record".to_string(),
        ))
    })?;
    let record: DelegatedInvocation = serde_json::from_slice(&raw)?;
    if record.schema_version != 1
        || record.job_id != job_id
        || record.capability_id != capability_id
        || record.authority.job_id != job_id
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation identity does not match its protected record".to_string(),
        )));
    }

    let mut token_bytes = Vec::new();
    std::io::stdin()
        .take(MAX_DELEGATION_TOKEN_BYTES + 1)
        .read_to_end(&mut token_bytes)?;
    if token_bytes.len() as u64 > MAX_DELEGATION_TOKEN_BYTES {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation token exceeded its bounded size".to_string(),
        )));
    }
    let token = std::str::from_utf8(&token_bytes)
        .map_err(|_| {
            CliError::Core(DeadreckonError::InvalidInput(
                "delegated invocation token was not UTF-8".to_string(),
            ))
        })?
        .trim();
    if token.is_empty() || deadreckon_core::flight::sha256_text(token) != record.token_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation did not receive its private one-time capability".to_string(),
        )));
    }
    if invocation_argv_sha256(std::env::args_os().skip(1))? != record.argv_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation arguments changed after authorization".to_string(),
        )));
    }
    if canonical_invocation_path(&std::env::current_dir()?, "working directory")? != record.cwd {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation working directory changed after authorization".to_string(),
        )));
    }
    let scope_root = std::env::var_os("DEADRECKON_SCOPE_ROOT")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "delegated invocation is missing its exact scope root".to_string(),
            ))
        })?;
    if canonical_invocation_path(&scope_root, "scope root")? != record.scope_root {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "delegated invocation scope changed after authorization".to_string(),
        )));
    }
    let lease_token = guarded_authority_lease_token(&paths, &record.authority)?;
    validate_delegated_action(&paths, &record)?;

    let campaign_launch = release_campaign_sub_invocation(&paths, &record, &lease_token)?;
    let consumed = delegation_consumed_path(&paths, job_id, &capability_id);
    claim_delegation_record(&pending, &consumed, &raw)?;
    if let Some(launch) = campaign_launch {
        campaign_sub_launch_test_failpoint_once(
            &paths,
            &launch,
            "after_sub_process_claim_before_link",
        )?;
        link_campaign_sub_invocation(&paths, launch, &lease_token)?;
    }
    match &record.action {
        DelegatedAction::PlanChild {
            plan_id,
            task_id,
            task_index,
            task_attempt,
            run_id,
        } => {
            DELEGATED_PLAN_AUTHORITY
                .set(record.authority.clone())
                .map_err(|_| {
                    CliError::Core(DeadreckonError::InvalidInput(
                        "this process already consumed another Plan child authority".to_string(),
                    ))
                })?;
            DELEGATED_PLAN_CHILD
                .set(deadreckon_core::RunOwnership::plan_task(
                    record.job_id.clone(),
                    plan_id.clone(),
                    task_id.clone(),
                    *task_index,
                    *task_attempt,
                ))
                .map_err(|_| {
                    CliError::Core(DeadreckonError::InvalidInput(
                        "this process already consumed another Plan child capability".to_string(),
                    ))
                })?;
            DELEGATED_CHILD_RUN_ID.set(run_id.clone()).map_err(|_| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "this process already consumed another delegated Run identity".to_string(),
                ))
            })?;
        }
        DelegatedAction::PlanFork { .. }
        | DelegatedAction::PlanMerge { .. }
        | DelegatedAction::CampaignSub { .. } => {
            install_driver_context(&paths, record.authority, false)?;
        }
        DelegatedAction::MergeRepair {
            root_artifact_id,
            repair_id,
            repair_round,
            run_id,
            proof_dir,
            repair_request_sha256,
            repair_plan_sha256,
        } => {
            DELEGATED_REPAIR_AUTHORITY
                .set(record.authority.clone())
                .map_err(|_| {
                    CliError::Core(DeadreckonError::InvalidInput(
                        "this process already consumed another repair driver authority".to_string(),
                    ))
                })?;
            DELEGATED_PLAN_CHILD
                .set(deadreckon_core::RunOwnership::merge_repair(
                    record.job_id,
                    deadreckon_core::MergeRepairOwnership {
                        root_artifact_id: root_artifact_id.clone(),
                        repair_id: repair_id.clone(),
                        repair_round: *repair_round,
                        run_id: run_id.clone(),
                        proof_dir: proof_dir.clone(),
                        repair_request_sha256: repair_request_sha256.clone(),
                        repair_plan_sha256: repair_plan_sha256.clone(),
                    },
                ))
                .map_err(|_| {
                    CliError::Core(DeadreckonError::InvalidInput(
                        "this process already consumed another merge repair capability".to_string(),
                    ))
                })?;
            DELEGATED_CHILD_RUN_ID.set(run_id.clone()).map_err(|_| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "this process already consumed another delegated Run identity".to_string(),
                ))
            })?;
        }
    }
    Ok(true)
}

pub(crate) fn require_current_driver_for_job_artifact(
    paths: &DeadreckonPaths,
    artifact_id: &str,
    expected_shape: JobShape,
    operation: &str,
) -> Result<bool> {
    let owner = if expected_shape == JobShape::Graph {
        let plan = deadreckon_core::load_plan(paths, artifact_id)?;
        resolve_plan_owner(paths, &plan)?
    } else if paths.job_json(artifact_id).is_file() {
        let job = deadreckon_core::load_job(paths, artifact_id)?;
        if job.job_id.as_ref() != artifact_id || job.shape != expected_shape {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "{operation} found artifact {artifact_id} colliding with incompatible durable Job {}",
                job.job_id
            ))));
        }
        Some(ResolvedPlanOwner {
            root_plan_id: artifact_id.to_string(),
            lineage: vec![artifact_id.to_string()],
            job,
        })
    } else {
        None
    };
    let Some(owner) = owner else {
        return Ok(false);
    };
    if owner.root_plan_id.trim().is_empty() || owner.lineage.is_empty() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "{operation} found incomplete durable Plan ownership for {artifact_id}"
        ))));
    }
    if let Some(context) = DRIVER_CONTEXT
        .get()
        .filter(|context| context.job_id == owner.job.job_id.as_ref())
    {
        if context.authority.job_id != owner.job.job_id.as_ref()
            || !commands::supervisor::guarded_driver_authority_is_live(paths, &context.authority)?
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "{operation} refused stale authority for durable Job {}",
                owner.job.job_id
            ))));
        }
        validate_owned_plan_lineage(paths, &owner)?;
        return Ok(true);
    }
    Err(CliError::Core(deadreckon_core::user_error(
        &format!(
            "{operation} cannot mutate {} because it belongs to durable Job {}",
            run_prefix(artifact_id),
            run_prefix(owner.job.job_id.as_ref())
        ),
        &format!(
            "deadreckon attach {}",
            run_prefix(owner.job.job_id.as_ref())
        ),
    )))
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedRunOwner {
    pub(crate) job: deadreckon_protocol::Job,
    owners: Vec<ResolvedPlanOwner>,
}

fn plan_task_references_run(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
    run_id: &str,
) -> Result<BTreeSet<String>> {
    let mut tasks = BTreeSet::new();
    for task in &plan.tasks {
        let recorded = task.child_run_id.as_deref() == Some(run_id)
            || task
                .attempts
                .iter()
                .any(|attempt| attempt.run_id.as_deref() == Some(run_id));
        let launched = fs::read_to_string(
            paths
                .plan_dir(&plan.plan_id)
                .join("launch")
                .join(&task.task_id)
                .join("run-id"),
        )
        .ok()
        .is_some_and(|recorded| recorded.trim() == run_id);
        if recorded || launched {
            tasks.insert(task.task_id.clone());
        }
    }
    for event in deadreckon_core::read_plan_events(paths, &plan.plan_id)? {
        let (task_id, event_run_id) = match event.event {
            deadreckon_core::PlanEventKind::TaskRunDiscovered {
                task_id, run_id, ..
            }
            | deadreckon_core::PlanEventKind::TaskCompleted {
                task_id, run_id, ..
            }
            | deadreckon_core::PlanEventKind::TaskKilled {
                task_id, run_id, ..
            } => (Some(task_id), run_id),
            deadreckon_core::PlanEventKind::TaskApplied {
                task_id, run_id, ..
            } => (Some(task_id), Some(run_id)),
            deadreckon_core::PlanEventKind::TaskRetrying {
                task_id,
                parent_run_id,
                ..
            } => (Some(task_id), parent_run_id),
            deadreckon_core::PlanEventKind::MergeRepairRunDiscovered { run_id, .. }
            | deadreckon_core::PlanEventKind::MergeCompleted {
                merged_run_id: run_id,
            } => (None, Some(run_id)),
            deadreckon_core::PlanEventKind::MergeRepaired { repair_run_id, .. } => {
                (None, repair_run_id)
            }
            _ => (None, None),
        };
        if event_run_id.as_deref() == Some(run_id) {
            tasks.insert(task_id.unwrap_or_default());
        }
    }
    if plan.merged_run_id.as_deref() == Some(run_id) {
        tasks.insert(String::new());
    }
    Ok(tasks)
}

fn stamped_run_owner(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    stamped: &deadreckon_core::RunOwnership,
) -> Result<ResolvedRunOwner> {
    if stamped.schema_version != 1 || stamped.job_id.trim().is_empty() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Run {} has incomplete durable ownership",
            state.run_id
        ))));
    }
    let job = deadreckon_core::load_job(paths, &stamped.job_id)?;
    if job.job_id.as_ref() != stamped.job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Run {} changed durable Job owner from {} to {}",
            state.run_id, stamped.job_id, job.job_id
        ))));
    }

    let mut owners = Vec::new();
    match &stamped.artifact {
        deadreckon_core::RunOwnershipArtifact::PlanTask {
            plan_id,
            task_id,
            task_index,
            task_attempt,
        } => {
            if plan_id.trim().is_empty() || task_id.trim().is_empty() || *task_attempt == 0 {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Run {} has incomplete durable Plan task ownership",
                    state.run_id
                ))));
            }
            let plan = deadreckon_core::load_plan(paths, plan_id)?;
            let owner = resolve_plan_owner(paths, &plan)?.ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Run {} names Plan {} without a durable Job owner",
                    state.run_id, plan_id
                )))
            })?;
            let task = plan
                .tasks
                .get(*task_index as usize)
                .filter(|task| task.task_id == *task_id)
                .ok_or_else(|| {
                    CliError::Core(DeadreckonError::InvalidInput(format!(
                        "Run {} no longer names an existing owned Plan task",
                        state.run_id
                    )))
                })?;
            let recorded = task.child_run_id.as_deref() == Some(state.run_id.as_str())
                || task
                    .attempts
                    .iter()
                    .any(|attempt| attempt.run_id.as_deref() == Some(state.run_id.as_str()));
            let awaiting_parent_record = task.status == deadreckon_core::PlanTaskStatus::Running
                && *task_attempt == task.attempts.len() as u32 + 1;
            if !recorded && !awaiting_parent_record {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Run {} ownership is not retained by Plan task {}",
                    state.run_id, task_id
                ))));
            }
            if owner.job.job_id != job.job_id {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Run {} Plan owner does not match durable Job {}",
                    state.run_id, job.job_id
                ))));
            }
            owners.push(owner);
        }
        deadreckon_core::RunOwnershipArtifact::PlanResult { plan_id } => {
            let plan = deadreckon_core::load_plan(paths, plan_id)?;
            let owner = resolve_plan_owner(paths, &plan)?.ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Run {} names Plan {} without a durable Job owner",
                    state.run_id, plan_id
                )))
            })?;
            if owner.job.job_id != job.job_id
                || plan
                    .merged_run_id
                    .as_deref()
                    .is_some_and(|run_id| run_id != state.run_id)
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Run {} does not match its durable Plan result authority",
                    state.run_id
                ))));
            }
            owners.push(owner);
        }
        deadreckon_core::RunOwnershipArtifact::CampaignResult { campaign_id } => {
            let campaign = deadreckon_core::campaign::read_campaign(&paths.plan_dir(campaign_id))?;
            if job.shape != JobShape::LegacyCampaign
                || campaign.campaign_id != *campaign_id
                || campaign.campaign_id != job.job_id.as_ref()
                || campaign
                    .merged_run_id
                    .as_deref()
                    .is_some_and(|run_id| run_id != state.run_id)
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Run {} does not match its durable Campaign result authority",
                    state.run_id
                ))));
            }
        }
        deadreckon_core::RunOwnershipArtifact::MergeRepair {
            root_artifact_id,
            repair_id,
            repair_round,
            run_id,
            proof_dir,
            repair_request_sha256,
            repair_plan_sha256,
        } => {
            let repair_record_path = proof_dir.join("repair-run.json");
            let repair_record: serde_json::Value =
                serde_json::from_slice(&fs::read(&repair_record_path)?).map_err(|source| {
                    CliError::Core(DeadreckonError::Json {
                        path: repair_record_path.clone(),
                        source,
                    })
                })?;
            if root_artifact_id != job.job_id.as_ref()
                || repair_id.trim().is_empty()
                || *repair_round == 0
                || run_id != &state.run_id
                || repair_record
                    .get("run_id")
                    .and_then(serde_json::Value::as_str)
                    != Some(state.run_id.as_str())
                || repair_record
                    .get("repair_id")
                    .and_then(serde_json::Value::as_str)
                    != Some(repair_id.as_str())
                || repair_record
                    .get("repair_round")
                    .and_then(serde_json::Value::as_u64)
                    != Some(u64::from(*repair_round))
                || deadreckon_core::flight::sha256_file(&proof_dir.join("repair-request.json"))?
                    != *repair_request_sha256
                || deadreckon_core::flight::sha256_file(&proof_dir.join("repair-plan.json"))?
                    != *repair_plan_sha256
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Run {} does not match its immutable merge repair authority",
                    state.run_id
                ))));
            }
        }
        deadreckon_core::RunOwnershipArtifact::ParentResult => {
            if state.run_id != job.job_id.as_ref()
                || !matches!(job.shape, JobShape::Graph | JobShape::LegacyCampaign)
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Run {} does not match its durable parent result authority",
                    state.run_id
                ))));
            }
        }
    }
    Ok(ResolvedRunOwner { job, owners })
}

pub(crate) fn resolve_run_owner(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
) -> Result<Option<ResolvedRunOwner>> {
    if let Some(stamped) = state.ownership.as_ref() {
        return stamped_run_owner(paths, state, stamped).map(Some);
    }

    let mut matches = Vec::new();
    for plan_id in commands::reference::plan_ids_matching(paths, "")? {
        let Ok(plan) = deadreckon_core::load_plan(paths, &plan_id) else {
            continue;
        };
        let Some(owner) = resolve_plan_owner(paths, &plan)? else {
            continue;
        };
        let task_ids = plan_task_references_run(paths, &plan, &state.run_id)?;
        if task_ids.is_empty() {
            continue;
        }
        matches.push(owner);
    }

    let mut campaign_owner = None;
    if paths.plans_dir().is_dir() {
        for entry in fs::read_dir(paths.plans_dir())?.filter_map(std::result::Result::ok) {
            let campaign_dir = entry.path();
            if !deadreckon_core::campaign::campaign_path_for_plan_dir(&campaign_dir).is_file() {
                continue;
            }
            let Ok(campaign) = deadreckon_core::campaign::read_campaign(&campaign_dir) else {
                continue;
            };
            if campaign.merged_run_id.as_deref() != Some(state.run_id.as_str())
                || !paths.job_json(&campaign.campaign_id).is_file()
            {
                continue;
            }
            let job = deadreckon_core::load_job(paths, &campaign.campaign_id)?;
            if job.shape != JobShape::LegacyCampaign || job.job_id.as_ref() != campaign.campaign_id
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Run {} is claimed by an incompatible durable Campaign",
                    state.run_id
                ))));
            }
            if campaign_owner
                .as_ref()
                .is_some_and(|owner: &deadreckon_protocol::Job| owner.job_id != job.job_id)
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Run {} is claimed by more than one durable Campaign",
                    state.run_id
                ))));
            }
            campaign_owner = Some(job);
        }
    }

    let Some(first_job) = matches
        .first()
        .map(|owner| owner.job.clone())
        .or(campaign_owner.clone())
    else {
        return Ok(None);
    };
    if matches
        .iter()
        .any(|owner| owner.job.job_id != first_job.job_id)
        || campaign_owner
            .as_ref()
            .is_some_and(|owner| owner.job_id != first_job.job_id)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Run {} is claimed by more than one durable Job",
            state.run_id
        ))));
    }
    Ok(Some(ResolvedRunOwner {
        job: first_job,
        owners: matches,
    }))
}

pub(crate) fn require_current_driver_for_job_owned_run(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
    operation: &str,
) -> Result<()> {
    let Some(owner) = resolve_run_owner(paths, state)? else {
        return Ok(());
    };
    if let Some(context) = DRIVER_CONTEXT
        .get()
        .filter(|context| context.job_id == owner.job.job_id.as_ref())
    {
        if context.authority.job_id != owner.job.job_id.as_ref()
            || !commands::supervisor::guarded_driver_authority_is_live(paths, &context.authority)?
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "{operation} refused stale authority for durable Job {}",
                owner.job.job_id
            ))));
        }
        for plan_owner in &owner.owners {
            validate_owned_plan_lineage(paths, plan_owner)?;
        }
        return Ok(());
    }
    Err(CliError::Core(deadreckon_core::user_error(
        &format!(
            "{operation} cannot mutate {} because it belongs to durable Job {}",
            run_prefix(&state.run_id),
            run_prefix(owner.job.job_id.as_ref())
        ),
        &format!(
            "deadreckon attach {}",
            run_prefix(owner.job.job_id.as_ref())
        ),
    )))
}

pub(crate) fn record_current_artifact(
    paths: &DeadreckonPaths,
    kind: DriverKind,
    artifact_kind: &str,
    artifact_id: &str,
) -> Result<()> {
    let Some(context) = DRIVER_CONTEXT.get() else {
        return Ok(());
    };
    if !context.root_artifact {
        return Ok(());
    }
    let job_id = context.job_id.as_str();
    if job_id != artifact_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "advanced driver artifact {artifact_id} does not retain parent job identity {job_id}"
        ))));
    }
    write_driver_state(paths, job_id, kind, artifact_kind, artifact_id)
}

fn write_driver_state(
    paths: &DeadreckonPaths,
    job_id: &str,
    kind: DriverKind,
    artifact_kind: &str,
    artifact_id: &str,
) -> Result<()> {
    if job_id != artifact_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "advanced artifact {artifact_id} does not retain parent Job identity {job_id}"
        ))));
    }
    let state = DriverState {
        schema_version: 1,
        job_id: JobId(job_id.to_string()),
        kind,
        artifact_kind: artifact_kind.to_string(),
        artifact_id: artifact_id.to_string(),
        recorded_at: Utc::now(),
    };
    let path = driver_state_path(paths, job_id);
    if path.exists() {
        let existing = load_driver_state(paths, job_id)?;
        if existing.job_id != state.job_id
            || existing.kind != state.kind
            || existing.artifact_kind != state.artifact_kind
            || existing.artifact_id != state.artifact_id
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "advanced artifact mapping for job {job_id} changed across attempts"
            ))));
        }
        return Ok(());
    }
    commands::job::write_json_synced(&path, &state)?;
    Ok(())
}

fn validate_root_planner_accounting(
    accounting: &deadreckon_core::plan::RootPlannerAccounting,
) -> Result<()> {
    let has_identity = accounting
        .provider
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        && accounting
            .model
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty());
    let numeric_valid = accounting.cost_usd.is_finite()
        && accounting.cost_usd >= 0.0
        && accounting.wall_seconds.is_finite()
        && accounting.wall_seconds >= 0.0;
    let empty_snapshot = accounting.provider.is_none()
        && accounting.model.is_none()
        && accounting.input_tokens == 0
        && accounting.output_tokens == 0
        && accounting.cost_usd == 0.0
        && !accounting.subscription
        && accounting.wall_seconds == 0.0;
    if accounting.schema_version != 1
        || !numeric_valid
        || (accounting.planner_invoked && !has_identity)
        || (!accounting.planner_invoked && !empty_snapshot)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "root planner accounting snapshot is malformed or internally inconsistent".to_string(),
        )));
    }
    Ok(())
}

pub(crate) fn root_planner_budget_exhaustion(
    accounting: &deadreckon_core::plan::RootPlannerAccounting,
    max_spend_usd: f64,
    max_wall_seconds: f64,
) -> Result<Option<RootPlannerBudgetExhaustion>> {
    validate_root_planner_accounting(accounting)?;
    if accounting.cost_usd >= max_spend_usd {
        return Ok(Some(RootPlannerBudgetExhaustion {
            dimension: deadreckon_core::plan::BudgetDimension::Spend,
            stop_reason: StopReason::SpendCap,
            reason: format!(
                "root planner exhausted the approved spend cap before child launch (${:.6} used of ${max_spend_usd:.6})",
                accounting.cost_usd
            ),
        }));
    }
    if accounting.wall_seconds >= max_wall_seconds {
        return Ok(Some(RootPlannerBudgetExhaustion {
            dimension: deadreckon_core::plan::BudgetDimension::Wall,
            stop_reason: StopReason::WallCap,
            reason: format!(
                "root planner exhausted the approved wall-time cap before child launch ({:.3}s used of {max_wall_seconds:.3}s)",
                accounting.wall_seconds
            ),
        }));
    }
    Ok(None)
}

fn restore_plan_planner_accounting(
    paths: &DeadreckonPaths,
    plan_id: &str,
    accounting: &deadreckon_core::plan::RootPlannerAccounting,
) -> Result<()> {
    validate_root_planner_accounting(accounting)?;
    let path = paths.plan_dir(plan_id).join(PLAN_PLANNER_ACCOUNTING_FILE);
    if path.is_file() {
        let existing = load_plan_planner_accounting(paths, plan_id)?;
        if existing != *accounting {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "Plan {plan_id} root planner accounting disagrees with its crash-safe snapshot"
            ))));
        }
        return Ok(());
    }
    record_plan_planner_accounting_snapshot(paths, plan_id, accounting)
}

fn validated_driver_spec_for_recovery(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
) -> Result<DriverSpec> {
    let launch_path = paths.job_launch_plan(job.job_id.as_ref());
    if deadreckon_core::flight::sha256_file(&launch_path)? != job.launch_plan_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "advanced Job launch plan changed before artifact recovery".to_string(),
        )));
    }
    let authority_path = paths.job_authority(job.job_id.as_ref());
    if deadreckon_core::flight::sha256_file(&authority_path)? != job.authority_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "advanced Job authority changed before artifact recovery".to_string(),
        )));
    }
    let launch = commands::course::load_launch_plan(&launch_path)?;
    driver_spec(&launch)
}

/// Repair only the mapping side of a crash-partial advanced root creation.
///
/// The root ID is deterministic: it is the durable Job ID. Recovery therefore
/// validates that exact pending artifact and reconstructs its derived
/// ownership/accounting/mapping records. It never asks a planner to create a
/// replacement and never invents zero planner usage.
pub(crate) fn recover_pending_driver_state(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    token: &deadreckon_core::LeaseToken,
) -> Result<PendingDriverRecovery> {
    if token.job_id != job.job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "advanced recovery token belongs to {}, not Job {}",
            token.job_id, job.job_id
        ))));
    }
    if !matches!(job.shape, JobShape::Graph | JobShape::LegacyCampaign) {
        return Ok(PendingDriverRecovery::Unchanged);
    }
    let view = deadreckon_core::JobView::load(paths, job.job_id.as_ref())?;
    if view.projection.is_terminal() || view.projection.attempt_count == 0 {
        return Ok(PendingDriverRecovery::Unchanged);
    }
    let mapping_exists = driver_state_path(paths, job.job_id.as_ref()).exists();
    let driver = validated_driver_spec_for_recovery(paths, job)?;
    let authority: deadreckon_protocol::JobAuthority =
        serde_json::from_slice(&fs::read(paths.job_authority(job.job_id.as_ref()))?)?;
    match job.shape {
        JobShape::Graph => {
            if !paths.plan_json(job.job_id.as_ref()).is_file() {
                if mapping_exists {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                        "Graph Job {} has an artifact mapping but no root Plan",
                        job.job_id
                    ))));
                }
                return Ok(PendingDriverRecovery::Unchanged);
            }
            let mut plan = deadreckon_core::plan::load_plan(paths, job.job_id.as_ref())?;
            let expected_kind = match plan.mode {
                deadreckon_core::plan::PlanMode::Review => DriverKind::Review,
                deadreckon_core::plan::PlanMode::FullPlan => DriverKind::FullPlan,
            };
            let expected_parent_cwd = expected_plan_parent_cwd(paths, job, &authority, &driver)?;
            if plan.plan_id != job.job_id.as_ref()
                || plan.owner_job_id.as_deref() != Some(job.job_id.as_ref())
                || plan.parent_plan_id.is_some()
                || plan.root_goal != job.goal
                || plan.parent_scope.as_deref() != Some(job.scope.as_str())
                || plan.parent_cwd.as_deref() != Some(expected_parent_cwd.as_path())
                || plan.acceptance_path.as_deref()
                    != Some(
                        commands::job::job_acceptance_path(paths, job.job_id.as_ref()).as_path(),
                    )
                || driver.kind != expected_kind
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "pending Plan {} does not match durable Graph Job {}",
                    plan.plan_id, job.job_id
                ))));
            }
            reconcile_plan_task_run_links(paths, token, &plan)?;
            plan = deadreckon_core::plan::load_plan(paths, job.job_id.as_ref())?;
            if plan.status != deadreckon_core::plan::PlanStatus::Pending {
                if mapping_exists {
                    return Ok(PendingDriverRecovery::Unchanged);
                }
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "unmapped root Plan {} is no longer pending",
                    plan.plan_id
                ))));
            }
            let accounting = plan.root_planner_accounting.as_ref().ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "pending Plan {} has no crash-safe root planner accounting",
                    plan.plan_id
                )))
            })?;
            restore_plan_planner_accounting(paths, &plan.plan_id, accounting)?;
            record_owned_plan_tree(paths, &plan)?;
            write_driver_state(
                paths,
                job.job_id.as_ref(),
                expected_kind,
                "plan",
                &plan.plan_id,
            )?;
            if let Some(exhaustion) = root_planner_budget_exhaustion(
                accounting,
                job.policy.max_spend_usd,
                job.policy.max_wall_seconds as f64,
            )? {
                if !deadreckon_core::read_plan_events(paths, &plan.plan_id)?
                    .iter()
                    .any(|event| {
                        matches!(
                            event.event,
                            deadreckon_core::PlanEventKind::RootBudgetExhausted { .. }
                        )
                    })
                {
                    deadreckon_core::append_owned_plan_event_fenced(
                        paths,
                        token,
                        &plan.plan_id,
                        deadreckon_core::PlanEventKind::RootBudgetExhausted {
                            dimension: exhaustion.dimension,
                            reason: exhaustion.reason.clone(),
                        },
                    )?;
                }
                plan.status = deadreckon_core::plan::PlanStatus::Failed;
                deadreckon_core::save_owned_plan_fenced(paths, token, &plan)?;
                return Ok(PendingDriverRecovery::BudgetExhausted {
                    stop_reason: exhaustion.stop_reason,
                    reason: exhaustion.reason,
                });
            }
            Ok(if mapping_exists {
                PendingDriverRecovery::Unchanged
            } else {
                PendingDriverRecovery::Recovered
            })
        }
        JobShape::LegacyCampaign => {
            let campaign_dir = paths.plan_dir(job.job_id.as_ref());
            if !campaign_dir.join("campaign.json").is_file() {
                if mapping_exists {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                        "Campaign Job {} has an artifact mapping but no root Campaign",
                        job.job_id
                    ))));
                }
                return Ok(PendingDriverRecovery::Unchanged);
            }
            let mut campaign = deadreckon_core::campaign::read_campaign(&campaign_dir)?;
            if driver.kind != DriverKind::Campaign
                || campaign.campaign_id != job.job_id.as_ref()
                || campaign.root_goal != job.goal
                || campaign.depth != 0
                || driver
                    .child_count
                    .is_some_and(|count| u32::from(count) != campaign.n)
            {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "pending Campaign {} does not match durable Campaign Job {}",
                    campaign.campaign_id, job.job_id
                ))));
            }
            if campaign.status != deadreckon_core::campaign::CampaignStatus::Pending {
                if mapping_exists {
                    return Ok(PendingDriverRecovery::Unchanged);
                }
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "unmapped root Campaign {} is no longer pending",
                    campaign.campaign_id
                ))));
            }
            let accounting = campaign.root_planner_accounting.as_ref().ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "pending Campaign {} has no crash-safe root planner accounting",
                    campaign.campaign_id
                )))
            })?;
            validate_root_planner_accounting(accounting)?;
            commands::campaign::restore_campaign_planner_accounting_snapshot(
                &campaign_dir,
                accounting,
            )?;
            write_owned_campaign_record(paths, job.job_id.as_ref(), &campaign)?;
            write_driver_state(
                paths,
                job.job_id.as_ref(),
                DriverKind::Campaign,
                "campaign",
                &campaign.campaign_id,
            )?;
            if let Some(exhaustion) = root_planner_budget_exhaustion(
                accounting,
                job.policy.max_spend_usd,
                job.policy.max_wall_seconds as f64,
            )? {
                if !deadreckon_core::campaign::read_campaign_events(&campaign_dir)?
                    .iter()
                    .any(|event| {
                        event.kind == "budget_exhausted"
                            && event
                                .detail
                                .get("phase")
                                .and_then(serde_json::Value::as_str)
                                == Some("root_planner")
                    })
                {
                    deadreckon_core::campaign::append_campaign_event(
                        &campaign_dir,
                        "budget_exhausted",
                        json!({
                            "reason": exhaustion.reason,
                            "phase": "root_planner",
                            "child_launches": 0,
                            "stop_reason": exhaustion.stop_reason,
                        }),
                    )?;
                }
                campaign.status = deadreckon_core::campaign::CampaignStatus::Failed;
                deadreckon_core::campaign::write_campaign(&campaign_dir, &campaign)?;
                return Ok(PendingDriverRecovery::BudgetExhausted {
                    stop_reason: exhaustion.stop_reason,
                    reason: exhaustion.reason,
                });
            }
            Ok(if mapping_exists {
                PendingDriverRecovery::Unchanged
            } else {
                PendingDriverRecovery::Recovered
            })
        }
        JobShape::Single | JobShape::LegacyChain => Ok(PendingDriverRecovery::Unchanged),
    }
}

pub(crate) async fn drive_job_command(job_id: String) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let guarded = commands::supervisor::require_guarded_driver_launch(&paths, &job_id)?;
    let job = deadreckon_core::load_job(&paths, &job_id)?;
    if job.job_id.as_ref() != job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "advanced driver job identity mismatch".to_string(),
        )));
    }
    let plan = commands::course::load_launch_plan(&paths.job_launch_plan(&job_id))?;
    let driver = driver_spec(&plan)?;
    install_driver_context(&paths, guarded.clone(), true)?;
    std::env::set_current_dir(&job.source_cwd)?;

    match driver.kind {
        DriverKind::Review | DriverKind::FullPlan => {
            if job.shape != JobShape::Graph {
                return Err(driver_shape_error(&job_id));
            }
            if load_parent_repair_intent(&paths, &job_id)?.is_some() {
                return run_pending_plan_parent_repair(&paths, &job, &driver, &guarded).await;
            }
            drive_plan(&paths, &job, plan, driver).await
        }
        DriverKind::Campaign => {
            if job.shape != JobShape::LegacyCampaign {
                return Err(driver_shape_error(&job_id));
            }
            if load_parent_repair_intent(&paths, &job_id)?.is_some() {
                return run_pending_campaign_parent_repair(&paths, &job, &driver, &guarded).await;
            }
            drive_campaign(&paths, &job, driver).await
        }
    }
}

async fn drive_plan(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    plan: commands::course::LaunchPlan,
    driver: DriverSpec,
) -> Result<()> {
    let authority: deadreckon_protocol::JobAuthority =
        serde_json::from_slice(&fs::read(paths.job_authority(job.job_id.as_ref()))?)?;
    let execution_cwd = if driver.apply == deadreckon_core::plan::ApplyWhen::PerNode {
        let token = current_driver_lease_token(paths, job.job_id.as_ref())?;
        prepare_ordered_candidate(paths, job, &authority, &token)?
    } else {
        job.source_cwd.clone()
    };
    std::env::set_current_dir(&execution_cwd)?;
    let execution = commands::orchestrate::durable_orchestration_spec(&plan)?.unwrap_or_default();
    if driver_state_path(paths, job.job_id.as_ref()).is_file() {
        return resume_plan(paths, job, &authority, driver, execution).await;
    }
    let mode = match driver.kind {
        DriverKind::Review => CliPlanMode::Review,
        DriverKind::FullPlan => CliPlanMode::FullPlan,
        DriverKind::Campaign => unreachable!("campaign has its own driver"),
    };
    let n = match mode {
        CliPlanMode::Review => 2,
        CliPlanMode::FullPlan => driver.child_count.unwrap_or(3),
    };
    commands::orchestrate::orchestrate_command(commands::orchestrate::OrchestrateRunArgs {
        seed_pieces: plan.pieces,
        accepted_launch_plan: None,
        deadline: job.policy.deadline,
        plan: PlanCommandArgs {
            goal: job.goal.clone(),
            n,
            mode,
            apply: driver.apply,
            max_spend: Some(job.policy.max_spend_usd),
            max_wall_seconds: Some(job.policy.max_wall_seconds as f64),
            sandbox: Some(authority.sandbox_requested),
            planner_provider: driver.planner_provider,
            provider: driver.child_provider,
            child_provider: driver.child_provider_overrides,
            coder_provider: driver.coder_provider,
            reviewer_provider: driver.reviewer_provider,
            planner_model: driver.planner_model.or(execution.planner_model),
            model: driver.child_model.or(driver.model.clone()),
            child_model: if driver.child_model_overrides.is_empty() {
                execution.child_models
            } else {
                driver.child_model_overrides
            },
            coder_model: driver
                .coder_model
                .or(execution.coder_model)
                .or_else(|| driver.model.clone()),
            reviewer_model: driver
                .reviewer_model
                .or(execution.reviewer_model)
                .or(driver.model),
            init_git: driver.source_init_git,
            acceptance: Some(commands::job::job_acceptance_path(
                paths,
                job.job_id.as_ref(),
            )),
            skip_acceptance_prompt: true,
            no_hints: true,
            quiet: true,
            json: false,
            plain: true,
        },
        preview: false,
        yes: true,
        no_repair: execution.no_repair,
        completion_surface: false,
        narrate: execution.narrate,
        narrator_model: execution.narrator_model,
    })
    .await
}

async fn resume_plan(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    authority: &deadreckon_protocol::JobAuthority,
    driver: DriverSpec,
    execution: commands::orchestrate::DurableOrchestrationSpec,
) -> Result<()> {
    use deadreckon_core::plan::PlanStatus;

    let plan = deadreckon_core::plan::load_plan(paths, job.job_id.as_ref())?;
    match plan.status {
        PlanStatus::Merged => return Ok(()),
        PlanStatus::Failed => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "graph job {} has a failed plan and cannot be resumed automatically",
                job.job_id
            ))));
        }
        PlanStatus::Pending | PlanStatus::Forked => {}
    }
    let all_children_completed = plan
        .tasks
        .iter()
        .all(|task| task.status.is_successful_terminal());
    if !all_children_completed || commands::plan::plan_requires_durable_resume(paths, &plan)? {
        commands::plan::fork_command(ForkCommandArgs {
            plan_id: job.job_id.as_ref().to_string(),
            max_spend: Some(job.policy.max_spend_usd),
            max_wall_seconds: Some(job.policy.max_wall_seconds as f64),
            deadline: job.policy.deadline,
            sandbox: Some(authority.sandbox_requested.clone()),
            provider: driver.child_provider,
            child_provider: driver.child_provider_overrides,
            coder_provider: driver.coder_provider,
            reviewer_provider: driver.reviewer_provider,
            no_repair: execution.no_repair,
            repair_provider: None,
            yes: true,
            no_hints: true,
            quiet: true,
            plain: true,
            completion_surface: false,
            narrate: execution.narrate,
            narrator_model: execution.narrator_model.clone(),
        })
        .await?;
    }
    commands::merge::merge_command(MergeCommandArgs {
        plan_id: job.job_id.as_ref().to_string(),
        strategy: "dag-aware".to_string(),
        prefer_child: None,
        no_repair: execution.no_repair,
        repair_provider: None,
        repair_mode: "auto".to_string(),
        repair_attempts: 1,
        yes: true,
        no_gate: false,
        no_hints: true,
        quiet: true,
        plain: true,
        completion_surface: false,
    })
    .await
}

async fn drive_campaign(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    driver: DriverSpec,
) -> Result<()> {
    let authority: deadreckon_protocol::JobAuthority =
        serde_json::from_slice(&fs::read(paths.job_authority(job.job_id.as_ref()))?)?;
    let args = commands::campaign::CampaignArgs {
        goal: job.goal.clone(),
        n: driver.child_count,
        planner_provider: driver.planner_provider,
        provider: driver.child_provider,
        planner_model: driver.planner_model,
        model: driver.child_model.or(driver.model),
        max_spend: Some(job.policy.max_spend_usd),
        max_wall_seconds: Some(job.policy.max_wall_seconds as f64),
        deadline: job.policy.deadline,
        sandbox: Some(authority.sandbox_requested),
        acceptance: Some(commands::job::job_acceptance_path(
            paths,
            job.job_id.as_ref(),
        )),
        preview: false,
        yes: true,
        no_hints: true,
        quiet: true,
        plain: true,
        narrate: false,
        no_narrate: true,
        narrator_model: None,
    };
    if driver_state_path(paths, job.job_id.as_ref()).is_file() {
        commands::campaign::resume_campaign_job(paths, job, &args).await
    } else {
        commands::campaign::campaign_command(args).await
    }
}

async fn run_pending_plan_parent_repair(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    driver: &DriverSpec,
    guarded: &commands::supervisor::GuardedDriverAuthority,
) -> Result<()> {
    let plan = deadreckon_core::plan::load_plan(paths, job.job_id.as_ref())?;
    if plan.status != deadreckon_core::plan::PlanStatus::Merged {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "graph parent repair requires the existing merged Plan {}",
            job.job_id
        ))));
    }
    let owner = resolve_plan_owner(paths, &plan)?.ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "graph parent repair lost its protected Plan owner".to_string(),
        ))
    })?;
    validate_owned_plan_lineage(paths, &owner)?;
    let merged_run_id = plan.merged_run_id.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "graph parent repair has no immutable merged baseline".to_string(),
        ))
    })?;
    let merged = deadreckon_core::load_run(paths, merged_run_id)?;
    run_pending_parent_repair(
        paths,
        job,
        driver,
        guarded,
        merged_run_id,
        &merged,
        &plan.providers,
        plan_execution_usage(paths, &plan)?,
    )
    .await
}

async fn run_pending_campaign_parent_repair(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    driver: &DriverSpec,
    guarded: &commands::supervisor::GuardedDriverAuthority,
) -> Result<()> {
    let campaign_dir = paths.plan_dir(job.job_id.as_ref());
    let campaign = deadreckon_core::campaign::read_campaign(&campaign_dir)?;
    if campaign.status != deadreckon_core::campaign::CampaignStatus::Merged {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "campaign parent repair requires the existing merged Campaign {}",
            job.job_id
        ))));
    }
    validate_owned_campaign(paths, &campaign, job.job_id.as_ref())?;
    let merged_run_id = campaign.merged_run_id.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "campaign parent repair has no immutable merged baseline".to_string(),
        ))
    })?;
    let merged = deadreckon_core::load_run(paths, merged_run_id)?;
    validate_campaign_rollup(paths, &campaign, &merged)?;
    run_pending_parent_repair(
        paths,
        job,
        driver,
        guarded,
        merged_run_id,
        &merged,
        &campaign.providers,
        campaign_execution_usage(paths, &campaign)?,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_pending_parent_repair(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    _driver: &DriverSpec,
    guarded: &commands::supervisor::GuardedDriverAuthority,
    merged_run_id: &str,
    merged: &deadreckon_core::PipelineState,
    providers: &deadreckon_core::plan::PlanProviders,
    execution_usage: ParentExecutionUsage,
) -> Result<()> {
    if !commands::supervisor::guarded_driver_authority_is_live(paths, guarded)? {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent repair refused stale fenced driver authority".to_string(),
        )));
    }
    let intent = load_parent_repair_intent(paths, job.job_id.as_ref())?.ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "parent repair request disappeared before its guarded attempt".to_string(),
        ))
    })?;
    validate_parent_repair_intent_lineage(
        paths,
        job,
        &parent_repair_intent_path(paths, job.job_id.as_ref()),
        &intent,
        None,
    )?;
    if intent.shape != job.shape
        || intent.merged_run_id != merged_run_id
        || intent.merged_tree_sha256 != parent_tree_sha256(merged)?
        || guarded.attempt <= intent.requested_after_attempt
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent repair request does not match the frozen merged result or bounded attempt"
                .to_string(),
        )));
    }
    let mut parent = deadreckon_core::load_run(paths, job.job_id.as_ref())?;
    validate_parent_identity(job, &parent)?;
    let candidate_path =
        deadreckon_core::parent_repair_candidate_path_for_run_root(&parent.run_root);
    if candidate_path.is_file() {
        validate_parent_repair_candidate(paths, job, &parent, &intent)?;
        return Ok(());
    }
    let manifest_path = deadreckon_core::parent_repair_manifest_path_for_run_root(&parent.run_root);
    let previous_manifest = load_parent_repair_manifest(&manifest_path)?;
    if previous_manifest.is_none() && parent_tree_sha256(&parent)? != intent.pre_repair_tree_sha256
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent result changed before the first authorized repair attempt".to_string(),
        )));
    }
    if let Some(previous) = previous_manifest.as_ref() {
        validate_parent_repair_manifest(job, &intent, previous)?;
        validate_parent_repair_manifest_history(paths, job, previous)?;
    }
    let intent_path = parent_repair_intent_path(paths, job.job_id.as_ref());
    let intent_sha256 = deadreckon_core::flight::sha256_file(&intent_path)?;
    let reuse_manifest = previous_manifest.as_ref().is_some_and(|manifest| {
        manifest.attempt == guarded.attempt
            && manifest.launch_id == guarded.launch_id
            && manifest.lease_epoch == guarded.lease_epoch
            && manifest.intent_sha256 == intent_sha256
    });
    if !reuse_manifest {
        if let Some(previous) = previous_manifest.as_ref() {
            archive_active_repair_file(
                &parent,
                intent.round,
                &manifest_path,
                &format!("attempt-{}.json", previous.attempt),
            )?;
        }
        remove_if_exists(&candidate_path)?;
        let manifest = ParentRepairAttemptManifest {
            schema_version: 1,
            job_id: job.job_id.as_ref().to_string(),
            shape: job.shape,
            round: intent.round,
            merged_run_id: merged_run_id.to_string(),
            merged_tree_sha256: intent.merged_tree_sha256.clone(),
            pre_repair_tree_sha256: intent.pre_repair_tree_sha256.clone(),
            intent_sha256: intent_sha256.clone(),
            attempt: guarded.attempt,
            launch_id: guarded.launch_id.clone(),
            lease_epoch: guarded.lease_epoch,
            attempt_baseline_tree_sha256: parent_tree_sha256(&parent)?,
            started_at: Utc::now(),
        };
        commands::job::replace_json_synced(&manifest_path, &manifest)?;
    }
    let manifest_sha256 = deadreckon_core::flight::sha256_file(&manifest_path)?;

    let parent_budget_spend = job.policy.max_spend_usd - execution_usage.spend_usd;
    let parent_budget_wall = job.policy.max_wall_seconds as f64 - execution_usage.wall_seconds;
    if parent_budget_spend <= parent.total_spend_usd
        || parent_budget_wall <= parent.total_wall_seconds
    {
        let reason = if parent_budget_spend <= parent.total_spend_usd {
            "approved aggregate spend cap was exhausted before parent repair"
        } else {
            "approved aggregate wall-time cap was exhausted before parent repair"
        };
        parent.pause_reason = Some(reason.to_string());
        parent.failure_reason = Some(reason.to_string());
        parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
        deadreckon_core::save_state(&parent)?;
        return Ok(());
    }
    let (provider, model) = repair_provider_selection(providers);
    if provider != intent.provider || model != intent.model {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent repair provider selection changed after the revision request".to_string(),
        )));
    }
    parent.provider = provider;
    parent.max_spend_usd = Some(parent_budget_spend);
    parent.max_wall_seconds = Some(parent_budget_wall);
    parent.sandbox = job
        .policy
        .execution
        .as_ref()
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "parent repair requires immutable execution policy".to_string(),
            ))
        })?
        .sandbox_requested
        .clone();
    deadreckon_core::save_state(&parent)?;
    super::super::resume_parent_repair_command(
        paths,
        parent,
        model.as_deref(),
        deadreckon_runtime::ParentRepairCandidateContext {
            path: candidate_path,
            job_id: job.job_id.as_ref().to_string(),
            round: intent.round,
            attempt: guarded.attempt,
            launch_id: guarded.launch_id.clone(),
            lease_epoch: guarded.lease_epoch,
            intent_sha256,
            manifest_sha256,
            feedback: intent.feedback,
        },
    )
    .await
}

/// Verify a merged graph as the parent Job's result.
///
/// The orchestration merge run is a compatibility artifact with its own
/// random identity and a synthetic marker. It is useful evidence, but it is
/// not completion authority. This function copies that actual merged result
/// into a run whose identity is the durable Job, executes the native gate
/// there, asks a fresh read-only semantic judge, and seals with the existing
/// two-key receipt implementation.
pub(crate) async fn complete_merged_plan_parent(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    authority: &deadreckon_protocol::JobAuthority,
    plan: &deadreckon_core::plan::Plan,
) -> Result<ParentCompletion> {
    let merged_run_id = plan.merged_run_id.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "merged graph {} has no evidence-bearing result run",
            job.job_id
        )))
    })?;
    let merged = deadreckon_core::load_run(paths, merged_run_id)?;
    if merged.status != deadreckon_core::RunStatus::Completed {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "merged graph result {merged_run_id} is not completed"
        ))));
    }
    let owner = resolve_plan_owner(paths, plan)?.ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "merged graph lost its protected durable owner".to_string(),
        ))
    })?;
    validate_owned_plan_lineage(paths, &owner)?;
    validate_durable_chain_completion_evidence(paths, job, authority, plan)?;
    if let Ok(mut existing) = deadreckon_core::load_run(paths, job.job_id.as_ref()) {
        if deadreckon_core::cancel_marker_present(&existing) {
            remove_if_exists(&paths.job_receipt(job.job_id.as_ref()))?;
            return parent_cancelled(
                &mut existing,
                "operator cancelled before the existing parent receipt was classified",
            );
        }
        if let Some(completion) =
            pending_parent_repair_completion(paths, job, &mut existing, &merged)?
        {
            return Ok(completion);
        }
        verify_parent_result_identity(paths, job, &existing, &merged)?;
        let cancellation = ParentCompletionCancellation::start(&existing)?;
        let phase_deadline = parent_completion_phase_deadline(paths, job)?;
        match deadreckon_core::validate_completion_receipt_bounded(
            paths,
            &existing,
            parent_completion_git_scope(
                &existing,
                &cancellation,
                phase_deadline,
                "existing graph parent receipt validation",
            ),
        ) {
            Ok(receipt) => {
                let receipt = match validate_and_promote_parent(
                    paths,
                    &mut existing,
                    &receipt,
                    &cancellation,
                    phase_deadline,
                ) {
                    Ok(receipt) => receipt,
                    Err(CliError::Core(error)) => {
                        if let Some(terminal) = settle_parent_process_boundary(
                            &mut existing,
                            &error,
                            "existing graph parent receipt promotion",
                        )? {
                            return Ok(terminal);
                        }
                        return Err(CliError::Core(error));
                    }
                    Err(error) => return Err(error),
                };
                return Ok(ParentCompletion::Verified(Box::new(receipt)));
            }
            Err(error) => {
                if let Some(terminal) = settle_parent_process_boundary(
                    &mut existing,
                    &error,
                    "existing graph parent receipt validation",
                )? {
                    return Ok(terminal);
                }
            }
        }
        if let Some(reason) = existing
            .failure_reason
            .as_deref()
            .and_then(|reason| reason.strip_prefix("NEEDS_REVIEW: "))
        {
            let decision = persisted_semantic_judgment(&existing)
                .ok()
                .flatten()
                .map(|judgment| judgment.decision);
            let stop_reason = semantic_decision_stop_reason(decision).unwrap_or_else(|| {
                if reason.contains("not contained") {
                    StopReason::LostContainment
                } else {
                    StopReason::SemanticUnavailable
                }
            });
            return Ok(ParentCompletion::NeedsReview {
                reason: reason.to_string(),
                decision,
                stop_reason,
            });
        }
        if let Some(reason) = existing
            .failure_reason
            .as_deref()
            .and_then(|reason| reason.strip_prefix("deterministic graph gate failed: "))
        {
            return Ok(ParentCompletion::GateFailed(reason.to_string()));
        }
    }
    let mut parent = prepare_parent_result_run(paths, job, authority, &merged)?;
    let cancellation = ParentCompletionCancellation::start(&parent)?;
    let phase_deadline = parent_completion_phase_deadline(paths, job)?;
    if cancellation.requested() {
        return parent_cancelled(
            &mut parent,
            "operator cancelled before graph parent verification",
        );
    }
    let backend = authority
        .sandbox_requested
        .parse::<deadreckon_sandbox::SandboxBackend>()
        .map_err(|error| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "approved sandbox backend is invalid: {error}"
            )))
        })?;
    let marker_path = deadreckon_core::marker_path_for_run_root(&parent.run_root);
    let marker = if marker_path.exists() {
        match deadreckon_core::validate_acceptance_marker(&parent) {
            Ok(marker) => marker,
            Err(error) => {
                parent.failure_reason =
                    Some(format!("deterministic graph proof is invalid: {error}"));
                parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
                deadreckon_core::save_state(&parent)?;
                return Ok(ParentCompletion::GateFailed(error.to_string()));
            }
        }
    } else {
        let launch_owner = parent_gate_launch_owner(paths, job)?;
        let gate = deadreckon_runtime::run_deterministic_gate_work_phase(
            &parent,
            backend,
            Some(&launch_owner),
            cancellation.token(),
            phase_deadline,
        )
        .await?;
        let gate = match settle_parent_gate_phase(
            &mut parent,
            "graph deterministic verification",
            gate,
        )? {
            ParentGateSettlement::Completed(result) => result,
            ParentGateSettlement::Terminal(completion) => return Ok(completion),
        };
        if let Err(error) = gate {
            if cancellation.requested() {
                return parent_cancelled(
                    &mut parent,
                    "operator cancelled during graph deterministic verification",
                );
            }
            parent.failure_reason = Some(format!("deterministic graph gate failed: {error}"));
            parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
            deadreckon_core::save_state(&parent)?;
            return Ok(ParentCompletion::GateFailed(error.to_string()));
        }
        match deadreckon_core::validate_acceptance_marker(&parent) {
            Ok(marker) => marker,
            Err(error) => {
                parent.failure_reason =
                    Some(format!("deterministic graph proof is invalid: {error}"));
                parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
                deadreckon_core::save_state(&parent)?;
                return Ok(ParentCompletion::GateFailed(error.to_string()));
            }
        }
    };
    if !marker.is_native_gate_proof() {
        return Ok(ParentCompletion::GateFailed(
            "merged graph did not produce a native dr-gate proof".to_string(),
        ));
    }
    if cancellation.requested() {
        return parent_cancelled(
            &mut parent,
            "operator cancelled after graph deterministic verification",
        );
    }
    if !marker.contained || marker.sandbox_backend == "none" {
        return parent_needs_review(
            &mut parent,
            "deterministic checks passed, but the graph result was not contained",
            None,
            StopReason::LostContainment,
        );
    }
    let persisted_achieved = persisted_semantic_judgment(&parent)?
        .filter(|judgment| judgment.decision == SemanticDecision::Achieved);
    let execution_usage = match plan_execution_usage(paths, plan) {
        Ok(usage) => usage,
        Err(error) => {
            return parent_failed(
                &mut parent,
                &format!("graph budget accounting is incomplete or corrupt: {error}"),
                StopReason::CorruptHistory,
            );
        }
    };
    let current_usage = combined_parent_usage(execution_usage, &parent)?;
    if let Some(judgment) = persisted_achieved {
        if let Some((stop_reason, reason)) = semantic_budget_overrun(job, current_usage) {
            return parent_budget_exhausted(&mut parent, stop_reason, &reason);
        }
        return seal_achieved_parent(
            paths,
            job,
            &mut parent,
            authority,
            &marker,
            &judgment,
            &cancellation,
            phase_deadline,
        );
    }
    if let Some((stop_reason, reason)) = semantic_budget_exhaustion(job, current_usage) {
        return parent_budget_exhausted(&mut parent, stop_reason, &reason);
    }

    let router = match semantic_router(paths, plan) {
        Ok(router) => router,
        Err(error) => {
            return parent_needs_review(
                &mut parent,
                &format!("strict semantic judge unavailable: {error}"),
                None,
                StopReason::SemanticUnavailable,
            );
        }
    };
    let semantic =
        match deadreckon_runtime::run_semantic_judge_against_source_with_deadline_and_cancellation(
            &parent,
            &marker,
            &router,
            backend,
            &job.source_cwd,
            remaining_semantic_budget(job, current_usage),
            phase_deadline,
            Some(cancellation.token()),
        )
        .await
        {
            Ok(run) => run,
            Err(error) => {
                if cancellation.requested() {
                    return parent_cancelled(
                        &mut parent,
                        "operator cancelled during graph semantic verification",
                    );
                }
                return parent_needs_review(
                    &mut parent,
                    &format!("strict semantic judge unavailable: {error}"),
                    None,
                    StopReason::SemanticUnavailable,
                );
            }
        };
    record_parent_semantic_accounting(&mut parent, job, &semantic)?;
    if let deadreckon_runtime::SemanticJudgeResult::LostContainment(reason) = &semantic.result {
        return parent_failed(&mut parent, reason, StopReason::LostContainment);
    }
    if cancellation.requested() {
        return parent_cancelled(
            &mut parent,
            "operator cancelled during graph semantic verification",
        );
    }
    let final_usage = combined_parent_usage(execution_usage, &parent)?;
    if let Some(dimension) = semantic.budget_exhaustion {
        let (stop_reason, reason) = semantic_judge_budget_exhaustion(job, final_usage, dimension);
        return parent_budget_exhausted(&mut parent, stop_reason, &reason);
    }
    if let Some((stop_reason, reason)) = semantic_budget_overrun(job, final_usage) {
        return parent_budget_exhausted(&mut parent, stop_reason, &reason);
    }
    if let Some(judgment) = semantic.result.judgment()
        && let Err(error) =
            deadreckon_runtime::persist_semantic_judgment(&parent.run_root, judgment)
    {
        return parent_needs_review(
            &mut parent,
            &format!("strict semantic judgment could not be persisted after accounting: {error}"),
            None,
            StopReason::SemanticUnavailable,
        );
    }
    match semantic.result {
        deadreckon_runtime::SemanticJudgeResult::Achieved(judgment) => seal_achieved_parent(
            paths,
            job,
            &mut parent,
            authority,
            &marker,
            &judgment,
            &cancellation,
            phase_deadline,
        ),
        deadreckon_runtime::SemanticJudgeResult::Revise(judgment) => request_parent_repair(
            paths,
            job,
            &mut parent,
            &merged,
            &marker,
            &judgment,
            &plan.providers,
        ),
        deadreckon_runtime::SemanticJudgeResult::NeedsReview(judgment) => parent_needs_review(
            &mut parent,
            &format!(
                "independent semantic judge was uncertain: {}",
                judgment.summary
            ),
            Some(SemanticDecision::Uncertain),
            StopReason::SemanticUncertain,
        ),
        deadreckon_runtime::SemanticJudgeResult::Unavailable(reason) => {
            parent_needs_review(&mut parent, &reason, None, StopReason::SemanticUnavailable)
        }
        deadreckon_runtime::SemanticJudgeResult::LostContainment(reason) => {
            parent_failed(&mut parent, &reason, StopReason::LostContainment)
        }
    }
}

pub(crate) async fn complete_merged_campaign_parent(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    authority: &deadreckon_protocol::JobAuthority,
    campaign: &deadreckon_core::campaign::Campaign,
) -> Result<ParentCompletion> {
    let merged_run_id = campaign.merged_run_id.as_deref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "merged campaign {} has no evidence-bearing result run",
            job.job_id
        )))
    })?;
    let merged = deadreckon_core::load_run(paths, merged_run_id)?;
    if merged.status != deadreckon_core::RunStatus::Completed {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "merged campaign result {merged_run_id} is not completed"
        ))));
    }
    let rollup = match validate_campaign_rollup(paths, campaign, &merged) {
        Ok(rollup) => rollup,
        Err(error) => {
            return Ok(ParentCompletion::GateFailed(format!(
                "campaign no-laundering rollup refused: {error}"
            )));
        }
    };
    validate_owned_campaign(paths, campaign, job.job_id.as_ref())?;
    if let Ok(mut existing) = deadreckon_core::load_run(paths, job.job_id.as_ref()) {
        if deadreckon_core::cancel_marker_present(&existing) {
            remove_if_exists(&paths.job_receipt(job.job_id.as_ref()))?;
            return parent_cancelled(
                &mut existing,
                "operator cancelled before the existing campaign receipt was classified",
            );
        }
        if let Some(completion) =
            pending_parent_repair_completion(paths, job, &mut existing, &merged)?
        {
            return Ok(completion);
        }
        verify_parent_result_identity(paths, job, &existing, &merged)?;
        let parent_rollup_path =
            deadreckon_core::campaign::rollup_path_at_run_root(&existing.run_root);
        if parent_rollup_path.is_file() {
            let parent_rollup: deadreckon_core::campaign::CampaignRollup =
                serde_json::from_slice(&fs::read(&parent_rollup_path)?).map_err(|source| {
                    CliError::Core(DeadreckonError::Json {
                        path: parent_rollup_path.clone(),
                        source,
                    })
                })?;
            if parent_rollup != rollup {
                return Ok(ParentCompletion::GateFailed(
                    "campaign parent receipt is bound to a different rollup".to_string(),
                ));
            }
        } else {
            deadreckon_core::campaign::write_campaign_rollup_at_run_root(
                &existing.run_root,
                &rollup,
            )?;
        }
        let cancellation = ParentCompletionCancellation::start(&existing)?;
        let phase_deadline = parent_completion_phase_deadline(paths, job)?;
        match deadreckon_core::validate_completion_receipt_bounded(
            paths,
            &existing,
            parent_completion_git_scope(
                &existing,
                &cancellation,
                phase_deadline,
                "existing campaign parent receipt validation",
            ),
        ) {
            Ok(receipt) => {
                let receipt = match validate_and_promote_parent(
                    paths,
                    &mut existing,
                    &receipt,
                    &cancellation,
                    phase_deadline,
                ) {
                    Ok(receipt) => receipt,
                    Err(CliError::Core(error)) => {
                        if let Some(terminal) = settle_parent_process_boundary(
                            &mut existing,
                            &error,
                            "existing campaign parent receipt promotion",
                        )? {
                            return Ok(terminal);
                        }
                        return Err(CliError::Core(error));
                    }
                    Err(error) => return Err(error),
                };
                return Ok(ParentCompletion::Verified(Box::new(receipt)));
            }
            Err(error) => {
                if let Some(terminal) = settle_parent_process_boundary(
                    &mut existing,
                    &error,
                    "existing campaign parent receipt validation",
                )? {
                    return Ok(terminal);
                }
            }
        }
        if let Some(reason) = existing
            .failure_reason
            .as_deref()
            .and_then(|reason| reason.strip_prefix("NEEDS_REVIEW: "))
        {
            let decision = persisted_semantic_judgment(&existing)
                .ok()
                .flatten()
                .map(|judgment| judgment.decision);
            let stop_reason = semantic_decision_stop_reason(decision).unwrap_or_else(|| {
                if reason.contains("not contained") {
                    StopReason::LostContainment
                } else {
                    StopReason::SemanticUnavailable
                }
            });
            return Ok(ParentCompletion::NeedsReview {
                reason: reason.to_string(),
                decision,
                stop_reason,
            });
        }
        if let Some(reason) = existing
            .failure_reason
            .as_deref()
            .and_then(|reason| reason.strip_prefix("deterministic campaign gate failed: "))
        {
            return Ok(ParentCompletion::GateFailed(reason.to_string()));
        }
    }

    let mut parent = prepare_parent_result_run(paths, job, authority, &merged)?;
    let cancellation = ParentCompletionCancellation::start(&parent)?;
    let phase_deadline = parent_completion_phase_deadline(paths, job)?;
    if cancellation.requested() {
        return parent_cancelled(
            &mut parent,
            "operator cancelled before campaign parent verification",
        );
    }
    let parent_rollup = deadreckon_core::campaign::rollup_path_at_run_root(&parent.run_root);
    if parent_rollup.is_file() {
        let persisted: deadreckon_core::campaign::CampaignRollup =
            serde_json::from_slice(&fs::read(&parent_rollup)?).map_err(|source| {
                CliError::Core(DeadreckonError::Json {
                    path: parent_rollup.clone(),
                    source,
                })
            })?;
        if persisted != rollup {
            return Ok(ParentCompletion::GateFailed(
                "campaign parent rollup changed across completion attempts".to_string(),
            ));
        }
    } else {
        deadreckon_core::campaign::write_campaign_rollup_at_run_root(&parent.run_root, &rollup)?;
    }
    let backend = authority
        .sandbox_requested
        .parse::<deadreckon_sandbox::SandboxBackend>()
        .map_err(|error| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "approved sandbox backend is invalid: {error}"
            )))
        })?;
    let marker_path = deadreckon_core::marker_path_for_run_root(&parent.run_root);
    let marker = if marker_path.exists() {
        match deadreckon_core::validate_acceptance_marker(&parent) {
            Ok(marker) => marker,
            Err(error) => {
                parent.failure_reason =
                    Some(format!("deterministic campaign proof is invalid: {error}"));
                parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
                deadreckon_core::save_state(&parent)?;
                return Ok(ParentCompletion::GateFailed(error.to_string()));
            }
        }
    } else {
        let launch_owner = parent_gate_launch_owner(paths, job)?;
        let gate = deadreckon_runtime::run_deterministic_gate_work_phase(
            &parent,
            backend,
            Some(&launch_owner),
            cancellation.token(),
            phase_deadline,
        )
        .await?;
        let gate = match settle_parent_gate_phase(
            &mut parent,
            "campaign deterministic verification",
            gate,
        )? {
            ParentGateSettlement::Completed(result) => result,
            ParentGateSettlement::Terminal(completion) => return Ok(completion),
        };
        if let Err(error) = gate {
            if cancellation.requested() {
                return parent_cancelled(
                    &mut parent,
                    "operator cancelled during campaign deterministic verification",
                );
            }
            parent.failure_reason = Some(format!("deterministic campaign gate failed: {error}"));
            parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
            deadreckon_core::save_state(&parent)?;
            return Ok(ParentCompletion::GateFailed(error.to_string()));
        }
        match deadreckon_core::validate_acceptance_marker(&parent) {
            Ok(marker) => marker,
            Err(error) => {
                parent.failure_reason =
                    Some(format!("deterministic campaign proof is invalid: {error}"));
                parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
                deadreckon_core::save_state(&parent)?;
                return Ok(ParentCompletion::GateFailed(error.to_string()));
            }
        }
    };
    if !marker.is_native_gate_proof() {
        return Ok(ParentCompletion::GateFailed(
            "merged campaign did not produce a native dr-gate proof".to_string(),
        ));
    }
    if cancellation.requested() {
        return parent_cancelled(
            &mut parent,
            "operator cancelled after campaign deterministic verification",
        );
    }
    if !marker.contained || marker.sandbox_backend == "none" {
        return parent_needs_review(
            &mut parent,
            "deterministic checks passed, but the campaign result was not contained",
            None,
            StopReason::LostContainment,
        );
    }
    let persisted_achieved = persisted_semantic_judgment(&parent)?
        .filter(|judgment| judgment.decision == SemanticDecision::Achieved);
    let execution_usage = match campaign_execution_usage(paths, campaign) {
        Ok(usage) => usage,
        Err(error) => {
            return parent_failed(
                &mut parent,
                &format!("campaign budget accounting is incomplete or corrupt: {error}"),
                StopReason::CorruptHistory,
            );
        }
    };
    let current_usage = combined_parent_usage(execution_usage, &parent)?;
    if let Some(judgment) = persisted_achieved {
        if let Some((stop_reason, reason)) = semantic_budget_overrun(job, current_usage) {
            return parent_budget_exhausted(&mut parent, stop_reason, &reason);
        }
        return seal_achieved_parent(
            paths,
            job,
            &mut parent,
            authority,
            &marker,
            &judgment,
            &cancellation,
            phase_deadline,
        );
    }
    if let Some((stop_reason, reason)) = semantic_budget_exhaustion(job, current_usage) {
        return parent_budget_exhausted(&mut parent, stop_reason, &reason);
    }

    let router = match campaign_semantic_router(paths, &campaign.providers) {
        Ok(router) => router,
        Err(error) => {
            return parent_needs_review(
                &mut parent,
                &format!("strict semantic judge unavailable: {error}"),
                None,
                StopReason::SemanticUnavailable,
            );
        }
    };
    let semantic =
        match deadreckon_runtime::run_semantic_judge_against_source_with_deadline_and_cancellation(
            &parent,
            &marker,
            &router,
            backend,
            &job.source_cwd,
            remaining_semantic_budget(job, current_usage),
            phase_deadline,
            Some(cancellation.token()),
        )
        .await
        {
            Ok(run) => run,
            Err(error) => {
                if cancellation.requested() {
                    return parent_cancelled(
                        &mut parent,
                        "operator cancelled during campaign semantic verification",
                    );
                }
                return parent_needs_review(
                    &mut parent,
                    &format!("strict semantic judge unavailable: {error}"),
                    None,
                    StopReason::SemanticUnavailable,
                );
            }
        };
    record_parent_semantic_accounting(&mut parent, job, &semantic)?;
    if let deadreckon_runtime::SemanticJudgeResult::LostContainment(reason) = &semantic.result {
        return parent_failed(&mut parent, reason, StopReason::LostContainment);
    }
    if cancellation.requested() {
        return parent_cancelled(
            &mut parent,
            "operator cancelled during campaign semantic verification",
        );
    }
    let final_usage = combined_parent_usage(execution_usage, &parent)?;
    if let Some(dimension) = semantic.budget_exhaustion {
        let (stop_reason, reason) = semantic_judge_budget_exhaustion(job, final_usage, dimension);
        return parent_budget_exhausted(&mut parent, stop_reason, &reason);
    }
    if let Some((stop_reason, reason)) = semantic_budget_overrun(job, final_usage) {
        return parent_budget_exhausted(&mut parent, stop_reason, &reason);
    }
    if let Some(judgment) = semantic.result.judgment()
        && let Err(error) =
            deadreckon_runtime::persist_semantic_judgment(&parent.run_root, judgment)
    {
        return parent_needs_review(
            &mut parent,
            &format!("strict semantic judgment could not be persisted after accounting: {error}"),
            None,
            StopReason::SemanticUnavailable,
        );
    }
    match semantic.result {
        deadreckon_runtime::SemanticJudgeResult::Achieved(judgment) => seal_achieved_parent(
            paths,
            job,
            &mut parent,
            authority,
            &marker,
            &judgment,
            &cancellation,
            phase_deadline,
        ),
        deadreckon_runtime::SemanticJudgeResult::Revise(judgment) => request_parent_repair(
            paths,
            job,
            &mut parent,
            &merged,
            &marker,
            &judgment,
            &campaign.providers,
        ),
        deadreckon_runtime::SemanticJudgeResult::NeedsReview(judgment) => parent_needs_review(
            &mut parent,
            &format!(
                "independent semantic judge was uncertain: {}",
                judgment.summary
            ),
            Some(SemanticDecision::Uncertain),
            StopReason::SemanticUncertain,
        ),
        deadreckon_runtime::SemanticJudgeResult::Unavailable(reason) => {
            parent_needs_review(&mut parent, &reason, None, StopReason::SemanticUnavailable)
        }
        deadreckon_runtime::SemanticJudgeResult::LostContainment(reason) => {
            parent_failed(&mut parent, &reason, StopReason::LostContainment)
        }
    }
}

fn validate_campaign_rollup(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
    merged: &deadreckon_core::PipelineState,
) -> Result<deadreckon_core::campaign::CampaignRollup> {
    use deadreckon_core::campaign::{self, RollupVerdict};
    use deadreckon_core::tamper::AcceptanceTamperVerdict;

    deadreckon_core::validate_acceptance_marker(merged)?;
    let campaign_dir = paths.plan_dir(&campaign.campaign_id);
    let persisted = campaign::read_campaign_rollup(&campaign_dir)?;
    let merged_path = campaign::rollup_path_at_run_root(&merged.run_root);
    let merged_rollup: campaign::CampaignRollup = serde_json::from_slice(&fs::read(&merged_path)?)
        .map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: merged_path.clone(),
                source,
            })
        })?;
    if persisted != merged_rollup || persisted.campaign_id != campaign.campaign_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "campaign rollup does not match the actual merged result".to_string(),
        )));
    }
    let fresh = campaign::build_rollup(campaign, |run_id| {
        let Ok(state) = deadreckon_core::load_run(paths, run_id) else {
            return (
                "missing".to_string(),
                AcceptanceTamperVerdict::Refuse,
                Vec::new(),
            );
        };
        let tamper = deadreckon_core::tamper::read_acceptance_tamper_for_run_root(&state.run_root)
            .ok()
            .flatten();
        let verdict = tamper
            .as_ref()
            .map(|tamper| tamper.verdict)
            .unwrap_or(AcceptanceTamperVerdict::Clean);
        let caveats = tamper.map(|tamper| tamper.caveats).unwrap_or_default();
        let gate = if deadreckon_core::validate_acceptance_marker(&state).is_ok() {
            "signed".to_string()
        } else {
            "refused".to_string()
        };
        (gate, verdict, caveats)
    });
    if persisted.leaves != fresh.leaves
        || persisted.rollup_verdict != fresh.rollup_verdict
        || persisted.refused_subs != fresh.refused_subs
        || persisted.caveat_subs != fresh.caveat_subs
        || persisted.rollup_verdict == RollupVerdict::Refused
        || !campaign::campaign_can_complete(campaign, &fresh)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "campaign worst-of rollup refuses parent completion".to_string(),
        )));
    }
    Ok(persisted)
}

fn campaign_semantic_router(
    paths: &DeadreckonPaths,
    providers: &deadreckon_core::plan::PlanProviders,
) -> Result<ProviderRouter> {
    let selection = [
        (
            providers.reviewer.as_deref(),
            providers.reviewer_model.as_deref(),
        ),
        (providers.coder.as_deref(), providers.coder_model.as_deref()),
        (
            providers.planner.as_deref(),
            providers.planner_model.as_deref(),
        ),
        (
            providers.default_child.as_deref(),
            providers.default_child_model.as_deref(),
        ),
    ]
    .into_iter()
    .find(|(provider, _)| provider.is_some())
    .unwrap_or((None, None));
    if selection
        .0
        .is_some_and(|provider| provider == "smoke" || provider.starts_with("smoke:"))
    {
        Ok(ProviderRouter::smoke())
    } else {
        Ok(ProviderRouter::from_config_path_with_model(
            &paths.config_path(),
            selection.0,
            selection.1,
        )?)
    }
}

fn repair_provider_selection(
    providers: &deadreckon_core::plan::PlanProviders,
) -> (Option<String>, Option<String>) {
    [
        (providers.coder.as_ref(), providers.coder_model.as_ref()),
        (
            providers.default_child.as_ref(),
            providers.default_child_model.as_ref(),
        ),
        (providers.planner.as_ref(), providers.planner_model.as_ref()),
        (
            providers.reviewer.as_ref(),
            providers.reviewer_model.as_ref(),
        ),
    ]
    .into_iter()
    .find_map(|(provider, model)| provider.map(|provider| (Some(provider.clone()), model.cloned())))
    .unwrap_or((None, None))
}

fn parent_tree_sha256(state: &deadreckon_core::PipelineState) -> Result<String> {
    let mut index = deadreckon_core::flight::build_deliverable_file_index(&state.working_dir)?;
    // Promotion and verified materialization add controller lifecycle
    // metadata after the parent result was sealed. Keep this identity check
    // aligned with completion-receipt validation so replay still compares the
    // actual result tree, not DeadReckon's own delivery bookkeeping.
    index.files.remove(Path::new("manifest.json"));
    index.files.remove(Path::new(".materialized-to"));
    Ok(index.tree_hash())
}

fn validate_parent_identity(
    job: &deadreckon_protocol::Job,
    parent: &deadreckon_core::PipelineState,
) -> Result<()> {
    if parent.run_id != job.job_id.as_ref()
        || parent.scope != job.scope
        || parent.goal != job.goal
        || parent.cwd != job.source_cwd
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "existing parent result run does not retain Job {} identity",
            job.job_id
        ))));
    }
    Ok(())
}

fn load_parent_repair_manifest(path: &Path) -> Result<Option<ParentRepairAttemptManifest>> {
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    serde_json::from_slice(&raw).map(Some).map_err(|source| {
        CliError::Core(DeadreckonError::Json {
            path: path.to_path_buf(),
            source,
        })
    })
}

fn validate_parent_repair_manifest(
    job: &deadreckon_protocol::Job,
    intent: &ParentRepairIntent,
    manifest: &ParentRepairAttemptManifest,
) -> Result<()> {
    if manifest.schema_version != 1
        || manifest.job_id != job.job_id.as_ref()
        || manifest.shape != job.shape
        || manifest.round != intent.round
        || manifest.merged_run_id != intent.merged_run_id
        || manifest.merged_tree_sha256 != intent.merged_tree_sha256
        || manifest.pre_repair_tree_sha256 != intent.pre_repair_tree_sha256
        || manifest.intent_sha256.is_empty()
        || manifest.attempt <= intent.requested_after_attempt
        || manifest.lease_epoch < intent.requested_after_lease_epoch
        || manifest.lease_epoch == 0
        || Uuid::parse_str(&manifest.launch_id).is_err()
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent repair attempt manifest is malformed or crosses authority generations"
                .to_string(),
        )));
    }
    Ok(())
}

fn validate_parent_repair_manifest_history(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    manifest: &ParentRepairAttemptManifest,
) -> Result<()> {
    let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))?;
    let attempt_started = history.events().iter().any(|event| {
        event.kind == deadreckon_protocol::JobEventKind::AttemptStarted
            && event.lease_epoch == manifest.lease_epoch
            && event
                .detail
                .get("attempt")
                .and_then(serde_json::Value::as_u64)
                == Some(u64::from(manifest.attempt))
    });
    let child_linked = history.events().iter().any(|event| {
        event.kind == deadreckon_protocol::JobEventKind::ChildLinked
            && event.lease_epoch == manifest.lease_epoch
            && event
                .detail
                .get("attempt")
                .and_then(serde_json::Value::as_u64)
                == Some(u64::from(manifest.attempt))
            && event
                .detail
                .get("launch_id")
                .and_then(serde_json::Value::as_str)
                == Some(manifest.launch_id.as_str())
    });
    if !attempt_started || !child_linked {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent repair manifest is not backed by its fenced Job attempt and launch".to_string(),
        )));
    }
    Ok(())
}

fn parent_repair_feedback(judgment: &deadreckon_protocol::SemanticJudgment) -> String {
    let missing = if judgment.missing.is_empty() {
        "no explicit missing clauses supplied".to_string()
    } else {
        judgment.missing.join("; ")
    };
    format!(
        "independent semantic judge requested parent revision: {}. Missing: {missing}",
        judgment.summary
    )
}

fn validate_parent_repair_intent_evidence(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    parent: &deadreckon_core::PipelineState,
    intent_path: &Path,
    intent: &ParentRepairIntent,
) -> Result<()> {
    if intent.schema_version != 1
        || intent.job_id != job.job_id.as_ref()
        || intent.shape != job.shape
        || intent.round == 0
        || intent.requested_after_attempt == 0
        || intent.requested_after_lease_epoch == 0
        || Uuid::parse_str(&intent.requested_after_launch_id).is_err()
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent repair intent is malformed or crosses Job authority".to_string(),
        )));
    }
    let round_dir = parent_repair_round_dir(parent, intent.round);
    let marker_path = round_dir.join("pre-repair-marker.json");
    let judgment_path = round_dir.join("revise-judgment.json");
    if deadreckon_core::flight::sha256_file(&marker_path)? != intent.revise_marker_sha256
        || deadreckon_core::flight::sha256_file(&judgment_path)? != intent.revise_judgment_sha256
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent repair intent no longer matches its archived deterministic and semantic evidence"
                .to_string(),
        )));
    }
    let marker: deadreckon_core::AcceptanceMarker =
        serde_json::from_slice(&fs::read(&marker_path)?).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: marker_path.clone(),
                source,
            })
        })?;
    let judgment: deadreckon_protocol::SemanticJudgment =
        serde_json::from_slice(&fs::read(&judgment_path)?).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: judgment_path.clone(),
                source,
            })
        })?;
    if marker.run_id != job.job_id.as_ref()
        || marker.status != "pass"
        || !marker.is_native_gate_proof()
        || !marker.contained
        || judgment.job_id.as_ref() != job.job_id.as_ref()
        || judgment.run_id.as_ref() != job.job_id.as_ref()
        || judgment.decision != SemanticDecision::Revise
        || judgment.input_sha256 != intent.revise_input_sha256
        || parent_repair_feedback(&judgment) != intent.feedback
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent repair intent is not backed by the archived same-Job revise decision"
                .to_string(),
        )));
    }
    let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))?;
    let requested_launch = history.events().iter().any(|event| {
        event.kind == deadreckon_protocol::JobEventKind::ChildLinked
            && event.lease_epoch == intent.requested_after_lease_epoch
            && event
                .detail
                .get("attempt")
                .and_then(serde_json::Value::as_u64)
                == Some(u64::from(intent.requested_after_attempt))
            && event
                .detail
                .get("launch_id")
                .and_then(serde_json::Value::as_str)
                == Some(intent.requested_after_launch_id.as_str())
    });
    if !requested_launch {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent repair intent is not tied to the fenced launch that obtained the revise decision"
                .to_string(),
        )));
    }

    let intent_sha256 = deadreckon_core::flight::sha256_file(intent_path)?;
    let same_round = history
        .events()
        .iter()
        .filter(|event| {
            event.kind == deadreckon_protocol::JobEventKind::SemanticJudgeRevise
                && event
                    .detail
                    .get("round")
                    .and_then(serde_json::Value::as_u64)
                    == Some(u64::from(intent.round))
        })
        .collect::<Vec<_>>();
    if !same_round.is_empty()
        && !same_round.iter().any(|event| {
            event
                .detail
                .get("intent_sha256")
                .and_then(serde_json::Value::as_str)
                == Some(intent_sha256.as_str())
                && event
                    .detail
                    .get("judgment_sha256")
                    .and_then(serde_json::Value::as_str)
                    == Some(intent.revise_judgment_sha256.as_str())
        })
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent repair Job event disagrees with the active intent or revise judgment"
                .to_string(),
        )));
    }
    Ok(())
}

fn parent_repair_round_chain_sha256(
    parent: &deadreckon_core::PipelineState,
    round: u32,
) -> Result<String> {
    let archive = parent_repair_round_dir(parent, round);
    let mut bound = String::new();
    for name in [
        "intent.json",
        "final-attempt.json",
        "candidate.json",
        "pre-repair-marker.json",
        "revise-judgment.json",
    ] {
        let digest = deadreckon_core::flight::sha256_file(&archive.join(name))?;
        bound.push_str(name);
        bound.push('=');
        bound.push_str(&digest);
        bound.push('\n');
    }
    Ok(deadreckon_core::flight::sha256_text(&bound))
}

fn validate_parent_repair_intent_lineage(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    active_intent_path: &Path,
    active: &ParentRepairIntent,
    parent: Option<&deadreckon_core::PipelineState>,
) -> Result<()> {
    let owned_parent;
    let parent = if let Some(parent) = parent {
        parent
    } else {
        owned_parent = deadreckon_core::load_run(paths, job.job_id.as_ref())?;
        &owned_parent
    };
    let mut current = active.clone();
    let mut current_path = active_intent_path.to_path_buf();
    loop {
        validate_parent_repair_intent_evidence(paths, job, parent, &current_path, &current)?;
        if current.round == 1 {
            if current.previous_round_sha256.is_some() {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "first parent repair round unexpectedly names prior repair evidence"
                        .to_string(),
                )));
            }
            break;
        }
        let previous_round = current.round - 1;
        let expected = parent_repair_round_chain_sha256(parent, previous_round)?;
        if current.previous_round_sha256.as_deref() != Some(expected.as_str()) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "parent repair history chain does not match the prior immutable round".to_string(),
            )));
        }
        let archive = parent_repair_round_dir(parent, previous_round);
        current_path = archive.join("intent.json");
        current = serde_json::from_slice(&fs::read(&current_path)?).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: current_path.clone(),
                source,
            })
        })?;
        let manifest_path = archive.join("final-attempt.json");
        let manifest: ParentRepairAttemptManifest =
            serde_json::from_slice(&fs::read(&manifest_path)?).map_err(|source| {
                CliError::Core(DeadreckonError::Json {
                    path: manifest_path.clone(),
                    source,
                })
            })?;
        validate_parent_repair_manifest(job, &current, &manifest)?;
        validate_parent_repair_manifest_history(paths, job, &manifest)?;
        let intent_sha256 = deadreckon_core::flight::sha256_file(&current_path)?;
        let candidate_path = archive.join("candidate.json");
        let candidate: deadreckon_runtime::ParentRepairCandidate =
            serde_json::from_slice(&fs::read(&candidate_path)?).map_err(|source| {
                CliError::Core(DeadreckonError::Json {
                    path: candidate_path.clone(),
                    source,
                })
            })?;
        if manifest.intent_sha256 != intent_sha256
            || candidate.job_id != job.job_id.as_ref()
            || candidate.run_id != job.job_id.as_ref()
            || candidate.round != current.round
            || candidate.attempt != manifest.attempt
            || candidate.launch_id != manifest.launch_id
            || candidate.lease_epoch != manifest.lease_epoch
            || candidate.intent_sha256 != intent_sha256
            || candidate.manifest_sha256 != deadreckon_core::flight::sha256_file(&manifest_path)?
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "archived parent repair round is not tied to its fenced candidate".to_string(),
            )));
        }
    }
    Ok(())
}

fn validate_parent_repair_candidate(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    parent: &deadreckon_core::PipelineState,
    intent: &ParentRepairIntent,
) -> Result<deadreckon_runtime::ParentRepairCandidate> {
    let intent_path = parent_repair_intent_path(paths, job.job_id.as_ref());
    validate_parent_repair_intent_lineage(paths, job, &intent_path, intent, Some(parent))?;
    let manifest_path = deadreckon_core::parent_repair_manifest_path_for_run_root(&parent.run_root);
    let manifest = load_parent_repair_manifest(&manifest_path)?.ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "parent repair candidate has no fenced attempt manifest".to_string(),
        ))
    })?;
    validate_parent_repair_manifest(job, intent, &manifest)?;
    validate_parent_repair_manifest_history(paths, job, &manifest)?;
    let intent_sha256 = deadreckon_core::flight::sha256_file(&intent_path)?;
    if manifest.intent_sha256 != intent_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent repair attempt no longer matches its trusted intent".to_string(),
        )));
    }
    let candidate_path =
        deadreckon_core::parent_repair_candidate_path_for_run_root(&parent.run_root);
    let candidate: deadreckon_runtime::ParentRepairCandidate =
        serde_json::from_slice(&fs::read(&candidate_path)?).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: candidate_path.clone(),
                source,
            })
        })?;
    if candidate.schema_version != 1
        || candidate.job_id != job.job_id.as_ref()
        || candidate.run_id != job.job_id.as_ref()
        || candidate.round != intent.round
        || candidate.attempt != manifest.attempt
        || candidate.launch_id != manifest.launch_id
        || candidate.lease_epoch != manifest.lease_epoch
        || candidate.intent_sha256 != intent_sha256
        || candidate.manifest_sha256 != deadreckon_core::flight::sha256_file(&manifest_path)?
        || candidate.result_tree_sha256 != parent_tree_sha256(parent)?
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent repair candidate does not match its fenced attempt and current result tree"
                .to_string(),
        )));
    }
    Ok(candidate)
}

fn archive_active_repair_file(
    state: &deadreckon_core::PipelineState,
    round: u32,
    source: &Path,
    archive_name: &str,
) -> Result<()> {
    let raw = match fs::read(source) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    write_immutable_bytes(
        &parent_repair_round_dir(state, round).join(archive_name),
        &raw,
    )
}

fn write_immutable_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Ok(existing) = fs::read(path) {
        if existing == bytes {
            return Ok(());
        }
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "immutable parent repair evidence changed at {}",
            path.display()
        ))));
    }
    let parent = path.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "parent repair archive path has no parent: {}",
            path.display()
        )))
    })?;
    fs::create_dir_all(parent)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn prepare_parent_result_run(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    authority: &deadreckon_protocol::JobAuthority,
    merged: &deadreckon_core::PipelineState,
) -> Result<deadreckon_core::PipelineState> {
    if let Ok(mut existing) = deadreckon_core::load_run(paths, job.job_id.as_ref()) {
        verify_parent_result_identity(paths, job, &existing, merged)?;
        let contract = super::job::job_acceptance_path(paths, job.job_id.as_ref());
        let parent_contract =
            deadreckon_core::acceptance_spec_path_for_run_root(&existing.run_root);
        if parent_contract.is_file() {
            let digest = deadreckon_core::flight::sha256_file(&parent_contract)?;
            if digest != authority.contract_sha256 {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "parent result contract changed after graph job {} was approved",
                    job.job_id
                ))));
            }
        } else {
            fs::copy(contract, parent_contract)?;
        }
        existing.turn = existing.turn.max(1);
        if existing.status != deadreckon_core::RunStatus::Completed
            && !existing
                .failure_reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with("NEEDS_REVIEW:"))
        {
            existing.set_phase_status(PhaseId(50), PhaseStatus::Executing)?;
            deadreckon_core::save_state(&existing)?;
        }
        return Ok(existing);
    }

    let mut state = deadreckon_core::create_owned_run(
        paths,
        deadreckon_core::RunOptions {
            goal: job.goal.clone(),
            cwd: job.source_cwd.clone(),
            sandbox: authority.sandbox_requested.clone(),
            provider: None,
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(job.policy.max_spend_usd),
            max_wall_seconds: Some(job.policy.max_wall_seconds as f64),
            run_id: Some(job.job_id.as_ref().to_string()),
            codebase: None,
        },
        deadreckon_core::RunOwnership::parent_result(job.job_id.as_ref()),
    )?;
    if state.scope != job.scope || state.run_id != job.job_id.as_ref() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "parent result run identity does not match graph job {}",
            job.job_id
        ))));
    }
    if state.working_dir.exists() {
        fs::remove_dir_all(&state.working_dir)?;
    }
    deadreckon_core::copy_deliverable_tree(&merged.working_dir, &state.working_dir)?;
    // A promoted child run's manifest is lifecycle metadata for that child.
    // The parent receipt binds only the merged result tree; parent promotion
    // writes its own manifest later.
    match fs::remove_file(state.working_dir.join("manifest.json")) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    deadreckon_core::write_codebase_record(
        &state.working_dir,
        &deadreckon_core::CodebaseRecord::fresh(),
    )?;
    fs::copy(
        super::job::job_acceptance_path(paths, job.job_id.as_ref()),
        deadreckon_core::acceptance_spec_path_for_run_root(&state.run_root),
    )?;
    state.turn = 1;
    state.set_phase_status(PhaseId(50), PhaseStatus::Executing)?;
    deadreckon_core::save_state(&state)?;
    Ok(state)
}

fn pending_parent_repair_completion(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    parent: &mut deadreckon_core::PipelineState,
    merged: &deadreckon_core::PipelineState,
) -> Result<Option<ParentCompletion>> {
    let Some(intent) = load_parent_repair_intent(paths, job.job_id.as_ref())? else {
        return Ok(None);
    };
    if intent.shape != job.shape
        || intent.merged_run_id != merged.run_id
        || intent.merged_tree_sha256 != parent_tree_sha256(merged)?
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "active parent repair intent no longer matches the frozen merged result".to_string(),
        )));
    }
    let intent_path = parent_repair_intent_path(paths, job.job_id.as_ref());
    validate_parent_repair_intent_lineage(paths, job, &intent_path, &intent, Some(parent))?;
    finalize_parent_repair_request_if_needed(parent, &intent)?;
    let candidate = deadreckon_core::parent_repair_candidate_path_for_run_root(&parent.run_root);
    if candidate.is_file() {
        validate_parent_repair_candidate(paths, job, parent, &intent)?;
        return Ok(None);
    }
    if parent.status == deadreckon_core::RunStatus::Failed
        && let Some(reason) = parent.pause_reason.as_deref()
    {
        let stop_reason = if reason.contains("spend") {
            StopReason::SpendCap
        } else if reason.contains("wall") {
            StopReason::WallCap
        } else {
            StopReason::AttemptLimit
        };
        return Ok(Some(ParentCompletion::BudgetExhausted {
            reason: reason.to_string(),
            stop_reason,
        }));
    }
    if parent.status == deadreckon_core::RunStatus::Killed {
        return Ok(Some(ParentCompletion::Cancelled {
            reason: parent
                .failure_reason
                .clone()
                .unwrap_or_else(|| "parent repair was cancelled".to_string()),
        }));
    }
    if parent.status == deadreckon_core::RunStatus::Failed
        && parent.provider_failure != Some(deadreckon_core::ProviderFailureDisposition::Retryable)
    {
        return Ok(Some(ParentCompletion::RepairFailed {
            reason: parent
                .failure_reason
                .clone()
                .unwrap_or_else(|| "parent repair failed without a verified candidate".to_string()),
            stop_reason: StopReason::FatalProvider,
        }));
    }
    if parent_repair_needs_projection(paths, job) {
        let judgment_path =
            parent_repair_round_dir(parent, intent.round).join("revise-judgment.json");
        let judgment_sha256 = deadreckon_core::flight::sha256_file(&judgment_path)?;
        if judgment_sha256 != intent.revise_judgment_sha256 {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "archived revise judgment no longer matches parent repair intent".to_string(),
            )));
        }
        return Ok(Some(ParentCompletion::ReviseRequested {
            reason: intent.feedback,
            round: intent.round,
            intent_sha256: deadreckon_core::flight::sha256_file(&intent_path)?,
            intent_path,
            judgment_path,
            judgment_sha256,
        }));
    }
    let (reason, stop_reason) = if parent.status == deadreckon_core::RunStatus::Failed
        && parent.provider_failure == Some(deadreckon_core::ProviderFailureDisposition::Retryable)
    {
        (
            parent
                .failure_reason
                .clone()
                .unwrap_or_else(|| "parent repair provider failed transiently".to_string()),
            StopReason::TransientProvider,
        )
    } else {
        (
            "parent semantic repair is waiting for or running its fenced candidate attempt"
                .to_string(),
            StopReason::LostContainment,
        )
    };
    Ok(Some(ParentCompletion::RepairPending {
        reason,
        round: intent.round,
        stop_reason,
    }))
}

fn finalize_parent_repair_request_if_needed(
    parent: &mut deadreckon_core::PipelineState,
    intent: &ParentRepairIntent,
) -> Result<()> {
    let marker_path = deadreckon_core::marker_path_for_run_root(&parent.run_root);
    let judgment_path = parent
        .run_root
        .join(deadreckon_core::SEMANTIC_JUDGMENT_JSON);
    let manifest_path = deadreckon_core::parent_repair_manifest_path_for_run_root(&parent.run_root);
    let candidate_path =
        deadreckon_core::parent_repair_candidate_path_for_run_root(&parent.run_root);
    let active_manifest = load_parent_repair_manifest(&manifest_path)?;
    let active_candidate = match fs::read(&candidate_path) {
        Ok(raw) => Some(
            serde_json::from_slice::<deadreckon_runtime::ParentRepairCandidate>(&raw).map_err(
                |source| {
                    CliError::Core(DeadreckonError::Json {
                        path: candidate_path.clone(),
                        source,
                    })
                },
            )?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let active_round = active_manifest
        .as_ref()
        .map(|manifest| manifest.round)
        .or_else(|| active_candidate.as_ref().map(|candidate| candidate.round));
    if active_manifest
        .as_ref()
        .zip(active_candidate.as_ref())
        .is_some_and(|(manifest, candidate)| manifest.round != candidate.round)
        || active_round.is_some_and(|round| round > intent.round)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "active parent repair files cross repair rounds".to_string(),
        )));
    }
    let stale_prior_round = active_round.is_some_and(|round| round < intent.round);
    let current_candidate_ready = active_manifest
        .as_ref()
        .zip(active_candidate.as_ref())
        .is_some_and(|(manifest, candidate)| {
            manifest.round == intent.round && candidate.round == intent.round
        });
    if active_round == Some(intent.round)
        && (marker_path.is_file() || judgment_path.is_file())
        && !current_candidate_ready
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "current parent repair attempt overlaps unretired revise proof".to_string(),
        )));
    }
    if ((marker_path.is_file() || judgment_path.is_file()) && !current_candidate_ready)
        || stale_prior_round
        || (active_round.is_none() && parent.status == deadreckon_core::RunStatus::Completed)
        || (active_manifest.is_none()
            && active_candidate
                .as_ref()
                .is_some_and(|candidate| candidate.round < intent.round))
    {
        finalize_parent_repair_request(parent, intent)?;
    }
    Ok(())
}

fn verify_parent_result_identity(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    parent: &deadreckon_core::PipelineState,
    merged: &deadreckon_core::PipelineState,
) -> Result<()> {
    validate_parent_identity(job, parent)?;
    let parent_hash = parent_tree_sha256(parent)?;
    let merged_hash = parent_tree_sha256(merged)?;
    if parent_hash != merged_hash {
        let intent = load_parent_repair_intent(paths, job.job_id.as_ref())?.ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "existing parent result for Job {} changed without a trusted repair intent",
                job.job_id
            )))
        })?;
        if intent.shape != job.shape
            || intent.merged_run_id != merged.run_id
            || intent.merged_tree_sha256 != merged_hash
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "parent repair lineage for Job {} does not match merged result {}",
                job.job_id, merged.run_id
            ))));
        }
        validate_parent_repair_candidate(paths, job, parent, &intent)?;
    }
    Ok(())
}

fn persisted_semantic_judgment(
    state: &deadreckon_core::PipelineState,
) -> Result<Option<deadreckon_protocol::SemanticJudgment>> {
    let path = state.run_root.join(deadreckon_core::SEMANTIC_JUDGMENT_JSON);
    let raw = match fs::read(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let judgment: deadreckon_protocol::SemanticJudgment =
        serde_json::from_slice(&raw).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: path.clone(),
                source,
            })
        })?;
    if judgment.job_id != deadreckon_protocol::JobId(state.run_id.clone())
        || judgment.run_id != deadreckon_protocol::RunId(state.run_id.clone())
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "persisted semantic judgment identity does not match parent result {}",
            state.run_id
        ))));
    }
    Ok(Some(judgment))
}

fn plan_execution_usage(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
) -> Result<ParentExecutionUsage> {
    let mut seen_runs = std::collections::BTreeSet::new();
    let mut seen_plans = std::collections::BTreeSet::new();
    let mut usage = ParentExecutionUsage::default();
    add_plan_execution_usage(
        paths,
        plan,
        &mut seen_runs,
        &mut seen_plans,
        &mut usage,
        true,
    )?;
    Ok(usage)
}

fn validate_merge_repair_evidence(
    paths: &DeadreckonPaths,
    state: &deadreckon_core::PipelineState,
) -> Result<()> {
    let ownership = state.ownership.as_ref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "merge-repair Run {} has no durable parent ownership",
            state.run_id
        )))
    })?;
    let deadreckon_core::RunOwnershipArtifact::MergeRepair {
        root_artifact_id,
        repair_id,
        repair_round,
        run_id,
        proof_dir,
        repair_request_sha256,
        repair_plan_sha256,
    } = &ownership.artifact
    else {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "merge-repair Run {} is stamped as another artifact kind",
            state.run_id
        ))));
    };
    let record_path = proof_dir.join("repair-run.json");
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(&record_path)?).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: record_path.clone(),
                source,
            })
        })?;
    let marker = deadreckon_core::validate_acceptance_marker(state)?;
    let library = paths.library_dir(&state.scope, &state.run_id);
    let result_tree_sha256 =
        deadreckon_core::flight::build_deliverable_file_index(&library)?.tree_hash();
    let marker_sha256 = deadreckon_core::flight::sha256_file(
        &deadreckon_core::marker_path_for_run_root(&state.run_root),
    )?;
    if ownership.job_id != *root_artifact_id
        || run_id != &state.run_id
        || record
            .get("root_artifact_id")
            .and_then(serde_json::Value::as_str)
            != Some(root_artifact_id.as_str())
        || record.get("repair_id").and_then(serde_json::Value::as_str) != Some(repair_id.as_str())
        || record
            .get("repair_round")
            .and_then(serde_json::Value::as_u64)
            != Some(u64::from(*repair_round))
        || record.get("run_id").and_then(serde_json::Value::as_str) != Some(state.run_id.as_str())
        || record
            .get("repair_request_sha256")
            .and_then(serde_json::Value::as_str)
            != Some(repair_request_sha256.as_str())
        || record
            .get("repair_plan_sha256")
            .and_then(serde_json::Value::as_str)
            != Some(repair_plan_sha256.as_str())
        || record.get("trusted").and_then(serde_json::Value::as_bool) != Some(true)
        || record
            .get("acceptance_marker_sha256")
            .and_then(serde_json::Value::as_str)
            != Some(marker_sha256.as_str())
        || record
            .get("result_tree_sha256")
            .and_then(serde_json::Value::as_str)
            != Some(result_tree_sha256.as_str())
        || deadreckon_core::flight::sha256_file(&proof_dir.join("repair-request.json"))?
            != *repair_request_sha256
        || deadreckon_core::flight::sha256_file(&proof_dir.join("repair-plan.json"))?
            != *repair_plan_sha256
        || state.sandbox == "none"
        || record
            .get("sandbox_requested")
            .and_then(serde_json::Value::as_str)
            != Some(state.sandbox.as_str())
        || !marker.is_native_gate_proof()
        || !marker.contained
        || marker.sandbox_backend == "none"
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "merge-repair Run {} has missing, stale, or tampered trusted evidence",
            state.run_id
        ))));
    }
    Ok(())
}

fn add_plan_execution_usage(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
    seen_runs: &mut std::collections::BTreeSet<String>,
    seen_plans: &mut std::collections::BTreeSet<String>,
    usage: &mut ParentExecutionUsage,
    require_complete: bool,
) -> Result<()> {
    if !seen_plans.insert(plan.plan_id.clone()) {
        return Ok(());
    }
    let planner = load_plan_planner_accounting(paths, &plan.plan_id)?;
    add_usage(
        "plan root planner",
        ParentExecutionUsage {
            spend_usd: planner.cost_usd,
            wall_seconds: planner.wall_seconds,
        },
        usage,
    )?;
    for task in &plan.tasks {
        if let Some(subplan_id) = task.subplan.as_deref() {
            let subplan = deadreckon_core::load_plan(paths, subplan_id).map_err(|error| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "cannot verify parent budget because nested sub-plan {subplan_id} is unreadable: {error}"
                )))
            })?;
            add_plan_execution_usage(
                paths,
                &subplan,
                seen_runs,
                seen_plans,
                usage,
                require_complete,
            )?;
        }
        for run_id in task
            .attempts
            .iter()
            .filter_map(|attempt| attempt.run_id.as_deref())
            .chain(task.child_run_id.as_deref())
        {
            if !seen_runs.insert(run_id.to_string()) {
                continue;
            }
            let state = deadreckon_core::load_run(paths, run_id).map_err(|error| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "cannot verify parent budget because task run {run_id} is unreadable: {error}"
                )))
            })?;
            add_usage(
                &format!("task run {run_id}"),
                ParentExecutionUsage {
                    spend_usd: state.total_spend_usd,
                    wall_seconds: state.total_wall_seconds,
                },
                usage,
            )?;
        }
        for attempt in task
            .attempts
            .iter()
            .filter(|attempt| attempt.run_id.is_none())
        {
            let Some(finished_at) = attempt.finished_at else {
                if require_complete {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                        "cannot verify parent budget because graph task {} has an unfinished attempt without a run ID",
                        task.task_id
                    ))));
                }
                continue;
            };
            let elapsed_ms = (finished_at - attempt.started_at).num_milliseconds();
            if elapsed_ms < 0 {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "cannot verify parent budget because graph task {} attempt {} ends before it starts",
                    task.task_id, attempt.attempt
                ))));
            }
            // A child can fail during source preparation or process spawn,
            // before any Run identity exists. TaskAttempt is the durable
            // evidence for exactly that case, so charge its recorded spend
            // and elapsed time instead of treating the permitted None as
            // corrupt history.
            add_usage(
                &format!(
                    "graph task {} attempt {} without a run ID",
                    task.task_id, attempt.attempt
                ),
                ParentExecutionUsage {
                    spend_usd: attempt.spend_usd,
                    wall_seconds: elapsed_ms as f64 / 1_000.0,
                },
                usage,
            )?;
        }
        if require_complete && task.child_run_id.is_none() {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot verify parent budget because graph task {} has no result run",
                task.task_id
            ))));
        }
    }
    for event in deadreckon_core::read_plan_events(paths, &plan.plan_id)? {
        let run_id = match event.event {
            deadreckon_core::PlanEventKind::MergeRepairRunDiscovered { run_id, .. } => Some(run_id),
            deadreckon_core::PlanEventKind::MergeRepaired {
                repair_run_id: Some(run_id),
                ..
            } => Some(run_id),
            _ => None,
        };
        let Some(run_id) = run_id else {
            continue;
        };
        if !seen_runs.insert(run_id.clone()) {
            continue;
        }
        let state = deadreckon_core::load_run(paths, &run_id).map_err(|error| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot verify parent budget because merge-repair run {run_id} is unreadable: {error}"
            )))
        })?;
        validate_merge_repair_evidence(paths, &state)?;
        add_usage(
            &format!("merge-repair run {run_id}"),
            ParentExecutionUsage {
                spend_usd: state.total_spend_usd,
                wall_seconds: state.total_wall_seconds,
            },
            usage,
        )?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RepairBudgetAvailability {
    Available {
        spend_usd: f64,
        wall_seconds: f64,
    },
    Exhausted {
        stop_reason: StopReason,
        reason: String,
    },
}

pub(crate) fn current_driver_remaining_repair_budget(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
    repair_planner_spend_usd: f64,
    repair_planner_wall_seconds: f64,
) -> Result<Option<RepairBudgetAvailability>> {
    let Some(context) = DRIVER_CONTEXT.get() else {
        return Ok(None);
    };
    let job = deadreckon_core::load_job(paths, &context.job_id)?;
    let mut usage = match job.shape {
        JobShape::Graph => graph_repair_execution_usage(paths, plan, &context.job_id)?,
        JobShape::LegacyCampaign => {
            if plan.plan_id != context.job_id {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Campaign merge-repair adapter {} does not match current Job {}",
                    plan.plan_id, context.job_id
                ))));
            }
            let campaign =
                deadreckon_core::campaign::read_campaign(&paths.plan_dir(&context.job_id))?;
            campaign_execution_usage(paths, &campaign)?
        }
        shape => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "merge repair cannot inherit a budget from incompatible Job shape {shape:?}"
            ))));
        }
    };
    add_usage(
        "merge-repair planner",
        ParentExecutionUsage {
            spend_usd: repair_planner_spend_usd,
            wall_seconds: repair_planner_wall_seconds,
        },
        &mut usage,
    )?;
    Ok(Some(repair_budget_availability(
        job.policy.max_spend_usd,
        job.policy.max_wall_seconds as f64,
        usage,
    )))
}

fn graph_repair_execution_usage(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
    current_job_id: &str,
) -> Result<ParentExecutionUsage> {
    let owner = resolve_plan_owner(paths, plan)?.ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "Graph merge-repair Plan {} has no durable Job owner",
            plan.plan_id
        )))
    })?;
    if owner.job.job_id.as_ref() != current_job_id || owner.root_plan_id != current_job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Graph merge-repair Plan {} does not belong to current root Job {current_job_id}",
            plan.plan_id
        ))));
    }
    let root = if plan.plan_id == owner.root_plan_id {
        plan.clone()
    } else {
        deadreckon_core::load_plan(paths, &owner.root_plan_id)?
    };
    let mut seen_runs = std::collections::BTreeSet::new();
    let mut seen_plans = std::collections::BTreeSet::new();
    let mut usage = ParentExecutionUsage::default();
    add_plan_execution_usage(
        paths,
        &root,
        &mut seen_runs,
        &mut seen_plans,
        &mut usage,
        false,
    )?;
    Ok(usage)
}

fn repair_budget_availability(
    max_spend_usd: f64,
    max_wall_seconds: f64,
    usage: ParentExecutionUsage,
) -> RepairBudgetAvailability {
    let remaining_spend = max_spend_usd - usage.spend_usd;
    let remaining_wall = max_wall_seconds - usage.wall_seconds;
    if remaining_spend <= 0.0 {
        return RepairBudgetAvailability::Exhausted {
            stop_reason: StopReason::SpendCap,
            reason: format!(
                "merge repair child refused because the parent Job exhausted its approved spend cap (${:.6} used of ${max_spend_usd:.6})",
                usage.spend_usd
            ),
        };
    }
    if remaining_wall <= 0.0 {
        return RepairBudgetAvailability::Exhausted {
            stop_reason: StopReason::WallCap,
            reason: format!(
                "merge repair child refused because the parent Job exhausted its approved wall-time cap ({:.3}s used of {max_wall_seconds:.3}s)",
                usage.wall_seconds
            ),
        };
    }
    RepairBudgetAvailability::Available {
        spend_usd: remaining_spend,
        wall_seconds: remaining_wall,
    }
}

fn campaign_execution_usage(
    paths: &DeadreckonPaths,
    campaign: &deadreckon_core::campaign::Campaign,
) -> Result<ParentExecutionUsage> {
    let mut usage = ParentExecutionUsage::default();
    let root = campaign.root_planner_accounting.as_ref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "campaign has no crash-safe root planner accounting snapshot".to_string(),
        ))
    })?;
    validate_root_planner_accounting(root)?;
    if root.planner_invoked {
        let event = deadreckon_core::campaign::read_campaign_events(
            &paths.plan_dir(&campaign.campaign_id),
        )?
        .into_iter()
        .rev()
        .find(|event| event.kind == "root_planner_accounting")
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "campaign root planner accounting event is missing".to_string(),
            ))
        })?;
        let event_cost = event
            .detail
            .get("cost_usd")
            .and_then(serde_json::Value::as_f64);
        let event_wall = event
            .detail
            .get("wall_seconds")
            .and_then(serde_json::Value::as_f64);
        if event_cost != Some(root.cost_usd) || event_wall != Some(root.wall_seconds) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "campaign root planner event disagrees with its crash-safe accounting snapshot"
                    .to_string(),
            )));
        }
    }
    add_usage(
        "campaign root planner",
        ParentExecutionUsage {
            spend_usd: root.cost_usd,
            wall_seconds: root.wall_seconds,
        },
        &mut usage,
    )?;
    let mut seen_runs = std::collections::BTreeSet::new();
    let mut seen_plans = std::collections::BTreeSet::new();
    for sub in &campaign.sub_goals {
        let plan_id = sub.sub_plan_id.as_deref().ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot verify campaign budget because sub-goal {} has no plan ID",
                sub.sub_id
            )))
        })?;
        let plan = deadreckon_core::load_plan(paths, plan_id).map_err(|error| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot verify campaign budget because sub-plan {plan_id} is unreadable: {error}"
            )))
        })?;
        add_plan_execution_usage(
            paths,
            &plan,
            &mut seen_runs,
            &mut seen_plans,
            &mut usage,
            true,
        )?;
    }
    let mut repair_run_ids = deadreckon_core::read_plan_events(paths, &campaign.campaign_id)?
        .into_iter()
        .filter_map(|event| match event.event {
            deadreckon_core::PlanEventKind::MergeRepairRunDiscovered { run_id, .. } => Some(run_id),
            deadreckon_core::PlanEventKind::MergeRepaired {
                repair_run_id: Some(run_id),
                ..
            } => Some(run_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    repair_run_ids.extend(
        deadreckon_core::read_job_history(&paths.job_events(&campaign.campaign_id))?
            .events()
            .iter()
            .filter(|event| {
                event.kind == JobEventKind::RepairChildAuthorityChanged
                    && event
                        .detail
                        .get("transition")
                        .and_then(serde_json::Value::as_str)
                        == Some("trusted")
            })
            .filter_map(|event| {
                event
                    .detail
                    .get("run_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            }),
    );
    for run_id in repair_run_ids {
        if !seen_runs.insert(run_id.clone()) {
            continue;
        }
        let state = deadreckon_core::load_run(paths, &run_id).map_err(|error| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot verify campaign budget because merge-repair run {run_id} is unreadable: {error}"
            )))
        })?;
        validate_merge_repair_evidence(paths, &state)?;
        add_usage(
            &format!("campaign merge-repair run {run_id}"),
            ParentExecutionUsage {
                spend_usd: state.total_spend_usd,
                wall_seconds: state.total_wall_seconds,
            },
            &mut usage,
        )?;
    }
    validate_parent_usage(usage)?;
    Ok(usage)
}

fn load_plan_planner_accounting(
    paths: &DeadreckonPaths,
    plan_id: &str,
) -> Result<deadreckon_core::plan::RootPlannerAccounting> {
    let path = paths.plan_dir(plan_id).join(PLAN_PLANNER_ACCOUNTING_FILE);
    let raw = fs::read(&path).map_err(|source| {
        CliError::Core(DeadreckonError::Io {
            path: path.clone(),
            source,
        })
    })?;
    let accounting: deadreckon_core::plan::RootPlannerAccounting = serde_json::from_slice(&raw)
        .map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: path.clone(),
                source,
            })
        })?;
    if accounting.schema_version != 1 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "plan {plan_id} has unsupported root planner accounting schema {}",
            accounting.schema_version
        ))));
    }
    Ok(accounting)
}

fn add_usage(
    source: &str,
    increment: ParentExecutionUsage,
    usage: &mut ParentExecutionUsage,
) -> Result<()> {
    validate_parent_usage(increment).map_err(|error| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "cannot verify parent budget because {source} has invalid accounting: {error}"
        )))
    })?;
    usage.spend_usd += increment.spend_usd;
    usage.wall_seconds += increment.wall_seconds;
    validate_parent_usage(*usage)
}

fn validate_parent_usage(usage: ParentExecutionUsage) -> Result<()> {
    if !usage.spend_usd.is_finite()
        || usage.spend_usd < 0.0
        || !usage.wall_seconds.is_finite()
        || usage.wall_seconds < 0.0
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent execution accounting must be finite and non-negative".to_string(),
        )));
    }
    Ok(())
}

fn combined_parent_usage(
    execution: ParentExecutionUsage,
    parent: &deadreckon_core::PipelineState,
) -> Result<ParentExecutionUsage> {
    let combined = ParentExecutionUsage {
        spend_usd: execution.spend_usd + parent.total_spend_usd,
        wall_seconds: execution.wall_seconds + parent.total_wall_seconds,
    };
    validate_parent_usage(combined)?;
    Ok(combined)
}

fn remaining_semantic_budget(
    job: &deadreckon_protocol::Job,
    usage: ParentExecutionUsage,
) -> deadreckon_runtime::SemanticJudgeBudget {
    deadreckon_runtime::SemanticJudgeBudget {
        remaining_spend_usd: Some(job.policy.max_spend_usd - usage.spend_usd),
        remaining_wall_seconds: Some(job.policy.max_wall_seconds as f64 - usage.wall_seconds),
    }
}

fn semantic_budget_exhaustion(
    job: &deadreckon_protocol::Job,
    usage: ParentExecutionUsage,
) -> Option<(StopReason, String)> {
    if usage.spend_usd >= job.policy.max_spend_usd {
        return Some((
            StopReason::SpendCap,
            format!(
                "approved spend cap was exhausted before semantic judging (${:.6} used of ${:.6})",
                usage.spend_usd, job.policy.max_spend_usd
            ),
        ));
    }
    if usage.wall_seconds >= job.policy.max_wall_seconds as f64 {
        return Some((
            StopReason::WallCap,
            format!(
                "approved wall-time cap was exhausted before semantic judging ({:.3}s used of {}s)",
                usage.wall_seconds, job.policy.max_wall_seconds
            ),
        ));
    }
    None
}

fn semantic_budget_overrun(
    job: &deadreckon_protocol::Job,
    usage: ParentExecutionUsage,
) -> Option<(StopReason, String)> {
    if usage.spend_usd > job.policy.max_spend_usd {
        return Some((
            StopReason::SpendCap,
            format!(
                "semantic judging exceeded the approved spend cap (${:.6} used of ${:.6})",
                usage.spend_usd, job.policy.max_spend_usd
            ),
        ));
    }
    if usage.wall_seconds > job.policy.max_wall_seconds as f64 {
        return Some((
            StopReason::WallCap,
            format!(
                "semantic judging exceeded the approved wall-time cap ({:.3}s used of {}s)",
                usage.wall_seconds, job.policy.max_wall_seconds
            ),
        ));
    }
    None
}

fn semantic_judge_budget_exhaustion(
    job: &deadreckon_protocol::Job,
    usage: ParentExecutionUsage,
    dimension: deadreckon_runtime::SemanticBudgetExhaustion,
) -> (StopReason, String) {
    match dimension {
        deadreckon_runtime::SemanticBudgetExhaustion::Spend => (
            StopReason::SpendCap,
            format!(
                "semantic judging exhausted the approved spend cap (${:.6} used of ${:.6})",
                usage.spend_usd, job.policy.max_spend_usd
            ),
        ),
        deadreckon_runtime::SemanticBudgetExhaustion::Wall => (
            StopReason::WallCap,
            format!(
                "semantic judging exhausted the approved wall-time cap ({:.3}s used of {}s)",
                usage.wall_seconds, job.policy.max_wall_seconds
            ),
        ),
    }
}

fn parent_budget_exhausted(
    parent: &mut deadreckon_core::PipelineState,
    stop_reason: StopReason,
    reason: &str,
) -> Result<ParentCompletion> {
    parent.pause_reason = Some(reason.to_string());
    parent.failure_reason = Some(reason.to_string());
    parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
    deadreckon_core::save_state(parent)?;
    Ok(ParentCompletion::BudgetExhausted {
        reason: reason.to_string(),
        stop_reason,
    })
}

fn seal_achieved_parent(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    parent: &mut deadreckon_core::PipelineState,
    authority: &deadreckon_protocol::JobAuthority,
    marker: &deadreckon_core::AcceptanceMarker,
    judgment: &deadreckon_protocol::SemanticJudgment,
    cancellation: &ParentCompletionCancellation,
    phase_deadline: ProviderPhaseDeadline,
) -> Result<ParentCompletion> {
    if cancellation.requested() {
        return parent_cancelled(
            parent,
            "operator cancelled before the verified parent receipt was sealed",
        );
    }
    if let Err(error) = deadreckon_runtime::validate_semantic_judgment_input_against_source(
        parent,
        marker,
        &job.source_cwd,
        judgment,
    ) {
        return parent_needs_review(
            parent,
            &format!(
                "semantic judgment achieved, but it does not bind the current parent evidence: {error}"
            ),
            Some(SemanticDecision::Achieved),
            StopReason::SemanticUnavailable,
        );
    }
    if tokio::time::Instant::now() >= phase_deadline.work_expires_at {
        return parent_budget_exhausted(
            parent,
            StopReason::WallCap,
            "approved Job work cutoff reached before the verified parent receipt was sealed",
        );
    }
    let receipt_scope = parent_completion_git_scope(
        parent,
        cancellation,
        phase_deadline,
        "parent completion receipt sealing",
    );
    let receipt = match deadreckon_core::seal_completion_receipt_bounded(
        paths,
        parent,
        authority,
        marker,
        judgment,
        receipt_scope,
    ) {
        Ok(receipt) => receipt,
        Err(error) => {
            if let Some(terminal) =
                settle_parent_process_boundary(parent, &error, "parent completion receipt sealing")?
            {
                return Ok(terminal);
            }
            return parent_needs_review(
                parent,
                &format!(
                    "semantic judgment achieved, but the parent receipt could not be sealed: {error}"
                ),
                Some(SemanticDecision::Achieved),
                StopReason::SemanticUnavailable,
            );
        }
    };
    if cancellation.requested() {
        remove_if_exists(&paths.job_receipt(job.job_id.as_ref()))?;
        return parent_cancelled(
            parent,
            "operator cancelled while the verified parent receipt was being sealed",
        );
    }
    if tokio::time::Instant::now() >= phase_deadline.work_expires_at {
        remove_if_exists(&paths.job_receipt(job.job_id.as_ref()))?;
        return parent_budget_exhausted(
            parent,
            StopReason::WallCap,
            "approved Job work cutoff reached while the verified parent receipt was being sealed",
        );
    }
    let receipt =
        match validate_and_promote_parent(paths, parent, &receipt, cancellation, phase_deadline) {
            Ok(receipt) => receipt,
            Err(CliError::Core(error)) => {
                if let Some(terminal) = settle_parent_process_boundary(
                    parent,
                    &error,
                    "parent receipt validation or promotion",
                )? {
                    return Ok(terminal);
                }
                return Err(CliError::Core(error));
            }
            Err(error) => return Err(error),
        };
    Ok(ParentCompletion::Verified(Box::new(receipt)))
}

fn parent_completion_git_scope(
    parent: &deadreckon_core::PipelineState,
    cancellation: &ParentCompletionCancellation,
    phase_deadline: ProviderPhaseDeadline,
    operation: &str,
) -> deadreckon_core::git::WorkBoundaryScope {
    let token = cancellation.token().clone();
    deadreckon_core::git::WorkBoundaryScope::new(
        phase_deadline.work_expires_at.into_std(),
        phase_deadline.cleanup_budget,
        move || token.is_cancelled(),
        operation,
    )
    .with_authority_dir(parent.run_root.join("child-pids"))
}

fn settle_parent_process_boundary(
    parent: &mut deadreckon_core::PipelineState,
    error: &DeadreckonError,
    context: &str,
) -> Result<Option<ParentCompletion>> {
    let DeadreckonError::ProcessBoundary {
        kind,
        authority,
        detail,
        ..
    } = error
    else {
        return Ok(None);
    };
    let terminal = match kind {
        deadreckon_core::ProcessBoundaryKind::WorkExpired => parent_budget_exhausted(
            parent,
            StopReason::WallCap,
            &format!("approved Job work cutoff reached during {context}"),
        )?,
        deadreckon_core::ProcessBoundaryKind::Cancelled => {
            parent_cancelled(parent, &format!("operator cancelled during {context}"))?
        }
        deadreckon_core::ProcessBoundaryKind::CleanupIncomplete => parent_failed(
            parent,
            &format!(
                "LOST_CONTAINMENT: {context} retained process authority{}: {detail}",
                authority
                    .as_deref()
                    .map(|path| format!(" at {}", path.display()))
                    .unwrap_or_default()
            ),
            StopReason::LostContainment,
        )?,
        deadreckon_core::ProcessBoundaryKind::SupervisionFailed => parent_needs_review(
            parent,
            &format!("{context} could not be supervised after cleanup was proven: {detail}"),
            Some(SemanticDecision::Achieved),
            StopReason::SemanticUnavailable,
        )?,
    };
    Ok(Some(terminal))
}

fn parent_cancelled(
    parent: &mut deadreckon_core::PipelineState,
    reason: &str,
) -> Result<ParentCompletion> {
    parent.pause_reason = None;
    parent.failure_reason = Some(reason.to_string());
    parent.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
    parent.status = deadreckon_core::RunStatus::Killed;
    parent.killed_at = Some(Utc::now());
    deadreckon_core::save_state(parent)?;
    Ok(ParentCompletion::Cancelled {
        reason: reason.to_string(),
    })
}

fn validate_and_promote_parent(
    paths: &DeadreckonPaths,
    parent: &mut deadreckon_core::PipelineState,
    expected: &CompletionReceipt,
    cancellation: &ParentCompletionCancellation,
    phase_deadline: ProviderPhaseDeadline,
) -> Result<CompletionReceipt> {
    // Receipt validation is deliberately before promotion: no unverified
    // parent tree may enter the library that `finish` delivers.
    let validated = deadreckon_core::validate_completion_receipt_bounded(
        paths,
        parent,
        parent_completion_git_scope(
            parent,
            cancellation,
            phase_deadline,
            "parent receipt validation before promotion",
        ),
    )?;
    if &validated != expected {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "sealed graph receipt did not round-trip through the finish validator".to_string(),
        )));
    }
    parent.failure_reason = None;
    if parent.status != deadreckon_core::RunStatus::Completed {
        parent.set_phase_status(PhaseId(60), PhaseStatus::Completed)?;
        deadreckon_core::save_state(parent)?;
    }
    deadreckon_core::promotion::promote_completed_run_bounded(
        paths,
        parent,
        parent_completion_git_scope(
            parent,
            cancellation,
            phase_deadline,
            "parent result promotion",
        ),
    )?;
    let promoted = deadreckon_core::validate_completion_receipt_bounded(
        paths,
        parent,
        parent_completion_git_scope(
            parent,
            cancellation,
            phase_deadline,
            "parent receipt validation after promotion",
        ),
    )?;
    if promoted != validated {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "graph receipt changed while promoting the parent result".to_string(),
        )));
    }
    Ok(promoted)
}

fn semantic_router(
    paths: &DeadreckonPaths,
    plan: &deadreckon_core::plan::Plan,
) -> Result<ProviderRouter> {
    let selection = [
        (
            plan.providers.reviewer.as_deref(),
            plan.providers.reviewer_model.as_deref(),
        ),
        (
            plan.providers.coder.as_deref(),
            plan.providers.coder_model.as_deref(),
        ),
        (
            plan.providers.planner.as_deref(),
            plan.providers.planner_model.as_deref(),
        ),
        (
            plan.providers.default_child.as_deref(),
            plan.providers.default_child_model.as_deref(),
        ),
    ]
    .into_iter()
    .find(|(provider, _)| provider.is_some())
    .unwrap_or((None, None));
    if selection
        .0
        .is_some_and(|provider| provider == "smoke" || provider.starts_with("smoke:"))
    {
        Ok(ProviderRouter::smoke())
    } else {
        Ok(ProviderRouter::from_config_path_with_model(
            &paths.config_path(),
            selection.0,
            selection.1,
        )?)
    }
}

fn record_parent_semantic_accounting(
    state: &mut deadreckon_core::PipelineState,
    job: &deadreckon_protocol::Job,
    semantic: &deadreckon_runtime::semantic_judge::SemanticJudgeRun,
) -> Result<()> {
    let accounting = &semantic.accounting;
    state.total_spend_usd += accounting.cost_usd;
    state.total_wall_seconds += accounting.wall_time_seconds;
    deadreckon_core::append_spend(
        state,
        &SpendRecord {
            timestamp: Utc::now(),
            turn: state.turn,
            provider: accounting.provider.clone(),
            model: accounting.model.clone(),
            input_tokens: accounting.input_tokens,
            output_tokens: accounting.output_tokens,
            cost_usd: accounting.cost_usd,
            total_cost_usd: state.total_spend_usd,
            cap_usd: Some(job.policy.max_spend_usd),
            subscription: accounting.subscription,
            estimated: false,
            wall_time_seconds: Some(accounting.wall_time_seconds),
            wall_time_cap_seconds: Some(job.policy.max_wall_seconds as f64),
            kind: "semantic_judge".to_string(),
        },
    )?;
    let (event, reason) = match &semantic.result {
        deadreckon_runtime::SemanticJudgeResult::Achieved(_) => ("semantic_judge.achieved", None),
        deadreckon_runtime::SemanticJudgeResult::Revise(_) => ("semantic_judge.revise", None),
        deadreckon_runtime::SemanticJudgeResult::NeedsReview(_) => {
            ("semantic_judge.uncertain", None)
        }
        deadreckon_runtime::SemanticJudgeResult::Unavailable(reason) => {
            ("semantic_judge.unavailable", Some(reason.as_str()))
        }
        deadreckon_runtime::SemanticJudgeResult::LostContainment(reason) => {
            ("semantic_judge.lost_containment", Some(reason.as_str()))
        }
    };
    let judgment = semantic.result.judgment();
    deadreckon_core::append_trace(
        state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: state.turn,
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
                "decision": judgment.map(|judgment| judgment.decision),
                "input_sha256": judgment.map(|judgment| judgment.input_sha256.as_str()),
                "summary": judgment.map(|judgment| judgment.summary.as_str()),
                "missing": judgment.map(|judgment| judgment.missing.as_slice()),
                "reason": reason,
                "graph_parent": true,
            }),
        },
    )?;
    deadreckon_core::save_state(state)?;
    Ok(())
}

fn parent_gate_launch_owner(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
) -> Result<deadreckon_runtime::GateLaunchOwner> {
    parent_gate_launch_lineage(paths, job).map(|(owner, _)| owner)
}

fn parent_gate_launch_lineage(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
) -> Result<(deadreckon_runtime::GateLaunchOwner, u64)> {
    let view = deadreckon_core::JobView::load(paths, job.job_id.as_ref())?;
    let attempt = view.projection.attempt_count;
    let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))?;
    let linked = history
        .events()
        .iter()
        .rev()
        .find(|event| {
            event.kind == deadreckon_protocol::JobEventKind::ChildLinked
                && event
                    .detail
                    .get("attempt")
                    .and_then(serde_json::Value::as_u64)
                    == Some(u64::from(attempt))
        })
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "strict parent gate has no durable outer launch for Job attempt {attempt}"
            )))
        })?;
    let outer_launch_id = linked
        .detail
        .get("launch_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "strict parent gate has no launch identity for Job attempt {attempt}"
            )))
        })?;
    let owner = deadreckon_runtime::GateLaunchOwner::new(attempt, outer_launch_id)
        .map_err(CliError::Core)?;
    if linked.lease_epoch == 0 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "strict parent gate launch has no fenced lease epoch".to_string(),
        )));
    }
    Ok((owner, linked.lease_epoch))
}

#[allow(clippy::too_many_arguments)]
fn request_parent_repair(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    parent: &mut deadreckon_core::PipelineState,
    merged: &deadreckon_core::PipelineState,
    marker: &deadreckon_core::AcceptanceMarker,
    judgment: &deadreckon_protocol::SemanticJudgment,
    providers: &deadreckon_core::plan::PlanProviders,
) -> Result<ParentCompletion> {
    if judgment.decision != SemanticDecision::Revise
        || judgment.job_id.as_ref() != job.job_id.as_ref()
        || judgment.run_id.as_ref() != job.job_id.as_ref()
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent repair requires a revise judgment for the same Job and result".to_string(),
        )));
    }
    if deadreckon_core::validate_acceptance_marker(parent)? != *marker {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "parent repair marker changed after semantic judging".to_string(),
        )));
    }
    deadreckon_runtime::validate_semantic_judgment_input_against_source(
        parent,
        marker,
        &job.source_cwd,
        judgment,
    )
    .map_err(CliError::Core)?;
    let marker_path = deadreckon_core::marker_path_for_run_root(&parent.run_root);
    let judgment_path = parent
        .run_root
        .join(deadreckon_core::SEMANTIC_JUDGMENT_JSON);
    let marker_sha256 = deadreckon_core::flight::sha256_file(&marker_path)?;
    let judgment_sha256 = deadreckon_core::flight::sha256_file(&judgment_path)?;
    let (launch_owner, requested_after_lease_epoch) = parent_gate_launch_lineage(paths, job)?;
    let intent_path = parent_repair_intent_path(paths, job.job_id.as_ref());

    if let Some(existing) = load_parent_repair_intent(paths, job.job_id.as_ref())?
        && existing.revise_judgment_sha256 == judgment_sha256
    {
        let archive = parent_repair_round_dir(parent, existing.round);
        write_immutable_bytes(
            &archive.join("pre-repair-marker.json"),
            &fs::read(&marker_path)?,
        )?;
        write_immutable_bytes(
            &archive.join("revise-judgment.json"),
            &fs::read(&judgment_path)?,
        )?;
        finalize_parent_repair_request(parent, &existing)?;
        return Ok(ParentCompletion::ReviseRequested {
            reason: existing.feedback.clone(),
            round: existing.round,
            intent_path: intent_path.clone(),
            intent_sha256: deadreckon_core::flight::sha256_file(&intent_path)?,
            judgment_path: archive.join("revise-judgment.json"),
            judgment_sha256,
        });
    }

    let mut previous_round_sha256 = None;
    let round = if let Some(existing) = load_parent_repair_intent(paths, job.job_id.as_ref())? {
        validate_parent_repair_candidate(paths, job, parent, &existing)?;
        archive_active_repair_file(parent, existing.round, &intent_path, "intent.json")?;
        archive_active_repair_file(
            parent,
            existing.round,
            &deadreckon_core::parent_repair_manifest_path_for_run_root(&parent.run_root),
            "final-attempt.json",
        )?;
        archive_active_repair_file(
            parent,
            existing.round,
            &deadreckon_core::parent_repair_candidate_path_for_run_root(&parent.run_root),
            "candidate.json",
        )?;
        previous_round_sha256 = Some(parent_repair_round_chain_sha256(parent, existing.round)?);
        existing.round.checked_add(1).ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "parent repair round overflowed its bounded counter".to_string(),
            ))
        })?
    } else {
        1
    };
    let round_dir = parent_repair_round_dir(parent, round);
    let archived_marker = round_dir.join("pre-repair-marker.json");
    let archived_judgment = round_dir.join("revise-judgment.json");
    write_immutable_bytes(&archived_marker, &fs::read(&marker_path)?)?;
    write_immutable_bytes(&archived_judgment, &fs::read(&judgment_path)?)?;
    if deadreckon_core::flight::sha256_file(&archived_marker)? != marker_sha256
        || deadreckon_core::flight::sha256_file(&archived_judgment)? != judgment_sha256
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "archived parent revision evidence changed while becoming durable".to_string(),
        )));
    }
    let feedback = parent_repair_feedback(judgment);
    let (provider, model) = repair_provider_selection(providers);
    let intent = ParentRepairIntent {
        schema_version: 1,
        job_id: job.job_id.as_ref().to_string(),
        shape: job.shape,
        round,
        merged_run_id: merged.run_id.clone(),
        merged_tree_sha256: parent_tree_sha256(merged)?,
        pre_repair_tree_sha256: parent_tree_sha256(parent)?,
        revise_marker_sha256: marker_sha256,
        revise_judgment_sha256: judgment_sha256.clone(),
        revise_input_sha256: judgment.input_sha256.clone(),
        requested_after_attempt: launch_owner.attempt(),
        requested_after_launch_id: launch_owner.outer_launch_id().to_string(),
        requested_after_lease_epoch,
        provider,
        model,
        feedback: feedback.clone(),
        previous_round_sha256,
        requested_at: Utc::now(),
    };
    commands::job::replace_json_synced(&intent_path, &intent)?;
    finalize_parent_repair_request(parent, &intent)?;
    Ok(ParentCompletion::ReviseRequested {
        reason: feedback,
        round,
        intent_sha256: deadreckon_core::flight::sha256_file(&intent_path)?,
        intent_path,
        judgment_path: archived_judgment,
        judgment_sha256,
    })
}

#[cfg(test)]
pub(crate) fn request_parent_repair_for_test(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    parent: &mut deadreckon_core::PipelineState,
    merged: &deadreckon_core::PipelineState,
    marker: &deadreckon_core::AcceptanceMarker,
    judgment: &deadreckon_protocol::SemanticJudgment,
    providers: &deadreckon_core::plan::PlanProviders,
) -> Result<ParentCompletion> {
    request_parent_repair(paths, job, parent, merged, marker, judgment, providers)
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn install_parent_repair_candidate_for_test(
    paths: &DeadreckonPaths,
    job: &deadreckon_protocol::Job,
    parent: &deadreckon_core::PipelineState,
    attempt: u32,
    launch_id: &str,
    lease_epoch: u64,
    attempt_baseline_tree_sha256: String,
) -> Result<deadreckon_runtime::ParentRepairCandidate> {
    let intent_path = parent_repair_intent_path(paths, job.job_id.as_ref());
    let intent = load_parent_repair_intent(paths, job.job_id.as_ref())?.ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "test parent repair candidate requires an active intent".to_string(),
        ))
    })?;
    validate_parent_repair_intent_lineage(paths, job, &intent_path, &intent, Some(parent))?;
    let intent_sha256 = deadreckon_core::flight::sha256_file(&intent_path)?;
    let manifest_path = deadreckon_core::parent_repair_manifest_path_for_run_root(&parent.run_root);
    let manifest = ParentRepairAttemptManifest {
        schema_version: 1,
        job_id: job.job_id.as_ref().to_string(),
        shape: job.shape,
        round: intent.round,
        merged_run_id: intent.merged_run_id.clone(),
        merged_tree_sha256: intent.merged_tree_sha256.clone(),
        pre_repair_tree_sha256: intent.pre_repair_tree_sha256.clone(),
        intent_sha256: intent_sha256.clone(),
        attempt,
        launch_id: launch_id.to_string(),
        lease_epoch,
        attempt_baseline_tree_sha256,
        started_at: Utc::now(),
    };
    validate_parent_repair_manifest(job, &intent, &manifest)?;
    validate_parent_repair_manifest_history(paths, job, &manifest)?;
    commands::job::replace_json_synced(&manifest_path, &manifest)?;
    let candidate = deadreckon_runtime::ParentRepairCandidate {
        schema_version: 1,
        job_id: job.job_id.as_ref().to_string(),
        run_id: job.job_id.as_ref().to_string(),
        round: intent.round,
        attempt,
        launch_id: launch_id.to_string(),
        lease_epoch,
        intent_sha256,
        manifest_sha256: deadreckon_core::flight::sha256_file(&manifest_path)?,
        result_tree_sha256: parent_tree_sha256(parent)?,
        turn: parent.turn,
        ready_at: Utc::now(),
    };
    commands::job::replace_json_synced(
        &deadreckon_core::parent_repair_candidate_path_for_run_root(&parent.run_root),
        &candidate,
    )?;
    validate_parent_repair_candidate(paths, job, parent, &intent)
}

fn finalize_parent_repair_request(
    parent: &mut deadreckon_core::PipelineState,
    intent: &ParentRepairIntent,
) -> Result<()> {
    remove_if_exists(&deadreckon_core::marker_path_for_run_root(&parent.run_root))?;
    remove_if_exists(
        &parent
            .run_root
            .join(deadreckon_core::SEMANTIC_JUDGMENT_JSON),
    )?;
    remove_if_exists(&deadreckon_core::parent_repair_manifest_path_for_run_root(
        &parent.run_root,
    ))?;
    remove_if_exists(&deadreckon_core::parent_repair_candidate_path_for_run_root(
        &parent.run_root,
    ))?;
    parent.failure_reason = Some(intent.feedback.clone());
    parent.pause_reason = None;
    parent.provider_failure = None;
    parent.status = deadreckon_core::RunStatus::Planned;
    parent.current_phase_id = PhaseId(50);
    deadreckon_core::save_state(parent)?;
    Ok(())
}

fn parent_needs_review(
    state: &mut deadreckon_core::PipelineState,
    reason: &str,
    decision: Option<SemanticDecision>,
    stop_reason: StopReason,
) -> Result<ParentCompletion> {
    state.failure_reason = Some(format!("NEEDS_REVIEW: {reason}"));
    state.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
    deadreckon_core::save_state(state)?;
    Ok(ParentCompletion::NeedsReview {
        reason: reason.to_string(),
        decision,
        stop_reason,
    })
}

fn parent_failed(
    state: &mut deadreckon_core::PipelineState,
    reason: &str,
    stop_reason: StopReason,
) -> Result<ParentCompletion> {
    state.failure_reason = Some(reason.to_string());
    state.set_phase_status(PhaseId(60), PhaseStatus::Failed)?;
    deadreckon_core::save_state(state)?;
    Ok(ParentCompletion::Failed {
        reason: reason.to_string(),
        stop_reason,
    })
}

fn driver_shape_error(job_id: &str) -> CliError {
    CliError::Core(DeadreckonError::InvalidInput(format!(
        "advanced driver kind does not match job {job_id}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned_plan_fixture() -> (
        tempfile::TempDir,
        DeadreckonPaths,
        deadreckon_core::plan::Plan,
        deadreckon_core::plan::Plan,
    ) {
        use deadreckon_core::plan::{Plan, PlanMode, PlanProviders, PlanRole, PlanTask, save_plan};

        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source_cwd = temp.path().join("source");
        std::fs::create_dir_all(&source_cwd).expect("source");
        let job_id = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        deadreckon_core::write_job(
            &paths,
            &deadreckon_protocol::Job {
                schema_version: deadreckon_protocol::JobSchemaVersion::CURRENT,
                job_id: JobId(job_id.to_string()),
                scope: "owned-plan-test".to_string(),
                goal: "complete the owned graph".to_string(),
                shape: JobShape::Graph,
                created_at: Utc::now(),
                source_cwd,
                launch_plan_sha256: "sha256:launch".to_string(),
                authority_sha256: "sha256:authority".to_string(),
                policy: deadreckon_protocol::JobPolicy {
                    max_spend_usd: 1.0,
                    max_wall_seconds: 60,
                    max_attempts: 1,
                    deadline: None,
                    semantic_judge: deadreckon_protocol::SemanticJudgeMode::Required,
                    execution: None,
                },
            },
        )
        .expect("Job");

        let tasks = || {
            vec![
                PlanTask::new(0, "first", "complete first", PlanRole::Child, None),
                PlanTask::new(1, "second", "complete second", PlanRole::Child, None),
            ]
        };
        let mut child = Plan::new(
            "complete the nested work",
            PlanMode::FullPlan,
            tasks(),
            PlanProviders::default(),
            Some("owned-plan-test".to_string()),
            "test",
        )
        .expect("child Plan");
        child.plan_id = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string();
        child.owner_job_id = Some(job_id.to_string());
        child.parent_plan_id = Some(job_id.to_string());

        let mut root = Plan::new(
            "complete the owned graph",
            PlanMode::FullPlan,
            tasks(),
            PlanProviders::default(),
            Some("owned-plan-test".to_string()),
            "test",
        )
        .expect("root Plan");
        root.plan_id = job_id.to_string();
        root.owner_job_id = Some(job_id.to_string());
        root.tasks[0].subplan = Some(child.plan_id.clone());
        save_plan(&paths, &child).expect("save child");
        save_plan(&paths, &root).expect("save root");
        (temp, paths, root, child)
    }

    #[test]
    fn plan_task_run_is_linked_once_in_owning_job_history() {
        let (_temp, paths, root, _child) = owned_plan_fixture();
        let owner = deadreckon_core::LeaseOwner {
            owner_id: "plan-child-link-test".to_string(),
            boot_id: "plan-child-link-boot".to_string(),
            pid: std::process::id(),
            process_group: std::process::id(),
        };
        let token = deadreckon_core::claim_job_lease(
            &paths,
            &JobId(root.plan_id.clone()),
            &owner,
            Utc::now(),
            std::time::Duration::from_secs(60),
        )
        .expect("claim owning Job")
        .token();
        let task = &root.tasks[0];
        let run_id = plan_task_run_id(&root.plan_id, &root.plan_id, &task.task_id, 1);
        prepare_plan_task_run_fenced(
            &paths,
            &token,
            &root.plan_id,
            &task.task_id,
            task.index,
            1,
            &run_id,
        )
        .expect("prepare Plan task Run");
        let ownership = deadreckon_core::RunOwnership::plan_task(
            root.plan_id.clone(),
            root.plan_id.clone(),
            task.task_id.clone(),
            task.index,
            1,
        );
        let job = deadreckon_core::load_job(&paths, &root.plan_id).expect("Job");
        let state = deadreckon_core::create_owned_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: task.goal.clone(),
                cwd: job.source_cwd,
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(30.0),
                run_id: Some(run_id.clone()),
                codebase: None,
            },
            ownership,
        )
        .expect("owned Plan task Run");

        reconcile_plan_task_run_links(&paths, &token, &root)
            .expect("recovery links the preassigned Run after a crash window");
        link_plan_task_run_fenced(
            &paths,
            &token,
            &state,
            &root.plan_id,
            &task.task_id,
            task.index,
            1,
        )
        .expect("same link is idempotent");

        let history = deadreckon_core::read_job_history(&paths.job_events(&root.plan_id))
            .expect("Job history");
        let links = history
            .events()
            .iter()
            .filter(|event| {
                event.kind == JobEventKind::ChildLinked
                    && event
                        .detail
                        .get("relationship")
                        .and_then(serde_json::Value::as_str)
                        == Some("plan_task")
            })
            .collect::<Vec<_>>();
        assert_eq!(links.len(), 1);
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| {
                    plan_task_event_matches(
                        event,
                        JobEventKind::ChildLaunchPrepared,
                        &root.plan_id,
                        &task.task_id,
                        task.index,
                        1,
                    )
                })
                .count(),
            1
        );
        assert_eq!(links[0].detail["run_id"], state.run_id);
        let view = deadreckon_core::JobView::load(&paths, &root.plan_id).expect("Job view");
        assert!(
            view.projection.child_run_ids.contains(&state.run_id),
            "the Job projection must own its leaf Run"
        );
        assert_eq!(
            deadreckon_core::load_plan(&paths, &root.plan_id)
                .expect("recovered Plan")
                .tasks[0]
                .child_run_id
                .as_deref(),
            Some(state.run_id.as_str()),
            "recovery must also restore the Plan's child pointer"
        );

        let mut conflicting = state.clone();
        conflicting.run_id = "different-plan-child-run".to_string();
        let error = link_plan_task_run_fenced(
            &paths,
            &token,
            &conflicting,
            &root.plan_id,
            &task.task_id,
            task.index,
            1,
        )
        .expect_err("same task attempt cannot link another Run");
        assert!(error.to_string().contains("has no fenced launch record"));
    }

    struct ParentRepairAuthorityFixture {
        _temp: tempfile::TempDir,
        paths: DeadreckonPaths,
        job: deadreckon_protocol::Job,
        merged: deadreckon_core::PipelineState,
        parent: deadreckon_core::PipelineState,
        token: deadreckon_core::LeaseToken,
        intent_path: PathBuf,
        round_dir: PathBuf,
    }

    fn append_parent_repair_authority_event(
        paths: &DeadreckonPaths,
        token: &deadreckon_core::LeaseToken,
        kind: deadreckon_protocol::JobEventKind,
        detail: serde_json::Value,
    ) {
        let projection =
            deadreckon_core::JobView::load(paths, token.job_id.as_ref()).expect("Job projection");
        let sequence = projection
            .projection
            .last_sequence
            .checked_add(1)
            .and_then(deadreckon_protocol::JobEventSequence::new)
            .expect("next sequence");
        let now = Utc::now();
        deadreckon_core::append_fenced_job_event(
            paths,
            token,
            now,
            &deadreckon_protocol::JobEvent {
                schema_version: deadreckon_protocol::JobSchemaVersion::CURRENT,
                job_id: token.job_id.clone(),
                sequence,
                event_id: Uuid::new_v4().to_string(),
                causation_id: format!("parent-repair-test:{kind:?}"),
                timestamp: now,
                lease_epoch: token.epoch,
                kind,
                detail,
            },
        )
        .expect("fenced authority event");
    }

    fn parent_repair_authority_fixture() -> ParentRepairAuthorityFixture {
        let (temp, paths, root, _child) = owned_plan_fixture();
        let job = deadreckon_core::load_job(&paths, root.plan_id.as_str()).expect("Job");
        let authority = deadreckon_protocol::JobAuthority {
            schema_version: deadreckon_protocol::JobSchemaVersion::CURRENT,
            job_id: job.job_id.clone(),
            run_id: deadreckon_protocol::RunId(job.job_id.as_ref().to_string()),
            approved_at: Utc::now(),
            accepted_by: deadreckon_protocol::AuthorityAcceptedBy::Operator,
            goal_sha256: deadreckon_core::flight::sha256_text(&job.goal),
            contract_sha256: "sha256:test-contract".to_string(),
            effective_policy_sha256: deadreckon_core::flight::sha256_text(
                &serde_json::to_string(&job.policy).expect("policy json"),
            ),
            launch_plan_sha256: job.launch_plan_sha256.clone(),
            source_tree_sha256: deadreckon_core::flight::build_deliverable_file_index(
                &job.source_cwd,
            )
            .expect("source index")
            .tree_hash(),
            source_revision: None,
            sandbox_requested: "none".to_string(),
            semantic_judge_mode: deadreckon_protocol::SemanticJudgeMode::Required,
            gate_evaluator_sha256: None,
        };
        let authority_path = paths.job_authority(job.job_id.as_ref());
        fs::write(
            &authority_path,
            serde_json::to_vec_pretty(&authority).expect("authority json"),
        )
        .expect("authority");
        let now = Utc::now();
        let first_owner = deadreckon_core::LeaseOwner {
            owner_id: "repair-owner-1".to_string(),
            boot_id: "repair-boot-1".to_string(),
            pid: 1,
            process_group: 1,
        };
        let first = deadreckon_core::claim_job_lease(
            &paths,
            &job.job_id,
            &first_owner,
            now,
            std::time::Duration::from_secs(60),
        )
        .expect("first lease");
        assert_eq!(first.lease.epoch, 1);
        let second_owner = deadreckon_core::LeaseOwner {
            owner_id: "repair-owner-2".to_string(),
            boot_id: "repair-boot-2".to_string(),
            pid: 2,
            process_group: 2,
        };
        let token = deadreckon_core::claim_job_lease(
            &paths,
            &job.job_id,
            &second_owner,
            now,
            std::time::Duration::from_secs(60),
        )
        .expect("reclaimed lease")
        .token();
        assert_eq!(token.epoch, 2);

        append_parent_repair_authority_event(
            &paths,
            &token,
            deadreckon_protocol::JobEventKind::AttemptStarted,
            json!({ "attempt": 1 }),
        );
        let revise_launch = Uuid::new_v4().to_string();
        append_parent_repair_authority_event(
            &paths,
            &token,
            deadreckon_protocol::JobEventKind::ChildLinked,
            json!({
                "attempt": 1,
                "launch_id": revise_launch,
            }),
        );

        let merged = deadreckon_core::create_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: job.goal.clone(),
                cwd: job.source_cwd.clone(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "test".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("parent-repair-merged".to_string()),
                codebase: None,
            },
        )
        .expect("merged run");
        fs::write(merged.working_dir.join("result.txt"), "merged result\n").expect("merged result");
        deadreckon_core::save_state(&merged).expect("merged state");

        let mut parent = deadreckon_core::create_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: job.goal.clone(),
                cwd: job.source_cwd.clone(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "test".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some(job.job_id.as_ref().to_string()),
                codebase: None,
            },
        )
        .expect("parent run");
        fs::write(parent.working_dir.join("result.txt"), "parent result\n").expect("parent result");
        deadreckon_core::save_state(&parent).expect("parent state");

        let key =
            deadreckon_core::read_gate_key(&paths, job.job_id.as_ref()).expect("parent gate key");
        let marker = deadreckon_core::write_native_acceptance_marker_with_results_and_key(
            &parent.run_root,
            parent.run_id.clone(),
            parent.working_dir.clone(),
            vec![deadreckon_core::AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: true,
                must_pass: true,
                detail: "parent result exists".to_string(),
                command: None,
                cwd: None,
                duration_ms: Some(1),
                stdout: None,
                stderr: None,
            }],
            &key,
            deadreckon_core::AcceptanceContainment::contained("test"),
        )
        .expect("parent marker");
        let evidence = deadreckon_runtime::build_semantic_evidence_against_source(
            &parent,
            &marker,
            &job.source_cwd,
        )
        .expect("semantic evidence");
        let judgment = deadreckon_protocol::SemanticJudgment {
            schema_version: deadreckon_protocol::JobSchemaVersion::CURRENT,
            job_id: job.job_id.clone(),
            run_id: deadreckon_protocol::RunId(job.job_id.as_ref().to_string()),
            judged_at: Utc::now(),
            provider: "test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Revise,
            summary: "the parent requires repair".to_string(),
            goal_coverage: Vec::new(),
            missing: vec!["repair the result".to_string()],
            input_sha256: deadreckon_core::flight::sha256_text(
                &serde_json::to_string(&evidence).expect("evidence json"),
            ),
            spend_usd: 0.0,
        };
        deadreckon_runtime::persist_semantic_judgment(&parent.run_root, &judgment)
            .expect("revise judgment");
        let requested = request_parent_repair(
            &paths,
            &job,
            &mut parent,
            &merged,
            &marker,
            &judgment,
            &root.providers,
        )
        .expect("parent repair request");
        let ParentCompletion::ReviseRequested {
            round, intent_path, ..
        } = requested
        else {
            panic!("fixture must create a repair request")
        };
        let round_dir = parent_repair_round_dir(&parent, round);
        let intent = load_parent_repair_intent(&paths, job.job_id.as_ref())
            .expect("intent")
            .expect("active intent");
        append_parent_repair_authority_event(
            &paths,
            &token,
            deadreckon_protocol::JobEventKind::SemanticJudgeRevise,
            json!({
                "round": round,
                "intent_sha256": deadreckon_core::flight::sha256_file(&intent_path)
                    .expect("intent digest"),
                "judgment_sha256": intent.revise_judgment_sha256.as_str(),
            }),
        );
        validate_parent_repair_intent_lineage(&paths, &job, &intent_path, &intent, Some(&parent))
            .expect("valid fixture lineage");

        ParentRepairAuthorityFixture {
            _temp: temp,
            paths,
            job,
            merged,
            parent,
            token,
            intent_path,
            round_dir,
        }
    }

    #[test]
    fn parent_repair_candidate_rejects_stale_lease_and_mismatched_launch() {
        let fixture = parent_repair_authority_fixture();
        append_parent_repair_authority_event(
            &fixture.paths,
            &fixture.token,
            deadreckon_protocol::JobEventKind::AttemptStarted,
            json!({ "attempt": 2 }),
        );
        let launch_id = Uuid::new_v4().to_string();
        append_parent_repair_authority_event(
            &fixture.paths,
            &fixture.token,
            deadreckon_protocol::JobEventKind::ChildLinked,
            json!({
                "attempt": 2,
                "launch_id": launch_id,
            }),
        );
        let baseline = parent_tree_sha256(&fixture.parent).expect("parent tree");

        let stale = install_parent_repair_candidate_for_test(
            &fixture.paths,
            &fixture.job,
            &fixture.parent,
            2,
            &launch_id,
            fixture.token.epoch - 1,
            baseline.clone(),
        )
        .expect_err("stale lease epoch must be rejected");
        assert!(
            stale.to_string().contains("crosses authority generations"),
            "{stale}"
        );

        let mismatched = install_parent_repair_candidate_for_test(
            &fixture.paths,
            &fixture.job,
            &fixture.parent,
            2,
            &Uuid::new_v4().to_string(),
            fixture.token.epoch,
            baseline.clone(),
        )
        .expect_err("unrecorded launch must be rejected");
        assert!(
            mismatched
                .to_string()
                .contains("not backed by its fenced Job attempt and launch"),
            "{mismatched}"
        );

        install_parent_repair_candidate_for_test(
            &fixture.paths,
            &fixture.job,
            &fixture.parent,
            2,
            &launch_id,
            fixture.token.epoch,
            baseline,
        )
        .expect("matching fenced candidate");
    }

    #[test]
    fn parent_repair_lineage_rejects_tampered_archived_proofs_and_chain() {
        let fixture = parent_repair_authority_fixture();

        for proof in ["pre-repair-marker.json", "revise-judgment.json"] {
            let path = fixture.round_dir.join(proof);
            let original = fs::read(&path).expect("archived proof");
            let mut tampered = original.clone();
            tampered.push(b'\n');
            fs::write(&path, tampered).expect("tamper archived proof");
            let intent = load_parent_repair_intent(&fixture.paths, fixture.job.job_id.as_ref())
                .expect("intent")
                .expect("active intent");
            let error = validate_parent_repair_intent_lineage(
                &fixture.paths,
                &fixture.job,
                &fixture.intent_path,
                &intent,
                Some(&fixture.parent),
            )
            .expect_err("tampered archived proof must fail closed");
            assert!(
                error
                    .to_string()
                    .contains("no longer matches its archived deterministic and semantic evidence"),
                "{error}"
            );
            fs::write(&path, original).expect("restore archived proof");
        }

        let original_intent = fs::read(&fixture.intent_path).expect("intent bytes");
        let mut intent: ParentRepairIntent =
            serde_json::from_slice(&original_intent).expect("intent json");
        intent.previous_round_sha256 = Some("sha256:forged-lineage".to_string());
        fs::write(
            &fixture.intent_path,
            serde_json::to_vec_pretty(&intent).expect("tampered intent json"),
        )
        .expect("tampered intent");
        let error = validate_parent_repair_intent_lineage(
            &fixture.paths,
            &fixture.job,
            &fixture.intent_path,
            &intent,
            Some(&fixture.parent),
        )
        .expect_err("forged first-round lineage must fail closed");
        assert!(
            error
                .to_string()
                .contains("parent repair Job event disagrees with the active intent"),
            "{error}"
        );

        fs::write(&fixture.intent_path, original_intent).expect("restore intent");
        let restored = load_parent_repair_intent(&fixture.paths, fixture.job.job_id.as_ref())
            .expect("restored intent")
            .expect("active intent");
        validate_parent_repair_intent_lineage(
            &fixture.paths,
            &fixture.job,
            &fixture.intent_path,
            &restored,
            Some(&fixture.parent),
        )
        .expect("restored lineage");
    }

    #[test]
    fn parent_repair_pending_projects_typed_retry_failure_budget_and_containment_reasons() {
        let mut fixture = parent_repair_authority_fixture();
        fixture.parent.status = deadreckon_core::RunStatus::Failed;
        fixture.parent.failure_reason = Some("provider connection reset".to_string());
        fixture.parent.provider_failure =
            Some(deadreckon_core::ProviderFailureDisposition::Retryable);
        deadreckon_core::save_state(&fixture.parent).expect("retryable parent");

        let transient = pending_parent_repair_completion(
            &fixture.paths,
            &fixture.job,
            &mut fixture.parent,
            &fixture.merged,
        )
        .expect("retryable projection")
        .expect("repair remains pending");
        assert!(matches!(
            transient,
            ParentCompletion::RepairPending {
                stop_reason: StopReason::TransientProvider,
                ..
            }
        ));

        fixture.parent.status = deadreckon_core::RunStatus::Executing;
        fixture.parent.failure_reason = None;
        fixture.parent.provider_failure = None;
        deadreckon_core::save_state(&fixture.parent).expect("interrupted parent");
        let interrupted = pending_parent_repair_completion(
            &fixture.paths,
            &fixture.job,
            &mut fixture.parent,
            &fixture.merged,
        )
        .expect("interrupted projection")
        .expect("repair remains pending");
        assert!(matches!(
            interrupted,
            ParentCompletion::RepairPending {
                stop_reason: StopReason::LostContainment,
                ..
            }
        ));

        fixture.parent.status = deadreckon_core::RunStatus::Failed;
        fixture.parent.failure_reason = Some("provider rejected the repair".to_string());
        fixture.parent.provider_failure = Some(deadreckon_core::ProviderFailureDisposition::Fatal);
        deadreckon_core::save_state(&fixture.parent).expect("fatal parent");
        let fatal = pending_parent_repair_completion(
            &fixture.paths,
            &fixture.job,
            &mut fixture.parent,
            &fixture.merged,
        )
        .expect("fatal projection")
        .expect("repair has a terminal classification");
        assert!(matches!(
            fatal,
            ParentCompletion::RepairFailed {
                stop_reason: StopReason::FatalProvider,
                ..
            }
        ));

        fixture.parent.pause_reason =
            Some("approved aggregate spend cap was exhausted before parent repair".to_string());
        fixture.parent.failure_reason = fixture.parent.pause_reason.clone();
        fixture.parent.provider_failure = None;
        deadreckon_core::save_state(&fixture.parent).expect("budget parent");
        let budget = pending_parent_repair_completion(
            &fixture.paths,
            &fixture.job,
            &mut fixture.parent,
            &fixture.merged,
        )
        .expect("budget projection")
        .expect("repair has a terminal budget classification");
        assert!(matches!(
            budget,
            ParentCompletion::BudgetExhausted {
                stop_reason: StopReason::SpendCap,
                ..
            }
        ));
    }

    #[test]
    fn owned_plan_lineage_and_definition_fail_closed() {
        use deadreckon_core::plan::{PlanStatus, PlanTaskStatus, save_plan};

        let (_temp, paths, root, mut child) = owned_plan_fixture();
        let owner = resolve_plan_owner(&paths, &child)
            .expect("resolve")
            .expect("owned");
        assert_eq!(owner.job.job_id.as_ref(), root.plan_id);
        assert_eq!(owner.root_plan_id, root.plan_id);
        assert_eq!(
            owner.lineage,
            vec![child.plan_id.clone(), root.plan_id.clone()]
        );

        record_owned_plan_tree(&paths, &root).expect("freeze definitions");
        validate_owned_plan_if_present(&paths, &root.plan_id, &root.plan_id, &root.plan_id)
            .expect("root definition");
        validate_owned_plan_if_present(&paths, &child.plan_id, &root.plan_id, &root.plan_id)
            .expect("child definition");

        let mut tampered_root = root.clone();
        tampered_root.tasks[0].goal = "tampered parent scheduling goal".to_string();
        save_plan(&paths, &tampered_root).expect("save tampered parent");
        let owner = resolve_plan_owner(&paths, &child)
            .expect("resolve tampered lineage")
            .expect("owned");
        let parent_error =
            validate_owned_plan_lineage(&paths, &owner).expect_err("parent tampering must fail");
        assert!(
            parent_error
                .to_string()
                .contains("protected Job-owned definition"),
            "{parent_error}"
        );
        save_plan(&paths, &root).expect("restore parent");

        child.status = PlanStatus::Forked;
        child.conductor_pid = Some(123);
        child.tasks[0].status = PlanTaskStatus::Running;
        save_plan(&paths, &child).expect("save mutable lifecycle");
        validate_owned_plan_if_present(&paths, &child.plan_id, &root.plan_id, &root.plan_id)
            .expect("mutable lifecycle fields are not authority");

        child.tasks[0].goal = "tampered approved goal".to_string();
        save_plan(&paths, &child).expect("save tampered definition");
        let error =
            validate_owned_plan_if_present(&paths, &child.plan_id, &root.plan_id, &root.plan_id)
                .expect_err("definition tampering must fail");
        assert!(
            error.to_string().contains("protected Job-owned definition"),
            "{error}"
        );
    }

    #[test]
    fn owned_plan_lineage_rejects_broken_links_cycles_and_excess_depth() {
        use deadreckon_core::plan::{Plan, PlanMode, PlanProviders, PlanRole, PlanTask, save_plan};

        let (_temp, paths, mut root, child) = owned_plan_fixture();
        root.tasks[0].subplan = None;
        save_plan(&paths, &root).expect("broken root");
        let broken = resolve_plan_owner(&paths, &child).expect_err("broken reverse link");
        assert!(
            broken.to_string().contains("unique parent-task link"),
            "{broken}"
        );

        let (_temp, paths, _root, mut child) = owned_plan_fixture();
        child.parent_plan_id = Some(child.plan_id.clone());
        child.tasks[0].subplan = Some(child.plan_id.clone());
        save_plan(&paths, &child).expect("cyclic child");
        let cycle = resolve_plan_owner(&paths, &child).expect_err("cycle");
        assert!(cycle.to_string().contains("cycle"), "{cycle}");

        let (_temp, paths, root, mut child) = owned_plan_fixture();
        let tasks = vec![
            PlanTask::new(
                0,
                "deep first",
                "complete deep first",
                PlanRole::Child,
                None,
            ),
            PlanTask::new(
                1,
                "deep second",
                "complete deep second",
                PlanRole::Child,
                None,
            ),
        ];
        let mut grandchild = Plan::new(
            "unapproved excessive nesting",
            PlanMode::FullPlan,
            tasks,
            PlanProviders::default(),
            Some("owned-plan-test".to_string()),
            "test",
        )
        .expect("grandchild");
        grandchild.plan_id = "cccccccccccccccccccccccccccccccc".to_string();
        grandchild.owner_job_id = root.owner_job_id.clone();
        grandchild.parent_plan_id = Some(child.plan_id.clone());
        child.tasks[0].subplan = Some(grandchild.plan_id.clone());
        save_plan(&paths, &child).expect("deep child");
        save_plan(&paths, &grandchild).expect("grandchild");
        let depth = resolve_plan_owner(&paths, &grandchild).expect_err("excess depth");
        assert!(depth.to_string().contains("nesting cap"), "{depth}");
    }

    #[test]
    fn merge_repair_authority_cannot_roll_back_to_an_older_committed_value() {
        let repair_id = "sha256:repair";
        let old = json!({"status": "process_prepared", "run_id": "run-1"});
        let current = json!({"status": "trusted", "run_id": "run-1"});
        let event = |sequence, authority: &serde_json::Value| deadreckon_protocol::JobEvent {
            schema_version: deadreckon_protocol::JobSchemaVersion::CURRENT,
            job_id: JobId("authority-rollback-job".to_string()),
            sequence: deadreckon_protocol::JobEventSequence::new(sequence).expect("sequence"),
            event_id: Uuid::new_v4().to_string(),
            causation_id: format!("authority:{sequence}"),
            timestamp: Utc::now(),
            lease_epoch: 1,
            kind: JobEventKind::RepairChildAuthorityChanged,
            detail: json!({
                "repair_id": repair_id,
                "authority": authority,
            }),
        };
        let events = vec![event(1, &old), event(2, &current)];

        assert!(latest_merge_repair_authority_matches(
            &events, repair_id, &current
        ));
        assert!(!latest_merge_repair_authority_matches(
            &events, repair_id, &old
        ));
    }

    #[test]
    fn owned_campaign_definition_excludes_lifecycle_but_detects_schedule_tampering() {
        use deadreckon_core::campaign::{Campaign, CampaignStatus, SubGoalStatus, build_sub_goals};
        use deadreckon_core::plan::PlanProviders;

        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let job_id = "dddddddddddddddddddddddddddddddd";
        let mut campaign = Campaign::new(
            "complete the protected campaign",
            build_sub_goals(
                vec!["complete first".to_string(), "complete second".to_string()],
                2,
            )
            .expect("sub-goals"),
            PlanProviders::default(),
            0,
            Some(2.0),
            Some(60.0),
            "test",
        )
        .expect("Campaign");
        campaign.campaign_id = job_id.to_string();
        let record = OwnedCampaignRecord {
            schema_version: 1,
            job_id: job_id.to_string(),
            immutable_definition_sha256: immutable_campaign_sha256(&campaign).expect("digest"),
            recorded_at: Utc::now(),
        };
        commands::job::write_json_synced(&owned_campaign_record_path(&paths, job_id), &record)
            .expect("protected Campaign record");
        validate_owned_campaign(&paths, &campaign, job_id).expect("initial definition");

        campaign.status = CampaignStatus::Forked;
        campaign.sub_goals[0].status = SubGoalStatus::Merged;
        campaign.sub_goals[0].sub_plan_id = Some("runtime-plan".to_string());
        campaign.sub_goals[0].result_run_id = Some("runtime-result".to_string());
        validate_owned_campaign(&paths, &campaign, job_id)
            .expect("mutable lifecycle remains valid");

        campaign.sub_goals[0].goal = "tampered scheduled work".to_string();
        let error = validate_owned_campaign(&paths, &campaign, job_id)
            .expect_err("schedule tampering must fail");
        assert!(
            error.to_string().contains("protected Job-owned definition"),
            "{error}"
        );
    }

    #[test]
    fn delegation_claim_refuses_replay_when_pending_and_consumed_both_exist() {
        let temp = tempfile::TempDir::new().expect("temp");
        let pending = temp.path().join("pending").join("capability.json");
        let consumed = temp.path().join("consumed").join("capability.json");
        fs::create_dir_all(pending.parent().expect("pending parent")).expect("pending directory");
        fs::write(&pending, b"first protected record").expect("pending record");

        claim_delegation_record(&pending, &consumed, b"first protected record")
            .expect("first claim");
        assert!(!pending.exists());
        assert_eq!(
            fs::read(&consumed).expect("consumed record"),
            b"first protected record"
        );

        fs::write(&pending, b"replayed protected record").expect("replayed pending record");
        let error = claim_delegation_record(&pending, &consumed, b"replayed protected record")
            .expect_err("consumed tombstone must fail closed");
        assert!(error.to_string().contains("already consumed"), "{error}");
        assert!(
            pending.exists(),
            "the replay record remains available for recovery diagnostics"
        );
        assert_eq!(
            fs::read(&consumed).expect("original tombstone"),
            b"first protected record",
            "a replay must not replace the original consumed evidence"
        );
    }

    #[cfg(unix)]
    #[test]
    fn delegation_claim_rejects_a_symlinked_pending_capability() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().expect("temp");
        let target = temp.path().join("attacker.json");
        let pending = temp.path().join("pending").join("capability.json");
        let consumed = temp.path().join("consumed").join("capability.json");
        fs::write(&target, b"attacker controlled").expect("target");
        fs::create_dir_all(pending.parent().expect("pending parent")).expect("pending directory");
        symlink(&target, &pending).expect("pending symlink");

        let error = claim_delegation_record(&pending, &consumed, b"attacker controlled")
            .expect_err("symlinked pending record must fail closed");
        assert!(error.to_string().contains("non-symlink"), "{error}");
        assert!(!consumed.exists(), "no consumed authority may be minted");
        assert_eq!(
            fs::read(&target).expect("target remains"),
            b"attacker controlled"
        );
    }

    fn campaign_sub_launch_fixture(released: bool, linked: bool) -> CampaignSubLaunchAuthority {
        let launch_id = Uuid::new_v4().to_string();
        let outer_launch_id = Uuid::new_v4().to_string();
        let mut process = deadreckon_core::SupervisedProcessRecord::prepared(
            deadreckon_core::SupervisedProcess {
                pid: std::process::id(),
                pgid: None,
            },
            launch_id.clone(),
            2,
            Some(outer_launch_id.clone()),
            "sha256:release".to_string(),
        )
        .expect("process identity");
        #[cfg(unix)]
        {
            process.process.pgid = Some(process.process.pid);
        }
        process.phase = deadreckon_core::SupervisedProcessPhase::Running;
        CampaignSubLaunchAuthority {
            schema_version: 1,
            launch_protocol: CAMPAIGN_SUB_LAUNCH_PROTOCOL.to_string(),
            parent_job_id: "campaign-job".to_string(),
            campaign_id: "campaign-job".to_string(),
            sub_id: "sub-1".to_string(),
            plan_id: "reserved-plan".to_string(),
            attempt: 2,
            lease_epoch: 7,
            outer_launch_id,
            launch_id,
            capability_id: Uuid::new_v4().to_string(),
            release_token_sha256: "sha256:release".to_string(),
            process,
            released,
            linked,
            adopted: false,
            adopted_by_attempt: None,
            adopted_by_lease_epoch: None,
            prepared_at: Utc::now(),
            released_at: released.then(Utc::now),
            linked_at: linked.then(Utc::now),
            adopted_at: None,
        }
    }

    #[test]
    fn campaign_sub_crash_before_release_is_provably_safe_to_relaunch() {
        let launch = campaign_sub_launch_fixture(false, false);
        assert_eq!(
            classify_campaign_sub_recovery(
                &launch,
                deadreckon_core::SupervisedProcessIdentity::Exited,
                false,
                true,
                false,
                false,
            )
            .expect("classify pre-release crash"),
            CampaignSubRecoveryDisposition::RelaunchNonexecuted
        );
    }

    #[test]
    fn campaign_sub_crash_after_release_before_link_is_still_nonexecuted() {
        let launch = campaign_sub_launch_fixture(true, false);
        assert_eq!(
            classify_campaign_sub_recovery(
                &launch,
                deadreckon_core::SupervisedProcessIdentity::Exited,
                true,
                true,
                false,
                false,
            )
            .expect("classify release-before-link crash"),
            CampaignSubRecoveryDisposition::RelaunchNonexecuted
        );
        assert_eq!(
            classify_campaign_sub_recovery(
                &launch,
                deadreckon_core::SupervisedProcessIdentity::Exited,
                true,
                false,
                true,
                false,
            )
            .expect("classify capability-consumed-before-link crash"),
            CampaignSubRecoveryDisposition::RelaunchNonexecuted,
            "consuming the capability is not execution: only the durable link releases CLI dispatch"
        );
    }

    #[test]
    fn campaign_sub_link_projection_without_its_fenced_event_is_not_execution() {
        let launch = campaign_sub_launch_fixture(true, true);
        assert_eq!(
            classify_campaign_sub_recovery(
                &launch,
                deadreckon_core::SupervisedProcessIdentity::Exited,
                true,
                false,
                true,
                false,
            )
            .expect("classify uncommitted link projection"),
            CampaignSubRecoveryDisposition::RelaunchNonexecuted
        );
    }

    #[test]
    fn campaign_sub_link_before_dispatch_relaunches_after_a_simulated_boot_change() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let launch = campaign_sub_launch_fixture(true, true);
        fs::create_dir_all(paths.plan_dir(&launch.campaign_id)).expect("Campaign directory");

        let disposition = classify_campaign_sub_recovery(
            &launch,
            deadreckon_core::SupervisedProcessIdentity::DifferentBoot,
            true,
            false,
            true,
            true,
        )
        .expect("classify rebooted linked launch");
        assert_eq!(
            disposition,
            CampaignSubRecoveryDisposition::RecoverLinkedArtifacts
        );
        assert!(matches!(
            recover_linked_campaign_sub_artifacts(
                &paths,
                &launch,
                deadreckon_core::SupervisedProcessIdentity::DifferentBoot,
            )
            .expect("recover empty dispatch after boot change"),
            CampaignSubLaunchRecovery::Relaunch
        ));
    }

    #[test]
    fn campaign_sub_authority_rejects_unknown_outer_and_process_fields() {
        let launch = campaign_sub_launch_fixture(false, false);
        let mut outer = serde_json::to_value(&launch).expect("serialize authority");
        outer
            .as_object_mut()
            .expect("authority object")
            .insert("forged".to_string(), json!(true));
        assert!(
            serde_json::from_value::<CampaignSubLaunchAuthority>(outer).is_err(),
            "unknown outer authority fields must fail closed"
        );

        let mut nested = serde_json::to_value(&launch).expect("serialize authority");
        nested
            .get_mut("process")
            .and_then(serde_json::Value::as_object_mut)
            .expect("process object")
            .insert("forged".to_string(), json!(true));
        assert!(
            serde_json::from_value::<CampaignSubLaunchAuthority>(nested).is_err(),
            "unknown embedded process fields must fail closed"
        );

        let action = json!({
            "kind": "campaign_sub",
            "campaign_id": "campaign-job",
            "sub_id": "sub-1",
            "plan_id": "reserved-plan",
            "forged": true,
        });
        assert!(
            serde_json::from_value::<DelegatedAction>(action).is_err(),
            "unknown delegated action fields must fail closed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn campaign_sub_authority_rejects_a_mismatched_process_group() {
        let mut launch = campaign_sub_launch_fixture(false, false);
        launch.process.process.pgid = Some(launch.process.process.pid.saturating_add(1));
        let error = validate_campaign_sub_launch_identity(&launch)
            .expect_err("mismatched Campaign process group must fail closed");
        assert!(
            error.to_string().contains("mismatched process group"),
            "{error}"
        );
    }

    #[test]
    fn campaign_sub_control_reads_are_bounded() {
        let temp = tempfile::TempDir::new().expect("temp");
        let path = temp.path().join("oversized.json");
        fs::write(&path, vec![b'x'; MAX_DELEGATION_RECORD_BYTES as usize + 1])
            .expect("oversized fixture");
        let error = read_bounded_regular_control_file(&path, "test control record")
            .expect_err("oversized control record must fail closed");
        assert!(error.to_string().contains("bounded size"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn campaign_sub_authority_read_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let launch = campaign_sub_launch_fixture(false, false);
        let target = temp.path().join("attacker-authority.json");
        fs::write(
            &target,
            serde_json::to_vec_pretty(&launch).expect("authority JSON"),
        )
        .expect("authority target");
        let link = campaign_sub_launch_path(
            &paths,
            &launch.parent_job_id,
            &launch.sub_id,
            &launch.plan_id,
        );
        fs::create_dir_all(link.parent().expect("authority parent")).expect("authority directory");
        symlink(&target, &link).expect("authority symlink");
        let error = load_campaign_sub_launch(
            &paths,
            &launch.parent_job_id,
            &launch.sub_id,
            &launch.plan_id,
        )
        .expect_err("symlinked authority must fail closed");
        assert!(error.to_string().contains("non-symlink"), "{error}");
    }

    #[cfg(unix)]
    fn spawned_campaign_sub_fixture(
        paths: &DeadreckonPaths,
        job_id: &str,
        suffix: &str,
    ) -> (
        std::process::Child,
        Box<dyn deadreckon_core::ChildTerminator>,
        CampaignSubLaunchAuthority,
    ) {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "trap '' HUP; sleep 30 & wait"]);
        let (child, terminator) = deadreckon_core::spawn_grouped(command).expect("grouped child");
        let launch_id = Uuid::new_v4().to_string();
        let outer_launch_id = Uuid::new_v4().to_string();
        let mut process = deadreckon_core::SupervisedProcessRecord::prepared(
            deadreckon_core::SupervisedProcess {
                pid: child.id(),
                pgid: None,
            },
            launch_id.clone(),
            1,
            Some(outer_launch_id.clone()),
            format!("sha256:release-{suffix}"),
        )
        .expect("process identity");
        process.process.pgid = Some(child.id());
        process.phase = deadreckon_core::SupervisedProcessPhase::Running;
        let launch = CampaignSubLaunchAuthority {
            schema_version: 1,
            launch_protocol: CAMPAIGN_SUB_LAUNCH_PROTOCOL.to_string(),
            parent_job_id: job_id.to_string(),
            campaign_id: job_id.to_string(),
            sub_id: format!("sub-{suffix}"),
            plan_id: format!("plan-{suffix}"),
            attempt: 1,
            lease_epoch: 1,
            outer_launch_id,
            launch_id,
            capability_id: Uuid::new_v4().to_string(),
            release_token_sha256: format!("sha256:release-{suffix}"),
            process,
            released: false,
            linked: false,
            adopted: false,
            adopted_by_attempt: None,
            adopted_by_lease_epoch: None,
            prepared_at: Utc::now(),
            released_at: None,
            linked_at: None,
            adopted_at: None,
        };
        write_campaign_sub_launch(paths, &launch).expect("launch authority");
        let pending = delegation_pending_path(paths, &launch.parent_job_id, &launch.capability_id);
        fs::create_dir_all(pending.parent().expect("pending parent")).expect("pending directory");
        fs::write(&pending, b"pending").expect("pending capability");
        (child, terminator, launch)
    }

    #[cfg(unix)]
    #[test]
    fn campaign_cancellation_terminates_the_nested_process_group_and_revokes_pending() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let job_id = "campaign-cancel";
        let (mut child, _terminator, launch) =
            spawned_campaign_sub_fixture(&paths, job_id, "cancel");

        reconcile_campaign_sub_processes_for_job(&paths, job_id, std::time::Duration::ZERO)
            .expect("reconcile Campaign sub-processes");

        child.wait().expect("reap Campaign sub-process");
        assert!(
            !delegation_pending_path(&paths, job_id, &launch.capability_id).exists(),
            "cancellation must revoke the unconsumed child capability"
        );
    }

    #[cfg(unix)]
    #[test]
    fn campaign_cancellation_validates_the_whole_inventory_before_signalling() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let job_id = "campaign-cancel-closed";
        let (mut child, terminator, _launch) =
            spawned_campaign_sub_fixture(&paths, job_id, "closed");
        let malformed = paths
            .job_dir(job_id)
            .join(CAMPAIGN_SUB_LAUNCH_DIR)
            .join("zz-malformed.json");
        fs::write(&malformed, b"{}").expect("malformed authority");

        reconcile_campaign_sub_processes_for_job(&paths, job_id, std::time::Duration::ZERO)
            .expect_err("malformed sibling authority must fail closed");
        assert!(
            child.try_wait().expect("poll child").is_none(),
            "no nested process may be signalled before the complete inventory validates"
        );

        let _ = terminator.terminate(std::time::Duration::ZERO);
        let _ = child.wait();
    }

    #[cfg(unix)]
    fn spawned_merge_repair_fixture(
        paths: &DeadreckonPaths,
        job_id: &str,
    ) -> (
        std::process::Child,
        Box<dyn deadreckon_core::ChildTerminator>,
        serde_json::Value,
        String,
    ) {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "trap '' HUP; sleep 30 & wait"]);
        let (child, terminator) = deadreckon_core::spawn_grouped(command).expect("grouped child");
        let capability_id = Uuid::new_v4().to_string();
        let outer_launch_id = Uuid::new_v4().to_string();
        let run_id = Uuid::new_v4().simple().to_string();
        let repair_id = deadreckon_core::flight::sha256_text("merge repair fixture");
        let delegated_token_sha256 = "sha256:delegated-release".to_string();
        let mut process = deadreckon_core::SupervisedProcessRecord::prepared(
            deadreckon_core::SupervisedProcess {
                pid: child.id(),
                pgid: None,
            },
            capability_id.clone(),
            2,
            Some(outer_launch_id.clone()),
            delegated_token_sha256.clone(),
        )
        .expect("process identity");
        process.process.pgid = Some(child.id());
        process.phase = deadreckon_core::SupervisedProcessPhase::Running;
        let source = paths.home().join("merge-repair-source");
        let proof_dir = paths.home().join("merge-repair-proof");
        fs::create_dir_all(&source).expect("source");
        fs::create_dir_all(&proof_dir).expect("proof");
        let delegation = DelegatedInvocation {
            schema_version: 1,
            capability_id: capability_id.clone(),
            job_id: job_id.to_string(),
            authority: commands::supervisor::GuardedDriverAuthority {
                job_id: job_id.to_string(),
                attempt: 2,
                launch_id: outer_launch_id.clone(),
                lease_epoch: 7,
                release_token_sha256: "sha256:outer-release".to_string(),
            },
            action: DelegatedAction::MergeRepair {
                root_artifact_id: job_id.to_string(),
                repair_id: repair_id.clone(),
                repair_round: 1,
                run_id: run_id.clone(),
                proof_dir: proof_dir.clone(),
                repair_request_sha256: "sha256:request".to_string(),
                repair_plan_sha256: "sha256:plan".to_string(),
            },
            immutable_plan_sha256: None,
            immutable_campaign_sha256: None,
            argv_sha256: "sha256:argv".to_string(),
            cwd: source.clone(),
            scope_root: proof_dir,
            token_sha256: delegated_token_sha256,
            campaign_sub_launch_id: None,
            issued_at: Utc::now(),
        };
        let pending = delegation_pending_path(paths, job_id, &capability_id);
        commands::job::write_json_synced(&pending, &delegation).expect("delegation authority");
        let authority = json!({
            "schema_version": 2,
            "plan_id": job_id,
            "root_artifact_id": job_id,
            "repair_id": repair_id,
            "repair_round": 1,
            "repair_request_sha256": "sha256:request",
            "repair_plan_sha256": "sha256:plan",
            "capability_id": capability_id,
            "run_id": run_id,
            "status": "process_prepared",
            "sandbox_requested": "auto",
            "planner_spend_usd": 0.1,
            "planner_wall_seconds": 1.0,
            "source": source,
            "created_at": Utc::now(),
            "updated_at": Utc::now(),
            "process": process,
            "process_prepared_at": Utc::now(),
        });
        let authority_path = merge_repair_authority_path(paths, job_id, &repair_id);
        commands::job::write_json_synced(&authority_path, &authority)
            .expect("merge repair authority");
        let now = Utc::now();
        let events = [
            deadreckon_protocol::JobEvent {
                schema_version: deadreckon_protocol::JobSchemaVersion::CURRENT,
                job_id: JobId(job_id.to_string()),
                sequence: deadreckon_protocol::JobEventSequence::new(1).expect("sequence"),
                event_id: Uuid::new_v4().to_string(),
                causation_id: "merge-repair-test:outer".to_string(),
                timestamp: now,
                lease_epoch: 7,
                kind: JobEventKind::ChildLinked,
                detail: json!({
                    "root_id": job_id,
                    "attempt": 2,
                    "launch_id": outer_launch_id,
                }),
            },
            deadreckon_protocol::JobEvent {
                schema_version: deadreckon_protocol::JobSchemaVersion::CURRENT,
                job_id: JobId(job_id.to_string()),
                sequence: deadreckon_protocol::JobEventSequence::new(2).expect("sequence"),
                event_id: Uuid::new_v4().to_string(),
                causation_id: "merge-repair-test:authority".to_string(),
                timestamp: now,
                lease_epoch: 7,
                kind: JobEventKind::RepairChildAuthorityChanged,
                detail: json!({
                    "repair_id": repair_id,
                    "transition": "process_prepared",
                    "run_id": run_id,
                    "authority": authority,
                }),
            },
        ];
        let mut history = Vec::new();
        for event in events {
            history.extend(serde_json::to_vec(&event).expect("event JSON"));
            history.push(b'\n');
        }
        fs::write(paths.job_events(job_id), history).expect("Job history");
        (child, terminator, authority, capability_id)
    }

    #[cfg(unix)]
    #[test]
    fn merge_repair_cleanup_terminates_the_exact_event_and_delegation_bound_process() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let job_id = "merge-repair-cleanup";
        let (mut child, _terminator, _authority, capability_id) =
            spawned_merge_repair_fixture(&paths, job_id);

        reconcile_merge_repair_processes_for_job(&paths, job_id, std::time::Duration::ZERO)
            .expect("reconcile merge-repair process");

        child.wait().expect("reap merge-repair child");
        assert!(
            !delegation_pending_path(&paths, job_id, &capability_id).exists(),
            "cleanup must revoke an unconsumed merge-repair capability"
        );
    }

    #[cfg(unix)]
    #[test]
    fn merge_repair_cleanup_fails_closed_for_foreign_or_malformed_authority() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let job_id = "merge-repair-closed";
        let (mut child, terminator, authority, _capability_id) =
            spawned_merge_repair_fixture(&paths, job_id);
        let directory = paths.job_dir(job_id).join(MERGE_REPAIR_AUTHORITY_DIR);
        let foreign_path = directory.join("zz-foreign.json");
        let mut foreign = authority.clone();
        foreign["root_artifact_id"] = json!("another-job");
        commands::job::write_json_synced(&foreign_path, &foreign).expect("foreign authority");

        validate_merge_repair_process_inventory_for_job(&paths, job_id)
            .expect_err("foreign authority must fail closed");
        assert!(
            child.try_wait().expect("poll child").is_none(),
            "the complete inventory must validate before any process is signalled"
        );

        fs::remove_file(&foreign_path).expect("remove foreign fixture");
        let malformed_path = directory.join("zz-malformed.json");
        fs::write(&malformed_path, b"{}").expect("malformed authority");
        validate_merge_repair_process_inventory_for_job(&paths, job_id)
            .expect_err("malformed authority must fail closed");
        assert!(
            child.try_wait().expect("poll child").is_none(),
            "malformed authority must not weaken containment"
        );

        let _ = terminator.terminate(std::time::Duration::ZERO);
        let _ = child.wait();
    }

    #[cfg(unix)]
    #[test]
    fn merge_repair_cleanup_rejects_a_crossed_outer_launch_identity() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let job_id = "merge-repair-crossed-launch";
        let (mut child, terminator, authority, capability_id) =
            spawned_merge_repair_fixture(&paths, job_id);
        let pending = delegation_pending_path(&paths, job_id, &capability_id);
        let mut delegation: DelegatedInvocation =
            serde_json::from_slice(&fs::read(&pending).expect("delegation bytes"))
                .expect("delegation");
        delegation.authority.launch_id = Uuid::new_v4().to_string();
        commands::job::replace_json_synced(&pending, &delegation).expect("crossed delegation");

        let error = validate_merge_repair_process_inventory_for_job(&paths, job_id)
            .expect_err("crossed launch must fail closed");
        assert!(
            error.to_string().contains("exact Job, launch, process"),
            "{error}"
        );
        assert!(child.try_wait().expect("poll child").is_none());
        assert_eq!(
            authority["root_artifact_id"], job_id,
            "the failure is the crossed launch, not a foreign Job fixture"
        );

        let _ = terminator.terminate(std::time::Duration::ZERO);
        let _ = child.wait();
    }

    #[cfg(unix)]
    #[test]
    fn campaign_linked_recovery_terminates_residual_process_group_before_relaunch() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let job_id = "campaign-linked-residual";
        let (mut child, terminator, mut launch) =
            spawned_campaign_sub_fixture(&paths, job_id, "linked-residual");
        launch.released = true;
        launch.released_at = Some(Utc::now());
        launch.linked = true;
        launch.linked_at = Some(Utc::now());
        write_campaign_sub_launch(&paths, &launch).expect("linked authority");
        fs::create_dir_all(paths.plan_dir(&launch.campaign_id)).expect("Campaign directory");

        std::thread::sleep(std::time::Duration::from_millis(50));
        nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(child.id()).expect("pid")),
            nix::sys::signal::Signal::SIGKILL,
        )
        .expect("kill group leader only");
        child.wait().expect("reap group leader");

        let recovery = recover_linked_campaign_sub_artifacts(
            &paths,
            &launch,
            deadreckon_core::SupervisedProcessIdentity::Exited,
        )
        .expect("terminate residual group before relaunch");
        assert!(matches!(recovery, CampaignSubLaunchRecovery::Relaunch));

        let _ = terminator.terminate(std::time::Duration::ZERO);
    }

    #[test]
    fn campaign_sub_recovery_adopts_linked_live_process_without_duplicate_execution() {
        let launch = campaign_sub_launch_fixture(true, true);
        assert_eq!(
            classify_campaign_sub_recovery(
                &launch,
                deadreckon_core::SupervisedProcessIdentity::Current,
                true,
                false,
                true,
                true,
            )
            .expect("classify live linked launch"),
            CampaignSubRecoveryDisposition::AdoptLinked
        );
        assert_eq!(
            classify_campaign_sub_recovery(
                &launch,
                deadreckon_core::SupervisedProcessIdentity::Exited,
                true,
                false,
                true,
                true,
            )
            .expect("classify exited linked launch"),
            CampaignSubRecoveryDisposition::RecoverLinkedArtifacts,
            "an executed launch must never be classified for relaunch"
        );
        let error = classify_campaign_sub_recovery(
            &launch,
            deadreckon_core::SupervisedProcessIdentity::Reused,
            true,
            false,
            true,
            true,
        )
        .expect_err("PID reuse must fail closed");
        assert!(
            error.to_string().contains("conflicting or unverifiable"),
            "{error}"
        );
    }

    #[test]
    fn campaign_adoption_event_names_the_original_launch_and_fenced_new_owner() {
        let mut launch = campaign_sub_launch_fixture(true, true);
        launch.adopted = true;
        launch.adopted_by_attempt = Some(3);
        launch.adopted_by_lease_epoch = Some(8);
        launch.adopted_at = Some(Utc::now());

        let detail = campaign_sub_launch_detail(&launch);
        assert_eq!(detail["attempt"], 2);
        assert_eq!(detail["lease_epoch"], 7);
        assert_eq!(detail["adopted"], true);
        assert_eq!(detail["adopted_by_attempt"], 3);
        assert_eq!(detail["adopted_by_lease_epoch"], 8);
        assert!(detail["adopted_at"].as_str().is_some());
        assert!(detail["launch_id"].as_str().is_some());
        assert!(detail["outer_launch_id"].as_str().is_some());
        assert!(detail["process_start_identity"].as_str().is_some());
    }

    #[test]
    fn campaign_sub_recovery_rejects_inconsistent_release_link_and_unknown_identity() {
        let released = campaign_sub_launch_fixture(true, false);
        let missing_ack = classify_campaign_sub_recovery(
            &released,
            deadreckon_core::SupervisedProcessIdentity::Exited,
            false,
            true,
            false,
            false,
        )
        .expect_err("released state without ack must fail closed");
        assert!(
            missing_ack
                .to_string()
                .contains("without a matching acknowledgement"),
            "{missing_ack}"
        );

        let linked = campaign_sub_launch_fixture(true, true);
        let still_pending = classify_campaign_sub_recovery(
            &linked,
            deadreckon_core::SupervisedProcessIdentity::Current,
            true,
            true,
            true,
            true,
        )
        .expect_err("linked state cannot retain pending capability");
        assert!(
            still_pending.to_string().contains("one-time capability"),
            "{still_pending}"
        );
        let missing_consumed = classify_campaign_sub_recovery(
            &linked,
            deadreckon_core::SupervisedProcessIdentity::Exited,
            true,
            false,
            false,
            true,
        )
        .expect_err("linked state needs consumed capability");
        assert!(
            missing_consumed.to_string().contains("one-time capability"),
            "{missing_consumed}"
        );
        for identity in [
            deadreckon_core::SupervisedProcessIdentity::Reused,
            deadreckon_core::SupervisedProcessIdentity::Unverifiable,
        ] {
            let unknown =
                classify_campaign_sub_recovery(&linked, identity, true, false, true, true)
                    .expect_err("unknown identity must fail closed");
            assert!(
                unknown.to_string().contains("conflicting or unverifiable"),
                "{unknown}"
            );
        }
    }

    #[test]
    fn campaign_sub_launch_projection_advances_without_replacing_its_identity() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let mut launch = campaign_sub_launch_fixture(false, false);
        write_campaign_sub_launch(&paths, &launch).expect("write prepared launch");

        launch.released = true;
        launch.released_at = Some(Utc::now());
        write_campaign_sub_launch(&paths, &launch).expect("advance released launch");
        launch.linked = true;
        launch.linked_at = Some(Utc::now());
        write_campaign_sub_launch(&paths, &launch).expect("advance linked launch");

        let persisted = load_campaign_sub_launch(
            &paths,
            &launch.parent_job_id,
            &launch.sub_id,
            &launch.plan_id,
        )
        .expect("load launch")
        .expect("persisted launch");
        assert_eq!(persisted.launch_id, launch.launch_id);
        assert!(persisted.released);
        assert!(persisted.linked);
        assert!(!persisted.adopted);
    }

    #[cfg(unix)]
    #[test]
    fn delegated_argv_hash_accepts_non_utf8_arguments_without_loss() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let first = vec![
            OsString::from("run"),
            OsString::from_vec(vec![b'p', b'a', b't', b'h', 0xff]),
        ];
        let second = vec![
            OsString::from("run"),
            OsString::from_vec(vec![b'p', b'a', b't', b'h', 0xfe]),
        ];
        assert_eq!(
            invocation_argv_sha256(&first).expect("first digest"),
            invocation_argv_sha256(first.iter()).expect("stable digest")
        );
        assert_ne!(
            invocation_argv_sha256(&first).expect("first digest"),
            invocation_argv_sha256(&second).expect("second digest")
        );
    }

    #[test]
    fn semantic_revise_has_its_own_typed_stop_reason() {
        assert_eq!(
            semantic_decision_stop_reason(Some(SemanticDecision::Revise)),
            Some(StopReason::SemanticRevise)
        );
        assert_eq!(
            semantic_decision_stop_reason(Some(SemanticDecision::Uncertain)),
            Some(StopReason::SemanticUncertain)
        );
        assert_eq!(
            semantic_decision_stop_reason(Some(SemanticDecision::Achieved)),
            None
        );
    }

    #[test]
    fn parent_completion_uses_the_earliest_authoritative_cutoff() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-08-04T12:00:00Z")
            .expect("fixed time")
            .with_timezone(&Utc);
        let work_remaining = std::time::Duration::from_secs(10 * 60);

        assert_eq!(
            parent_completion_remaining_at(
                work_remaining,
                Some(now + chrono::Duration::minutes(20)),
                Some(now + chrono::Duration::minutes(5)),
                now,
            ),
            std::time::Duration::from_secs(5 * 60)
        );
        assert_eq!(
            parent_completion_remaining_at(
                work_remaining,
                Some(now + chrono::Duration::minutes(2)),
                Some(now + chrono::Duration::minutes(5)),
                now,
            ),
            std::time::Duration::from_secs(2 * 60)
        );
        assert_eq!(
            parent_completion_remaining_at(work_remaining, None, None, now),
            work_remaining
        );
        assert_eq!(
            parent_completion_remaining_at(
                work_remaining,
                None,
                Some(now - chrono::Duration::seconds(1)),
                now,
            ),
            std::time::Duration::ZERO
        );
    }

    #[test]
    fn parent_receipt_work_expiry_stays_a_wall_cap_terminal() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        std::fs::create_dir_all(temp.path().join("source")).expect("source");
        let mut parent = deadreckon_core::create_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: "bounded parent receipt".to_string(),
                cwd: temp.path().join("source"),
                sandbox: "sandbox-exec".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(60.0),
                run_id: Some("parent-boundary".to_string()),
                codebase: None,
            },
        )
        .expect("parent run");
        let error = DeadreckonError::ProcessBoundary {
            kind: deadreckon_core::ProcessBoundaryKind::WorkExpired,
            operation: "parent receipt".to_string(),
            authority: None,
            detail: "cutoff reached".to_string(),
        };

        let terminal = settle_parent_process_boundary(
            &mut parent,
            &error,
            "parent completion receipt sealing",
        )
        .expect("settlement")
        .expect("typed boundary");

        assert!(matches!(
            terminal,
            ParentCompletion::BudgetExhausted {
                stop_reason: StopReason::WallCap,
                ..
            }
        ));
        assert!(
            parent
                .pause_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("work cutoff"))
        );
    }

    #[test]
    fn parent_receipt_incomplete_cleanup_fails_as_lost_containment() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        std::fs::create_dir_all(temp.path().join("source")).expect("source");
        let mut parent = deadreckon_core::create_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: "contained parent receipt".to_string(),
                cwd: temp.path().join("source"),
                sandbox: "sandbox-exec".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(60.0),
                run_id: Some("parent-containment".to_string()),
                codebase: None,
            },
        )
        .expect("parent run");
        let authority = parent.run_root.join("child-pids/git-test.json");
        let error = DeadreckonError::ProcessBoundary {
            kind: deadreckon_core::ProcessBoundaryKind::CleanupIncomplete,
            operation: "parent receipt".to_string(),
            authority: Some(authority.clone()),
            detail: "process group still alive".to_string(),
        };

        let terminal = settle_parent_process_boundary(
            &mut parent,
            &error,
            "parent completion receipt sealing",
        )
        .expect("settlement")
        .expect("typed boundary");

        assert!(matches!(
            terminal,
            ParentCompletion::Failed {
                stop_reason: StopReason::LostContainment,
                ..
            }
        ));
        assert!(
            parent
                .failure_reason
                .as_deref()
                .is_some_and(|reason| reason.contains(authority.to_string_lossy().as_ref()))
        );
    }

    #[test]
    fn parent_semantic_budget_boundary_returns_typed_cap_reasons() {
        let temp = tempfile::TempDir::new().expect("temp");
        let source_cwd = temp.path().join("source");
        std::fs::create_dir_all(&source_cwd).expect("source");
        let job = deadreckon_protocol::Job {
            schema_version: deadreckon_protocol::JobSchemaVersion::CURRENT,
            job_id: JobId("budget-parent".to_string()),
            scope: "scope".to_string(),
            goal: "bounded parent".to_string(),
            shape: JobShape::Graph,
            created_at: Utc::now(),
            source_cwd,
            launch_plan_sha256: "sha256:launch".to_string(),
            authority_sha256: "sha256:authority".to_string(),
            policy: deadreckon_protocol::JobPolicy {
                max_spend_usd: 2.0,
                max_wall_seconds: 30,
                max_attempts: 1,
                deadline: None,
                semantic_judge: deadreckon_protocol::SemanticJudgeMode::Required,
                execution: None,
            },
        };

        let spend = semantic_budget_exhaustion(
            &job,
            ParentExecutionUsage {
                spend_usd: 2.0,
                wall_seconds: 1.0,
            },
        )
        .expect("spend cap");
        assert_eq!(spend.0, StopReason::SpendCap);

        let wall = semantic_budget_exhaustion(
            &job,
            ParentExecutionUsage {
                spend_usd: 1.0,
                wall_seconds: 30.0,
            },
        )
        .expect("wall cap");
        assert_eq!(wall.0, StopReason::WallCap);

        assert!(
            semantic_budget_exhaustion(
                &job,
                ParentExecutionUsage {
                    spend_usd: 1.99,
                    wall_seconds: 29.99,
                },
            )
            .is_none()
        );
        assert!(
            semantic_budget_overrun(
                &job,
                ParentExecutionUsage {
                    spend_usd: 2.0,
                    wall_seconds: 30.0,
                },
            )
            .is_none(),
            "exactly using a cap remains within the approved policy"
        );
        assert_eq!(
            semantic_budget_overrun(
                &job,
                ParentExecutionUsage {
                    spend_usd: 2.01,
                    wall_seconds: 1.0,
                },
            )
            .expect("spend overrun")
            .0,
            StopReason::SpendCap
        );
        assert_eq!(
            semantic_budget_overrun(
                &job,
                ParentExecutionUsage {
                    spend_usd: 1.0,
                    wall_seconds: 30.01,
                },
            )
            .expect("wall overrun")
            .0,
            StopReason::WallCap
        );
    }

    #[test]
    fn graph_usage_loads_run_totals_and_deduplicates_attempt_and_result_ids() {
        use deadreckon_core::plan::{
            Plan, PlanMode, PlanProviders, PlanRole, PlanTask, TaskAttempt,
        };

        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = temp.path().join("source");
        std::fs::create_dir_all(&cwd).expect("source");
        let mut run = deadreckon_core::create_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: "child".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "test".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        run.total_spend_usd = 1.25;
        run.total_wall_seconds = 4.5;
        deadreckon_core::save_state(&run).expect("state");

        let mut task = PlanTask::new(0, "child", "do work", PlanRole::Child, None);
        task.child_run_id = Some(run.run_id.clone());
        task.attempts.push(TaskAttempt::failed(
            1,
            Some(run.run_id.clone()),
            Some("retry reused the same durable run".to_string()),
            1.25,
        ));
        let mut second_task =
            PlanTask::new(1, "same child", "reuse durable work", PlanRole::Child, None);
        second_task.child_run_id = Some(run.run_id.clone());
        let mut failed_spawn = TaskAttempt::failed(
            1,
            None,
            Some("source preparation failed before Run creation".to_string()),
            0.1,
        );
        failed_spawn.started_at = Utc::now();
        failed_spawn.finished_at = Some(failed_spawn.started_at + chrono::Duration::seconds(2));
        second_task.attempts.push(failed_spawn);
        let plan = Plan::new(
            "bounded graph",
            PlanMode::FullPlan,
            vec![task, second_task],
            PlanProviders::default(),
            None,
            "test",
        )
        .expect("plan");
        let missing = plan_execution_usage(&paths, &plan)
            .expect_err("missing planner accounting must fail closed");
        assert!(
            missing.to_string().contains(PLAN_PLANNER_ACCOUNTING_FILE),
            "{missing}"
        );
        record_plan_planner_accounting(
            &paths,
            &plan.plan_id,
            Some(&commands::plan::PlannerAccounting {
                spend: deadreckon_providers::SpendEstimate {
                    provider: "planner".to_string(),
                    model: "planner-model".to_string(),
                    input_tokens: 10,
                    output_tokens: 5,
                    cost_usd: 0.25,
                    subscription: false,
                    wall_time_seconds: Some(1.0),
                },
                wall_seconds: 1.0,
            }),
        )
        .expect("planner accounting");

        let usage = plan_execution_usage(&paths, &plan).expect("usage");
        assert_eq!(
            usage,
            ParentExecutionUsage {
                spend_usd: 1.6,
                wall_seconds: 7.5,
            }
        );
    }

    #[test]
    fn nested_graph_repair_usage_includes_root_siblings() {
        use deadreckon_core::plan::save_plan;

        let (temp, paths, mut root, mut child) = owned_plan_fixture();
        let source = temp.path().join("source");
        let create_costed_run = |goal: &str, spend_usd: f64, wall_seconds: f64| {
            let mut run = deadreckon_core::create_run(
                &paths,
                deadreckon_core::RunOptions {
                    goal: goal.to_string(),
                    cwd: source.clone(),
                    sandbox: "none".to_string(),
                    provider: Some("smoke".to_string()),
                    skill_name: "test".to_string(),
                    max_spend_usd: None,
                    max_wall_seconds: None,
                    run_id: None,
                    codebase: None,
                },
            )
            .expect("run");
            run.total_spend_usd = spend_usd;
            run.total_wall_seconds = wall_seconds;
            deadreckon_core::save_state(&run).expect("state");
            run.run_id
        };
        child.tasks[0].child_run_id = Some(create_costed_run("nested", 0.3, 3.0));
        root.tasks[1].child_run_id = Some(create_costed_run("root sibling", 0.4, 4.0));
        save_plan(&paths, &child).expect("nested Plan");
        save_plan(&paths, &root).expect("root Plan");
        record_plan_planner_accounting(&paths, &root.plan_id, None)
            .expect("root planner accounting");
        record_plan_planner_accounting(&paths, &child.plan_id, None)
            .expect("nested planner accounting");

        let usage = graph_repair_execution_usage(&paths, &child, &root.plan_id)
            .expect("root-scoped repair usage");
        assert!((usage.spend_usd - 0.7).abs() < f64::EPSILON);
        assert!((usage.wall_seconds - 7.0).abs() < f64::EPSILON);
    }

    #[test]
    fn campaign_usage_reads_subplan_runs_instead_of_zero_cost_merge_runs() {
        use deadreckon_core::campaign::{
            Campaign, SubGoalStatus, append_campaign_event, build_sub_goals,
        };
        use deadreckon_core::plan::{Plan, PlanMode, PlanProviders, PlanRole, PlanTask, save_plan};

        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = temp.path().join("source");
        std::fs::create_dir_all(&cwd).expect("source");
        let mut plan_ids = Vec::new();
        for (index, (spend_usd, wall_seconds)) in [(0.6, 2.0), (0.7, 3.0)].into_iter().enumerate() {
            let mut run = deadreckon_core::create_run(
                &paths,
                deadreckon_core::RunOptions {
                    goal: format!("child {index}"),
                    cwd: cwd.clone(),
                    sandbox: "none".to_string(),
                    provider: Some("smoke".to_string()),
                    skill_name: "test".to_string(),
                    max_spend_usd: None,
                    max_wall_seconds: None,
                    run_id: None,
                    codebase: None,
                },
            )
            .expect("run");
            run.total_spend_usd = spend_usd;
            run.total_wall_seconds = wall_seconds;
            deadreckon_core::save_state(&run).expect("state");

            let mut first = PlanTask::new(0, "first", "work", PlanRole::Child, None);
            first.child_run_id = Some(run.run_id.clone());
            let mut second = PlanTask::new(1, "second", "reuse", PlanRole::Child, None);
            second.child_run_id = Some(run.run_id.clone());
            let mut plan = Plan::new(
                format!("subplan {index}"),
                PlanMode::FullPlan,
                vec![first, second],
                PlanProviders::default(),
                None,
                "test",
            )
            .expect("plan");
            plan.plan_id = format!("subplan-{index}");
            save_plan(&paths, &plan).expect("plan state");
            record_plan_planner_accounting(&paths, &plan.plan_id, None)
                .expect("planner accounting");
            plan_ids.push(plan.plan_id);
        }

        let mut campaign = Campaign::new(
            "bounded campaign",
            build_sub_goals(vec!["one".to_string(), "two".to_string()], 2).expect("sub-goals"),
            PlanProviders::default(),
            0,
            Some(2.0),
            Some(30.0),
            "test",
        )
        .expect("campaign");
        for (sub, plan_id) in campaign.sub_goals.iter_mut().zip(plan_ids) {
            sub.sub_plan_id = Some(plan_id);
            sub.status = SubGoalStatus::Merged;
        }
        campaign.root_planner_accounting = Some(deadreckon_core::plan::RootPlannerAccounting {
            schema_version: 1,
            planner_invoked: true,
            provider: Some("test-planner".to_string()),
            model: Some("test-model".to_string()),
            input_tokens: 10,
            output_tokens: 5,
            cost_usd: 0.2,
            subscription: false,
            wall_seconds: 1.0,
            recorded_at: Utc::now(),
        });
        append_campaign_event(
            &paths.plan_dir(&campaign.campaign_id),
            "root_planner_accounting",
            json!({
                "cost_usd": 0.2,
                "wall_seconds": 1.0,
            }),
        )
        .expect("planner accounting");

        let usage = campaign_execution_usage(&paths, &campaign).expect("usage");
        assert!((usage.spend_usd - 1.5).abs() < f64::EPSILON);
        assert!((usage.wall_seconds - 6.0).abs() < f64::EPSILON);
    }

    #[test]
    fn driver_spec_round_trips_through_the_immutable_launch_plan() {
        let mut plan = commands::course::trivial_operator_plan(
            "durable graph",
            commands::course::CourseShape::Plan,
            "test",
        );
        let spec = DriverSpec {
            kind: DriverKind::FullPlan,
            child_count: Some(3),
            apply: deadreckon_core::plan::ApplyWhen::AtEnd,
            planner_provider: Some("planner".to_string()),
            child_provider: Some("worker".to_string()),
            child_provider_overrides: vec!["1=reviewer".to_string()],
            coder_provider: None,
            reviewer_provider: None,
            planner_model: Some("planner-model".to_string()),
            child_model: Some("model".to_string()),
            child_model_overrides: vec!["1=review-model".to_string()],
            coder_model: None,
            reviewer_model: None,
            model: None,
            source_init_git: false,
        };
        embed_driver_spec(&mut plan, &spec).expect("embed");
        assert_eq!(driver_spec(&plan).expect("read"), spec);
    }

    #[test]
    fn pre_execution_team_driver_deserializes_with_legacy_model_fallback() {
        let driver: DriverSpec = serde_json::from_value(serde_json::json!({
            "kind": "full_plan",
            "child_count": 3,
            "apply": "at-end",
            "planner_provider": "planner",
            "child_provider": "worker",
            "child_provider_overrides": [],
            "coder_provider": null,
            "reviewer_provider": null,
            "model": "legacy-model",
            "source_init_git": false
        }))
        .expect("legacy driver");

        assert_eq!(driver.model.as_deref(), Some("legacy-model"));
        assert_eq!(driver.planner_model, None);
        assert_eq!(driver.child_model, None);
        assert!(driver.child_model_overrides.is_empty());
    }

    #[test]
    fn durable_graph_driver_preserves_per_node_apply_for_isolated_execution() {
        let mut plan = commands::course::trivial_operator_plan(
            "verify only after the isolated graph is merged",
            commands::course::CourseShape::Plan,
            "test",
        );
        let requested = DriverSpec {
            kind: DriverKind::FullPlan,
            child_count: Some(3),
            apply: deadreckon_core::plan::ApplyWhen::PerNode,
            planner_provider: Some("planner".to_string()),
            child_provider: Some("worker".to_string()),
            child_provider_overrides: Vec::new(),
            coder_provider: None,
            reviewer_provider: None,
            planner_model: None,
            child_model: None,
            child_model_overrides: Vec::new(),
            coder_model: None,
            reviewer_model: None,
            model: None,
            source_init_git: false,
        };

        embed_driver_spec(&mut plan, &requested).expect("embed");
        let frozen = driver_spec(&plan).expect("frozen driver");
        assert_eq!(requested.apply, deadreckon_core::plan::ApplyWhen::PerNode);
        assert_eq!(frozen.apply, deadreckon_core::plan::ApplyWhen::PerNode);
    }

    #[test]
    fn ordered_candidate_rejects_clean_unledgered_commit_and_leaves_source_untouched() {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("approved.txt"), "approved\n").expect("approved source");
        let source_tree_sha256 = deadreckon_core::flight::build_deliverable_file_index(&source)
            .expect("source index")
            .tree_hash();
        let job_id = JobId("ordered-candidate-job".to_string());
        let job = deadreckon_protocol::Job {
            schema_version: deadreckon_protocol::JobSchemaVersion::CURRENT,
            job_id: job_id.clone(),
            scope: "ordered-candidate".to_string(),
            goal: "land two ordered changes".to_string(),
            shape: JobShape::Graph,
            created_at: Utc::now(),
            source_cwd: source.clone(),
            launch_plan_sha256: "sha256:launch".to_string(),
            authority_sha256: "sha256:authority".to_string(),
            policy: deadreckon_protocol::JobPolicy {
                max_spend_usd: 1.0,
                max_wall_seconds: 60,
                max_attempts: 2,
                deadline: None,
                semantic_judge: deadreckon_protocol::SemanticJudgeMode::Required,
                execution: None,
            },
        };
        deadreckon_core::write_job(&paths, &job).expect("Job");
        let authority = deadreckon_protocol::JobAuthority {
            schema_version: deadreckon_protocol::JobSchemaVersion::CURRENT,
            job_id: job_id.clone(),
            run_id: deadreckon_protocol::RunId(job_id.as_ref().to_string()),
            approved_at: Utc::now(),
            accepted_by: deadreckon_protocol::AuthorityAcceptedBy::Operator,
            goal_sha256: deadreckon_core::flight::sha256_text(&job.goal),
            contract_sha256: "sha256:contract".to_string(),
            effective_policy_sha256: "sha256:policy".to_string(),
            launch_plan_sha256: job.launch_plan_sha256.clone(),
            source_tree_sha256: source_tree_sha256.clone(),
            source_revision: None,
            sandbox_requested: "sandbox-exec".to_string(),
            semantic_judge_mode: deadreckon_protocol::SemanticJudgeMode::Required,
            gate_evaluator_sha256: None,
        };
        let token = deadreckon_core::claim_job_lease(
            &paths,
            &job_id,
            &deadreckon_core::LeaseOwner {
                owner_id: "ordered-candidate-owner".to_string(),
                boot_id: "ordered-candidate-boot".to_string(),
                pid: std::process::id(),
                process_group: std::process::id(),
            },
            Utc::now(),
            std::time::Duration::from_secs(60),
        )
        .expect("lease")
        .token();

        let candidate = prepare_ordered_candidate(&paths, &job, &authority, &token)
            .expect("prepare ordered candidate");
        assert_ne!(candidate, source);
        assert_eq!(
            fs::read_to_string(candidate.join("approved.txt")).expect("candidate source"),
            "approved\n"
        );
        assert!(!source.join(".git").exists());

        fs::write(candidate.join("landed.txt"), "first node\n").expect("landed result");
        git_status(&candidate, &["add", "--all"]).expect("stage landed result");
        git_status(
            &candidate,
            &[
                "-c",
                "user.name=DeadReckon",
                "-c",
                "user.email=deadreckon@localhost",
                "commit",
                "--quiet",
                "-m",
                "land first node",
            ],
        )
        .expect("commit landed result");
        fs::write(
            source.join("operator-later.txt"),
            "outside the isolated Job\n",
        )
        .expect("operator source can move independently");
        let error = prepare_ordered_candidate(&paths, &job, &authority, &token)
            .expect_err("clean commits without an application fact must fail closed");
        assert!(error.to_string().contains("clean unledgered HEAD"));
        assert!(!source.join("landed.txt").exists());

        git_status(&candidate, &["reset", "--hard", "HEAD^"])
            .expect("restore manifest initial revision");
        assert_eq!(
            prepare_ordered_candidate(&paths, &job, &authority, &token)
                .expect("recover exact initial candidate"),
            candidate
        );

        let history =
            deadreckon_core::read_job_history(&paths.job_events(job_id.as_ref())).expect("history");
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| event.kind == JobEventKind::WorkspacePrepared)
                .count(),
            1
        );
        fs::write(candidate.join("partial.txt"), "incomplete landing\n").expect("partial landing");
        let error = prepare_ordered_candidate(&paths, &job, &authority, &token)
            .expect_err("dirty recovery must fail closed");
        assert!(error.to_string().contains("incomplete landing"));
    }

    #[test]
    fn durable_campaign_driver_also_freezes_apply_at_end() {
        let mut plan = commands::course::trivial_operator_plan(
            "verify only after the isolated campaign is merged",
            commands::course::CourseShape::Campaign,
            "test",
        );
        let requested = DriverSpec {
            kind: DriverKind::Campaign,
            child_count: Some(3),
            apply: deadreckon_core::plan::ApplyWhen::PerNode,
            planner_provider: Some("planner".to_string()),
            child_provider: Some("worker".to_string()),
            child_provider_overrides: Vec::new(),
            coder_provider: None,
            reviewer_provider: None,
            planner_model: None,
            child_model: None,
            child_model_overrides: Vec::new(),
            coder_model: None,
            reviewer_model: None,
            model: None,
            source_init_git: false,
        };

        embed_driver_spec(&mut plan, &requested).expect("embed");
        assert_eq!(
            driver_spec(&plan).expect("frozen driver").apply,
            deadreckon_core::plan::ApplyWhen::AtEnd
        );
    }

    fn dependency_repair_link_fixture(
        tamper_plan_digest: bool,
    ) -> (
        tempfile::TempDir,
        DeadreckonPaths,
        deadreckon_core::PipelineState,
        PathBuf,
    ) {
        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let working = temp.path().join("dependency-working");
        let proof_dir = temp.path().join("task-launch").join("merge-proofs");
        fs::create_dir_all(&working).expect("working");
        fs::create_dir_all(&proof_dir).expect("proofs");
        let proof_dir = fs::canonicalize(&proof_dir).expect("canonical proofs");
        let request_sha = deadreckon_core::flight::sha256_text("request");
        let plan_sha = deadreckon_core::flight::sha256_text("plan");
        let ownership = deadreckon_core::RunOwnership::merge_repair(
            "parent-job",
            deadreckon_core::MergeRepairOwnership {
                root_artifact_id: "parent-job".to_string(),
                repair_id: "repair-id".to_string(),
                repair_round: 1,
                run_id: "dependency-repair-run".to_string(),
                proof_dir: proof_dir.clone(),
                repair_request_sha256: request_sha.clone(),
                repair_plan_sha256: plan_sha.clone(),
            },
        );
        let state = deadreckon_core::create_owned_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: "dependency repair".to_string(),
                cwd: working,
                sandbox: "sandbox-exec".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(30.0),
                run_id: Some("dependency-repair-run".to_string()),
                codebase: None,
            },
            ownership,
        )
        .expect("owned repair Run");
        commands::job::write_json_synced(
            &proof_dir.join("repair-run.json"),
            &json!({
                "schema_version": 2,
                "root_artifact_id": "parent-job",
                "repair_id": "repair-id",
                "repair_round": 1,
                "repair_request_sha256": request_sha,
                "repair_plan_sha256": if tamper_plan_digest {
                    deadreckon_core::flight::sha256_text("tampered")
                } else {
                    plan_sha
                },
                "run_id": "dependency-repair-run",
                "status": "launch_prepared",
            }),
        )
        .expect("launch record");
        (temp, paths, state, proof_dir)
    }

    #[test]
    fn dependency_repair_links_exact_run_before_output_and_refuses_duplicate() {
        let (_temp, _paths, state, proof_dir) = dependency_repair_link_fixture(false);
        link_delegated_owned_run(&state).expect("link exact repair Run");
        let linked: Value = serde_json::from_slice(
            &fs::read(proof_dir.join("repair-run.json")).expect("linked record"),
        )
        .expect("linked JSON");
        assert_eq!(linked["run_id"], state.run_id);
        assert_eq!(linked["status"], "child_linked");

        let mut duplicate = state.clone();
        duplicate.run_id = "duplicate-repair-run".to_string();
        link_delegated_owned_run(&duplicate)
            .expect_err("durable authority must not link a second repair Run");
        let unchanged: Value = serde_json::from_slice(
            &fs::read(proof_dir.join("repair-run.json")).expect("unchanged record"),
        )
        .expect("unchanged JSON");
        assert_eq!(unchanged["run_id"], state.run_id);
    }

    #[test]
    fn dependency_repair_digest_tamper_fails_before_run_link() {
        let (_temp, _paths, state, proof_dir) = dependency_repair_link_fixture(true);
        link_delegated_owned_run(&state).expect_err("tampered immutable repair plan must not link");
        let record: Value = serde_json::from_slice(
            &fs::read(proof_dir.join("repair-run.json")).expect("launch record"),
        )
        .expect("launch JSON");
        assert_eq!(record["run_id"], "dependency-repair-run");
    }

    #[test]
    fn preassigned_repair_run_survives_crash_before_link_without_duplicate_identity() {
        let (_temp, paths, state, proof_dir) = dependency_repair_link_fixture(false);
        let authority: Value = serde_json::from_slice(
            &fs::read(proof_dir.join("repair-run.json")).expect("launch authority"),
        )
        .expect("launch authority JSON");
        assert_eq!(authority["run_id"], state.run_id);
        let recovered =
            deadreckon_core::load_run(&paths, &state.run_id).expect("preassigned owned Run");
        assert_eq!(recovered.ownership, state.ownership);
        assert_eq!(
            recovered
                .ownership
                .as_ref()
                .and_then(|ownership| match &ownership.artifact {
                    deadreckon_core::RunOwnershipArtifact::MergeRepair { run_id, .. } => {
                        Some(run_id.as_str())
                    }
                    _ => None,
                }),
            Some(state.run_id.as_str())
        );
    }

    #[test]
    fn exhausted_parent_budget_refuses_repair_child_with_a_bounded_reason() {
        assert!(matches!(
            repair_budget_availability(
                4.0,
                90.0,
                ParentExecutionUsage {
                    spend_usd: 4.0,
                    wall_seconds: 1.0,
                },
            ),
            RepairBudgetAvailability::Exhausted {
                stop_reason: StopReason::SpendCap,
                ..
            }
        ));
        assert!(matches!(
            repair_budget_availability(
                4.0,
                90.0,
                ParentExecutionUsage {
                    spend_usd: 1.0,
                    wall_seconds: 90.0,
                },
            ),
            RepairBudgetAvailability::Exhausted {
                stop_reason: StopReason::WallCap,
                ..
            }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn durable_chain_executes_approved_bytes_after_original_mutation_and_deletion() {
        use deadreckon_core::chain::{
            ApplyMode, ApplyStrategy, BranchPolicy, Chain, ChainHookName, ChainHookSource,
            ChainNewOptions, DurableChainAdapterManifest, DurableChainHookEventKind,
            FrozenChainHook, OnFail,
        };

        let temp = tempfile::TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let hook_path = temp.path().join("pre-step");
        fs::write(
            &hook_path,
            format!(
                "#!/bin/sh\nif [ \"$DEADRECKON_HOME\" = \"{}\" ]; then echo controller-home-exposed; exit 8; fi\necho approved-program\nexit 0\n",
                paths.home().display()
            ),
        )
        .expect("approved hook");
        let hook = FrozenChainHook::freeze(
            ChainHookName::PreStep,
            ChainHookSource::Workspace,
            &hook_path,
        )
        .expect("freeze hook");
        let chain = Chain::new(ChainNewOptions {
            root_goal: "manual: 2 steps".to_string(),
            goals: vec!["one".to_string(), "two".to_string()],
            scope: "hook-test".to_string(),
            base_branch: "main".to_string(),
            base_sha: "approved-base".to_string(),
            cwd: temp.path().to_path_buf(),
            provider: Some("smoke".to_string()),
            model: None,
            sandbox: "auto".to_string(),
            branch_policy: BranchPolicy::Stack,
            apply_mode: ApplyMode::Auto,
            apply_strategy: ApplyStrategy::Squash,
            apply_allowlist: Vec::new(),
            on_fail: OnFail::Stop,
            circuit_breaker_threshold: 2,
            max_spend_usd: Some(1.0),
            max_wall_seconds: Some(60.0),
            deadreckon_version: "test".to_string(),
        })
        .expect("chain");
        let adapter = DurableChainAdapterManifest::new(&chain, vec![hook.clone()]);
        fs::write(&hook_path, b"#!/bin/sh\necho mutable-program\nexit 9\n")
            .expect("mutate original hook");
        fs::remove_file(&hook_path).expect("delete original hook");

        let job_id = JobId("approved-chain-hook-job".to_string());
        let owner = deadreckon_core::LeaseOwner {
            owner_id: "approved-hook-owner".to_string(),
            boot_id: "approved-hook-boot".to_string(),
            pid: std::process::id(),
            process_group: std::process::id(),
        };
        let token = deadreckon_core::claim_job_lease(
            &paths,
            &job_id,
            &owner,
            Utc::now(),
            // This fixture exercises immutable hook bytes, not lease expiry.
            // Keep the authority valid even when the full debug suite is
            // descheduled under linker and process-launch load.
            std::time::Duration::from_secs(300),
        )
        .expect("claim")
        .token();

        let exit_code = invoke_frozen_durable_chain_hook(
            &paths,
            &token,
            &adapter,
            &hook,
            Some(0),
            1,
            temp.path(),
            &json!({"step_goal": "one", "base_ref": "approved-base"}),
        )
        .expect("invoke approved hook bytes");

        assert_eq!(exit_code, 0);
        assert!(!hook_path.exists());
        let events = deadreckon_core::chain::read_durable_chain_hook_events(&paths, &job_id)
            .expect("hook evidence");
        let completed = events
            .iter()
            .find(|event| event.kind == DurableChainHookEventKind::Completed)
            .expect("completed event");
        assert!(completed.stdout.contains("approved-program"));
        assert!(!completed.stdout.contains("mutable-program"));
        assert!(!completed.stdout.contains("controller-home-exposed"));
        assert_eq!(completed.hook.approved_bytes, hook.approved_bytes);
    }
}
