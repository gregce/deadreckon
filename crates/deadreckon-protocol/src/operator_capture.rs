//! Persisted wire vocabulary for trusted operator capture sessions.

use std::{collections::BTreeMap, num::NonZeroU64};

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use schemars::schema::{InstanceType, Schema, SchemaObject};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::{JobId, JobOutcome, JobShape, StopReason};

/// The only operator-capture wire version understood by this release.
pub const OPERATOR_CAPTURE_SCHEMA_VERSION: u32 = 2;

/// Checked discriminator for every persisted operator-capture artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct OperatorCaptureSchemaVersion(u32);

impl OperatorCaptureSchemaVersion {
    pub const CURRENT: Self = Self(OPERATOR_CAPTURE_SCHEMA_VERSION);

    pub const fn get(self) -> u32 {
        self.0
    }
}

impl<'de> Deserialize<'de> for OperatorCaptureSchemaVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let version = u32::deserialize(deserializer)?;
        if version == OPERATOR_CAPTURE_SCHEMA_VERSION {
            Ok(Self::CURRENT)
        } else {
            Err(D::Error::custom(format!(
                "unsupported operator capture schema version {version}; expected \
                 {OPERATOR_CAPTURE_SCHEMA_VERSION}"
            )))
        }
    }
}

impl JsonSchema for OperatorCaptureSchemaVersion {
    fn schema_name() -> String {
        "OperatorCaptureSchemaVersion".to_string()
    }

    fn json_schema(_generator: &mut schemars::r#gen::SchemaGenerator) -> Schema {
        Schema::Object(SchemaObject {
            instance_type: Some(InstanceType::Integer.into()),
            enum_values: Some(vec![Value::from(OPERATOR_CAPTURE_SCHEMA_VERSION)]),
            ..SchemaObject::default()
        })
    }

    fn is_referenceable() -> bool {
        false
    }
}

/// Immutable identity approved before the first capture event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperatorCaptureBinding {
    pub schema_version: OperatorCaptureSchemaVersion,
    pub job_id: JobId,
    pub session_id: String,
    pub trial_id: String,
    pub created_at: DateTime<Utc>,
    pub source_revision: String,
    pub source_tree_sha256: String,
    pub deadreckon_source_revision: String,
    pub manifest_sha256: String,
    pub result_schema_sha256: String,
    pub recorder_sha256: String,
    /// Absolute interpreter route approved for deterministic recorder replay.
    pub recorder_interpreter: String,
    pub recorder_interpreter_sha256: String,
    pub capture_binary: String,
    pub capture_binary_sha256: String,
    pub deadreckon_binary: String,
    pub deadreckon_binary_sha256: String,
    pub deadreckon_version: String,
    pub declared_shape: JobShape,
    pub declared_backend: String,
    /// Approved provider routes keyed by manifest-declared execution role.
    pub provider_routes: BTreeMap<String, Vec<String>>,
    /// Exact registry-backed provider endpoint approved for the network-loss
    /// trial. Other trials must not carry a network probe authority.
    pub network_probe: Option<OperatorCaptureNetworkProbe>,
    pub replay_sha256: String,
    pub pass_capable: bool,
    /// Exact product terminal results approved before the fault is applied.
    /// A `verified/verified` result still requires a valid CompletionReceipt;
    /// every other accepted pair requires authenticated terminal-history
    /// lineage instead.
    pub allowed_terminal_results: Vec<OperatorCaptureExpectedJobResult>,
    pub required_captures: Vec<OperatorCaptureRequirement>,
    pub signature: String,
}

/// Immutable provider route and endpoint selected before a network fault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperatorCaptureNetworkProbe {
    pub provider_role: String,
    pub provider_route: String,
    pub endpoint: String,
}

/// Connectivity result emitted by the official provider registry probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperatorCaptureConnectivity {
    Reachable,
    Unreachable,
}

/// Deliberately narrow network failure vocabulary. Other provider probe
/// failures (for example missing credentials) cannot masquerade as an outage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperatorCaptureNetworkErrorKind {
    EndpointUnreachable,
}

/// Exact supervised attempt affected by a network observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperatorCaptureNetworkAttempt {
    pub run_id: String,
    pub attempt: u32,
    pub lease_epoch: u64,
    pub launch_id: String,
    pub pid: u32,
    pub boot_id: String,
    pub process_start_identity: String,
}

/// Canonical reachable/unreachable fact for one signed provider route and one
/// durable supervised attempt. The after observation retains the affected
/// attempt identity even when that process has already exited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperatorCaptureNetworkObservation {
    pub schema_version: OperatorCaptureSchemaVersion,
    pub job_id: JobId,
    pub session_id: String,
    pub trial_id: String,
    pub phase: OperatorCapturePhase,
    pub observed_at: DateTime<Utc>,
    pub provider_role: String,
    pub provider_route: String,
    pub endpoint: String,
    pub connectivity: OperatorCaptureConnectivity,
    pub error_kind: Option<OperatorCaptureNetworkErrorKind>,
    pub job_last_sequence: u64,
    pub attempt: OperatorCaptureNetworkAttempt,
}

/// One exact outcome/reason pair that may satisfy a dogfood trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperatorCaptureExpectedJobResult {
    pub outcome: JobOutcome,
    pub stop_reason: StopReason,
}

impl OperatorCaptureExpectedJobResult {
    pub fn is_valid(self) -> bool {
        match self.outcome {
            JobOutcome::Verified => self.stop_reason == StopReason::Verified,
            JobOutcome::NeedsReview => matches!(
                self.stop_reason,
                StopReason::SemanticRevise
                    | StopReason::SemanticUncertain
                    | StopReason::SemanticUnavailable
            ),
            JobOutcome::Blocked => matches!(
                self.stop_reason,
                StopReason::OperatorInputRequired | StopReason::LostContainment
            ),
            JobOutcome::BudgetExhausted => {
                matches!(self.stop_reason, StopReason::SpendCap | StopReason::WallCap)
            }
            JobOutcome::DeadlineReached => self.stop_reason == StopReason::Deadline,
            JobOutcome::RetryExhausted => self.stop_reason == StopReason::AttemptLimit,
            JobOutcome::Cancelled => self.stop_reason == StopReason::CancelRequested,
            JobOutcome::Failed => matches!(
                self.stop_reason,
                StopReason::TransientProvider
                    | StopReason::FatalProvider
                    | StopReason::FatalGate
                    | StopReason::LostContainment
                    | StopReason::CorruptHistory
                    | StopReason::LegacyUnknown
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperatorCaptureRequirement {
    pub subject: String,
    pub phase: OperatorCapturePhase,
    pub source: OperatorCaptureSource,
    pub media_type: String,
}

/// A non-zero position in an append-only capture history.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct OperatorCaptureEventSequence(NonZeroU64);

impl OperatorCaptureEventSequence {
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

impl From<OperatorCaptureEventSequence> for u64 {
    fn from(value: OperatorCaptureEventSequence) -> Self {
        value.get()
    }
}

/// Operator-capture phase, kept separate from Job lifecycle state.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum OperatorCapturePhase {
    Prepared,
    Before,
    Intervention,
    After,
    Cleanup,
    Finalized,
}

/// Factual kind of a trusted capture event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperatorCaptureEventKind {
    SessionPrepared,
    EvidenceCaptured,
    OperatorAttestation,
    InterventionRecorded,
    CleanupRecorded,
    ResultFinalized,
}

/// Strength and origin of the captured fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperatorCaptureProvenance {
    TrustedSupervisor,
    PublicDeadreckon,
    AuthoritativeHost,
    OperatorAttested,
}

/// Closed source vocabulary. Only `ManualFile` accepts a caller-provided path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperatorCaptureSource {
    Binding,
    JobView,
    JobEvents,
    JobIntervention,
    JobCleanup,
    Job,
    Authority,
    LaunchPlan,
    Lease,
    JobReport,
    Receipt,
    SupervisedChild,
    HostBootId,
    SemanticJudgment,
    ParentRepairManifest,
    ParentRepairCandidate,
    Doctor,
    SupervisorServiceStatus,
    ParentArtifact,
    ParentEvents,
    Campaign,
    CampaignEvents,
    ActivePlan,
    ActivePlanEvents,
    NetworkConnectivityObservation,
    SandboxBoundaryObservation,
    CampaignIntervention,
    ResultEnvelope,
    ManualFile,
    UnavailableObjective,
}

/// One authenticated append-only capture fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperatorCaptureEvent {
    pub schema_version: OperatorCaptureSchemaVersion,
    pub job_id: JobId,
    pub session_id: String,
    pub binding_sha256: String,
    pub sequence: OperatorCaptureEventSequence,
    pub event_id: String,
    pub causation_id: String,
    pub timestamp: DateTime<Utc>,
    pub phase: OperatorCapturePhase,
    pub kind: OperatorCaptureEventKind,
    pub provenance: OperatorCaptureProvenance,
    pub source: OperatorCaptureSource,
    pub subject: String,
    pub content_sha256: String,
    pub content_bytes: u64,
    pub previous_event_sha256: Option<String>,
    pub signature: String,
}

/// Final authenticated summary of a complete capture history and result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperatorCaptureReceipt {
    pub schema_version: OperatorCaptureSchemaVersion,
    pub job_id: JobId,
    pub session_id: String,
    pub binding_sha256: String,
    pub issued_at: DateTime<Utc>,
    pub event_count: u64,
    pub final_event_sha256: String,
    pub result_sha256: String,
    pub result_bytes: u64,
    pub completion_lineage: Option<OperatorCaptureCompletionLineage>,
    pub terminal_lineage: Option<OperatorCaptureTerminalLineage>,
    pub status: OperatorCaptureStatus,
    pub signature: String,
}

/// Exact validated completion identity sealed into a passed capture receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperatorCaptureCompletionLineage {
    pub completion_receipt_sha256: String,
    pub authority_sha256: String,
    pub contract_sha256: String,
    pub effective_policy_sha256: String,
    pub launch_plan_sha256: String,
    pub source_tree_sha256: String,
    pub source_revision: Option<String>,
    pub result_tree_sha256: String,
    pub result_revision: Option<String>,
}

/// Exact authenticated Job history behind a passed non-Verified trial.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OperatorCaptureTerminalLineage {
    pub authority_sha256: String,
    pub goal_sha256: String,
    pub contract_sha256: String,
    pub effective_policy_sha256: String,
    pub launch_plan_sha256: String,
    pub source_tree_sha256: String,
    pub source_revision: Option<String>,
    pub job_history_sha256: String,
    pub job_history_bytes: u64,
    pub terminal_event_sha256: String,
    pub terminal_sequence: u64,
    pub outcome: JobOutcome,
    pub stop_reason: StopReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperatorCaptureStatus {
    Passed,
    Failed,
    Inconclusive,
    NotRun,
}

#[cfg(test)]
mod tests {
    use super::{OperatorCaptureEventSequence, OperatorCaptureSchemaVersion};

    #[test]
    fn capture_wire_discriminators_reject_unsupported_values() {
        assert!(serde_json::from_str::<OperatorCaptureSchemaVersion>("3").is_err());
        assert!(serde_json::from_str::<OperatorCaptureEventSequence>("0").is_err());
        assert_eq!(
            serde_json::from_str::<OperatorCaptureEventSequence>("1")
                .expect("non-zero sequence")
                .get(),
            1
        );
    }
}
