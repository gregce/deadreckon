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
        }
    }
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
    TransientProvider,
    FatalProvider,
    FatalGate,
    LostContainment,
    CorruptHistory,
    LegacyUnknown,
}

impl StopReason {
    pub const ALL: [Self; 16] = [
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
        Self::TransientProvider,
        Self::FatalProvider,
        Self::FatalGate,
        Self::LostContainment,
        Self::CorruptHistory,
        Self::LegacyUnknown,
    ];
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
    ChildLaunchPrepared,
    AttemptStarted,
    ChildLinked,
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
    pub contained: bool,
    pub sandbox_backend: String,
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
