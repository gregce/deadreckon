//! JSON Schema generation from protocol types.

use schemars::schema::RootSchema;
use schemars::schema_for;

use crate::job::{
    CompletionReceipt, Job, JobAuthority, JobEvent, JobLease, SandboxBoundaryObservation,
    SemanticJudgment,
};
use crate::ledger::{
    FlightEvent, LedgerItem, NarrativeSnapshotRef, RunEvent, SpendRecord, TraceRecord,
};
use crate::notify::NotifyEvent;
use crate::operator_capture::{
    OperatorCaptureBinding, OperatorCaptureEvent, OperatorCaptureReceipt,
};

/// Returns the checked schema set for the union and every persisted line kind.
pub fn all_schemas() -> Vec<(&'static str, RootSchema)> {
    vec![
        ("ledger-item", schema_for!(LedgerItem)),
        ("run-event", schema_for!(RunEvent)),
        ("spend-record", schema_for!(SpendRecord)),
        ("trace-record", schema_for!(TraceRecord)),
        ("flight-event", schema_for!(FlightEvent)),
        ("narrative-snapshot-ref", schema_for!(NarrativeSnapshotRef)),
        ("notify-event", schema_for!(NotifyEvent)),
        ("job", schema_for!(Job)),
        ("job-event", schema_for!(JobEvent)),
        ("job-lease", schema_for!(JobLease)),
        ("job-authority", schema_for!(JobAuthority)),
        ("semantic-judgment", schema_for!(SemanticJudgment)),
        (
            "sandbox-boundary-observation",
            schema_for!(SandboxBoundaryObservation),
        ),
        ("completion-receipt", schema_for!(CompletionReceipt)),
        (
            "operator-capture-binding",
            schema_for!(OperatorCaptureBinding),
        ),
        ("operator-capture-event", schema_for!(OperatorCaptureEvent)),
        (
            "operator-capture-receipt",
            schema_for!(OperatorCaptureReceipt),
        ),
    ]
}
