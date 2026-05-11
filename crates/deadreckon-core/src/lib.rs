//! Core state, locking, and run artifacts for the deadreckon harness.

pub mod artifacts;
pub mod error;
pub mod lock;
pub mod paths;
pub mod state;
pub mod turn_loop;

pub use artifacts::{
    ProvenanceRecord, SpendRecord, TraceRecord, append_provenance, append_spend, append_trace,
    inventory_files, restore_snapshot, snapshot_working,
};
pub use error::{DeadreckonError, Result};
pub use lock::{
    LockGuard, LockState, LockStatus, acquire_lock, lock_status, pid_is_alive, release_lock_file,
    terminate_pid,
};
pub use paths::{DEFAULT_DEADRECKON_HOME, DeadreckonPaths, SOURCE_ROOT};
pub use state::{
    CurrentRunPointer, PhaseId, PhaseState, PhaseStatus, PipelineState, RunListEntry, RunOptions,
    RunStatus, create_run, list_runs, load_run, save_state,
};
pub use turn_loop::{RunLoopConfig, RunLoopOutcome, run_turn_loop};
