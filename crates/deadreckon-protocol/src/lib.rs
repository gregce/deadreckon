#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

//! Pure persisted-wire types for DeadReckon.
//!
//! This crate may depend on serialization and schema libraries, but never on
//! I/O, async runtimes, or another DeadReckon crate.

pub mod ids;
pub mod job;
pub mod ledger;
pub mod notify;
pub mod operator_capture;
pub mod policy;
pub mod schema;

pub use ids::{JobId, PlanId, RunId, TurnId};
pub use job::{
    AppliedGitDeliveryReceipt, AuthorityAcceptedBy, CompletionExecutionEvidence,
    CompletionProofKind, CompletionReceipt, CompletionReceiptIssuer, DOCKER_GATE_GUEST_PATH,
    DockerGateIdentity, GATE_EVALUATOR_IDENTITY_SCHEMA_VERSION, GATE_EVALUATOR_PROTOCOL_MARKER,
    GATE_EVALUATOR_PROTOCOL_VERSION, GateBinaryIdentity, GateEvaluatorIdentity, GitDeliveryIntent,
    GitDeliveryRepositoryIdentity, GitDeliveryStrategy, GoalCoverage, GoalCoverageStatus,
    JOB_SCHEMA_VERSION, Job, JobAuthority, JobEvent, JobEventKind, JobEventSequence,
    JobExecutionPolicy, JobGateNetworkAccess, JobGatePolicy, JobLease, JobOutcome, JobPhase,
    JobPolicy, JobSchemaVersion, JobShape, JobToolPolicy, SandboxBoundaryObservation,
    SandboxBoundaryObservationIssuer, SemanticDecision, SemanticJudgeMode, SemanticJudgment,
    StopReason,
};
pub use ledger::{
    EventLine, FlightEvent, FlightEventKind, FlightLine, FlightUsage, LedgerFile, LedgerItem,
    NarrativeSnapshotRef, NarrativeSnapshotRefLine, RunEvent, RunEventKind, SpendLine, SpendRecord,
    TraceLine, TraceRecord, spend_kind_loop,
};
pub use notify::{
    NOTIFY_EVENT_SCHEMA_VERSION, NotifyDeliveryAttempt, NotifyDeliveryTransition, NotifyEvent,
    OperatorAttentionEvent, OperatorAttentionKind, OperatorAttentionReason,
};
pub use operator_capture::{
    OPERATOR_CAPTURE_SCHEMA_VERSION, OperatorCaptureBinding, OperatorCaptureCompletionLineage,
    OperatorCaptureConnectivity, OperatorCaptureEvent, OperatorCaptureEventKind,
    OperatorCaptureEventSequence, OperatorCaptureExpectedJobResult, OperatorCaptureNetworkAttempt,
    OperatorCaptureNetworkErrorKind, OperatorCaptureNetworkObservation,
    OperatorCaptureNetworkProbe, OperatorCapturePhase, OperatorCaptureProvenance,
    OperatorCaptureReceipt, OperatorCaptureRequirement, OperatorCaptureSchemaVersion,
    OperatorCaptureSource, OperatorCaptureStatus, OperatorCaptureTerminalLineage,
};
pub use policy::{is_persisted, ledger_file_for, redact_for_persistence};
pub use schema::all_schemas;
