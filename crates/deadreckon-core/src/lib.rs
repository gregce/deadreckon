//! Core state, locking, and run artifacts for the deadreckon harness.

pub mod error;
pub mod paths;
pub mod state;

pub use error::{DeadreckonError, Result};
pub use paths::{DEFAULT_DEADRECKON_HOME, DeadreckonPaths, SOURCE_ROOT};
pub use state::{
    CurrentRunPointer, PhaseId, PhaseState, PhaseStatus, PipelineState, RunListEntry, RunOptions,
    RunStatus,
};
