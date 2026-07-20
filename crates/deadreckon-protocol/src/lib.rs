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
    FlightEvent, FlightEventKind, FlightUsage, NarrativeSnapshotRef, RunEvent, RunEventKind,
    SpendRecord, TraceRecord, spend_kind_loop,
};
