//! Persisted vocabulary for durable jobs and their completion authority.
//!
//! This module contains wire types only. Event I/O, lease fencing, history
//! reduction, and receipt verification belong to higher-level crates.

use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use schemars::schema::{InstanceType, Schema, SchemaObject};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::{JobId, RunId};

/// The only job wire version understood by this release.
pub const JOB_SCHEMA_VERSION: u32 = 1;
pub const GATE_EVALUATOR_IDENTITY_SCHEMA_VERSION: u32 = 1;
/// Behavioural contract spoken by `deadreckon` and every `dr-gate` helper.
///
/// This is deliberately independent of the package version. Bump it when the
/// persisted invocation or wire contract becomes incompatible. A separate
/// exact bundle-build identity rejects stale same-protocol source builds.
pub const GATE_EVALUATOR_PROTOCOL_VERSION: u32 = 1;
/// Marker embedded in every compatible gate helper, including cross-platform
/// evaluator sidecars that the host cannot execute during admission.
pub const GATE_EVALUATOR_PROTOCOL_MARKER: &str = "deadreckon-gate-evaluator-protocol-v1";
pub const DOCKER_GATE_GUEST_PATH: &str = "/usr/local/bin/dr-gate-evaluate";

/// A checked numeric discriminator for every persisted job artifact.
///
/// This is intentionally not a plain `u32`: unsupported versions must fail at
/// deserialization rather than being interpreted with today's meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct JobSchemaVersion(u32);

impl JobSchemaVersion {
    pub const CURRENT: Self = Self(JOB_SCHEMA_VERSION);

    pub const fn get(self) -> u32 {
        self.0
    }
}

impl<'de> Deserialize<'de> for JobSchemaVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let version = u32::deserialize(deserializer)?;
        if version == JOB_SCHEMA_VERSION {
            Ok(Self::CURRENT)
        } else {
            Err(D::Error::custom(format!(
                "unsupported job schema version {version}; expected {JOB_SCHEMA_VERSION}"
            )))
        }
    }
}

impl JsonSchema for JobSchemaVersion {
    fn schema_name() -> String {
        "JobSchemaVersion".to_string()
    }

    fn json_schema(_generator: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        Schema::Object(SchemaObject {
            instance_type: Some(InstanceType::Integer.into()),
            enum_values: Some(vec![Value::from(JOB_SCHEMA_VERSION)]),
            ..SchemaObject::default()
        })
    }

    fn is_referenceable() -> bool {
        false
    }
}

/// Immutable identity and approved policy persisted in `job.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Job {
    pub schema_version: JobSchemaVersion,
    pub job_id: JobId,
    pub scope: String,
    pub goal: String,
    pub shape: JobShape,
    pub created_at: DateTime<Utc>,
    pub source_cwd: PathBuf,
    pub launch_plan_sha256: String,
    pub authority_sha256: String,
    pub policy: JobPolicy,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JobPolicy {
    pub max_spend_usd: f64,
    pub max_wall_seconds: u64,
    pub max_attempts: u32,
    pub deadline: Option<DateTime<Utc>>,
    pub semantic_judge: SemanticJudgeMode,
    /// Resolved provider capability policy approved before the first turn.
    ///
    /// `None` is retained only for reading pre-Watchkeeper-policy jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<JobExecutionPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobExecutionPolicy {
    pub sandbox_requested: String,
    pub require_containment: bool,
    pub tools: BTreeMap<String, JobToolPolicy>,
    /// Exact controller/evaluator toolchain approved before the first turn.
    ///
    /// `None` keeps pre-identity Jobs readable. A new strict Job must persist
    /// this field together with matching authority and boundary-observation
    /// digests; partial identity state fails closed during verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_evaluator: Option<GateEvaluatorIdentity>,
}

impl JobExecutionPolicy {
    /// The default unattended coding capability set: workspace-local reads and
    /// writes, with no network authority.
    pub fn workspace_only(sandbox_requested: impl Into<String>) -> Self {
        let mut tools = BTreeMap::new();
        for name in ["bash", "write_file"] {
            tools.insert(
                name.to_string(),
                JobToolPolicy {
                    workspace_read: true,
                    workspace_write: true,
                    network_allowlist: Vec::new(),
                },
            );
        }
        Self {
            sandbox_requested: sandbox_requested.into(),
            require_containment: true,
            tools,
            gate_evaluator: None,
        }
    }
}

/// Content identity of one trusted `dr-gate` executable.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GateBinaryIdentity {
    /// SHA-256 of the exact executable bytes.
    pub sha256: String,
    /// Operating system expected by the executable, for example `macos` or
    /// `linux`.
    pub os: String,
    /// Architecture expected by the executable, for example `aarch64` or
    /// `x86_64`.
    pub arch: String,
}

/// Immutable Docker execution identity for a Linux gate evaluator.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DockerGateIdentity {
    /// Content-addressed Docker image ID, never a mutable tag.
    pub image_id: String,
    /// Explicit Docker platform in `<os>/<architecture>` form.
    pub platform: String,
    /// Fixed absolute path of the evaluator inside the immutable image.
    pub guest_path: PathBuf,
}

/// Versioned controller/evaluator identity approved for one durable Job.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GateEvaluatorIdentity {
    pub schema_version: u32,
    pub protocol_version: u32,
    /// Host-compatible binary used for guarded release and trusted signing.
    pub controller: GateBinaryIdentity,
    /// Binary that executes deterministic checks inside containment.
    pub evaluator: GateBinaryIdentity,
    /// Required for Docker evaluation and absent for native containment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker: Option<DockerGateIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobToolPolicy {
    pub workspace_read: bool,
    pub workspace_write: bool,
    #[serde(default)]
    pub network_allowlist: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobShape {
    Single,
    Graph,
    // Reserved compatibility discriminator only. The durable scheduler does
    // not execute this shape: legacy Chain owns a separate conductor plus
    // hook/apply/undo side effects that cannot yet be adopted exactly once.
    LegacyChain,
    LegacyCampaign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SemanticJudgeMode {
    Required,
    Optional,
    Disabled,
}

/// Current execution position. Terminal meaning lives in `JobOutcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobPhase {
    Queued,
    Running,
    VerifyingChecks,
    VerifyingMeaning,
    Waiting,
    Terminal,
}

/// Terminal classification, separate from both phase and causal stop reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobOutcome {
    Verified,
    NeedsReview,
    Blocked,
    BudgetExhausted,
    DeadlineReached,
    RetryExhausted,
    Cancelled,
    Failed,
}

/// Why execution stopped. New jobs persist this typed value instead of
/// recovering meaning from free-form failure text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    Verified,
    SemanticRevise,
    SemanticUncertain,
    SemanticUnavailable,
    OperatorInputRequired,
    SpendCap,
    WallCap,
    Deadline,
    AttemptLimit,
    CancelRequested,
    DeterministicRevise,
    TransientProvider,
    FatalProvider,
    FatalGate,
    LostContainment,
    CorruptHistory,
    LegacyUnknown,
}

impl StopReason {
    pub const ALL: [Self; 17] = [
        Self::Verified,
        Self::SemanticRevise,
        Self::SemanticUncertain,
        Self::SemanticUnavailable,
        Self::OperatorInputRequired,
        Self::SpendCap,
        Self::WallCap,
        Self::Deadline,
        Self::AttemptLimit,
        Self::CancelRequested,
        Self::DeterministicRevise,
        Self::TransientProvider,
        Self::FatalProvider,
        Self::FatalGate,
        Self::LostContainment,
        Self::CorruptHistory,
        Self::LegacyUnknown,
    ];
}

impl JobOutcome {
    /// Return whether `reason` is a valid causal explanation for this terminal
    /// classification. This is the canonical vocabulary shared by lifecycle
    /// projection, operator evidence and every user-facing status surface.
    pub const fn accepts_stop_reason(self, reason: StopReason) -> bool {
        match self {
            Self::Verified => matches!(reason, StopReason::Verified),
            Self::NeedsReview => matches!(
                reason,
                StopReason::SemanticRevise
                    | StopReason::SemanticUncertain
                    | StopReason::SemanticUnavailable
            ),
            Self::Blocked => matches!(
                reason,
                StopReason::OperatorInputRequired | StopReason::LostContainment
            ),
            Self::BudgetExhausted => {
                matches!(reason, StopReason::SpendCap | StopReason::WallCap)
            }
            Self::DeadlineReached => matches!(reason, StopReason::Deadline),
            Self::RetryExhausted => matches!(reason, StopReason::AttemptLimit),
            Self::Cancelled => matches!(reason, StopReason::CancelRequested),
            Self::Failed => matches!(
                reason,
                StopReason::TransientProvider
                    | StopReason::FatalProvider
                    | StopReason::FatalGate
                    | StopReason::CorruptHistory
                    | StopReason::LegacyUnknown
            ),
        }
    }
}

/// A non-zero position in a job's append-only lifecycle history.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct JobEventSequence(NonZeroU64);

impl JobEventSequence {
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl From<JobEventSequence> for u64 {
    fn from(value: JobEventSequence) -> Self {
        value.get()
    }
}

/// One append-only lifecycle fact.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JobEvent {
    pub schema_version: JobSchemaVersion,
    pub job_id: JobId,
    pub sequence: JobEventSequence,
    pub event_id: String,
    pub causation_id: String,
    pub timestamp: DateTime<Utc>,
    /// Epoch zero is reserved for trusted controller events before a lease is
    /// acquired. Worker control events require the current non-zero epoch.
    pub lease_epoch: u64,
    pub kind: JobEventKind,
    pub detail: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobEventKind {
    Created,
    ContractApproved,
    Queued,
    LeaseAcquired,
    LeaseReclaimed,
    WorkspacePrepared,
    ChildLaunchPrepared,
    AttemptStarted,
    ChildLinked,
    CampaignSubAuthorityChanged,
    RepairChildAuthorityChanged,
    AttemptStopped,
    RetryScheduled,
    DeterministicGatePassed,
    DeterministicGateFailed,
    SemanticJudgeAchieved,
    SemanticJudgeRevise,
    SemanticJudgeUncertain,
    NeedsReview,
    Blocked,
    BudgetExhausted,
    DeadlineReached,
    CancelRequested,
    Cancelled,
    Failed,
    Verified,
    ResultApplied,
    ResultExported,
    UndoStarted,
    UndoCompleted,
    UndoFailed,
}

/// Current fenced ownership persisted in `lease.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobLease {
    pub schema_version: JobSchemaVersion,
    pub job_id: JobId,
    pub owner_id: String,
    pub epoch: u64,
    pub acquired_at: DateTime<Utc>,
    pub heartbeat_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub boot_id: String,
    pub pid: u32,
    /// Same-boot process identity captured when the lease was acquired. Old
    /// checkpoints omit it and retain expiry-only recovery semantics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_start_identity: Option<String>,
    pub process_group: u32,
    pub child_pid: Option<u32>,
}

/// Immutable, operator-approved inputs persisted before the first agent turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobAuthority {
    pub schema_version: JobSchemaVersion,
    pub job_id: JobId,
    pub run_id: RunId,
    pub approved_at: DateTime<Utc>,
    pub accepted_by: AuthorityAcceptedBy,
    pub goal_sha256: String,
    pub contract_sha256: String,
    pub effective_policy_sha256: String,
    pub launch_plan_sha256: String,
    pub source_tree_sha256: String,
    pub source_revision: Option<String>,
    pub sandbox_requested: String,
    pub semantic_judge_mode: SemanticJudgeMode,
    /// SHA-256 of the canonical serialized `GateEvaluatorIdentity`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_evaluator_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityAcceptedBy {
    Operator,
    YesFlagGuardrail,
}

/// Independent read-only assessment persisted in
/// `proofs/semantic-judgment.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SemanticJudgment {
    pub schema_version: JobSchemaVersion,
    pub job_id: JobId,
    pub run_id: RunId,
    pub judged_at: DateTime<Utc>,
    pub provider: String,
    pub model: String,
    pub decision: SemanticDecision,
    pub summary: String,
    pub goal_coverage: Vec<GoalCoverage>,
    pub missing: Vec<String>,
    pub input_sha256: String,
    pub spend_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SemanticDecision {
    Achieved,
    Revise,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GoalCoverage {
    pub claim: String,
    pub status: GoalCoverageStatus,
    pub evidence: Vec<String>,
}

/// Exact controller-owned execution ledgers covered by a verified receipt.
/// Present for durable ordered chains, whose final tree alone cannot explain
/// which approved hooks ran or which child result was landed at each step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CompletionExecutionEvidence {
    pub ordered_candidate_manifest_sha256: String,
    pub candidate_application_events_sha256: Option<String>,
    pub chain_hook_events_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GoalCoverageStatus {
    Met,
    Missing,
    Unclear,
}

/// Cryptographically authenticated two-key completion result persisted in
/// `receipt.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompletionReceipt {
    pub schema_version: JobSchemaVersion,
    pub job_id: JobId,
    pub run_id: RunId,
    /// Durable Job attempt whose contained gate produced this result.
    pub attempt: u32,
    /// Exact supervisor child launch that owned the contained gate.
    pub outer_launch_id: String,
    pub issued_at: DateTime<Utc>,
    pub issuer: CompletionReceiptIssuer,
    pub proof_kind: CompletionProofKind,
    pub outcome: JobOutcome,
    pub stop_reason: StopReason,
    pub authority_sha256: String,
    pub goal_sha256: String,
    pub contract_sha256: String,
    pub effective_policy_sha256: String,
    pub launch_plan_sha256: String,
    pub source_tree_sha256: String,
    pub source_revision: Option<String>,
    pub result_tree_sha256: String,
    pub result_revision: Option<String>,
    pub deterministic_marker_sha256: String,
    pub semantic_judgment_sha256: String,
    pub sandbox_boundary_observation_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_evidence: Option<CompletionExecutionEvidence>,
    pub contained: bool,
    pub sandbox_backend: String,
    pub signature: String,
}

/// Stable identity of the exact Git worktree and repository selected for a
/// verified delivery. Both paths are canonical controller observations; a
/// workspace alias or a different worktree in the same repository is not the
/// same delivery target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitDeliveryRepositoryIdentity {
    pub worktree_root: PathBuf,
    pub git_common_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum GitDeliveryStrategy {
    Merge,
    Squash,
    CherryPick,
}

/// Controller-authenticated authority written before finish is allowed to
/// mutate the operator's Git repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitDeliveryIntent {
    pub schema_version: JobSchemaVersion,
    pub job_id: JobId,
    pub run_id: RunId,
    pub prepared_at: DateTime<Utc>,
    pub completion_receipt_sha256: String,
    pub repository: GitDeliveryRepositoryIdentity,
    /// Full symbolic ref, for example `refs/heads/main`.
    pub target_ref: String,
    pub pre_revision: String,
    pub signed_source_revision: String,
    pub signed_result_revision: String,
    pub effective_policy_sha256: String,
    pub strategy: GitDeliveryStrategy,
    pub signature: String,
}

/// Controller-authenticated after-state written once the exact intended
/// delivery has been re-proved in the operator repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppliedGitDeliveryReceipt {
    pub schema_version: JobSchemaVersion,
    pub job_id: JobId,
    pub run_id: RunId,
    pub issued_at: DateTime<Utc>,
    pub delivery_intent_sha256: String,
    pub completion_receipt_sha256: String,
    pub repository: GitDeliveryRepositoryIdentity,
    pub target_ref: String,
    pub pre_revision: String,
    pub applied_revision: String,
    pub signed_source_revision: String,
    pub signed_result_revision: String,
    pub effective_policy_sha256: String,
    pub strategy: GitDeliveryStrategy,
    pub signature: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CompletionReceiptIssuer {
    DeadreckonSupervisor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CompletionProofKind {
    TwoKeyCompletion,
}

/// Controller-produced result of actively probing the exact containment
/// backend used for strict deterministic verification.
///
/// This artifact lives in protected Job state, not in the coding workspace.
/// Its HMAC is independent of the final receipt HMAC; the receipt additionally
/// binds the exact persisted observation bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SandboxBoundaryObservation {
    pub schema_version: JobSchemaVersion,
    pub job_id: JobId,
    pub run_id: RunId,
    pub observed_at: DateTime<Utc>,
    pub issuer: SandboxBoundaryObservationIssuer,
    pub probe_id: String,
    pub attempt: u32,
    pub outer_launch_id: String,
    pub authority_sha256: String,
    pub contract_sha256: String,
    pub result_tree_sha256: String,
    pub sandbox_requested: String,
    pub sandbox_backend: String,
    pub contained: bool,
    pub gate_key_read_denied: bool,
    pub proof_write_denied: bool,
    pub control_write_denied: bool,
    pub operator_capture_read_denied: bool,
    pub operator_capture_write_denied: bool,
    pub signing_env_scrubbed: bool,
    pub probe_sha256: String,
    /// Evaluator identity digest approved by policy and authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_evaluator_sha256: Option<String>,
    pub signature: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxBoundaryObservationIssuer {
    DeadreckonController,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        DOCKER_GATE_GUEST_PATH, DockerGateIdentity, GATE_EVALUATOR_IDENTITY_SCHEMA_VERSION,
        GATE_EVALUATOR_PROTOCOL_VERSION, GateBinaryIdentity, GateEvaluatorIdentity, JobAuthority,
        JobExecutionPolicy, SandboxBoundaryObservation,
    };

    fn observation_json() -> serde_json::Value {
        json!({
            "schema_version": 1,
            "job_id": "job-1",
            "run_id": "job-1",
            "observed_at": "2026-07-30T12:00:00Z",
            "issuer": "deadreckon-controller",
            "probe_id": "2cd4df44-a9ce-4594-aa77-831245e05486",
            "attempt": 1,
            "outer_launch_id": "76ebebba-6d43-4c43-aaf4-690a6bd7ad6c",
            "authority_sha256": format!("sha256:{}", "1".repeat(64)),
            "contract_sha256": format!("sha256:{}", "2".repeat(64)),
            "result_tree_sha256": format!("sha256:{}", "3".repeat(64)),
            "sandbox_requested": "auto",
            "sandbox_backend": "sandbox-exec",
            "contained": true,
            "gate_key_read_denied": true,
            "proof_write_denied": true,
            "control_write_denied": true,
            "operator_capture_read_denied": true,
            "operator_capture_write_denied": true,
            "signing_env_scrubbed": true,
            "probe_sha256": format!("sha256:{}", "4".repeat(64)),
            "signature": "00".repeat(32)
        })
    }

    #[test]
    fn sandbox_observation_wire_shape_is_closed_and_complete() {
        serde_json::from_value::<SandboxBoundaryObservation>(observation_json())
            .expect("complete observation");

        let mut unknown = observation_json();
        unknown["agent_claim"] = json!(true);
        serde_json::from_value::<SandboxBoundaryObservation>(unknown)
            .expect_err("agent-selected fields are not part of the trusted wire shape");

        let mut incomplete = observation_json();
        incomplete
            .as_object_mut()
            .expect("object")
            .remove("gate_key_read_denied");
        serde_json::from_value::<SandboxBoundaryObservation>(incomplete)
            .expect_err("each denial fact is required");
    }

    #[test]
    fn legacy_policy_authority_and_observation_remain_readable_without_evaluator_identity() {
        let policy: JobExecutionPolicy = serde_json::from_value(json!({
            "sandbox_requested": "sandbox-exec",
            "require_containment": true,
            "tools": {}
        }))
        .expect("legacy execution policy");
        assert!(policy.gate_evaluator.is_none());

        let authority: JobAuthority = serde_json::from_value(json!({
            "schema_version": 1,
            "job_id": "job-1",
            "run_id": "job-1",
            "approved_at": "2026-07-30T12:00:00Z",
            "accepted_by": "operator",
            "goal_sha256": format!("sha256:{}", "1".repeat(64)),
            "contract_sha256": format!("sha256:{}", "2".repeat(64)),
            "effective_policy_sha256": format!("sha256:{}", "3".repeat(64)),
            "launch_plan_sha256": format!("sha256:{}", "4".repeat(64)),
            "source_tree_sha256": format!("sha256:{}", "5".repeat(64)),
            "source_revision": null,
            "sandbox_requested": "sandbox-exec",
            "semantic_judge_mode": "required"
        }))
        .expect("legacy authority");
        assert!(authority.gate_evaluator_sha256.is_none());

        let observation: SandboxBoundaryObservation =
            serde_json::from_value(observation_json()).expect("legacy observation");
        assert!(observation.gate_evaluator_sha256.is_none());
    }

    #[test]
    fn evaluator_identity_wire_shape_is_versioned_and_closed() {
        let identity = GateEvaluatorIdentity {
            schema_version: GATE_EVALUATOR_IDENTITY_SCHEMA_VERSION,
            protocol_version: GATE_EVALUATOR_PROTOCOL_VERSION,
            controller: GateBinaryIdentity {
                sha256: format!("sha256:{}", "1".repeat(64)),
                os: "macos".to_string(),
                arch: "aarch64".to_string(),
            },
            evaluator: GateBinaryIdentity {
                sha256: format!("sha256:{}", "2".repeat(64)),
                os: "linux".to_string(),
                arch: "aarch64".to_string(),
            },
            docker: Some(DockerGateIdentity {
                image_id: format!("sha256:{}", "3".repeat(64)),
                platform: "linux/arm64".to_string(),
                guest_path: DOCKER_GATE_GUEST_PATH.into(),
            }),
        };
        let encoded = serde_json::to_value(&identity).expect("identity JSON");
        assert_eq!(
            serde_json::from_value::<GateEvaluatorIdentity>(encoded.clone())
                .expect("identity round trip"),
            identity
        );

        let mut unknown = encoded;
        unknown["controller"]["path"] = json!("/tmp/agent-selected");
        serde_json::from_value::<GateEvaluatorIdentity>(unknown)
            .expect_err("nested evaluator identity is a closed wire shape");
    }
}
