#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

//! Core state, locking, and run artifacts for the deadreckon harness.

pub mod acceptance_defaults;
pub mod artifact_policy;
pub mod artifacts;
pub mod campaign;
pub mod cancel;
pub mod chain;
pub mod codebase;
pub mod completion;
pub mod delivery;
pub mod docs;
pub mod error;
pub mod events;
pub mod exec;
pub mod flight;
pub mod gate;
pub mod git;
pub mod glossary;
pub mod install_receipt;
pub mod job;
pub mod job_lease;
pub mod learning;
pub mod ledger_io;
pub mod lock;
pub mod operator_capture;
pub mod paths;
pub mod plan;
pub mod polish_subcalls;
pub mod promotion;
pub mod run_view;
pub mod sandbox_observation;
pub mod state;
pub mod steer_inbox;
pub mod tamper;
pub mod update_cache;
pub mod workspace_capture;

pub use artifact_policy::{
    WorkspacePathClass, classify_workspace_path, delivery_git_exclude_pathspecs,
    evidence_only_roots, is_checkpointable_workspace_path, is_deliverable_workspace_path,
    is_promotable_workspace_path, is_recoverable_workspace_path, runtime_output_root,
};
pub use artifacts::{
    DiffSummary, FileDelta, FileDeltaStatus, ProvenanceRecord, SNAPSHOT_CAPTURE_MANIFESTS_DIR,
    append_provenance, append_spend, append_trace, copy_artifact_path, copy_deliverable_tree,
    copy_promotable_tree, copy_recoverable_tree, copy_recoverable_tree_with_policy, copy_tree,
    diff_snapshots, diff_working_trees, inventory_files, inventory_recoverable_files,
    inventory_recoverable_files_for_state, inventory_recoverable_files_with_policy,
    remove_artifact_path, restore_snapshot, snapshot_capture_manifest_path, snapshot_diff,
    snapshot_working,
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
    TRUSTED_CODEBASE_RECORD, WorktreeOptions, codebase_record_path, copy_source_to_working,
    copy_source_to_working_with_policy, create_worktree, find_git_root, prepare_worktree_record,
    preview_git_state, read_codebase_record, read_run_codebase_record,
    read_trusted_codebase_record, record_for_resolved_mode, resolve_mode, user_error,
    write_codebase_record, write_trusted_codebase_record,
};
pub use completion::{
    SEMANTIC_JUDGMENT_JSON, seal_completion_receipt, validate_completion_receipt,
    validate_strict_contract,
};
pub use delivery::{
    GitDeliveryTarget, JobOperationLock, ValidatedAppliedGitDeliveryReceipt,
    ValidatedGitDeliveryIntent, acquire_job_operation_lock, seal_applied_git_delivery_receipt,
    seal_git_delivery_intent, validate_applied_git_delivery_receipt,
    validate_applied_git_delivery_receipt_snapshot, validate_git_delivery_intent,
    validate_git_delivery_intent_snapshot,
};
pub use docs::{
    AS_BUILT_DELTA, DOCS_DIR, DocKind, DocsStatus, FileChange, FrontmatterFields,
    IMPLEMENTATION_NOTES_HTML, INCREMENTAL_JSONL, ImplementationNotesStatus, POLISH_JSON,
    PUBLIC_DOCS_DIR, RUN_AS_BUILT, RUN_DECISIONS, RUN_NARRATIVE, TurnDocInput, TurnRecord,
    append_parent_narrative_update, append_turn_doc, apply_commit_body, as_built_path, auto_title,
    capture_diff_samples, capture_response_full, capture_response_summary, capture_tool_stdio,
    changed_doc_files, check_implementation_notes_current, coalesce_into_phases,
    copy_public_docs_from_internal, decisions_path, delta_path, diff_samples_markdown,
    doc_path_for_kind, docs_dir, docs_inventory, docs_status_for_state, ensure_docs_started,
    ensure_implementation_notes_started, frontmatter, implementation_notes_path, incremental_path,
    is_decision_candidate, is_documentable_path, missing_files_in_narrative, narrative_path,
    polish_path, public_doc_path, public_docs_dir, publish_docs_for_promotion, read_turn_records,
    rewrite_templated_docs, should_emit_delta, source_layout, tool_stdio_markdown,
};
pub use error::{DeadreckonError, Result, is_retryable_io_kind};
pub use events::{RUN_EVENTS_JSONL, RunEventBus, emit_event, event_preview};
#[cfg(unix)]
pub use exec::ProcessGroupTerminator;
pub use exec::{
    ChildTerminator, HeadTailBuffer, RawPidTerminator, SUPERVISED_PROCESS_RECORD_SCHEMA_VERSION,
    SupervisedProcess, SupervisedProcessIdentity, SupervisedProcessPhase, SupervisedProcessRecord,
    TerminationOutcome, TruncationPolicy, boot_identities_match, boot_identity,
    normalize_boot_identity, process_start_identity, read_supervised_process,
    read_supervised_process_record, remove_supervised_process_record_if_matches,
    remove_supervised_process_record_if_same, spawn_grouped, write_supervised_process,
    write_supervised_process_record,
};
pub use gate::{
    ACCEPTANCE_PROGRESS_JSONL, AcceptanceCheck, AcceptanceCheckResult, AcceptanceContainment,
    AcceptanceMarker, AcceptanceProgressEntry, AcceptanceProofKind, AcceptanceSignatureStrength,
    AcceptanceSpec, GATE_CONTAINED_ENV, GATE_EVALUATION_SCHEMA_VERSION, GATE_KEY_ENV,
    GATE_SANDBOX_BACKEND_ENV, GateEvaluation, PARENT_REPAIR_CANDIDATE_JSON,
    PARENT_REPAIR_MANIFEST_JSON, acceptance_progress_path_for_run_root,
    acceptance_spec_path_for_run_root, create_gate_key, decode_gate_key, encode_gate_key,
    evaluate_acceptance, evaluate_acceptance_checks, evaluate_acceptance_checks_with_progress,
    evaluate_gate, gate_key_path, gate_key_path_for_run_root, gate_nonce_path_for_run_root,
    marker_path_for_run_root, parent_repair_candidate_path_for_run_root,
    parent_repair_manifest_path_for_run_root, read_gate_key, read_gate_key_for_run_root,
    sign_gate_evaluation_with_key, validate_acceptance_marker, validate_gate_evaluation,
    validate_gate_evaluation_integrity, verify_v2_marker_signature, write_acceptance_marker,
    write_acceptance_marker_with_results, write_gate_key,
    write_native_acceptance_marker_with_results_and_key,
};
pub use glossary::{
    NOUN_CHAIN, NOUN_CHILD, NOUN_PLAN, NOUN_RUN, StatusLabel, chain_status_label,
    chain_step_status_label, phase_status_label, plan_status_label, plan_task_status_label,
    run_status_label, status_label,
};
pub use job::{
    JOB_CONTROL_LOCK, JOB_EVENTS_JSONL, JOB_JSON, JOB_PROJECTION_JSON, JobDelivery,
    JobDeliveryKind, JobHistory, JobProjection, JobView, LegacyJobKind, LegacyJobView,
    append_job_event, legacy_campaign_job_view, legacy_chain_job_view, legacy_plan_job_view,
    legacy_run_job_view, load_job, load_job_projection, read_job_history, rebuild_job_projection,
    reduce_job_history, write_job,
};
pub use job_lease::{
    CreateFencedJobJsonDisposition, FencedJobJsonEvent, LeaseClaim, LeaseClaimDisposition,
    LeaseOwner, LeaseReclaimReason, LeaseToken, append_fenced_job_event,
    append_next_fenced_job_event, claim_job_lease, create_fenced_job_json_and_append_event,
    heartbeat_job_lease, load_job_lease, replace_fenced_job_json_and_append_event,
};
pub use lock::{
    LockGuard, LockState, LockStatus, acquire_lock, lock_status, pid_is_alive, release_lock_file,
    terminate_pid,
};
pub use operator_capture::{
    OPERATOR_CAPTURE_BINDING_JSON, OPERATOR_CAPTURE_EVENTS_JSONL, OPERATOR_CAPTURE_RECEIPT_JSON,
    OperatorCaptureEventDraft, OperatorCaptureHistory, OperatorCapturePassLineage,
    append_operator_capture_event, load_operator_capture_binding, operator_capture_binding_sha256,
    read_operator_capture_history, seal_operator_capture_receipt,
    validate_operator_capture_history, validate_operator_capture_receipt,
    write_operator_capture_binding,
};
pub use paths::{DeadreckonPaths, default_deadreckon_home, source_root};
pub use plan::{
    COORDINATOR_JSON, CapabilityPreview, CoordinatorChild, CoordinatorState, NetworkCapability,
    PLAN_CHILD_PARENT_JSON, PLAN_EVENTS_JSONL, PLAN_JSON, PLAN_MESSAGES_JSONL, Plan,
    PlanChildMarker, PlanEvent, PlanEventKind, PlanMessage, PlanMessageKind, PlanMode,
    PlanProviders, PlanRole, PlanStatus, PlanTask, PlanTaskStatus, SUMMARIES_DIR, WORKER_SPECS_DIR,
    append_owned_plan_event_fenced, append_owned_plan_message_fenced, append_plan_event,
    append_plan_message, child_summary_relative_path, load_plan, plan_task_key, read_plan_events,
    read_plan_messages, save_owned_plan_fenced, save_plan, validate_task_count,
    validate_task_graph, worker_spec_relative_path, write_child_summary, write_coordinator_state,
    write_owned_coordinator_state_fenced, write_plan_child_marker, write_worker_spec,
};
pub use polish_subcalls::{
    DEFAULT_DOC_POLISH_TOKEN_BUDGET, DEFAULT_DOC_SUBSKILLS, DocProviderSelection,
    DocProviderSource, PolishDiffCoverage, PolishSubcallRecord,
};
pub use promotion::{PromotionManifest, promote_completed_run, recover_promotion};
pub use run_view::{
    Artifact, CheckOutcome, ExchangeRef, Money, ProofBand, RunIdentity, RunView, RunViewDocKind,
    SandboxEvent, SandboxFact, SignatureFact, SignatureStatus, SpendBand, TurnView, VerdictBand,
    WhyBand,
};
pub use sandbox_observation::{
    SANDBOX_BOUNDARY_OBSERVATION_JSON, gate_evaluator_identity_sha256,
    sandbox_boundary_observation_sha256, sandbox_boundary_result_tree_sha256,
    seal_sandbox_boundary_observation, validate_sandbox_boundary_observation,
};
pub use state::{
    CurrentRunPointer, MergeRepairOwnership, PhaseId, PhaseState, PhaseStatus, PipelineState,
    ProviderFailureDisposition, RunListEntry, RunOptions, RunOwnership, RunOwnershipArtifact,
    RunStatus, create_owned_run, create_run, list_runs, load_run, save_state,
};
pub use workspace_capture::{
    CaptureBudgets, CaptureEntry, CaptureEntryKind, CaptureMaterialization, CaptureOmission,
    CaptureOmissionReason, CaptureProjection, CapturePurpose, EncodedWorkspacePath,
    FrozenGitHydration, GeneratedOutputRoot, GeneratedOutputSource, GitHydrationState,
    SOURCE_HYDRATION_MANIFEST_JSON, WORKSPACE_BLOBS_DIR, WorkspaceCaptureManifest,
    WorkspaceCapturePlan, WorkspaceCapturePolicy, capture_workspace, capture_workspace_strict,
    ensure_workspace_capture_policy, freeze_git_hydration, freeze_workspace_capture_policy,
    materialize_capture_entry_with_blob_store, materialize_capture_plan,
    materialize_capture_plan_with_blob_store, read_capture_manifest, read_workspace_capture_policy,
    require_frozen_git_hydration, require_workspace_capture_policy, workspace_capture_policy_path,
    write_capture_manifest, write_workspace_capture_policy,
};
