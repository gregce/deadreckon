#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

//! Core state, locking, and run artifacts for the deadreckon harness.

pub mod artifacts;
pub mod cancel;
pub mod chain;
pub mod codebase;
pub mod docs;
pub mod error;
pub mod events;
pub mod gate;
pub mod glossary;
pub mod lock;
pub mod paths;
pub mod plan;
pub mod polish_subcalls;
pub mod promotion;
pub mod state;

pub use artifacts::{
    ProvenanceRecord, SpendRecord, TraceRecord, append_provenance, append_spend, append_trace,
    copy_tree, inventory_files, restore_snapshot, snapshot_working,
};
pub use cancel::{
    CANCEL_MARKER, CancelMarker, cancel_marker_path, cancel_marker_path_for_run_root,
    cancel_marker_present, clear_cancel_marker, write_cancel_marker,
};
pub use chain::{
    ApplyMode, ApplyStrategy, BranchPolicy, CHAIN_EVENTS_JSONL, CHAIN_JSON, CHAIN_LOCK_PREFIX,
    CHAIN_STEP_JSON, Chain, ChainEvent, ChainEventKind, ChainNewOptions, ChainStatus, ChainStep,
    ChainStepMarker, ChainStepStatus, ConductorState, OnFail, append_chain_event, chain_json_path,
    chain_task_key, load_chain, read_chain_step_marker, save_chain, validate_goal_count,
    write_chain_step_marker,
};
pub use codebase::{
    CODEBASE_RECORD_PATH, CodebaseMode, CodebaseRecord, ModeFlags, PreviewGitState, ResolvedMode,
    WorktreeOptions, codebase_record_path, copy_source_to_working, create_worktree, find_git_root,
    prepare_worktree_record, preview_git_state, read_codebase_record, record_for_resolved_mode,
    resolve_mode, user_error, write_codebase_record,
};
pub use docs::{
    AS_BUILT_DELTA, DOCS_DIR, DocKind, DocsStatus, FileChange, FrontmatterFields,
    INCREMENTAL_JSONL, POLISH_JSON, PUBLIC_DOCS_DIR, RUN_AS_BUILT, RUN_DECISIONS, RUN_NARRATIVE,
    TurnDocInput, TurnRecord, append_parent_narrative_update, append_turn_doc, apply_commit_body,
    as_built_path, auto_title, capture_diff_samples, capture_response_full,
    capture_response_summary, capture_tool_stdio, changed_doc_files, coalesce_into_phases,
    copy_public_docs_from_internal, decisions_path, delta_path, diff_samples_markdown,
    doc_path_for_kind, docs_dir, docs_inventory, docs_status_for_state, ensure_docs_started,
    frontmatter, incremental_path, is_decision_candidate, is_documentable_path,
    missing_files_in_narrative, narrative_path, polish_path, public_doc_path, public_docs_dir,
    publish_docs_for_promotion, read_turn_records, rewrite_templated_docs, should_emit_delta,
    source_layout, tool_stdio_markdown,
};
pub use error::{DeadreckonError, Result};
pub use events::{
    RUN_EVENTS_JSONL, RunEvent, RunEventBus, RunEventKind, emit_event, event_preview,
};
pub use gate::{
    ACCEPTANCE_PROGRESS_JSONL, AcceptanceCheck, AcceptanceCheckResult, AcceptanceMarker,
    AcceptanceProgressEntry, AcceptanceSpec, acceptance_progress_path_for_run_root,
    acceptance_spec_path_for_run_root, evaluate_acceptance, evaluate_acceptance_checks,
    evaluate_acceptance_checks_with_progress, gate_nonce_path_for_run_root,
    marker_path_for_run_root, validate_acceptance_marker, write_acceptance_marker,
    write_acceptance_marker_with_results,
};
pub use glossary::{
    NOUN_CHAIN, NOUN_CHILD, NOUN_PLAN, NOUN_RUN, StatusLabel, chain_status_label,
    chain_step_status_label, phase_status_label, plan_status_label, plan_task_status_label,
    run_status_label, status_label,
};
pub use lock::{
    LockGuard, LockState, LockStatus, acquire_lock, lock_status, pid_is_alive, release_lock_file,
    terminate_pid,
};
pub use paths::{DEFAULT_DEADRECKON_HOME, DeadreckonPaths, SOURCE_ROOT};
pub use plan::{
    COORDINATOR_JSON, CapabilityPreview, CoordinatorChild, CoordinatorState, NetworkCapability,
    PLAN_CHILD_PARENT_JSON, PLAN_JSON, PLAN_MESSAGES_JSONL, Plan, PlanChildMarker, PlanMessage,
    PlanMessageKind, PlanMode, PlanProviders, PlanRole, PlanStatus, PlanTask, PlanTaskStatus,
    SUMMARIES_DIR, WORKER_SPECS_DIR, append_plan_message, child_summary_relative_path, load_plan,
    plan_task_key, read_plan_messages, save_plan, validate_task_count, validate_task_graph,
    worker_spec_relative_path, write_child_summary, write_coordinator_state,
    write_plan_child_marker, write_worker_spec,
};
pub use polish_subcalls::{
    DEFAULT_DOC_POLISH_TOKEN_BUDGET, DEFAULT_DOC_SUBSKILLS, DocProviderSelection,
    DocProviderSource, PolishDiffCoverage, PolishSubcallRecord,
};
pub use promotion::{PromotionManifest, promote_completed_run, recover_promotion};
pub use state::{
    CurrentRunPointer, PhaseId, PhaseState, PhaseStatus, PipelineState, RunListEntry, RunOptions,
    RunStatus, create_run, list_runs, load_run, save_state,
};
