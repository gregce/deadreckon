#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

//! Pure persisted-wire types for DeadReckon.
//!
//! This crate may depend on serialization and schema libraries, but never on
//! I/O, async runtimes, or another DeadReckon crate.

pub mod ids;
pub mod ledger;
pub mod policy;
pub mod schema;

pub use ids::{PlanId, RunId, TurnId};
pub use ledger::{
    EventLine, FlightEvent, FlightEventKind, FlightLine, FlightUsage, LedgerFile, LedgerItem,
    NarrativeSnapshotRef, NarrativeSnapshotRefLine, RunEvent, RunEventKind, SpendLine, SpendRecord,
    TraceLine, TraceRecord, spend_kind_loop,
};
pub use policy::{is_persisted, ledger_file_for, redact_for_persistence};
