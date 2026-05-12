//! Core state, locking, and run artifacts for the deadreckon harness.

pub mod artifacts;
pub mod cancel;
pub mod codebase;
pub mod docs;
pub mod error;
pub mod events;
pub mod gate;
pub mod lock;
pub mod paths;
pub mod polish;
pub mod promotion;
pub mod state;
pub mod turn_loop;

pub use artifacts::{
    ProvenanceRecord, SpendRecord, TraceRecord, append_provenance, append_spend, append_trace,
    copy_tree, inventory_files, restore_snapshot, snapshot_working,
};
pub use cancel::{
    CANCEL_MARKER, CancelMarker, cancel_marker_path, cancel_marker_path_for_run_root,
    cancel_marker_present, clear_cancel_marker, write_cancel_marker,
};
pub use codebase::{
    CODEBASE_RECORD_PATH, CodebaseMode, CodebaseRecord, ModeFlags, PreviewGitState, ResolvedMode,
    WorktreeOptions, codebase_record_path, copy_source_to_working, create_worktree, find_git_root,
    prepare_worktree_record, preview_git_state, read_codebase_record, record_for_resolved_mode,
    resolve_mode, user_error, write_codebase_record,
};
pub use docs::{
    AS_BUILT_DELTA, DOCS_DIR, DocKind, DocsStatus, FrontmatterFields, INCREMENTAL_JSONL,
    POLISH_JSON, PUBLIC_DOCS_DIR, RUN_AS_BUILT, RUN_DECISIONS, RUN_NARRATIVE, TurnDocInput,
    TurnRecord, append_parent_narrative_update, append_turn_doc, apply_commit_body, as_built_path,
    auto_title, changed_doc_files, coalesce_into_phases, copy_public_docs_from_internal,
    decisions_path, delta_path, doc_path_for_kind, docs_dir, docs_inventory, docs_status_for_state,
    ensure_docs_started, frontmatter, incremental_path, is_decision_candidate,
    missing_files_in_narrative, narrative_path, polish_path, public_doc_path, public_docs_dir,
    publish_docs_for_promotion, read_turn_records, rewrite_templated_docs, should_emit_delta,
};
pub use error::{DeadreckonError, Result};
pub use events::{
    RUN_EVENTS_JSONL, RunEvent, RunEventBus, RunEventKind, emit_event, event_preview,
};
pub use gate::{
    AcceptanceCheck, AcceptanceCheckResult, AcceptanceMarker, AcceptanceSpec,
    acceptance_spec_path_for_run_root, evaluate_acceptance, gate_nonce_path_for_run_root,
    marker_path_for_run_root, validate_acceptance_marker, write_acceptance_marker,
    write_acceptance_marker_with_results,
};
pub use lock::{
    LockGuard, LockState, LockStatus, acquire_lock, lock_status, pid_is_alive, release_lock_file,
    terminate_pid,
};
pub use paths::{DEFAULT_DEADRECKON_HOME, DeadreckonPaths, SOURCE_ROOT};
pub use polish::{
    PolishConfig, PolishRecord, PolishedDocs, ResolvedSkill, SkillSource,
    default_polished_json_for_tests, inputs_hash, polish_run_docs, read_polish_record,
    resolve_skill, substitute_placeholders, templated_docs_json,
};
pub use promotion::{PromotionManifest, promote_completed_run, recover_promotion};
pub use state::{
    CurrentRunPointer, PhaseId, PhaseState, PhaseStatus, PipelineState, RunListEntry, RunOptions,
    RunStatus, create_run, list_runs, load_run, save_state,
};
pub use turn_loop::{RunLoopConfig, RunLoopDocsConfig, RunLoopOutcome, run_turn_loop};
