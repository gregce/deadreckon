//! Core state, locking, and run artifacts for the deadreckon harness.

pub mod artifacts;
pub mod codebase;
pub mod error;
pub mod events;
pub mod gate;
pub mod lock;
pub mod paths;
pub mod promotion;
pub mod state;
pub mod turn_loop;

pub use artifacts::{
    ProvenanceRecord, SpendRecord, TraceRecord, append_provenance, append_spend, append_trace,
    copy_tree, inventory_files, restore_snapshot, snapshot_working,
};
pub use codebase::{
    CODEBASE_RECORD_PATH, CodebaseMode, CodebaseRecord, ModeFlags, ResolvedMode,
    codebase_record_path, find_git_root, read_codebase_record, record_for_resolved_mode,
    resolve_mode, user_error, write_codebase_record,
};
pub use error::{DeadreckonError, Result};
pub use events::{
    RUN_EVENTS_JSONL, RunEvent, RunEventBus, RunEventKind, emit_event, event_preview,
};
pub use gate::{
    AcceptanceCheck, AcceptanceCheckResult, AcceptanceMarker, AcceptanceSpec,
    acceptance_spec_path_for_run_root, evaluate_acceptance, gate_nonce_path_for_run_root,
    marker_path_for_run_root, validate_acceptance_marker, write_acceptance_marker,
};
pub use lock::{
    LockGuard, LockState, LockStatus, acquire_lock, lock_status, pid_is_alive, release_lock_file,
    terminate_pid,
};
pub use paths::{DEFAULT_DEADRECKON_HOME, DeadreckonPaths, SOURCE_ROOT};
pub use promotion::{PromotionManifest, promote_completed_run, recover_promotion};
pub use state::{
    CurrentRunPointer, PhaseId, PhaseState, PhaseStatus, PipelineState, RunListEntry, RunOptions,
    RunStatus, create_run, list_runs, load_run, save_state,
};
pub use turn_loop::{RunLoopConfig, RunLoopOutcome, run_turn_loop};
