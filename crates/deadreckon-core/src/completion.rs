//! Cryptographically bound two-key completion receipts.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use deadreckon_protocol::{
    CompletionExecutionEvidence, CompletionProofKind, CompletionReceipt, CompletionReceiptIssuer,
    GoalCoverageStatus, Job, JobAuthority, JobEvent, JobEventKind, JobOutcome, JobSchemaVersion,
    JobShape, SemanticDecision, SemanticJudgment, StopReason,
};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::flight::{build_deliverable_file_index, sha256_file, sha256_text};
use crate::gate::{
    AcceptanceCheck, AcceptanceMarker, acceptance_checks_from_yaml, read_gate_key,
    validate_acceptance_marker_with_parent_repair_bytes,
};
use crate::job::{JobHistory, load_job, read_job_history, reduce_job_history};
use crate::paths::DeadreckonPaths;
use crate::sandbox_observation::{
    sandbox_boundary_observation_sha256, validate_sandbox_boundary_observation,
};
use crate::state::{PipelineState, atomic_write_json};
use crate::{
    CodebaseMode, WorkspacePathClass, classify_workspace_path, read_trusted_codebase_record,
};

const RECEIPT_MAGIC: &[u8] = b"deadreckon.completion-receipt.v1\0";
pub const SEMANTIC_JUDGMENT_JSON: &str = "proofs/semantic-judgment.json";
const PARENT_REPAIR_INTENT_JSON: &str = "parent-repair.json";
const PARENT_REPAIR_ARCHIVE_DIR: &str = "parent-repairs";
const DURABLE_CHAIN_ADAPTER_SIGNAL: &str = "watchkeeper_chain_adapter";
const ORDERED_CANDIDATE_MANIFEST_JSON: &str = "ordered-candidate.json";
const PARENT_REPAIR_ROUND_FILES: [&str; 5] = [
    "intent.json",
    "final-attempt.json",
    "candidate.json",
    "pre-repair-marker.json",
    "revise-judgment.json",
];

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ParentRepairIntentBinding {
    schema_version: u32,
    job_id: String,
    shape: JobShape,
    round: u32,
    merged_run_id: String,
    merged_tree_sha256: String,
    pre_repair_tree_sha256: String,
    revise_marker_sha256: String,
    revise_judgment_sha256: String,
    revise_input_sha256: String,
    requested_after_attempt: u32,
    requested_after_launch_id: String,
    requested_after_lease_epoch: u64,
    #[serde(rename = "provider")]
    _provider: Option<String>,
    #[serde(rename = "model")]
    _model: Option<String>,
    feedback: String,
    previous_round_sha256: Option<String>,
    #[serde(rename = "requested_at")]
    _requested_at: DateTime<Utc>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ParentRepairManifestBinding {
    schema_version: u32,
    job_id: String,
    shape: JobShape,
    round: u32,
    merged_run_id: String,
    merged_tree_sha256: String,
    pre_repair_tree_sha256: String,
    intent_sha256: String,
    attempt: u32,
    launch_id: String,
    lease_epoch: u64,
    attempt_baseline_tree_sha256: String,
    #[serde(rename = "started_at")]
    _started_at: DateTime<Utc>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ParentRepairCandidateBinding {
    schema_version: u32,
    job_id: String,
    run_id: String,
    round: u32,
    attempt: u32,
    launch_id: String,
    lease_epoch: u64,
    intent_sha256: String,
    manifest_sha256: String,
    result_tree_sha256: String,
    #[serde(rename = "turn")]
    _turn: u32,
    #[serde(rename = "ready_at")]
    _ready_at: DateTime<Utc>,
}

struct TrustedFileSnapshot {
    bytes: Vec<u8>,
    sha256: String,
}

struct ValidatedParentRepair {
    manifest: TrustedFileSnapshot,
    candidate: TrustedFileSnapshot,
}

struct ParentRepairRoundSnapshot {
    intent: TrustedFileSnapshot,
    manifest: TrustedFileSnapshot,
    candidate: TrustedFileSnapshot,
    marker: TrustedFileSnapshot,
    judgment: TrustedFileSnapshot,
}

fn completion_execution_evidence(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<Option<CompletionExecutionEvidence>> {
    let launch_path = paths.job_launch_plan(job_id);
    let launch = read_required_trusted_file_snapshot(&launch_path, "launch plan", job_id)?;
    let launch: serde_json::Value =
        serde_json::from_slice(&launch.bytes).with_json_path(&launch_path)?;
    if launch
        .get("signals")
        .and_then(serde_json::Value::as_object)
        .and_then(|signals| signals.get(DURABLE_CHAIN_ADAPTER_SIGNAL))
        .is_none()
    {
        return Ok(None);
    }

    let job_dir = paths.job_dir(job_id);
    let manifest = read_required_trusted_file_snapshot(
        &job_dir.join(ORDERED_CANDIDATE_MANIFEST_JSON),
        "ordered candidate manifest",
        job_id,
    )?;
    let applications = read_optional_trusted_file_snapshot(
        &job_dir.join(crate::plan::ORDERED_CANDIDATE_APPLICATION_EVENTS_JSONL),
        "ordered candidate application ledger",
        job_id,
    )?;
    let hooks = read_optional_trusted_file_snapshot(
        &job_dir.join(crate::chain::DURABLE_CHAIN_HOOK_EVENTS_JSONL),
        "durable chain hook ledger",
        job_id,
    )?;
    Ok(Some(CompletionExecutionEvidence {
        ordered_candidate_manifest_sha256: manifest.sha256,
        candidate_application_events_sha256: applications.map(|snapshot| snapshot.sha256),
        chain_hook_events_sha256: hooks.map(|snapshot| snapshot.sha256),
    }))
}

pub fn seal_completion_receipt(
    paths: &DeadreckonPaths,
    state: &PipelineState,
    authority: &JobAuthority,
    marker: &AcceptanceMarker,
    judgment: &SemanticJudgment,
) -> Result<CompletionReceipt> {
    let job = load_job(paths, authority.job_id.as_ref())?;
    if job.job_id != authority.job_id || state.run_id != authority.run_id.as_ref() {
        return Err(completion_error(
            authority.job_id.as_ref(),
            "job, authority, and run identities do not agree",
        ));
    }
    let parent_repair = validate_parent_repair_lineage_if_present(paths, state)?;
    let validated_marker = validate_acceptance_marker_with_parent_repair_bytes(
        state,
        parent_repair
            .as_ref()
            .map(|repair| repair.manifest.bytes.as_slice()),
        parent_repair
            .as_ref()
            .map(|repair| repair.candidate.bytes.as_slice()),
    )?;
    if validated_marker != *marker || !marker.is_native_gate_proof() {
        return Err(completion_error(
            authority.job_id.as_ref(),
            "deterministic proof is not a validated native dr-gate marker",
        ));
    }
    if !marker.contained || marker.sandbox_backend == "none" {
        return Err(completion_error(
            authority.job_id.as_ref(),
            "strict job completion requires a contained deterministic gate",
        ));
    }
    if judgment.job_id != authority.job_id
        || judgment.run_id != authority.run_id
        || judgment.decision != SemanticDecision::Achieved
    {
        return Err(completion_error(
            authority.job_id.as_ref(),
            "semantic judgment is not an achieved decision for this job and run",
        ));
    }
    validate_achieved_judgment(judgment, state.run_id.as_str())?;

    let authority_path = paths.job_authority(authority.job_id.as_ref());
    let launch_path = paths.job_launch_plan(authority.job_id.as_ref());
    let marker_path = crate::marker_path_for_run_root(&state.run_root);
    let semantic_path = state.run_root.join(SEMANTIC_JUDGMENT_JSON);
    verify_authority_inputs(&job, authority, &authority_path, &launch_path, state)?;
    let result_revision = validate_worktree_result_boundary(state, authority, None)?;
    let projection_required = crate::result_projection_required(paths, state.run_id.as_str())?;
    if projection_required && !crate::result_projection_exists(state) {
        return Err(completion_error(
            authority.job_id.as_ref(),
            "new strict result is missing its required controller-sealed projection",
        ));
    }
    if crate::result_projection_exists(state) {
        crate::validate_result_projection_at(state, &state.working_dir)?;
        crate::validate_result_projection_at(
            state,
            &crate::result_projection_candidate_path(state),
        )?;
    }
    let result_projection_sha256 = crate::result_projection_exists(state)
        .then(|| crate::result_projection_sha256(state))
        .transpose()?;
    let result_tree_sha256 = result_tree_hash(state)?;
    let sandbox_observation =
        validate_sandbox_boundary_observation(paths, state, authority, &marker.sandbox_backend)?;
    let execution_evidence = completion_execution_evidence(paths, authority.job_id.as_ref())?;
    let mut receipt = CompletionReceipt {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: authority.job_id.clone(),
        run_id: authority.run_id.clone(),
        attempt: sandbox_observation.attempt,
        outer_launch_id: sandbox_observation.outer_launch_id,
        issued_at: chrono::Utc::now(),
        issuer: CompletionReceiptIssuer::DeadreckonSupervisor,
        proof_kind: CompletionProofKind::TwoKeyCompletion,
        outcome: JobOutcome::Verified,
        stop_reason: StopReason::Verified,
        authority_sha256: sha256_file(&authority_path)?,
        goal_sha256: authority.goal_sha256.clone(),
        contract_sha256: authority.contract_sha256.clone(),
        effective_policy_sha256: authority.effective_policy_sha256.clone(),
        launch_plan_sha256: authority.launch_plan_sha256.clone(),
        source_tree_sha256: authority.source_tree_sha256.clone(),
        source_revision: authority.source_revision.clone(),
        result_projection_sha256,
        result_tree_sha256,
        result_revision: match result_revision {
            Some(revision) => Some(revision),
            None => current_git_revision(&state.working_dir)?,
        },
        deterministic_marker_sha256: sha256_file(&marker_path)?,
        semantic_judgment_sha256: sha256_file(&semantic_path)?,
        sandbox_boundary_observation_sha256: sandbox_boundary_observation_sha256(
            paths,
            authority.job_id.as_ref(),
        )?,
        execution_evidence,
        contained: marker.contained,
        sandbox_backend: marker.sandbox_backend.clone(),
        signature: String::new(),
    };
    if let Some(revision) = receipt.result_revision.as_deref() {
        retain_signed_result_revision(state, authority.job_id.as_ref(), revision)?;
    }
    let key = read_gate_key(paths, &state.run_id)?;
    receipt.signature = sign_receipt(&receipt, &key)?;
    let receipt_path = paths.job_receipt(authority.job_id.as_ref());
    let newly_sealed = !receipt_path.exists();
    atomic_write_json(&receipt_path, &receipt)?;
    if newly_sealed {
        // Display-only operator-attention signal owned by this sealing path
        // (docs/TAILING.md). Best-effort: the sealed receipt is the fact.
        let _ = crate::attention::append_operator_attention(
            &state.run_root,
            &crate::attention::verified_awaiting_promote_event(
                authority.job_id.as_ref(),
                &state.run_id,
                &state.scope,
            ),
        );
    }
    Ok(receipt)
}

/// Seal a receipt while every nested Git subprocess inherits the enclosing
/// Job's absolute work cutoff, cancellation signal, cleanup budget, and
/// durable process-authority directory.
pub fn seal_completion_receipt_bounded(
    paths: &DeadreckonPaths,
    state: &PipelineState,
    authority: &JobAuthority,
    marker: &AcceptanceMarker,
    judgment: &SemanticJudgment,
    scope: crate::git::WorkBoundaryScope,
) -> Result<CompletionReceipt> {
    let receipt_path = paths.job_receipt(authority.job_id.as_ref());
    let result = crate::git::with_git_command_scope(scope, || {
        seal_completion_receipt(paths, state, authority, marker, judgment)
    });
    if matches!(result, Err(DeadreckonError::ProcessBoundary { .. })) {
        match fs::remove_file(&receipt_path) {
            Ok(()) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(DeadreckonError::Io {
                    path: receipt_path,
                    source,
                });
            }
        }
    }
    result
}

/// One recorded observation from a completion-receipt audit: a named check,
/// whether it passed, and either a short pass detail or the strict path's
/// exact error message.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReceiptFact {
    pub name: String,
    pub pass: bool,
    pub detail: String,
}

/// The fact-collecting form of receipt validation. Where the strict path stops
/// at the first mismatch, the audit records every independently checkable fact
/// so an operator can see WHICH digest broke, then reproduces the strict
/// fail-first result exactly through [`ReceiptAudit::into_result`]. The audit
/// performs the same kinds of reads as the strict validator and nothing more;
/// it never writes, signs, or promotes.
#[derive(Debug)]
pub struct ReceiptAudit {
    /// Every audited fact, in the order the strict validator checks them.
    /// Facts that depend on an earlier failed precondition (an unreadable
    /// receipt or authority document, an unauthenticated sandbox observation)
    /// are omitted rather than guessed at.
    pub facts: Vec<ReceiptFact>,
    receipt: Option<CompletionReceipt>,
    first_error: Option<DeadreckonError>,
    run_id: String,
}

impl ReceiptAudit {
    /// True when every audited fact passed.
    pub fn passed(&self) -> bool {
        self.first_error.is_none()
    }

    /// Collapse the audit to the strict fail-first result: the FIRST failing
    /// fact's error, byte-identical to the historical monolithic validator.
    pub fn into_result(self) -> Result<CompletionReceipt> {
        match (self.first_error, self.receipt) {
            (Some(error), _) => Err(error),
            (None, Some(receipt)) => Ok(receipt),
            (None, None) => Err(completion_error(
                &self.run_id,
                "receipt audit finished without a receipt",
            )),
        }
    }
}

struct ReceiptAuditBuilder {
    facts: Vec<ReceiptFact>,
    first_error: Option<DeadreckonError>,
    /// When set, no further checks execute once the first failure is
    /// recorded — the strict validator's historical short-circuit, so failure
    /// paths never touch gate-key material or recompute result trees beyond
    /// the first mismatch.
    fail_fast: bool,
}

impl ReceiptAuditBuilder {
    fn new(fail_fast: bool) -> Self {
        Self {
            facts: Vec::new(),
            first_error: None,
            fail_fast,
        }
    }

    /// Record one fact: on `Ok` the value flows onward with `pass_detail`; on
    /// `Err` the fact fails with the error's message and the FIRST failure is
    /// retained verbatim for [`ReceiptAudit::into_result`]. In fail-fast mode
    /// the check closure is never even invoked after the first failure.
    fn record<T>(
        &mut self,
        name: &str,
        pass_detail: impl Into<String>,
        check: impl FnOnce() -> Result<T>,
    ) -> Option<T> {
        if self.fail_fast && self.first_error.is_some() {
            return None;
        }
        match check() {
            Ok(value) => {
                self.facts.push(ReceiptFact {
                    name: name.to_string(),
                    pass: true,
                    detail: pass_detail.into(),
                });
                Some(value)
            }
            Err(error) => {
                self.facts.push(ReceiptFact {
                    name: name.to_string(),
                    pass: false,
                    detail: error.to_string(),
                });
                if self.first_error.is_none() {
                    self.first_error = Some(error);
                }
                None
            }
        }
    }

    fn finish(self, run_id: &str, receipt: Option<CompletionReceipt>) -> ReceiptAudit {
        ReceiptAudit {
            facts: self.facts,
            receipt,
            first_error: self.first_error,
            run_id: run_id.to_string(),
        }
    }
}

/// Audit a completion receipt fact by fact, in exactly the strict validator's
/// order, without stopping at the first mismatch. Read-only inspection: it
/// reads only what the strict validator would read on a fully valid receipt
/// (including the same gate-key access for signature verification) and never
/// signs, promotes, or mutates run state.
pub fn audit_completion_receipt(paths: &DeadreckonPaths, state: &PipelineState) -> ReceiptAudit {
    audit_completion_receipt_inner(paths, state, false)
}

fn audit_completion_receipt_inner(
    paths: &DeadreckonPaths,
    state: &PipelineState,
    fail_fast: bool,
) -> ReceiptAudit {
    let mut audit = ReceiptAuditBuilder::new(fail_fast);
    let receipt_path = paths.job_receipt(&state.run_id);
    let receipt = audit.record("receipt_document", "receipt parsed", || {
        fs::read(&receipt_path)
            .with_path(&receipt_path)
            .and_then(|raw| {
                serde_json::from_slice::<CompletionReceipt>(&raw).with_json_path(&receipt_path)
            })
    });
    let Some(receipt) = receipt else {
        return audit.finish(&state.run_id, None);
    };
    audit.record("receipt_identity", "receipt names this job and run", || {
        if receipt.job_id.as_ref() != state.run_id || receipt.run_id.as_ref() != state.run_id {
            Err(completion_error(
                &state.run_id,
                "receipt identity does not match the requested run",
            ))
        } else {
            Ok(())
        }
    });
    audit.record(
        "receipt_provenance",
        "supervisor-issued two-key verified result",
        || {
            if receipt.outcome != JobOutcome::Verified
                || receipt.stop_reason != StopReason::Verified
                || receipt.proof_kind != CompletionProofKind::TwoKeyCompletion
                || receipt.issuer != CompletionReceiptIssuer::DeadreckonSupervisor
            {
                Err(completion_error(
                    &state.run_id,
                    "receipt is not a supervisor-issued two-key verified result",
                ))
            } else {
                Ok(())
            }
        },
    );

    let authority_path = paths.job_authority(&state.run_id);
    let launch_path = paths.job_launch_plan(&state.run_id);
    let authority_and_job = audit.record(
        "authority_document",
        "job authority and job record parsed",
        || {
            fs::read(&authority_path)
                .with_path(&authority_path)
                .and_then(|raw| {
                    serde_json::from_slice::<JobAuthority>(&raw).with_json_path(&authority_path)
                })
                .and_then(|authority| Ok((authority, load_job(paths, &state.run_id)?)))
        },
    );
    let Some((authority, job)) = authority_and_job else {
        return audit.finish(&state.run_id, None);
    };
    audit.record(
        "authority_inputs",
        "approved goal, launch plan, policy, and contract digests match",
        || verify_authority_inputs(&job, &authority, &authority_path, &launch_path, state),
    );
    audit.record(
        "result_boundary",
        "worktree result boundary is intact",
        || validate_worktree_result_boundary(state, &authority, Some(&receipt)).map(|_| ()),
    );
    let sandbox_observation = audit.record(
        "sandbox_observation",
        "sandbox boundary observation authenticated",
        || {
            validate_sandbox_boundary_observation(
                paths,
                state,
                &authority,
                &receipt.sandbox_backend,
            )
        },
    );
    if let Some(sandbox_observation) = &sandbox_observation {
        audit.record(
            "attempt_identity",
            format!(
                "attempt {} launch {}",
                receipt.attempt, receipt.outer_launch_id
            ),
            || {
                if receipt.attempt == 0
                    || receipt.attempt != sandbox_observation.attempt
                    || receipt.outer_launch_id != sandbox_observation.outer_launch_id
                {
                    Err(completion_error(
                        &state.run_id,
                        "receipt attempt identity does not match its authenticated sandbox observation",
                    ))
                } else {
                    Ok(())
                }
            },
        );
    }
    audit.record(
        "sandbox_observation_digest",
        receipt.sandbox_boundary_observation_sha256.clone(),
        || {
            sandbox_boundary_observation_sha256(paths, &state.run_id).and_then(|actual| {
                require_digest(
                    &receipt.sandbox_boundary_observation_sha256,
                    &actual,
                    "sandbox boundary observation",
                    &state.run_id,
                )
            })
        },
    );
    audit.record(
        "execution_evidence",
        "execution ledgers match the sealed receipt",
        || {
            completion_execution_evidence(paths, &state.run_id).and_then(|actual| {
                if receipt.execution_evidence != actual {
                    Err(completion_error(
                        &state.run_id,
                        "receipt execution ledgers changed after verified completion",
                    ))
                } else {
                    Ok(())
                }
            })
        },
    );
    audit.record("authority_digest", receipt.authority_sha256.clone(), || {
        sha256_file(&authority_path).and_then(|actual| {
            require_digest(
                &receipt.authority_sha256,
                &actual,
                "authority",
                &state.run_id,
            )
        })
    });
    audit.record(
        "launch_plan_digest",
        receipt.launch_plan_sha256.clone(),
        || {
            sha256_file(&launch_path).and_then(|actual| {
                require_digest(
                    &receipt.launch_plan_sha256,
                    &actual,
                    "launch plan",
                    &state.run_id,
                )
            })
        },
    );
    audit.record(
        "deterministic_marker_digest",
        receipt.deterministic_marker_sha256.clone(),
        || {
            sha256_file(&crate::marker_path_for_run_root(&state.run_root)).and_then(|actual| {
                require_digest(
                    &receipt.deterministic_marker_sha256,
                    &actual,
                    "deterministic marker",
                    &state.run_id,
                )
            })
        },
    );
    let semantic_path = state.run_root.join(SEMANTIC_JUDGMENT_JSON);
    audit.record(
        "semantic_judgment_digest",
        receipt.semantic_judgment_sha256.clone(),
        || {
            sha256_file(&semantic_path).and_then(|actual| {
                require_digest(
                    &receipt.semantic_judgment_sha256,
                    &actual,
                    "semantic judgment",
                    &state.run_id,
                )
            })
        },
    );
    audit.record(
        "judgment_achieved",
        "semantic judgment records achieved with evidence-backed coverage",
        || {
            fs::read(&semantic_path)
                .with_path(&semantic_path)
                .and_then(|raw| {
                    serde_json::from_slice::<SemanticJudgment>(&raw).with_json_path(&semantic_path)
                })
                .and_then(|judgment| {
                    if judgment.decision != SemanticDecision::Achieved
                        || judgment.job_id != receipt.job_id
                        || judgment.run_id != receipt.run_id
                    {
                        return Err(completion_error(
                            &state.run_id,
                            "semantic judgment no longer records achieved for this job",
                        ));
                    }
                    validate_achieved_judgment(&judgment, &state.run_id)
                })
        },
    );
    audit.record(
        "marker_signature",
        "signed acceptance marker validates for this run",
        || {
            validate_parent_repair_lineage_if_present(paths, state).and_then(|parent_repair| {
                let marker = validate_acceptance_marker_with_parent_repair_bytes(
                    state,
                    parent_repair
                        .as_ref()
                        .map(|repair| repair.manifest.bytes.as_slice()),
                    parent_repair
                        .as_ref()
                        .map(|repair| repair.candidate.bytes.as_slice()),
                )?;
                if !marker.is_native_gate_proof()
                    || marker.contained != receipt.contained
                    || marker.sandbox_backend != receipt.sandbox_backend
                {
                    return Err(completion_error(
                        &state.run_id,
                        "deterministic proof or containment does not match the receipt",
                    ));
                }
                Ok(())
            })
        },
    );
    audit.record(
        "result_projection_digest",
        receipt
            .result_projection_sha256
            .clone()
            .unwrap_or_else(|| "historical result projection".to_string()),
        || {
            let projection_required = crate::result_projection_required(paths, &state.run_id)?;
            if projection_required && !crate::result_projection_exists(state) {
                return Err(completion_error(
                    &state.run_id,
                    "new strict result is missing its required controller-sealed projection",
                ));
            }
            if crate::result_projection_exists(state) {
                let expected = receipt.result_projection_sha256.as_deref().ok_or_else(|| {
                    completion_error(
                        &state.run_id,
                        "new strict result receipt is missing its projection digest",
                    )
                })?;
                crate::result_projection_sha256(state).and_then(|actual| {
                    require_digest(expected, &actual, "result projection", &state.run_id)?;
                    crate::validate_result_projection_at(
                        state,
                        &crate::result_projection_candidate_path(state),
                    )?;
                    Ok(())
                })
            } else if receipt.result_projection_sha256.is_some() {
                Err(completion_error(
                    &state.run_id,
                    "receipt names a result projection that is no longer present",
                ))
            } else {
                Ok(())
            }
        },
    );
    audit.record(
        "result_tree_digest",
        receipt.result_tree_sha256.clone(),
        || {
            result_tree_hash(state).and_then(|actual| {
                require_digest(
                    &receipt.result_tree_sha256,
                    &actual,
                    "result tree",
                    &state.run_id,
                )
            })
        },
    );
    audit.record(
        "receipt_signature",
        "receipt HMAC signature verifies under the gate key",
        || {
            read_gate_key(paths, &state.run_id)
                .and_then(|key| verify_receipt_signature(&receipt, &key))
        },
    );
    audit.finish(&state.run_id, Some(receipt))
}

/// Strict fail-first receipt validation: stops at the FIRST failing fact,
/// exactly like the historical monolithic validator — a failure at an early
/// fact performs none of the later checks (no gate-key reads, no result-tree
/// hashing). The fact-collecting [`audit_completion_receipt`] is the explicit
/// inspection surface that keeps checking past failures.
pub fn validate_completion_receipt(
    paths: &DeadreckonPaths,
    state: &PipelineState,
) -> Result<CompletionReceipt> {
    audit_completion_receipt_inner(paths, state, true).into_result()
}

/// Validate a receipt under the same inherited Git boundary used to seal it.
pub fn validate_completion_receipt_bounded(
    paths: &DeadreckonPaths,
    state: &PipelineState,
    scope: crate::git::WorkBoundaryScope,
) -> Result<CompletionReceipt> {
    crate::git::with_git_command_scope(scope, || validate_completion_receipt(paths, state))
}

fn validate_parent_repair_lineage_if_present(
    paths: &DeadreckonPaths,
    state: &PipelineState,
) -> Result<Option<ValidatedParentRepair>> {
    let intent_path = paths.job_dir(&state.run_id).join(PARENT_REPAIR_INTENT_JSON);
    let manifest_path = crate::parent_repair_manifest_path_for_run_root(&state.run_root);
    let candidate_path = crate::parent_repair_candidate_path_for_run_root(&state.run_root);
    let intent_snapshot = read_optional_trusted_file_snapshot(
        &intent_path,
        "active parent repair intent",
        &state.run_id,
    )?;
    let manifest_snapshot = read_optional_trusted_file_snapshot(
        &manifest_path,
        "active parent repair manifest",
        &state.run_id,
    )?;
    let candidate_snapshot = read_optional_trusted_file_snapshot(
        &candidate_path,
        "active parent repair candidate",
        &state.run_id,
    )?;
    let (intent_snapshot, manifest_snapshot, candidate_snapshot) = match (
        intent_snapshot,
        manifest_snapshot,
        candidate_snapshot,
    ) {
        (None, None, None) => return Ok(None),
        (Some(intent), Some(manifest), Some(candidate)) => (intent, manifest, candidate),
        _ => {
            return Err(completion_error(
                &state.run_id,
                "parent repair authority is incomplete; intent, manifest and candidate must remain together",
            ));
        }
    };

    let job = load_job(paths, &state.run_id)?;
    let history = read_job_history(&paths.job_events(&state.run_id))?;
    let projection = reduce_job_history(&job.job_id, &history)?;
    let active: ParentRepairIntentBinding =
        parse_parent_repair_snapshot(&intent_snapshot, &intent_path)?;
    let manifest: ParentRepairManifestBinding =
        parse_parent_repair_snapshot(&manifest_snapshot, &manifest_path)?;
    let candidate: ParentRepairCandidateBinding =
        parse_parent_repair_snapshot(&candidate_snapshot, &candidate_path)?;
    let round_dir = parent_repair_round_dir(state, active.round);
    let marker_path = round_dir.join("pre-repair-marker.json");
    let judgment_path = round_dir.join("revise-judgment.json");
    let marker_snapshot = read_required_trusted_file_snapshot(
        &marker_path,
        "archived parent repair marker",
        &state.run_id,
    )?;
    let judgment_snapshot = read_required_trusted_file_snapshot(
        &judgment_path,
        "archived parent repair judgment",
        &state.run_id,
    )?;

    validate_parent_repair_intent(
        state,
        &job,
        &history,
        &active,
        &intent_snapshot,
        &marker_snapshot,
        &marker_path,
        &judgment_snapshot,
        &judgment_path,
    )?;
    validate_parent_repair_attempt(
        state,
        &job,
        &history,
        &active,
        &intent_snapshot,
        &manifest,
        &manifest_snapshot,
        &candidate,
        &candidate_snapshot,
        Some(&parent_repair_result_tree_hash(state)?),
    )?;
    if manifest.attempt != projection.attempt_count {
        return Err(completion_error(
            &state.run_id,
            "active parent repair candidate is not bound to the current Job attempt",
        ));
    }

    let merged = crate::state::load_run(paths, &active.merged_run_id).map_err(|_| {
        completion_error(
            &state.run_id,
            "parent repair no longer has its immutable merged result",
        )
    })?;
    let merged_tree_sha256 = parent_repair_result_tree_hash(&merged)?;
    if active.merged_tree_sha256 != merged_tree_sha256 {
        return Err(completion_error(
            &state.run_id,
            "parent repair intent no longer matches the immutable merged result tree",
        ));
    }

    let mut newer = active;
    let mut current_round = newer.round;
    while current_round > 1 {
        let previous_round = current_round - 1;
        let archive = read_parent_repair_round_snapshot(state, previous_round)?;
        let expected = parent_repair_round_chain_sha256(&archive);
        if newer.previous_round_sha256.as_deref() != Some(expected.as_str()) {
            return Err(completion_error(
                &state.run_id,
                "parent repair archive no longer matches the signed round chain",
            ));
        }
        let archive_dir = parent_repair_round_dir(state, previous_round);
        let archived_intent_path = archive_dir.join("intent.json");
        let archived_manifest_path = archive_dir.join("final-attempt.json");
        let archived_candidate_path = archive_dir.join("candidate.json");
        let archived_marker_path = archive_dir.join("pre-repair-marker.json");
        let archived_judgment_path = archive_dir.join("revise-judgment.json");
        let previous: ParentRepairIntentBinding =
            parse_parent_repair_snapshot(&archive.intent, &archived_intent_path)?;
        let previous_manifest: ParentRepairManifestBinding =
            parse_parent_repair_snapshot(&archive.manifest, &archived_manifest_path)?;
        let previous_candidate: ParentRepairCandidateBinding =
            parse_parent_repair_snapshot(&archive.candidate, &archived_candidate_path)?;
        validate_parent_repair_intent(
            state,
            &job,
            &history,
            &previous,
            &archive.intent,
            &archive.marker,
            &archived_marker_path,
            &archive.judgment,
            &archived_judgment_path,
        )?;
        validate_parent_repair_attempt(
            state,
            &job,
            &history,
            &previous,
            &archive.intent,
            &previous_manifest,
            &archive.manifest,
            &previous_candidate,
            &archive.candidate,
            None,
        )?;
        if previous.round != previous_round
            || previous.merged_run_id != newer.merged_run_id
            || previous.merged_tree_sha256 != newer.merged_tree_sha256
            || newer.requested_after_attempt != previous_candidate.attempt
            || newer.requested_after_launch_id != previous_candidate.launch_id
            || newer.requested_after_lease_epoch != previous_candidate.lease_epoch
            || newer.pre_repair_tree_sha256 != previous_candidate.result_tree_sha256
        {
            return Err(completion_error(
                &state.run_id,
                "adjacent parent repair rounds do not preserve fenced candidate lineage",
            ));
        }
        newer = previous;
        current_round = previous_round;
    }
    if newer.previous_round_sha256.is_some()
        || newer.pre_repair_tree_sha256 != newer.merged_tree_sha256
    {
        return Err(completion_error(
            &state.run_id,
            "first parent repair round does not start from the immutable merged result",
        ));
    }

    Ok(Some(ValidatedParentRepair {
        manifest: manifest_snapshot,
        candidate: candidate_snapshot,
    }))
}

fn parent_repair_round_dir(state: &PipelineState, round: u32) -> PathBuf {
    state
        .run_root
        .join("proofs")
        .join(PARENT_REPAIR_ARCHIVE_DIR)
        .join(format!("round-{round}"))
}

fn parent_repair_round_chain_sha256(archive: &ParentRepairRoundSnapshot) -> String {
    let mut bound = String::new();
    for (name, digest) in PARENT_REPAIR_ROUND_FILES.into_iter().zip([
        archive.intent.sha256.as_str(),
        archive.manifest.sha256.as_str(),
        archive.candidate.sha256.as_str(),
        archive.marker.sha256.as_str(),
        archive.judgment.sha256.as_str(),
    ]) {
        bound.push_str(name);
        bound.push('=');
        bound.push_str(digest);
        bound.push('\n');
    }
    sha256_text(&bound)
}

#[allow(clippy::too_many_arguments)]
fn validate_parent_repair_intent(
    state: &PipelineState,
    job: &Job,
    history: &JobHistory,
    intent: &ParentRepairIntentBinding,
    intent_snapshot: &TrustedFileSnapshot,
    marker_snapshot: &TrustedFileSnapshot,
    marker_path: &Path,
    judgment_snapshot: &TrustedFileSnapshot,
    judgment_path: &Path,
) -> Result<()> {
    if intent.schema_version != 1
        || intent.job_id != state.run_id
        || intent.shape != job.shape
        || !matches!(intent.shape, JobShape::Graph | JobShape::LegacyCampaign)
        || intent.round == 0
        || intent.round >= job.policy.max_attempts.max(1)
        || intent.merged_run_id.is_empty()
        || !is_sha256_digest(&intent.merged_tree_sha256)
        || !is_sha256_digest(&intent.pre_repair_tree_sha256)
        || !is_sha256_digest(&intent.revise_marker_sha256)
        || !is_sha256_digest(&intent.revise_judgment_sha256)
        || !is_sha256_digest(&intent.revise_input_sha256)
        || intent.requested_after_attempt == 0
        || intent.requested_after_attempt >= job.policy.max_attempts.max(1)
        || intent.requested_after_lease_epoch == 0
        || Uuid::parse_str(&intent.requested_after_launch_id).is_err()
    {
        return Err(completion_error(
            &state.run_id,
            "parent repair intent is malformed or crosses approved Job authority",
        ));
    }
    require_digest(
        &intent.revise_marker_sha256,
        &marker_snapshot.sha256,
        "archived parent repair marker",
        &state.run_id,
    )?;
    require_digest(
        &intent.revise_judgment_sha256,
        &judgment_snapshot.sha256,
        "archived parent repair judgment",
        &state.run_id,
    )?;
    let marker: AcceptanceMarker = parse_parent_repair_snapshot(marker_snapshot, marker_path)?;
    let judgment: SemanticJudgment =
        parse_parent_repair_snapshot(judgment_snapshot, judgment_path)?;
    if marker.run_id != state.run_id
        || marker.status != "pass"
        || !marker.is_native_gate_proof()
        || !marker.contained
        || marker.sandbox_backend == "none"
        || judgment.job_id.as_ref() != state.run_id
        || judgment.run_id.as_ref() != state.run_id
        || judgment.decision != SemanticDecision::Revise
        || judgment.input_sha256 != intent.revise_input_sha256
        || parent_repair_feedback(&judgment) != intent.feedback
    {
        return Err(completion_error(
            &state.run_id,
            "parent repair intent is not backed by an archived same-Job revise decision",
        ));
    }

    let requested_launch = history.events().iter().any(|event| {
        event.kind == JobEventKind::ChildLinked
            && event.lease_epoch == intent.requested_after_lease_epoch
            && event_detail_u32(event, "attempt") == Some(intent.requested_after_attempt)
            && event_detail_str(event, "launch_id")
                == Some(intent.requested_after_launch_id.as_str())
    });
    let revise_event = history.events().iter().any(|event| {
        event.kind == JobEventKind::SemanticJudgeRevise
            && event.lease_epoch == intent.requested_after_lease_epoch
            && event_detail_u32(event, "round") == Some(intent.round)
            && event_detail_str(event, "intent_sha256") == Some(intent_snapshot.sha256.as_str())
            && event_detail_str(event, "judgment_sha256")
                == Some(intent.revise_judgment_sha256.as_str())
            && event_detail_str(event, "merged_run_id") == Some(intent.merged_run_id.as_str())
    });
    if !requested_launch || !revise_event {
        return Err(completion_error(
            &state.run_id,
            "parent repair intent is not backed by its fenced launch and revise event",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_parent_repair_attempt(
    state: &PipelineState,
    job: &Job,
    history: &JobHistory,
    intent: &ParentRepairIntentBinding,
    intent_snapshot: &TrustedFileSnapshot,
    manifest: &ParentRepairManifestBinding,
    manifest_snapshot: &TrustedFileSnapshot,
    candidate: &ParentRepairCandidateBinding,
    candidate_snapshot: &TrustedFileSnapshot,
    expected_result_tree: Option<&str>,
) -> Result<()> {
    if manifest.schema_version != 1
        || candidate.schema_version != 1
        || manifest.job_id != state.run_id
        || candidate.job_id != state.run_id
        || candidate.run_id != state.run_id
        || manifest.shape != job.shape
        || manifest.shape != intent.shape
        || manifest.round != intent.round
        || candidate.round != intent.round
        || manifest.merged_run_id != intent.merged_run_id
        || manifest.merged_tree_sha256 != intent.merged_tree_sha256
        || manifest.pre_repair_tree_sha256 != intent.pre_repair_tree_sha256
        || manifest.intent_sha256 != intent_snapshot.sha256
        || candidate.intent_sha256 != intent_snapshot.sha256
        || candidate.manifest_sha256 != manifest_snapshot.sha256
        || candidate.attempt != manifest.attempt
        || candidate.launch_id != manifest.launch_id
        || candidate.lease_epoch != manifest.lease_epoch
        || manifest.attempt <= intent.requested_after_attempt
        || manifest.attempt <= intent.round
        || manifest.attempt > job.policy.max_attempts.max(1)
        || manifest.lease_epoch < intent.requested_after_lease_epoch
        || manifest.lease_epoch == 0
        || Uuid::parse_str(&manifest.launch_id).is_err()
        || !is_sha256_digest(&manifest.attempt_baseline_tree_sha256)
        || !is_sha256_digest(&candidate.result_tree_sha256)
        || candidate_snapshot.bytes.is_empty()
        || expected_result_tree
            .is_some_and(|expected| candidate.result_tree_sha256.as_str() != expected)
    {
        return Err(completion_error(
            &state.run_id,
            "parent repair candidate does not match its fenced attempt and result authority",
        ));
    }
    let attempt_started = history.events().iter().any(|event| {
        event.kind == JobEventKind::AttemptStarted
            && event.lease_epoch == manifest.lease_epoch
            && event_detail_u32(event, "attempt") == Some(manifest.attempt)
    });
    let child_linked = history.events().iter().any(|event| {
        event.kind == JobEventKind::ChildLinked
            && event.lease_epoch == manifest.lease_epoch
            && event_detail_u32(event, "attempt") == Some(manifest.attempt)
            && event_detail_str(event, "launch_id") == Some(manifest.launch_id.as_str())
    });
    if !attempt_started || !child_linked {
        return Err(completion_error(
            &state.run_id,
            "parent repair candidate is not backed by its fenced Job attempt and launch",
        ));
    }
    Ok(())
}

fn parent_repair_feedback(judgment: &SemanticJudgment) -> String {
    let missing = if judgment.missing.is_empty() {
        "no explicit missing clauses supplied".to_string()
    } else {
        judgment.missing.join("; ")
    };
    format!(
        "independent semantic judge requested parent revision: {}. Missing: {missing}",
        judgment.summary
    )
}

fn event_detail_u32(event: &JobEvent, key: &str) -> Option<u32> {
    event
        .detail
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn event_detail_str<'a>(event: &'a JobEvent, key: &str) -> Option<&'a str> {
    event.detail.get(key).and_then(serde_json::Value::as_str)
}

fn is_sha256_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn parent_repair_result_tree_hash(state: &PipelineState) -> Result<String> {
    let mut index = build_deliverable_file_index(&state.working_dir)?;
    index.files.remove(Path::new("manifest.json"));
    Ok(index.tree_hash())
}

fn read_parent_repair_round_snapshot(
    state: &PipelineState,
    round: u32,
) -> Result<ParentRepairRoundSnapshot> {
    let archive = parent_repair_round_dir(state, round);
    Ok(ParentRepairRoundSnapshot {
        intent: read_required_trusted_file_snapshot(
            &archive.join("intent.json"),
            "archived parent repair intent",
            &state.run_id,
        )?,
        manifest: read_required_trusted_file_snapshot(
            &archive.join("final-attempt.json"),
            "archived parent repair manifest",
            &state.run_id,
        )?,
        candidate: read_required_trusted_file_snapshot(
            &archive.join("candidate.json"),
            "archived parent repair candidate",
            &state.run_id,
        )?,
        marker: read_required_trusted_file_snapshot(
            &archive.join("pre-repair-marker.json"),
            "archived parent repair marker",
            &state.run_id,
        )?,
        judgment: read_required_trusted_file_snapshot(
            &archive.join("revise-judgment.json"),
            "archived parent repair judgment",
            &state.run_id,
        )?,
    })
}

fn parse_parent_repair_snapshot<T: serde::de::DeserializeOwned>(
    snapshot: &TrustedFileSnapshot,
    path: &Path,
) -> Result<T> {
    serde_json::from_slice(&snapshot.bytes).with_json_path(path)
}

fn read_required_trusted_file_snapshot(
    path: &Path,
    label: &str,
    run_id: &str,
) -> Result<TrustedFileSnapshot> {
    read_optional_trusted_file_snapshot(path, label, run_id)?.ok_or_else(|| {
        completion_error(run_id, &format!("{label} is missing at {}", path.display()))
    })
}

fn read_optional_trusted_file_snapshot(
    path: &Path,
    label: &str,
    run_id: &str,
) -> Result<Option<TrustedFileSnapshot>> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if before.file_type().is_symlink() || !before.file_type().is_file() {
        return Err(completion_error(
            run_id,
            &format!(
                "{label} at {} must be a regular non-symlink file",
                path.display()
            ),
        ));
    }

    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).map_err(|source| DeadreckonError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let opened = file.metadata().with_path(path)?;
    if !trusted_file_metadata_matches(&before, &opened) {
        return Err(completion_error(
            run_id,
            &format!("{label} changed identity while it was opened"),
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).with_path(path)?;
    let after = file.metadata().with_path(path)?;
    let post_path = fs::symlink_metadata(path).with_path(path)?;
    if !trusted_file_metadata_matches(&opened, &after)
        || !trusted_file_metadata_matches(&after, &post_path)
        || u64::try_from(bytes.len()).ok() != Some(after.len())
    {
        return Err(completion_error(
            run_id,
            &format!("{label} changed while its trusted bytes were captured"),
        ));
    }
    let sha256 = sha256_bytes(&bytes);
    Ok(Some(TrustedFileSnapshot { bytes, sha256 }))
}

#[cfg(unix)]
fn trusted_file_metadata_matches(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.file_type().is_file()
        && right.file_type().is_file()
        && left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(not(unix))]
fn trusted_file_metadata_matches(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.file_type().is_file()
        && right.file_type().is_file()
        && left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

fn validate_achieved_judgment(judgment: &SemanticJudgment, run_id: &str) -> Result<()> {
    if judgment.goal_coverage.is_empty()
        || !judgment.missing.is_empty()
        || judgment.goal_coverage.iter().any(|coverage| {
            coverage.status != GoalCoverageStatus::Met || coverage.evidence.is_empty()
        })
    {
        return Err(completion_error(
            run_id,
            "semantic achieved judgment lacks complete evidence-backed goal coverage",
        ));
    }
    Ok(())
}

fn verify_authority_inputs(
    job: &deadreckon_protocol::Job,
    authority: &JobAuthority,
    authority_path: &Path,
    launch_path: &Path,
    state: &PipelineState,
) -> Result<()> {
    let job_id = authority.job_id.as_ref();
    require_digest(
        &authority.goal_sha256,
        &sha256_text(&job.goal),
        "approved goal",
        job_id,
    )?;
    require_digest(
        &authority.launch_plan_sha256,
        &sha256_file(launch_path)?,
        "approved launch plan",
        job_id,
    )?;
    require_digest(
        &job.launch_plan_sha256,
        &authority.launch_plan_sha256,
        "job launch authority",
        job_id,
    )?;
    require_digest(
        &job.authority_sha256,
        &sha256_file(authority_path)?,
        "job authority",
        job_id,
    )?;
    let policy_sha256 = sha256_text(&serde_json::to_string(&job.policy).map_err(|source| {
        DeadreckonError::Json {
            path: PathBuf::from("job policy"),
            source,
        }
    })?);
    require_digest(
        &authority.effective_policy_sha256,
        &policy_sha256,
        "approved effective policy",
        job_id,
    )?;
    if authority.semantic_judge_mode != job.policy.semantic_judge {
        return Err(completion_error(
            job_id,
            "semantic judge policy changed after approval",
        ));
    }
    let execution = job.policy.execution.as_ref().ok_or_else(|| {
        completion_error(
            job_id,
            "job predates immutable execution policy; strict completion is unavailable",
        )
    })?;
    if !execution.require_containment
        || execution.sandbox_requested != authority.sandbox_requested
        || !execution.tools.contains_key("bash")
        || !execution.tools.contains_key("write_file")
    {
        return Err(completion_error(
            job_id,
            "approved execution policy does not match containment authority",
        ));
    }
    let contract_path = crate::acceptance_spec_path_for_run_root(&state.run_root);
    require_digest(
        &authority.contract_sha256,
        &sha256_file(&contract_path)?,
        "approved contract",
        job_id,
    )?;
    validate_strict_contract(&contract_path, job_id)
        .map_err(|error| completion_error(job_id, &error.to_string()))
}

/// Apply the same minimum contract-strength rules at Job admission and receipt
/// sealing. A durable Job must never start with a contract that its own strict
/// completion path can only reject later.
pub fn validate_strict_contract(contract_path: &Path, job_id: &str) -> Result<()> {
    let raw = fs::read_to_string(contract_path).with_path(contract_path)?;
    let checks = acceptance_checks_from_yaml(&raw)?;
    if checks.is_empty() {
        return Err(strict_contract_error(job_id, "contains no checks"));
    }
    let required = checks
        .iter()
        .filter(|check| check_is_required(check))
        .collect::<Vec<_>>();
    if required.is_empty() {
        return Err(strict_contract_error(job_id, "contains no required checks"));
    }
    if required.iter().all(|check| {
        matches!(
            check,
            AcceptanceCheck::FileExists { path, .. } if path.trim() == "{working_dir}"
        )
    }) {
        return Err(strict_contract_error(
            job_id,
            "only proves that its pre-created working directory exists",
        ));
    }
    let capabilities = crate::gate::acceptance_capabilities_from_yaml(&raw)?;
    let required_network = crate::gate::required_acceptance_network_access_from_yaml(&raw)?;
    if !capabilities.network.allows(required_network) {
        return Err(strict_contract_error(
            job_id,
            &format!(
                "requires {required_network} network access but declares {}",
                capabilities.network
            ),
        ));
    }
    Ok(())
}

fn strict_contract_error(job_id: &str, detail: &str) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!(
        "approved deterministic contract for Job {job_id} is not sealable: it {detail}"
    ))
}

fn check_is_required(check: &AcceptanceCheck) -> bool {
    match check {
        AcceptanceCheck::CargoTest { must_pass, .. }
        | AcceptanceCheck::FileExists { must_pass, .. }
        | AcceptanceCheck::ContentMatch { must_pass, .. }
        | AcceptanceCheck::BuildSuccess { must_pass, .. }
        | AcceptanceCheck::Shell { must_pass, .. } => *must_pass,
    }
}

fn validate_worktree_result_boundary(
    state: &PipelineState,
    authority: &JobAuthority,
    receipt: Option<&CompletionReceipt>,
) -> Result<Option<String>> {
    let job_id = authority.job_id.as_ref();
    let record = read_trusted_codebase_record(&state.run_root).map_err(|error| {
        completion_error(
            job_id,
            &format!("result lifecycle record is unavailable or invalid: {error}"),
        )
    })?;
    if record.mode != CodebaseMode::Worktree {
        return Ok(None);
    }

    let git_root = record.source_git_root.as_deref().ok_or_else(|| {
        completion_error(
            job_id,
            "worktree result is missing its source Git repository",
        )
    })?;
    let branch = record
        .branch_name
        .as_deref()
        .ok_or_else(|| completion_error(job_id, "worktree result is missing its result branch"))?;
    let base = record
        .base_sha
        .as_deref()
        .ok_or_else(|| completion_error(job_id, "worktree result is missing its approved base"))?;
    let approved_base = authority.source_revision.as_deref().ok_or_else(|| {
        completion_error(
            job_id,
            "worktree result authority is missing its approved source revision",
        )
    })?;
    if base != approved_base {
        return Err(completion_error(
            job_id,
            "worktree result base does not match the approved source revision",
        ));
    }

    let result_revision = if let Some(revision) = git_revision(git_root, branch)? {
        revision
    } else {
        let sealed = receipt
            .and_then(|receipt| receipt.result_revision.as_deref())
            .filter(|_| state.promoted_library_dir.is_some())
            .ok_or_else(|| completion_error(job_id, "worktree result branch is unavailable"))?;
        git_revision(git_root, sealed)?.ok_or_else(|| {
            completion_error(
                job_id,
                "signed result revision is no longer available after worktree cleanup",
            )
        })?
    };

    let gitlinks = gitlink_paths(git_root, &result_revision)?;
    if !gitlinks.is_empty() {
        return Err(completion_error(
            job_id,
            &format!(
                "strict result contains unsupported Git submodule entries: {}; stop for review instead of signing an incomplete filesystem projection",
                display_paths(&gitlinks)
            ),
        ));
    }

    let non_deliverable = non_deliverable_git_history_paths(git_root, base, &result_revision)?;
    if !non_deliverable.is_empty() {
        return Err(completion_error(
            job_id,
            &format!(
                "result history contains non-deliverable paths relative to the approved base: {}",
                display_paths(&non_deliverable)
            ),
        ));
    }

    if let Some(worktree) = record.worktree_path.as_deref().filter(|path| path.is_dir()) {
        let dirty_deliverables = uncommitted_deliverable_paths(worktree)?;
        if !dirty_deliverables.is_empty() {
            return Err(completion_error(
                job_id,
                &format!(
                    "worktree has uncommitted deliverable changes: {}",
                    display_paths(&dirty_deliverables)
                ),
            ));
        }
        let worktree_revision = git_revision(worktree, "HEAD")?.ok_or_else(|| {
            completion_error(job_id, "worktree result has no committed HEAD revision")
        })?;
        if worktree_revision != result_revision {
            return Err(completion_error(
                job_id,
                "worktree HEAD does not match its result branch",
            ));
        }
    } else if state.promoted_library_dir.is_none() {
        return Err(completion_error(
            job_id,
            "worktree result cannot be sealed or validated without its isolated checkout",
        ));
    }

    let artifact_root = record
        .worktree_path
        .as_deref()
        .filter(|path| path.is_dir())
        .unwrap_or(&state.working_dir);
    require_filesystem_matches_git_result(
        state,
        git_root,
        approved_base,
        &result_revision,
        artifact_root,
    )?;

    if let Some(receipt) = receipt {
        if receipt.result_revision.as_deref() != Some(result_revision.as_str()) {
            return Err(completion_error(
                job_id,
                "result branch revision does not match the sealed receipt",
            ));
        }
    } else if current_git_revision(&state.working_dir)?.as_deref() != Some(result_revision.as_str())
    {
        return Err(completion_error(
            job_id,
            "the filesystem being sealed is not the committed result branch",
        ));
    }
    Ok(Some(result_revision))
}

/// Return non-deliverable paths touched anywhere in `base..result`.
///
/// Finish uses this independently of receipt issuance so legacy result
/// branches cannot smuggle provider-private, lifecycle, or runtime blobs
/// through commit history, even when a later commit deletes the path and leaves
/// the final tree clean.
pub fn non_deliverable_git_history_paths(
    git_root: &Path,
    base: &str,
    result: &str,
) -> Result<Vec<PathBuf>> {
    Ok(git_history_paths(git_root, base, result)?
        .into_iter()
        .filter(|path| classify_workspace_path(path) != WorkspacePathClass::Deliverable)
        .collect())
}

/// Return every path changed by the delivered first-parent history.
///
/// A merge commit is compared only with its first parent: paths already
/// present on the operator's target branch are not delivery side effects.
/// Walking every first-parent commit still exposes an unexpected path that a
/// later delivery commit deletes before the final tree is inspected.
pub fn git_delivery_history_paths(
    git_root: &Path,
    before: &str,
    delivered: &str,
) -> Result<Vec<PathBuf>> {
    require_git_ancestor(git_root, before, delivered)?;
    let range = format!("{before}..{delivered}");
    let output = crate::git::run_git(
        git_root,
        &["rev-list", "--first-parent", "--reverse", &range],
    )?;
    require_git_success(git_root, &output, "enumerate delivery commits")?;
    let mut paths = BTreeSet::new();
    for revision in String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let parent = format!("{revision}^1");
        let output = crate::git::run_git(
            git_root,
            &[
                "diff",
                "--name-only",
                "--no-renames",
                "-z",
                &parent,
                revision,
                "--",
            ],
        )?;
        require_git_success(git_root, &output, "inspect delivery commit paths")?;
        paths.extend(nul_paths(&output.stdout)?);
    }
    Ok(paths.into_iter().collect())
}

/// Return deliverable paths changed by `result` relative to `base`.
///
/// Rename detection is disabled deliberately: both the removed and added path
/// must be proven at delivery time.
pub fn deliverable_git_delta_paths(
    git_root: &Path,
    base: &str,
    result: &str,
) -> Result<Vec<PathBuf>> {
    require_git_ancestor(git_root, base, result)?;
    Ok(git_delta_paths(git_root, base, result)?
        .into_iter()
        .filter(|path| classify_workspace_path(path) == WorkspacePathClass::Deliverable)
        .collect())
}

fn require_git_ancestor(git_root: &Path, base: &str, result: &str) -> Result<()> {
    let output = crate::git::run_git(git_root, &["merge-base", "--is-ancestor", base, result])?;
    if output.status.success() {
        Ok(())
    } else {
        Err(DeadreckonError::InvalidInput(format!(
            "approved base {base} is not an ancestor of result {result}"
        )))
    }
}

fn git_revisions(git_root: &Path, base: &str, result: &str) -> Result<Vec<String>> {
    let range = format!("{base}..{result}");
    let output = crate::git::run_git(git_root, &["rev-list", "--reverse", &range])?;
    require_git_success(git_root, &output, "enumerate result commits")?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn git_history_paths(git_root: &Path, base: &str, result: &str) -> Result<Vec<PathBuf>> {
    require_git_ancestor(git_root, base, result)?;
    let revisions = git_revisions(git_root, base, result)?;
    let mut paths = BTreeSet::new();
    for revision in revisions {
        let output = crate::git::run_git(
            git_root,
            &[
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-only",
                "--no-renames",
                "-r",
                "-m",
                "-z",
                &revision,
                "--",
            ],
        )?;
        require_git_success(git_root, &output, "inspect result commit paths")?;
        paths.extend(nul_paths(&output.stdout)?);
    }
    Ok(paths.into_iter().collect())
}

fn git_delta_paths(git_root: &Path, base: &str, result: &str) -> Result<Vec<PathBuf>> {
    let range = format!("{base}..{result}");
    let output = crate::git::run_git(
        git_root,
        &["diff", "--name-only", "--no-renames", "-z", &range, "--"],
    )?;
    require_git_success(git_root, &output, "inspect result branch paths")?;
    nul_paths(&output.stdout)
}

fn gitlink_paths(git_root: &Path, revision: &str) -> Result<Vec<PathBuf>> {
    let output = crate::git::run_git(
        git_root,
        &["ls-tree", "-r", "-z", "--full-tree", revision, "--"],
    )?;
    require_git_success(git_root, &output, "inspect result Git entry kinds")?;
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let tab = entry.iter().position(|byte| *byte == b'\t')?;
            entry[..tab]
                .starts_with(b"160000 ")
                .then_some(&entry[tab + 1..])
        })
        .map(path_from_git_bytes)
        .collect()
}

fn uncommitted_deliverable_paths(worktree: &Path) -> Result<Vec<PathBuf>> {
    let commands: [&[&str]; 4] = [
        &["diff", "--name-only", "--no-renames", "-z", "--"],
        &[
            "diff",
            "--cached",
            "--name-only",
            "--no-renames",
            "-z",
            "--",
        ],
        &["ls-files", "--others", "--exclude-standard", "-z", "--"],
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
            "--",
        ],
    ];
    let mut paths = BTreeSet::new();
    for args in commands {
        let output = crate::git::run_git(worktree, args)?;
        require_git_success(worktree, &output, "inspect uncommitted result paths")?;
        paths.extend(
            nul_paths(&output.stdout)?
                .into_iter()
                .filter(|path| classify_workspace_path(path) == WorkspacePathClass::Deliverable),
        );
    }
    paths.extend(masked_git_index_paths(worktree)?);
    Ok(paths.into_iter().collect())
}

fn masked_git_index_paths(worktree: &Path) -> Result<Vec<PathBuf>> {
    let output = crate::git::run_git(worktree, &["ls-files", "-v", "-z", "--"])?;
    require_git_success(worktree, &output, "inspect result index flags")?;
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| entry.len() > 2 && entry[1] == b' ')
        .filter(|entry| entry[0] == b'S' || entry[0].is_ascii_lowercase())
        .map(|entry| path_from_git_bytes(&entry[2..]))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| classify_workspace_path(path) == WorkspacePathClass::Deliverable)
        .collect())
}

fn require_filesystem_matches_git_result(
    state: &PipelineState,
    git_root: &Path,
    base_revision: &str,
    revision: &str,
    artifact_root: &Path,
) -> Result<()> {
    let materialized = tempfile::TempDir::new().map_err(|source| DeadreckonError::Io {
        path: std::env::temp_dir(),
        source,
    })?;
    let index_path = materialized.path().join("index");
    let tree_path = materialized.path().join("tree");
    fs::create_dir_all(&tree_path).with_path(&tree_path)?;

    let mut read_tree = crate::git::git_command(git_root, &["read-tree", revision]);
    read_tree.env("GIT_INDEX_FILE", &index_path);
    let output = crate::git::run_git_command(git_root, &mut read_tree)?;
    require_git_success(git_root, &output, "materialize signed result index")?;
    refuse_filtered_result_entries(git_root, &index_path)?;
    if base_revision != revision {
        let base_index_path = materialized.path().join("base-index");
        let mut read_base = crate::git::git_command(git_root, &["read-tree", base_revision]);
        read_base.env("GIT_INDEX_FILE", &base_index_path);
        let output = crate::git::run_git_command(git_root, &mut read_base)?;
        require_git_success(git_root, &output, "materialize approved base index")?;
        refuse_filtered_result_entries(git_root, &base_index_path)?;
    }

    let mut prefix = OsString::from("--prefix=");
    prefix.push(&tree_path);
    prefix.push(std::path::MAIN_SEPARATOR.to_string());
    let mut checkout = crate::git::git_command(git_root, &["checkout-index", "--all", "--force"]);
    checkout.env("GIT_INDEX_FILE", &index_path).arg(prefix);
    let output = crate::git::run_git_command(git_root, &mut checkout)?;
    require_git_success(git_root, &output, "materialize signed result tree")?;

    let committed = build_deliverable_file_index(&tree_path)?;
    let actual = result_deliverable_index(state, artifact_root)?;
    if committed == actual {
        return Ok(());
    }
    let paths = differing_index_paths(&committed, &actual);
    Err(completion_error(
        &state.run_id,
        &format!(
            "result filesystem does not exactly match signed Git revision {revision}: {}",
            display_paths(&paths)
        ),
    ))
}

fn refuse_filtered_result_entries(git_root: &Path, index_path: &Path) -> Result<()> {
    let mut list = crate::git::git_command(git_root, &["ls-files", "-z", "--"]);
    list.env("GIT_INDEX_FILE", index_path);
    let output = crate::git::run_git_command(git_root, &mut list)?;
    require_git_success(git_root, &output, "enumerate signed result paths")?;
    if output.stdout.is_empty() {
        return Ok(());
    }

    let mut check = crate::git::git_command(
        git_root,
        &["check-attr", "--cached", "-z", "--stdin", "filter"],
    );
    check.env("GIT_INDEX_FILE", index_path);
    let output = crate::git::run_git_command_with_input(git_root, &mut check, &output.stdout)?;
    require_git_success(git_root, &output, "inspect signed result filter attributes")?;

    let fields = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    if fields.len() % 3 != 0 {
        return Err(DeadreckonError::InvalidInput(
            "Git filter-attribute inventory returned a malformed record".to_string(),
        ));
    }
    let filtered = fields
        .chunks_exact(3)
        .filter(|record| record[2] != b"unspecified")
        .map(|record| path_from_git_bytes(record[0]))
        .collect::<Result<Vec<_>>>()?;
    if filtered.is_empty() {
        return Ok(());
    }
    Err(DeadreckonError::InvalidInput(format!(
        "strict result applies external Git filter attributes to signed paths: {}; refusing to execute mutable smudge commands during receipt verification",
        display_paths(&filtered)
    )))
}

fn differing_index_paths(
    expected: &crate::flight::ArtifactFileIndex,
    actual: &crate::flight::ArtifactFileIndex,
) -> Vec<PathBuf> {
    expected
        .files
        .keys()
        .chain(actual.files.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| expected.files.get(*path) != actual.files.get(*path))
        .cloned()
        .collect()
}

fn git_revision(git_root: &Path, revision: &str) -> Result<Option<String>> {
    let output = crate::git::run_git(git_root, &["rev-parse", "--verify", revision])?;
    if !output.status.success() {
        return Ok(None);
    }
    let revision = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!revision.is_empty()).then_some(revision))
}

fn require_git_success(
    git_root: &Path,
    output: &std::process::Output,
    operation: &str,
) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(DeadreckonError::InvalidInput(if stderr.is_empty() {
        format!("could not {operation} in {}", git_root.display())
    } else {
        format!("could not {operation} in {}: {stderr}", git_root.display())
    }))
}

fn nul_paths(raw: &[u8]) -> Result<Vec<PathBuf>> {
    raw.split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(path_from_git_bytes)
        .collect()
}

#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)] // The non-Unix implementation can reject unrepresentable paths.
fn path_from_git_bytes(raw: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    Ok(PathBuf::from(OsString::from_vec(raw.to_vec())))
}

#[cfg(not(unix))]
fn path_from_git_bytes(raw: &[u8]) -> Result<PathBuf> {
    String::from_utf8(raw.to_vec())
        .map(PathBuf::from)
        .map_err(|_| {
            DeadreckonError::InvalidInput(
                "Git returned a result path that cannot be represented on this platform"
                    .to_string(),
            )
        })
}

fn display_paths(paths: &[PathBuf]) -> String {
    let mut display = paths
        .iter()
        .take(8)
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > 8 {
        display.push_str(&format!(", and {} more", paths.len() - 8));
    }
    display
}

fn result_tree_hash(state: &PipelineState) -> Result<String> {
    Ok(result_deliverable_index(state, &state.working_dir)?.tree_hash())
}

fn result_deliverable_index(
    state: &PipelineState,
    root: &Path,
) -> Result<crate::flight::ArtifactFileIndex> {
    let mut index = if crate::result_projection_exists(state) {
        crate::result_projection_index_at(state, root)?
    } else {
        build_deliverable_file_index(root)?
    };
    // Promotion adds DeadReckon's own library manifest after the result was
    // sealed, and finish appends its delivery ledger. Both are lifecycle
    // metadata, not agent output, so the same receipt remains valid before
    // and after those operations.
    if state.promoted_library_dir.as_deref() == Some(state.working_dir.as_path()) {
        index.files.remove(Path::new("manifest.json"));
        index.files.remove(Path::new(".materialized-to"));
    }
    Ok(index)
}

fn retain_signed_result_revision(
    state: &PipelineState,
    job_id: &str,
    revision: &str,
) -> Result<()> {
    let record = read_trusted_codebase_record(&state.run_root)?;
    if record.mode != CodebaseMode::Worktree {
        return Ok(());
    }
    let git_root = record.source_git_root.as_deref().ok_or_else(|| {
        completion_error(
            job_id,
            "worktree result is missing its source Git repository",
        )
    })?;
    let ref_digest = sha256_text(job_id);
    let ref_digest = ref_digest.strip_prefix("sha256:").unwrap_or(&ref_digest);
    let retention_ref = format!("refs/deadreckon/results/{ref_digest}");
    if let Some(existing) = git_revision(git_root, &retention_ref)? {
        if existing == revision {
            return Ok(());
        }
        return Err(completion_error(
            job_id,
            "signed result retention ref already names a different revision",
        ));
    }
    let object_format = crate::git::run_git(git_root, &["rev-parse", "--show-object-format"])?;
    require_git_success(git_root, &object_format, "inspect Git object format")?;
    let zero_oid = match String::from_utf8_lossy(&object_format.stdout).trim() {
        "sha256" => "0".repeat(64),
        _ => "0".repeat(40),
    };
    let output = crate::git::run_git(
        git_root,
        &["update-ref", &retention_ref, revision, &zero_oid],
    )?;
    if output.status.success() {
        return Ok(());
    }
    require_git_success(git_root, &output, "retain signed result revision")
}

fn sign_receipt(receipt: &CompletionReceipt, key: &[u8]) -> Result<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
        completion_error(
            receipt.job_id.as_ref(),
            "HMAC-SHA-256 refused the receipt key",
        )
    })?;
    mac.update(&canonical_receipt_bytes(receipt)?);
    Ok(hex_encode(&mac.finalize().into_bytes()))
}

fn verify_receipt_signature(receipt: &CompletionReceipt, key: &[u8]) -> Result<()> {
    let signature = hex_decode(&receipt.signature).map_err(|detail| {
        completion_error(
            receipt.job_id.as_ref(),
            &format!("receipt signature is not valid hex: {detail}"),
        )
    })?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
        completion_error(
            receipt.job_id.as_ref(),
            "HMAC-SHA-256 refused the receipt key",
        )
    })?;
    mac.update(&canonical_receipt_bytes(receipt)?);
    mac.verify_slice(&signature).map_err(|_| {
        completion_error(
            receipt.job_id.as_ref(),
            "receipt signature verification failed",
        )
    })
}

fn canonical_receipt_bytes(receipt: &CompletionReceipt) -> Result<Vec<u8>> {
    let mut unsigned = receipt.clone();
    unsigned.signature.clear();
    let encoded = serde_json::to_vec(&unsigned).map_err(|source| DeadreckonError::Json {
        path: PathBuf::from("receipt.json"),
        source,
    })?;
    let mut bytes = RECEIPT_MAGIC.to_vec();
    let len = u64::try_from(encoded.len())
        .map_err(|_| completion_error(receipt.job_id.as_ref(), "receipt is too large to sign"))?;
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(&encoded);
    Ok(bytes)
}

fn require_digest(expected: &str, actual: &str, label: &str, job_id: &str) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(completion_error(
            job_id,
            &format!("{label} digest changed (expected {expected}, found {actual})"),
        ))
    }
}

fn current_git_revision(working_dir: &Path) -> Result<Option<String>> {
    let output = crate::git::run_git(working_dir, &["rev-parse", "HEAD"])?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty()))
}

fn completion_error(job_id: &str, detail: &str) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!(
        "completion receipt for {job_id} is invalid: {detail}; try: deadreckon verdict {job_id}"
    ))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn hex_decode(value: &str) -> std::result::Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("odd length".to_string());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Ok((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?))
        .collect()
}

fn hex_nibble(value: u8) -> std::result::Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err("non-hex character".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use chrono::Utc;
    use deadreckon_protocol::{
        AuthorityAcceptedBy, GateBinaryIdentity, GateEvaluatorIdentity, GoalCoverage,
        GoalCoverageStatus, Job, JobAuthority, JobEvent, JobEventKind, JobEventSequence, JobId,
        JobPolicy, JobSchemaVersion, JobShape, RunId, SandboxBoundaryObservation,
        SandboxBoundaryObservationIssuer, SemanticDecision, SemanticJudgeMode, SemanticJudgment,
    };
    use tempfile::TempDir;
    use uuid::Uuid;

    use super::{
        seal_completion_receipt, seal_completion_receipt_bounded, validate_completion_receipt,
    };
    use crate::codebase::{
        CodebaseMode, CodebaseRecord, write_codebase_record, write_trusted_codebase_record,
    };
    use crate::flight::{build_deliverable_file_index, sha256_file, sha256_text};
    use crate::gate::{
        AcceptanceCheckResult, AcceptanceContainment, read_gate_key,
        write_native_acceptance_marker_with_results_and_key,
    };
    use crate::job::{append_job_event, load_job, write_job};
    use crate::paths::DeadreckonPaths;
    use crate::state::{RunOptions, atomic_write_json, create_run};

    struct Fixture {
        _temp: TempDir,
        paths: DeadreckonPaths,
        state: crate::PipelineState,
        authority: JobAuthority,
        marker: crate::AcceptanceMarker,
        judgment: SemanticJudgment,
    }

    fn fixture_with_contract_and_launch(contract: &str, launch: &str) -> Fixture {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "ship verified change".to_string(),
                cwd: source,
                sandbox: "sandbox-exec".to_string(),
                provider: Some("judge".to_string()),
                skill_name: "deadreckon".to_string(),
                max_spend_usd: Some(2.0),
                max_wall_seconds: Some(60.0),
                run_id: Some("job-1".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        fs::create_dir_all(&state.working_dir).expect("working");
        fs::write(state.working_dir.join("result.txt"), "verified\n").expect("result");
        let contract_path = crate::acceptance_spec_path_for_run_root(&state.run_root);
        fs::write(&contract_path, contract).expect("contract");
        fs::create_dir_all(paths.job_dir("job-1")).expect("job dir");
        let launch_path = paths.job_launch_plan("job-1");
        fs::write(&launch_path, launch).expect("launch");
        let policy = JobPolicy {
            max_spend_usd: 2.0,
            max_wall_seconds: 60,
            max_attempts: 3,
            deadline: None,
            semantic_judge: SemanticJudgeMode::Required,
            execution: Some(deadreckon_protocol::JobExecutionPolicy::workspace_only(
                "sandbox-exec",
            )),
        };
        let authority = JobAuthority {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId("job-1".to_string()),
            run_id: RunId("job-1".to_string()),
            approved_at: Utc::now(),
            accepted_by: AuthorityAcceptedBy::Operator,
            goal_sha256: sha256_text("ship verified change"),
            contract_sha256: sha256_file(&contract_path).expect("contract digest"),
            effective_policy_sha256: sha256_text(
                &serde_json::to_string(&policy).expect("policy json"),
            ),
            launch_plan_sha256: sha256_file(&launch_path).expect("launch digest"),
            source_tree_sha256: build_deliverable_file_index(&state.working_dir)
                .expect("source index")
                .tree_hash(),
            source_revision: None,
            sandbox_requested: "sandbox-exec".to_string(),
            semantic_judge_mode: SemanticJudgeMode::Required,
            gate_evaluator_sha256: None,
        };
        atomic_write_json(&paths.job_authority("job-1"), &authority).expect("authority");
        let job = Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId("job-1".to_string()),
            scope: state.scope.clone(),
            goal: state.goal.clone(),
            shape: JobShape::Single,
            created_at: Utc::now(),
            source_cwd: state.cwd.clone(),
            launch_plan_sha256: authority.launch_plan_sha256.clone(),
            authority_sha256: sha256_file(&paths.job_authority("job-1")).expect("authority digest"),
            policy,
        };
        write_job(&paths, &job).expect("job");
        let key = read_gate_key(&paths, "job-1").expect("key");
        let marker = write_native_acceptance_marker_with_results_and_key(
            &state.run_root,
            "job-1".to_string(),
            state.working_dir.clone(),
            vec![AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: true,
                must_pass: true,
                detail: "result exists".to_string(),
                command: None,
                cwd: None,
                duration_ms: None,
                stdout: None,
                stderr: None,
            }],
            &key,
            AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("marker");
        let judgment = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId("job-1".to_string()),
            run_id: RunId("job-1".to_string()),
            judged_at: Utc::now(),
            provider: "judge".to_string(),
            model: "judge-model".to_string(),
            decision: SemanticDecision::Achieved,
            summary: "the result satisfies the approved goal".to_string(),
            goal_coverage: vec![GoalCoverage {
                claim: "ship verified change".to_string(),
                status: GoalCoverageStatus::Met,
                evidence: vec!["deterministic-gate".to_string()],
            }],
            missing: Vec::new(),
            input_sha256: sha256_text("evidence"),
            spend_usd: 0.01,
        };
        atomic_write_json(
            &state.run_root.join(super::SEMANTIC_JUDGMENT_JSON),
            &judgment,
        )
        .expect("judgment");
        let fixture = Fixture {
            _temp: temp,
            paths,
            state,
            authority,
            marker,
            judgment,
        };
        seal_boundary_observation(&fixture);
        fixture
    }

    fn fixture_with_contract(contract: &str) -> Fixture {
        fixture_with_contract_and_launch(
            contract,
            "{\"schema\":1,\"goal\":\"ship verified change\"}\n",
        )
    }

    fn fixture() -> Fixture {
        fixture_with_contract("name: result\nchecks:\n  - file_exists: result.txt\n")
    }

    fn projected_fixture() -> Fixture {
        let mut fixture = fixture();
        crate::activate_result_projection(&fixture.paths, &fixture.state.run_id)
            .expect("activate projection");
        crate::seal_result_projection(&fixture.state).expect("seal projection");
        let key = read_gate_key(&fixture.paths, &fixture.state.run_id).expect("key");
        fixture.marker = write_native_acceptance_marker_with_results_and_key(
            &fixture.state.run_root,
            fixture.state.run_id.clone(),
            fixture.state.working_dir.clone(),
            fixture.marker.checks.clone(),
            &key,
            AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("projected marker");
        fixture
    }

    fn durable_chain_fixture() -> Fixture {
        let fixture = fixture_with_contract_and_launch(
            "name: result\nchecks:\n  - file_exists: result.txt\n",
            "{\"schema\":1,\"goal\":\"ship verified change\",\"signals\":{\"watchkeeper_chain_adapter\":{}}}\n",
        );
        let job_dir = fixture.paths.job_dir(fixture.authority.job_id.as_ref());
        fs::write(
            job_dir.join(super::ORDERED_CANDIDATE_MANIFEST_JSON),
            "{\"schema_version\":1,\"ordered_candidates\":[]}\n",
        )
        .expect("ordered candidate manifest");
        fs::write(
            job_dir.join(crate::plan::ORDERED_CANDIDATE_APPLICATION_EVENTS_JSONL),
            "{\"kind\":\"prepared\",\"sequence\":1}\n",
        )
        .expect("candidate application ledger");
        fs::write(
            job_dir.join(crate::chain::DURABLE_CHAIN_HOOK_EVENTS_JSONL),
            "{\"kind\":\"started\",\"sequence\":1}\n",
        )
        .expect("chain hook ledger");
        fixture
    }

    fn identity_fixture() -> Fixture {
        let mut fixture = fixture();
        let binary = GateBinaryIdentity {
            sha256: format!("sha256:{}", "a".repeat(64)),
            os: "macos".to_string(),
            arch: "aarch64".to_string(),
        };
        let identity = GateEvaluatorIdentity {
            schema_version: deadreckon_protocol::GATE_EVALUATOR_IDENTITY_SCHEMA_VERSION,
            protocol_version: deadreckon_protocol::GATE_EVALUATOR_PROTOCOL_VERSION,
            controller: binary.clone(),
            evaluator: binary,
            docker: None,
        };
        let mut job = load_job(&fixture.paths, fixture.authority.job_id.as_ref()).expect("job");
        job.policy
            .execution
            .as_mut()
            .expect("execution policy")
            .gate_evaluator = Some(identity.clone());
        fixture.authority.gate_evaluator_sha256 =
            Some(crate::gate_evaluator_identity_sha256(&identity).expect("identity digest"));
        fixture.authority.effective_policy_sha256 =
            sha256_text(&serde_json::to_string(&job.policy).expect("policy JSON"));
        atomic_write_json(
            &fixture
                .paths
                .job_authority(fixture.authority.job_id.as_ref()),
            &fixture.authority,
        )
        .expect("identity-bound authority");
        job.authority_sha256 = sha256_file(
            &fixture
                .paths
                .job_authority(fixture.authority.job_id.as_ref()),
        )
        .expect("authority digest");
        atomic_write_json(
            &fixture.paths.job_json(fixture.authority.job_id.as_ref()),
            &job,
        )
        .expect("identity-bound job");
        seal_boundary_observation(&fixture);
        fixture
    }

    fn seal_boundary_observation(fixture: &Fixture) {
        let observation = SandboxBoundaryObservation {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: fixture.authority.job_id.clone(),
            run_id: fixture.authority.run_id.clone(),
            observed_at: Utc::now(),
            issuer: SandboxBoundaryObservationIssuer::DeadreckonController,
            probe_id: Uuid::new_v4().to_string(),
            attempt: 1,
            outer_launch_id: Uuid::new_v4().to_string(),
            authority_sha256: sha256_file(
                &fixture
                    .paths
                    .job_authority(fixture.authority.job_id.as_ref()),
            )
            .expect("authority digest"),
            contract_sha256: fixture.authority.contract_sha256.clone(),
            result_tree_sha256: crate::sandbox_boundary_result_tree_sha256(&fixture.state)
                .expect("result tree"),
            sandbox_requested: fixture.authority.sandbox_requested.clone(),
            sandbox_backend: "sandbox-exec".to_string(),
            contained: true,
            gate_key_read_denied: true,
            proof_write_denied: true,
            control_write_denied: true,
            operator_capture_read_denied: true,
            operator_capture_write_denied: true,
            signing_env_scrubbed: true,
            probe_sha256: sha256_text("fixed controller probe"),
            gate_evaluator_sha256: fixture.authority.gate_evaluator_sha256.clone(),
            signature: String::new(),
        };
        crate::seal_sandbox_boundary_observation(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &observation,
        )
        .expect("sandbox boundary observation");
    }

    struct RepairFixture {
        fixture: Fixture,
        intent_path: PathBuf,
        manifest_path: PathBuf,
        candidate_path: PathBuf,
        archived_marker_path: PathBuf,
        archived_judgment_path: PathBuf,
    }

    fn repair_fixture(max_attempts: u32) -> RepairFixture {
        let mut fixture = fixture();
        let mut job = load_job(&fixture.paths, "job-1").expect("job");
        job.shape = JobShape::Graph;
        job.policy.max_attempts = max_attempts;
        fixture.authority.effective_policy_sha256 =
            sha256_text(&serde_json::to_string(&job.policy).expect("updated policy json"));
        atomic_write_json(&fixture.paths.job_authority("job-1"), &fixture.authority)
            .expect("updated authority");
        job.authority_sha256 =
            sha256_file(&fixture.paths.job_authority("job-1")).expect("authority digest");
        atomic_write_json(&fixture.paths.job_json("job-1"), &job).expect("updated job");

        let merged = create_run(
            &fixture.paths,
            RunOptions {
                goal: fixture.state.goal.clone(),
                cwd: fixture.state.cwd.clone(),
                sandbox: "sandbox-exec".to_string(),
                provider: None,
                skill_name: "test".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("merged-result".to_string()),
                codebase: None,
            },
        )
        .expect("merged run");
        fs::write(merged.working_dir.join("result.txt"), "verified\n").expect("merged result");
        let merged_tree = super::parent_repair_result_tree_hash(&merged).expect("merged tree");
        let initial_marker = fs::read(crate::marker_path_for_run_root(&fixture.state.run_root))
            .expect("initial marker bytes");
        let revise = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId("job-1".to_string()),
            run_id: RunId("job-1".to_string()),
            judged_at: Utc::now(),
            provider: "independent-judge".to_string(),
            model: "judge-model".to_string(),
            decision: SemanticDecision::Revise,
            summary: "the parent result needs one bounded repair".to_string(),
            goal_coverage: vec![GoalCoverage {
                claim: "ship verified change".to_string(),
                status: GoalCoverageStatus::Missing,
                evidence: vec!["initial-result".to_string()],
            }],
            missing: vec!["repair the parent result".to_string()],
            input_sha256: sha256_text("revise evidence"),
            spend_usd: 0.01,
        };
        let round_dir = super::parent_repair_round_dir(&fixture.state, 1);
        fs::create_dir_all(&round_dir).expect("repair archive");
        let archived_marker_path = round_dir.join("pre-repair-marker.json");
        let archived_judgment_path = round_dir.join("revise-judgment.json");
        fs::write(&archived_marker_path, initial_marker).expect("archived marker");
        atomic_write_json(&archived_judgment_path, &revise).expect("archived revise judgment");

        let launch_one = Uuid::new_v4().to_string();
        append_repair_event(
            &fixture.paths,
            1,
            1,
            JobEventKind::LeaseAcquired,
            serde_json::json!({ "owner": "repair-test" }),
        );
        append_repair_event(
            &fixture.paths,
            2,
            1,
            JobEventKind::AttemptStarted,
            serde_json::json!({ "attempt": 1 }),
        );
        append_repair_event(
            &fixture.paths,
            3,
            1,
            JobEventKind::ChildLinked,
            serde_json::json!({
                "attempt": 1,
                "launch_id": launch_one,
                "run_id": "job-1",
            }),
        );

        let intent_path = fixture.paths.job_dir("job-1").join("parent-repair.json");
        let feedback = super::parent_repair_feedback(&revise);
        atomic_write_json(
            &intent_path,
            &serde_json::json!({
                "schema_version": 1,
                "job_id": "job-1",
                "shape": "graph",
                "round": 1,
                "merged_run_id": merged.run_id,
                "merged_tree_sha256": merged_tree,
                "pre_repair_tree_sha256": merged_tree,
                "revise_marker_sha256": sha256_file(&archived_marker_path)
                    .expect("marker digest"),
                "revise_judgment_sha256": sha256_file(&archived_judgment_path)
                    .expect("judgment digest"),
                "revise_input_sha256": revise.input_sha256,
                "requested_after_attempt": 1,
                "requested_after_launch_id": launch_one,
                "requested_after_lease_epoch": 1,
                "provider": "smoke",
                "model": null,
                "feedback": feedback,
                "previous_round_sha256": null,
                "requested_at": Utc::now(),
            }),
        )
        .expect("repair intent");
        let intent_sha256 = sha256_file(&intent_path).expect("intent digest");
        append_repair_event(
            &fixture.paths,
            4,
            1,
            JobEventKind::SemanticJudgeRevise,
            serde_json::json!({
                "round": 1,
                "intent_sha256": intent_sha256,
                "judgment_sha256": sha256_file(&archived_judgment_path)
                    .expect("judgment digest"),
                "merged_run_id": "merged-result",
            }),
        );
        append_repair_event(
            &fixture.paths,
            5,
            1,
            JobEventKind::RetryScheduled,
            serde_json::json!({ "after_attempt": 1, "round": 1 }),
        );
        append_repair_event(
            &fixture.paths,
            6,
            1,
            JobEventKind::AttemptStarted,
            serde_json::json!({ "attempt": 2 }),
        );
        let launch_two = Uuid::new_v4().to_string();
        append_repair_event(
            &fixture.paths,
            7,
            1,
            JobEventKind::ChildLinked,
            serde_json::json!({
                "attempt": 2,
                "launch_id": launch_two,
                "run_id": "job-1",
            }),
        );

        fs::write(
            fixture.state.working_dir.join("result.txt"),
            "verified and repaired\n",
        )
        .expect("repaired result");
        let manifest_path =
            crate::parent_repair_manifest_path_for_run_root(&fixture.state.run_root);
        atomic_write_json(
            &manifest_path,
            &serde_json::json!({
                "schema_version": 1,
                "job_id": "job-1",
                "shape": "graph",
                "round": 1,
                "merged_run_id": "merged-result",
                "merged_tree_sha256": merged_tree,
                "pre_repair_tree_sha256": merged_tree,
                "intent_sha256": intent_sha256,
                "attempt": 2,
                "launch_id": launch_two,
                "lease_epoch": 1,
                "attempt_baseline_tree_sha256": merged_tree,
                "started_at": Utc::now(),
            }),
        )
        .expect("repair manifest");
        let candidate_path =
            crate::parent_repair_candidate_path_for_run_root(&fixture.state.run_root);
        atomic_write_json(
            &candidate_path,
            &serde_json::json!({
                "schema_version": 1,
                "job_id": "job-1",
                "run_id": "job-1",
                "round": 1,
                "attempt": 2,
                "launch_id": launch_two,
                "lease_epoch": 1,
                "intent_sha256": intent_sha256,
                "manifest_sha256": sha256_file(&manifest_path).expect("manifest digest"),
                "result_tree_sha256": super::parent_repair_result_tree_hash(&fixture.state)
                    .expect("candidate tree"),
                "turn": 1,
                "ready_at": Utc::now(),
            }),
        )
        .expect("repair candidate");
        resign_repair_marker(&mut fixture);

        RepairFixture {
            fixture,
            intent_path,
            manifest_path,
            candidate_path,
            archived_marker_path,
            archived_judgment_path,
        }
    }

    fn append_repair_event(
        paths: &DeadreckonPaths,
        sequence: u64,
        lease_epoch: u64,
        kind: JobEventKind,
        detail: serde_json::Value,
    ) {
        append_job_event(
            paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: JobId("job-1".to_string()),
                sequence: JobEventSequence::new(sequence).expect("event sequence"),
                event_id: Uuid::new_v4().to_string(),
                causation_id: format!("repair-test-{sequence}"),
                timestamp: Utc::now(),
                lease_epoch,
                kind,
                detail,
            },
        )
        .expect("append repair event");
    }

    fn refresh_repair_hashes_and_resign(repair: &mut RepairFixture) {
        let intent_sha256 = sha256_file(&repair.intent_path).expect("intent digest");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&repair.manifest_path).expect("manifest bytes"))
                .expect("manifest json");
        manifest["intent_sha256"] = serde_json::Value::String(intent_sha256.clone());
        atomic_write_json(&repair.manifest_path, &manifest).expect("updated manifest");
        let mut candidate: serde_json::Value =
            serde_json::from_slice(&fs::read(&repair.candidate_path).expect("candidate bytes"))
                .expect("candidate json");
        candidate["intent_sha256"] = serde_json::Value::String(intent_sha256);
        candidate["manifest_sha256"] =
            serde_json::Value::String(sha256_file(&repair.manifest_path).expect("manifest digest"));
        atomic_write_json(&repair.candidate_path, &candidate).expect("updated candidate");
        resign_repair_marker(&mut repair.fixture);
    }

    fn resign_repair_marker(fixture: &mut Fixture) {
        let key = read_gate_key(&fixture.paths, "job-1").expect("gate key");
        fixture.marker = write_native_acceptance_marker_with_results_and_key(
            &fixture.state.run_root,
            "job-1".to_string(),
            fixture.state.working_dir.clone(),
            fixture.marker.checks.clone(),
            &key,
            AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("repair-aware marker");
        seal_boundary_observation(fixture);
    }

    #[test]
    fn repair_receipt_validates_full_fenced_parent_lineage() {
        let repair = repair_fixture(3);
        let receipt = seal_completion_receipt(
            &repair.fixture.paths,
            &repair.fixture.state,
            &repair.fixture.authority,
            &repair.fixture.marker,
            &repair.fixture.judgment,
        )
        .expect("seal valid repair");
        assert_eq!(
            validate_completion_receipt(&repair.fixture.paths, &repair.fixture.state)
                .expect("validate valid repair"),
            receipt
        );
    }

    #[test]
    fn graph_campaign_and_parent_repair_keep_two_key_completion() {
        // Parent-repair lineage remains an additional bound proof, never a
        // replacement for the deterministic+semantic two-key receipt.
        repair_receipt_validates_full_fenced_parent_lineage();
        let fixture = projected_fixture();
        let receipt = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("projected two-key receipt");
        assert_eq!(
            receipt.proof_kind,
            deadreckon_protocol::CompletionProofKind::TwoKeyCompletion
        );
        assert!(receipt.result_projection_sha256.is_some());
    }

    #[test]
    fn repair_receipt_refuses_shape_mismatch_before_seal() {
        for target in ["intent", "manifest"] {
            let mut repair = repair_fixture(3);
            let path = if target == "intent" {
                &repair.intent_path
            } else {
                &repair.manifest_path
            };
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(path).expect("repair bytes"))
                    .expect("repair json");
            value["shape"] = serde_json::Value::String("legacy_campaign".to_string());
            atomic_write_json(path, &value).expect("shape mutation");
            refresh_repair_hashes_and_resign(&mut repair);

            let error = seal_completion_receipt(
                &repair.fixture.paths,
                &repair.fixture.state,
                &repair.fixture.authority,
                &repair.fixture.marker,
                &repair.fixture.judgment,
            )
            .expect_err("cross-shape repair must not seal");
            assert!(
                error.to_string().contains("Job authority")
                    || error.to_string().contains("result authority"),
                "{target}: {error}"
            );
            assert!(!repair.fixture.paths.job_receipt("job-1").exists());
        }
    }

    #[test]
    fn repair_receipt_refuses_unfenced_attempt_launch_and_lease_before_seal() {
        for mutation in ["candidate-attempt", "launch", "lease"] {
            let mut repair = repair_fixture(3);
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(&repair.manifest_path).expect("manifest bytes"))
                    .expect("manifest json");
            let mut candidate: serde_json::Value =
                serde_json::from_slice(&fs::read(&repair.candidate_path).expect("candidate bytes"))
                    .expect("candidate json");
            match mutation {
                "candidate-attempt" => {
                    candidate["attempt"] = serde_json::Value::from(3);
                }
                "launch" => {
                    let launch = Uuid::new_v4().to_string();
                    manifest["launch_id"] = serde_json::Value::String(launch.clone());
                    candidate["launch_id"] = serde_json::Value::String(launch);
                }
                "lease" => {
                    manifest["lease_epoch"] = serde_json::Value::from(2);
                    candidate["lease_epoch"] = serde_json::Value::from(2);
                }
                _ => unreachable!(),
            }
            atomic_write_json(&repair.manifest_path, &manifest).expect("manifest mutation");
            atomic_write_json(&repair.candidate_path, &candidate).expect("candidate mutation");
            refresh_repair_hashes_and_resign(&mut repair);

            let error = seal_completion_receipt(
                &repair.fixture.paths,
                &repair.fixture.state,
                &repair.fixture.authority,
                &repair.fixture.marker,
                &repair.fixture.judgment,
            )
            .expect_err("unfenced repair must not seal");
            assert!(
                error.to_string().contains("fenced attempt")
                    || error.to_string().contains("fenced Job attempt"),
                "{mutation}: {error}"
            );
            assert!(!repair.fixture.paths.job_receipt("job-1").exists());
        }
    }

    #[test]
    fn repair_receipt_refuses_candidate_result_tree_mismatch_before_seal() {
        let mut repair = repair_fixture(3);
        let mut candidate: serde_json::Value =
            serde_json::from_slice(&fs::read(&repair.candidate_path).expect("candidate bytes"))
                .expect("candidate json");
        candidate["result_tree_sha256"] =
            serde_json::Value::String(format!("sha256:{}", "0".repeat(64)));
        atomic_write_json(&repair.candidate_path, &candidate).expect("tree mutation");
        refresh_repair_hashes_and_resign(&mut repair);

        let error = seal_completion_receipt(
            &repair.fixture.paths,
            &repair.fixture.state,
            &repair.fixture.authority,
            &repair.fixture.marker,
            &repair.fixture.judgment,
        )
        .expect_err("wrong repair result tree must not seal");
        assert!(error.to_string().contains("result authority"), "{error}");
        assert!(!repair.fixture.paths.job_receipt("job-1").exists());
    }

    #[test]
    fn repair_receipt_enforces_round_and_attempt_bounds() {
        let repair = repair_fixture(1);
        let error = seal_completion_receipt(
            &repair.fixture.paths,
            &repair.fixture.state,
            &repair.fixture.authority,
            &repair.fixture.marker,
            &repair.fixture.judgment,
        )
        .expect_err("repair cannot consume an unapproved second attempt");
        assert!(error.to_string().contains("Job authority"), "{error}");

        let mut repair = repair_fixture(3);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&repair.manifest_path).expect("manifest bytes"))
                .expect("manifest json");
        let mut candidate: serde_json::Value =
            serde_json::from_slice(&fs::read(&repair.candidate_path).expect("candidate bytes"))
                .expect("candidate json");
        manifest["attempt"] = serde_json::Value::from(4);
        candidate["attempt"] = serde_json::Value::from(4);
        atomic_write_json(&repair.manifest_path, &manifest).expect("manifest attempt");
        atomic_write_json(&repair.candidate_path, &candidate).expect("candidate attempt");
        refresh_repair_hashes_and_resign(&mut repair);
        let error = seal_completion_receipt(
            &repair.fixture.paths,
            &repair.fixture.state,
            &repair.fixture.authority,
            &repair.fixture.marker,
            &repair.fixture.judgment,
        )
        .expect_err("repair attempt cannot exceed the approved bound");
        assert!(error.to_string().contains("result authority"), "{error}");

        let repair = repair_fixture(3);
        seal_completion_receipt(
            &repair.fixture.paths,
            &repair.fixture.state,
            &repair.fixture.authority,
            &repair.fixture.marker,
            &repair.fixture.judgment,
        )
        .expect("bounded repair control seals");
    }

    #[cfg(unix)]
    #[test]
    fn repair_receipt_refuses_byte_identical_symlink_substitution() {
        use std::os::unix::fs::symlink;

        let repair = repair_fixture(3);
        seal_completion_receipt(
            &repair.fixture.paths,
            &repair.fixture.state,
            &repair.fixture.authority,
            &repair.fixture.marker,
            &repair.fixture.judgment,
        )
        .expect("seal valid repair");
        let paths = [
            repair.intent_path.clone(),
            repair.manifest_path.clone(),
            repair.candidate_path.clone(),
            repair.archived_marker_path.clone(),
            repair.archived_judgment_path.clone(),
        ];
        for (index, path) in paths.into_iter().enumerate() {
            let external = path.with_file_name(format!("trusted-copy-{index}.json"));
            fs::rename(&path, &external).expect("move trusted bytes");
            symlink(&external, &path).expect("substitute byte-identical symlink");
            let error = validate_completion_receipt(&repair.fixture.paths, &repair.fixture.state)
                .expect_err("symlinked repair evidence must fail closed");
            assert!(
                error.to_string().contains("regular non-symlink"),
                "{}: {error}",
                path.display()
            );
            fs::remove_file(&path).expect("remove symlink");
            fs::rename(&external, &path).expect("restore regular file");
            validate_completion_receipt(&repair.fixture.paths, &repair.fixture.state)
                .expect("restored regular proof validates");
        }
    }

    #[test]
    fn repair_receipt_post_seal_mutation_matrix_fails_closed() {
        let repair = repair_fixture(3);
        seal_completion_receipt(
            &repair.fixture.paths,
            &repair.fixture.state,
            &repair.fixture.authority,
            &repair.fixture.marker,
            &repair.fixture.judgment,
        )
        .expect("seal valid repair");
        let paths = [
            repair.intent_path.clone(),
            repair.manifest_path.clone(),
            repair.candidate_path.clone(),
            repair.archived_marker_path.clone(),
            repair.archived_judgment_path.clone(),
        ];
        for path in paths {
            let original = fs::read(&path).expect("proof bytes");
            let mut changed = original.clone();
            changed.push(b'\n');
            fs::write(&path, changed).expect("mutate proof");
            validate_completion_receipt(&repair.fixture.paths, &repair.fixture.state)
                .expect_err("post-seal repair mutation must fail closed");
            fs::write(&path, original).expect("restore proof");
            validate_completion_receipt(&repair.fixture.paths, &repair.fixture.state)
                .expect("restored proof validates");
        }

        let active = [
            repair.intent_path,
            repair.manifest_path,
            repair.candidate_path,
        ];
        let originals = active
            .iter()
            .map(|path| fs::read(path).expect("active proof"))
            .collect::<Vec<_>>();
        for path in &active {
            fs::remove_file(path).expect("remove active repair proof");
        }
        validate_completion_receipt(&repair.fixture.paths, &repair.fixture.state)
            .expect_err("removing the signed repair trio must fail closed");
        for (path, bytes) in active.iter().zip(originals) {
            fs::write(path, bytes).expect("restore active repair proof");
        }
        validate_completion_receipt(&repair.fixture.paths, &repair.fixture.state)
            .expect("restored active proofs validate");
    }

    fn worktree_fixture() -> Fixture {
        let mut fixture = fixture();
        git_ok(&fixture.state.working_dir, &["init"]);
        git_ok(
            &fixture.state.working_dir,
            &["config", "user.email", "deadreckon@example.invalid"],
        );
        git_ok(
            &fixture.state.working_dir,
            &["config", "user.name", "DeadReckon Test"],
        );
        fs::write(
            fixture.state.working_dir.join(".git/info/exclude"),
            ".deadreckon/\n",
        )
        .expect("git exclude");
        git_ok(&fixture.state.working_dir, &["add", "-A"]);
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "approved base"],
        );
        let base = git_stdout(&fixture.state.working_dir, &["rev-parse", "HEAD"]);
        fs::write(
            fixture.state.working_dir.join("result.txt"),
            "verified result\n",
        )
        .expect("result change");
        git_ok(&fixture.state.working_dir, &["add", "result.txt"]);
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "deliver result"],
        );
        let branch = git_stdout(
            &fixture.state.working_dir,
            &["symbolic-ref", "--short", "HEAD"],
        );
        let record = CodebaseRecord {
            schema_version: crate::codebase::CODEBASE_RECORD_VERSION,
            mode: CodebaseMode::Worktree,
            source_path: Some(fixture.state.working_dir.clone()),
            source_git_root: Some(fixture.state.working_dir.clone()),
            branch_name: Some(branch),
            base_ref: Some(base.clone()),
            base_sha: Some(base.clone()),
            parent_branch: None,
            worktree_path: Some(fixture.state.working_dir.clone()),
            dirty_files_seeded: false,
            head_was_detached: false,
            created_at: Utc::now(),
            deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
            doc_polish_hash: None,
        };
        write_codebase_record(&fixture.state.working_dir, &record).expect("worktree record");
        write_trusted_codebase_record(&fixture.state.run_root, &record)
            .expect("trusted worktree record");
        fixture.authority.source_revision = Some(base);
        atomic_write_json(&fixture.paths.job_authority("job-1"), &fixture.authority)
            .expect("worktree authority");
        let mut job = load_job(&fixture.paths, "job-1").expect("job");
        job.authority_sha256 =
            sha256_file(&fixture.paths.job_authority("job-1")).expect("authority digest");
        atomic_write_json(&fixture.paths.job_json("job-1"), &job).expect("updated job");
        seal_boundary_observation(&fixture);
        fixture
    }

    fn git_ok(cwd: &std::path::Path, args: &[&str]) {
        let output = crate::git::run_git(cwd, args).expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(cwd: &std::path::Path, args: &[&str]) -> String {
        let output = crate::git::run_git(cwd, args).expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn achieved_plus_gate_pass_seals_two_key_receipt() {
        let fixture = fixture();
        let receipt = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        assert_eq!(receipt.signature.len(), 64);
        assert!(receipt.contained);
        assert_eq!(
            validate_completion_receipt(&fixture.paths, &fixture.state).expect("validate"),
            receipt
        );
    }

    #[test]
    fn new_strict_receipt_binds_candidate_tree_and_projection() {
        let fixture = projected_fixture();
        let projection = crate::load_result_projection(&fixture.state).expect("projection");
        let receipt = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        assert_eq!(receipt.result_tree_sha256, projection.manifest.tree_sha256);
        assert_eq!(
            receipt.result_projection_sha256.as_deref(),
            Some(
                crate::result_projection_sha256(&fixture.state)
                    .expect("P")
                    .as_str()
            )
        );
        validate_completion_receipt(&fixture.paths, &fixture.state).expect("validate");
    }

    #[test]
    fn result_or_projection_mutation_invalidates_receipt() {
        for mutation in ["result", "candidate", "manifest", "policy"] {
            let fixture = projected_fixture();
            seal_completion_receipt(
                &fixture.paths,
                &fixture.state,
                &fixture.authority,
                &fixture.marker,
                &fixture.judgment,
            )
            .expect("seal");
            match mutation {
                "result" => fs::write(
                    fixture.state.working_dir.join("result.txt"),
                    "mutated result\n",
                )
                .expect("mutate result"),
                "candidate" => fs::write(
                    crate::result_projection_candidate_path(&fixture.state).join("result.txt"),
                    "mutated candidate\n",
                )
                .expect("mutate candidate"),
                "manifest" => fs::write(
                    crate::result_projection_manifest_path(&fixture.state),
                    "{}\n",
                )
                .expect("mutate manifest"),
                "policy" => fs::write(crate::result_projection_policy_path(&fixture.state), "{}\n")
                    .expect("mutate policy"),
                _ => unreachable!(),
            }
            assert!(
                validate_completion_receipt(&fixture.paths, &fixture.state).is_err(),
                "{mutation} mutation must invalidate the receipt"
            );
        }
    }

    #[test]
    fn historical_receipt_without_projection_keeps_historical_validation() {
        let fixture = fixture();
        let receipt = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("historical seal");
        assert!(receipt.result_projection_sha256.is_none());
        let encoded = serde_json::to_string(&receipt).expect("receipt json");
        assert!(!encoded.contains("result_projection_sha256"));
        validate_completion_receipt(&fixture.paths, &fixture.state).expect("historical validate");
    }

    #[test]
    fn receipt_cannot_hash_live_rules_after_seal() {
        let fixture = projected_fixture();
        let receipt = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        let frozen = crate::result_projection_index_at(&fixture.state, &fixture.state.working_dir)
            .expect("frozen index")
            .tree_hash();
        assert_eq!(receipt.result_tree_sha256, frozen);
        fs::write(
            fixture.state.working_dir.join(".gitignore"),
            "/result.txt\n",
        )
        .expect("late live rewrite");
        assert!(validate_completion_receipt(&fixture.paths, &fixture.state).is_err());
    }

    #[test]
    fn expired_job_boundary_prevents_receipt_sealing() {
        let fixture = fixture();
        let scope = crate::git::WorkBoundaryScope::new(
            Instant::now(),
            Duration::from_secs(3),
            || false,
            "completion receipt sealing",
        )
        .with_authority_dir(fixture.state.run_root.join("child-pids"));

        let error = seal_completion_receipt_bounded(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
            scope,
        )
        .expect_err("expired Job boundary must prevent sealing");

        assert!(matches!(
            error,
            crate::DeadreckonError::ProcessBoundary {
                kind: crate::ProcessBoundaryKind::WorkExpired,
                ..
            }
        ));
        assert!(!fixture.paths.job_receipt("job-1").exists());
    }

    #[test]
    fn receipt_rejects_attempt_and_outer_launch_identity_tamper() {
        let fixture = fixture();
        let receipt = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        let receipt_path = fixture.paths.job_receipt(fixture.authority.job_id.as_ref());

        let mut tampered = receipt.clone();
        tampered.attempt += 1;
        atomic_write_json(&receipt_path, &tampered).expect("tampered receipt attempt");
        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("a different attempt must invalidate the receipt");
        assert!(
            error
                .to_string()
                .contains("receipt attempt identity does not match")
        );

        let mut tampered = receipt;
        tampered.outer_launch_id = Uuid::new_v4().to_string();
        atomic_write_json(&receipt_path, &tampered).expect("tampered receipt launch");
        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("a different outer launch must invalidate the receipt");
        assert!(
            error
                .to_string()
                .contains("receipt attempt identity does not match")
        );
    }

    #[test]
    fn receipt_revalidates_every_durable_chain_execution_evidence_file() {
        let fixture = durable_chain_fixture();
        let receipt = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal durable chain receipt");
        let evidence = receipt
            .execution_evidence
            .as_ref()
            .expect("durable chain execution evidence");
        assert!(evidence.candidate_application_events_sha256.is_some());
        assert!(evidence.chain_hook_events_sha256.is_some());

        let job_dir = fixture.paths.job_dir(fixture.authority.job_id.as_ref());
        let evidence_files = [
            job_dir.join(super::ORDERED_CANDIDATE_MANIFEST_JSON),
            job_dir.join(crate::plan::ORDERED_CANDIDATE_APPLICATION_EVENTS_JSONL),
            job_dir.join(crate::chain::DURABLE_CHAIN_HOOK_EVENTS_JSONL),
        ];
        for path in &evidence_files {
            let original = fs::read(path).expect("execution evidence bytes");
            let mut changed = original.clone();
            changed.extend_from_slice(b"tampered\n");
            fs::write(path, changed).expect("mutate execution evidence");
            let error = validate_completion_receipt(&fixture.paths, &fixture.state)
                .expect_err("mutated execution evidence must invalidate the receipt");
            assert!(
                error
                    .to_string()
                    .contains("receipt execution ledgers changed")
            );
            fs::write(path, original).expect("restore execution evidence");
            validate_completion_receipt(&fixture.paths, &fixture.state)
                .expect("restored execution evidence validates");
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            for path in &evidence_files {
                let original = fs::read(path).expect("execution evidence bytes");
                let target = path.with_extension("trusted-copy");
                fs::write(&target, &original).expect("byte-identical evidence copy");
                fs::remove_file(path).expect("remove execution evidence");
                symlink(&target, path).expect("substitute execution evidence symlink");
                let error = validate_completion_receipt(&fixture.paths, &fixture.state)
                    .expect_err("symlinked execution evidence must fail closed");
                assert!(error.to_string().contains("regular non-symlink"));
                fs::remove_file(path).expect("remove execution evidence symlink");
                fs::write(path, original).expect("restore execution evidence");
                fs::remove_file(target).expect("remove evidence copy");
            }
        }
    }

    #[test]
    fn receipt_round_trips_identity_bound_observation_and_rejects_identity_tamper() {
        let fixture = identity_fixture();
        let receipt = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("identity-bound receipt");
        assert_eq!(
            validate_completion_receipt(&fixture.paths, &fixture.state).expect("validate"),
            receipt
        );

        let path = fixture
            .paths
            .job_sandbox_boundary_observation(fixture.authority.job_id.as_ref());
        let mut observation: SandboxBoundaryObservation =
            serde_json::from_slice(&fs::read(&path).expect("observation"))
                .expect("observation JSON");
        observation.gate_evaluator_sha256 = Some(format!("sha256:{}", "b".repeat(64)));
        atomic_write_json(&path, &observation).expect("tampered evaluator identity");
        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("identity tamper must invalidate receipt");
        assert!(
            error
                .to_string()
                .contains("observed gate evaluator identity digest changed")
        );
    }

    #[test]
    fn receipt_revalidates_missing_mutated_and_symlinked_sandbox_observation() {
        let fixture = fixture();
        seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        let path = fixture
            .paths
            .job_sandbox_boundary_observation(fixture.authority.job_id.as_ref());
        let original = fs::read(&path).expect("observation bytes");

        fs::remove_file(&path).expect("remove observation");
        validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("missing observation must invalidate receipt");
        fs::write(&path, &original).expect("restore observation");
        validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect("restored observation validates");

        let mut changed = original.clone();
        changed.push(b'\n');
        fs::write(&path, changed).expect("mutate observation bytes");
        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("byte mutation must invalidate receipt");
        assert!(
            error
                .to_string()
                .contains("sandbox boundary observation digest changed")
        );
        fs::write(&path, &original).expect("restore observation");

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let target = path.with_extension("copy.json");
            fs::write(&target, &original).expect("observation copy");
            fs::remove_file(&path).expect("remove observation");
            symlink(&target, &path).expect("observation symlink");
            let error = validate_completion_receipt(&fixture.paths, &fixture.state)
                .expect_err("symlink observation must invalidate receipt");
            assert!(error.to_string().contains("non-symlink"));
        }
    }

    #[test]
    fn receipt_binds_deliverable_tree_not_specstory_evidence() {
        let fixture = fixture();
        let private = fixture
            .state
            .working_dir
            .join(".specstory/history/session.md");
        fs::create_dir_all(private.parent().expect("private parent")).expect("private dir");
        fs::write(&private, "before sealing\n").expect("private evidence");
        let receipt = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");

        fs::write(&private, "changed after sealing\n").expect("mutate private evidence");

        assert_eq!(
            validate_completion_receipt(&fixture.paths, &fixture.state).expect("validate"),
            receipt
        );
    }

    #[test]
    fn worktree_receipt_refuses_uncommitted_deliverable_changes() {
        let fixture = worktree_fixture();
        fs::write(
            fixture.state.working_dir.join("uncommitted.txt"),
            "not in the result revision\n",
        )
        .expect("dirty deliverable");

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("uncommitted deliverable must not be sealed");

        assert!(
            error
                .to_string()
                .contains("uncommitted deliverable changes"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_refuses_ignored_uncommitted_deliverables() {
        let fixture = worktree_fixture();
        fs::write(
            fixture.state.working_dir.join(".gitignore"),
            "ignored.txt\n",
        )
        .expect("ignore rule");
        git_ok(&fixture.state.working_dir, &["add", ".gitignore"]);
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "ignore fixture output"],
        );
        fs::write(
            fixture.state.working_dir.join("ignored.txt"),
            "signed filesystem content absent from result revision\n",
        )
        .expect("ignored deliverable");

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("ignored uncommitted deliverable must not be sealed");

        assert!(
            error
                .to_string()
                .contains("uncommitted deliverable changes"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_refuses_assume_unchanged_deliverables() {
        let fixture = worktree_fixture();
        git_ok(
            &fixture.state.working_dir,
            &["update-index", "--assume-unchanged", "result.txt"],
        );
        fs::write(
            fixture.state.working_dir.join("result.txt"),
            "hidden from ordinary Git status\n",
        )
        .expect("hidden deliverable change");

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("assume-unchanged deliverable must not be sealed");

        assert!(
            error
                .to_string()
                .contains("uncommitted deliverable changes"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_refuses_skip_worktree_deliverables() {
        let fixture = worktree_fixture();
        git_ok(
            &fixture.state.working_dir,
            &["update-index", "--skip-worktree", "result.txt"],
        );

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("skip-worktree deliverable must not be sealed");

        assert!(
            error
                .to_string()
                .contains("uncommitted deliverable changes"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn worktree_receipt_compares_executable_mode_with_signed_revision() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = worktree_fixture();
        git_ok(
            &fixture.state.working_dir,
            &["config", "core.filemode", "false"],
        );
        let result = fixture.state.working_dir.join("result.txt");
        let mut permissions = fs::metadata(&result)
            .expect("result metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&result, permissions).expect("make result executable");

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("filesystem mode absent from signed revision must be refused");

        assert!(
            error
                .to_string()
                .contains("does not exactly match signed Git revision"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_refuses_gitlinks_instead_of_omitting_them() {
        let fixture = worktree_fixture();
        let nested = fixture.state.working_dir.join("vendor/nested-repository");
        fs::create_dir_all(&nested).expect("nested repository");
        git_ok(&nested, &["init", "-q", "-b", "main"]);
        git_ok(
            &nested,
            &["config", "user.email", "fixture@example.invalid"],
        );
        git_ok(&nested, &["config", "user.name", "fixture"]);
        fs::write(nested.join("README.md"), "nested result\n").expect("nested file");
        git_ok(&nested, &["add", "README.md"]);
        git_ok(&nested, &["commit", "-m", "nested base"]);
        git_ok(
            &fixture.state.working_dir,
            &["add", "vendor/nested-repository"],
        );
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "provider-created gitlink"],
        );

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("gitlink cannot be silently omitted from strict receipt");

        assert!(
            error.to_string().contains("unsupported Git submodule"),
            "{error}"
        );
        assert!(
            error.to_string().contains("vendor/nested-repository"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_refuses_filters_before_materializing_the_signed_tree() {
        let fixture = worktree_fixture();
        let sentinel = fixture.paths.home().join("smudge-filter-ran");
        fs::write(
            fixture.state.working_dir.join(".gitattributes"),
            "result.txt filter=evil\n",
        )
        .expect("filter attributes");
        git_ok(
            &fixture.state.working_dir,
            &[
                "config",
                "filter.evil.smudge",
                &format!("sh -c 'touch {}; cat'", sentinel.display()),
            ],
        );
        git_ok(&fixture.state.working_dir, &["add", ".gitattributes"]);
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "provider filter attribute"],
        );

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("external filter must be refused before checkout-index");

        assert!(error.to_string().contains("external Git filter"), "{error}");
        assert!(
            !sentinel.exists(),
            "receipt verification must not execute the configured smudge command"
        );
    }

    #[test]
    fn trusted_worktree_record_prevents_workspace_routing_tamper() {
        let fixture = worktree_fixture();
        write_codebase_record(&fixture.state.working_dir, &CodebaseRecord::fresh())
            .expect("forge workspace record");
        fs::write(
            fixture.state.working_dir.join("uncommitted.txt"),
            "not in the result revision\n",
        )
        .expect("dirty deliverable");

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("workspace record must not redirect the receipt boundary");

        assert!(
            error
                .to_string()
                .contains("uncommitted deliverable changes"),
            "{error}"
        );
    }

    #[test]
    fn strict_receipt_does_not_fall_back_to_workspace_routing() {
        let fixture = worktree_fixture();
        fs::remove_file(
            fixture
                .state
                .run_root
                .join(crate::codebase::TRUSTED_CODEBASE_RECORD),
        )
        .expect("remove trusted routing record");

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("strict receipt must not trust the workspace fallback");

        assert!(
            error
                .to_string()
                .contains("result lifecycle record is unavailable"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_refuses_private_path_added_then_deleted_in_history() {
        let fixture = worktree_fixture();
        let private_paths = [
            ".specstory/history/session.md",
            ".deadreckon/forged.json",
            "target/debug/provider-output",
        ];
        for relative in private_paths {
            let private = fixture.state.working_dir.join(relative);
            fs::create_dir_all(private.parent().expect("private parent")).expect("private dir");
            fs::write(&private, "private provider transcript\n").expect("private evidence");
        }
        git_ok(
            &fixture.state.working_dir,
            &[
                "add",
                "-f",
                ".specstory/history/session.md",
                ".deadreckon/forged.json",
                "target/debug/provider-output",
            ],
        );
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "accidentally add private evidence"],
        );
        for relative in private_paths {
            fs::remove_file(fixture.state.working_dir.join(relative))
                .expect("remove private evidence");
        }
        git_ok(
            &fixture.state.working_dir,
            &[
                "add",
                "-u",
                ".specstory/history/session.md",
                ".deadreckon/forged.json",
                "target/debug/provider-output",
            ],
        );
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "remove private evidence"],
        );
        assert!(
            super::deliverable_git_delta_paths(
                &fixture.state.working_dir,
                fixture.authority.source_revision.as_deref().expect("base"),
                "HEAD",
            )
            .expect("final deliverable diff")
            .iter()
            .all(|path| !path.starts_with(".specstory"))
        );
        assert_eq!(
            super::non_deliverable_git_history_paths(
                &fixture.state.working_dir,
                fixture.authority.source_revision.as_deref().expect("base"),
                "HEAD",
            )
            .expect("history paths"),
            [
                ".deadreckon/forged.json",
                ".specstory/history/session.md",
                "target/debug/provider-output",
            ]
            .map(PathBuf::from)
            .to_vec()
        );

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("private evidence in history must not be sealed");

        assert!(
            error.to_string().contains("non-deliverable paths"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_refuses_private_paths_hidden_in_merged_history() {
        let fixture = worktree_fixture();
        let result_branch = git_stdout(
            &fixture.state.working_dir,
            &["symbolic-ref", "--short", "HEAD"],
        );
        git_ok(
            &fixture.state.working_dir,
            &["switch", "-c", "provider-private-side"],
        );
        let private = fixture
            .state
            .working_dir
            .join(".specstory/history/merged-session.md");
        fs::create_dir_all(private.parent().expect("private parent")).expect("private dir");
        fs::write(&private, "private side-branch evidence\n").expect("private evidence");
        git_ok(
            &fixture.state.working_dir,
            &["add", "-f", ".specstory/history/merged-session.md"],
        );
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "side branch adds private evidence"],
        );
        fs::remove_file(&private).expect("remove private evidence");
        git_ok(
            &fixture.state.working_dir,
            &["add", "-u", ".specstory/history/merged-session.md"],
        );
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "side branch removes private evidence"],
        );
        git_ok(&fixture.state.working_dir, &["switch", &result_branch]);
        git_ok(
            &fixture.state.working_dir,
            &[
                "merge",
                "--no-ff",
                "provider-private-side",
                "-m",
                "merge provider side branch",
            ],
        );

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("private evidence in merged history must not be sealed");
        assert!(
            error.to_string().contains("non-deliverable paths"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_validation_refuses_post_seal_uncommitted_deliverable() {
        let fixture = worktree_fixture();
        seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        fs::write(
            fixture.state.working_dir.join("post-seal.txt"),
            "not signed\n",
        )
        .expect("post-seal deliverable");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("dirty result must invalidate receipt");

        assert!(
            error
                .to_string()
                .contains("uncommitted deliverable changes"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_retains_the_signed_result_revision() {
        let fixture = worktree_fixture();
        let receipt = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        let digest = sha256_text("job-1");
        let retention_ref = format!(
            "refs/deadreckon/results/{}",
            digest.strip_prefix("sha256:").expect("digest prefix")
        );

        assert_eq!(
            git_stdout(&fixture.state.working_dir, &["rev-parse", &retention_ref],),
            receipt.result_revision.expect("result revision")
        );
    }

    #[test]
    fn achieved_without_evidence_backed_coverage_cannot_be_sealed() {
        let fixture = fixture();
        let mut unsupported = fixture.judgment.clone();
        unsupported.goal_coverage.clear();
        atomic_write_json(
            &fixture.state.run_root.join(super::SEMANTIC_JUDGMENT_JSON),
            &unsupported,
        )
        .expect("unsupported judgment");

        assert!(
            seal_completion_receipt(
                &fixture.paths,
                &fixture.state,
                &fixture.authority,
                &fixture.marker,
                &unsupported,
            )
            .expect_err("semantic parser invariant is repeated at receipt sealing")
            .to_string()
            .contains("lacks complete evidence-backed goal coverage")
        );
        assert!(!fixture.paths.job_receipt("job-1").exists());
    }

    #[test]
    fn strict_job_cannot_seal_an_uncontained_gate() {
        let fixture = fixture();
        let key = read_gate_key(&fixture.paths, &fixture.state.run_id).expect("key");
        let marker = write_native_acceptance_marker_with_results_and_key(
            &fixture.state.run_root,
            fixture.state.run_id.clone(),
            fixture.state.working_dir.clone(),
            fixture.marker.checks.clone(),
            &key,
            AcceptanceContainment::uncontained("none"),
        )
        .expect("uncontained marker");
        assert!(
            seal_completion_receipt(
                &fixture.paths,
                &fixture.state,
                &fixture.authority,
                &marker,
                &fixture.judgment,
            )
            .expect_err("strict jobs fail closed")
            .to_string()
            .contains("requires a contained deterministic gate")
        );
    }

    #[test]
    fn strict_job_cannot_seal_an_empty_deterministic_contract() {
        let fixture = fixture_with_contract("name: empty\nchecks: []\n");

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("an empty contract is not a deterministic completion key");

        assert!(error.to_string().contains("contains no checks"), "{error}");
        assert!(!fixture.paths.job_receipt("job-1").exists());
    }

    #[test]
    fn strict_job_cannot_seal_the_unknown_project_directory_noop() {
        let fixture = fixture_with_contract(
            "name: deadreckon detected unknown\nchecks:\n  - kind: file_exists\n    path: '{working_dir}'\n    must_pass: true\n",
        );

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("the pre-created working directory is not a definition of done");

        assert!(
            error
                .to_string()
                .contains("only proves that its pre-created working directory exists"),
            "{error}"
        );
        assert!(!fixture.paths.job_receipt("job-1").exists());
    }

    #[test]
    fn receipt_stays_valid_after_library_promotion_adds_its_manifest() {
        let mut fixture = fixture();
        seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        crate::promote_completed_run(&fixture.paths, &mut fixture.state).expect("promote");
        validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect("receipt survives owned manifest");
    }

    #[test]
    fn semantic_judgment_mutation_invalidates_receipt() {
        let fixture = fixture();
        seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        let mut changed = fixture.judgment.clone();
        changed.summary = "worker edited the proof".to_string();
        atomic_write_json(
            &fixture.state.run_root.join(super::SEMANTIC_JUDGMENT_JSON),
            &changed,
        )
        .expect("mutate");
        assert!(
            validate_completion_receipt(&fixture.paths, &fixture.state)
                .expect_err("mutation refused")
                .to_string()
                .contains("semantic judgment digest changed")
        );
    }

    #[test]
    fn backdated_result_mutation_invalidates_receipt() {
        let fixture = fixture();
        seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        fs::write(
            fixture.state.working_dir.join("result.txt"),
            "changed after sealing\n",
        )
        .expect("mutate result");
        assert!(
            validate_completion_receipt(&fixture.paths, &fixture.state)
                .expect_err("tree digest refused")
                .to_string()
                .contains("result tree digest changed")
        );
    }

    // ---- G7 characterization: the strict path's exact error strings ----
    //
    // These tests pin the fail-first monolith's error messages byte-for-byte
    // so the fact-collecting `audit_completion_receipt(...).into_result()`
    // refactor is provably behavior-preserving. Do not loosen them to
    // `contains`; the whole point is exact equality.

    fn sealed_fixture() -> (Fixture, super::CompletionReceipt) {
        let fixture = fixture();
        let receipt = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        (fixture, receipt)
    }

    #[test]
    fn characterization_bad_marker_digest_error_is_exact() {
        let (fixture, receipt) = sealed_fixture();
        let marker_path = crate::marker_path_for_run_root(&fixture.state.run_root);
        let mut bytes = fs::read(&marker_path).expect("marker bytes");
        bytes.push(b'\n');
        fs::write(&marker_path, bytes).expect("tamper marker");
        let found = sha256_file(&marker_path).expect("tampered marker digest");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("marker tamper refused");

        assert_eq!(
            error.to_string(),
            format!(
                "invalid input: completion receipt for job-1 is invalid: deterministic marker digest changed (expected {}, found {found}); try: deadreckon verdict job-1",
                receipt.deterministic_marker_sha256
            )
        );
    }

    #[test]
    fn characterization_mutated_result_fails_at_the_boundary_observation_first() {
        // A mutated deliverable is caught by the authenticated sandbox boundary
        // observation BEFORE the receipt's own result-tree digest check runs;
        // this pins that ordering and its exact message.
        let (fixture, receipt) = sealed_fixture();
        fs::write(
            fixture.state.working_dir.join("result.txt"),
            "changed after sealing\n",
        )
        .expect("mutate result");
        let found = crate::sandbox_boundary_result_tree_sha256(&fixture.state)
            .expect("tampered tree digest");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("result tamper refused");

        assert_eq!(
            error.to_string(),
            format!(
                "invalid input: sandbox boundary observation for job-1 is invalid: result tree digest changed (expected {}, found {found})",
                receipt.result_tree_sha256
            )
        );
    }

    #[test]
    fn characterization_bad_result_tree_digest_error_is_exact() {
        let (fixture, receipt) = sealed_fixture();
        let mut tampered = receipt.clone();
        tampered.result_tree_sha256 = format!("sha256:{}", "0".repeat(64));
        atomic_write_json(&fixture.paths.job_receipt("job-1"), &tampered)
            .expect("tampered receipt");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("result-tree tamper refused");

        assert_eq!(
            error.to_string(),
            format!(
                "invalid input: completion receipt for job-1 is invalid: result tree digest changed (expected sha256:{}, found {}); try: deadreckon verdict job-1",
                "0".repeat(64),
                receipt.result_tree_sha256
            )
        );
    }

    #[test]
    fn characterization_bad_receipt_signature_error_is_exact() {
        let (fixture, receipt) = sealed_fixture();
        let mut tampered = receipt;
        tampered.signature = "0".repeat(64);
        atomic_write_json(&fixture.paths.job_receipt("job-1"), &tampered)
            .expect("tampered receipt");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("forged signature refused");

        assert_eq!(
            error.to_string(),
            "invalid input: completion receipt for job-1 is invalid: receipt signature verification failed; try: deadreckon verdict job-1"
        );
    }

    #[test]
    fn characterization_bad_receipt_identity_error_is_exact() {
        let (fixture, receipt) = sealed_fixture();
        let mut tampered = receipt;
        tampered.run_id = RunId("other-run".to_string());
        atomic_write_json(&fixture.paths.job_receipt("job-1"), &tampered)
            .expect("tampered receipt");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("identity tamper refused");

        assert_eq!(
            error.to_string(),
            "invalid input: completion receipt for job-1 is invalid: receipt identity does not match the requested run; try: deadreckon verdict job-1"
        );
    }

    #[test]
    fn characterization_bad_receipt_provenance_error_is_exact() {
        let (fixture, receipt) = sealed_fixture();
        let mut tampered = receipt;
        tampered.stop_reason = deadreckon_protocol::StopReason::SpendCap;
        atomic_write_json(&fixture.paths.job_receipt("job-1"), &tampered)
            .expect("tampered receipt");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("provenance tamper refused");

        assert_eq!(
            error.to_string(),
            "invalid input: completion receipt for job-1 is invalid: receipt is not a supervisor-issued two-key verified result; try: deadreckon verdict job-1"
        );
    }

    #[test]
    fn characterization_bad_authority_inputs_error_is_exact() {
        let (fixture, _receipt) = sealed_fixture();
        let launch_path = fixture.paths.job_launch_plan("job-1");
        let mut launch = fs::read(&launch_path).expect("launch plan");
        launch.push(b'\n');
        fs::write(&launch_path, launch).expect("tamper launch plan");
        let found = sha256_file(&launch_path).expect("tampered launch digest");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("launch plan tamper refused");

        assert_eq!(
            error.to_string(),
            format!(
                "invalid input: completion receipt for job-1 is invalid: approved launch plan digest changed (expected {}, found {found}); try: deadreckon verdict job-1",
                fixture.authority.launch_plan_sha256
            )
        );
    }

    #[test]
    fn characterization_bad_attempt_identity_error_is_exact() {
        let (fixture, receipt) = sealed_fixture();
        let mut tampered = receipt;
        tampered.attempt += 1;
        atomic_write_json(&fixture.paths.job_receipt("job-1"), &tampered)
            .expect("tampered receipt");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("attempt tamper refused");

        assert_eq!(
            error.to_string(),
            "invalid input: completion receipt for job-1 is invalid: receipt attempt identity does not match its authenticated sandbox observation; try: deadreckon verdict job-1"
        );
    }

    #[test]
    fn characterization_bad_execution_evidence_error_is_exact() {
        let (fixture, receipt) = sealed_fixture();
        let mut tampered = receipt;
        tampered.execution_evidence = Some(deadreckon_protocol::CompletionExecutionEvidence {
            ordered_candidate_manifest_sha256: sha256_text("forged"),
            candidate_application_events_sha256: None,
            chain_hook_events_sha256: None,
        });
        atomic_write_json(&fixture.paths.job_receipt("job-1"), &tampered)
            .expect("tampered receipt");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("execution evidence tamper refused");

        assert_eq!(
            error.to_string(),
            "invalid input: completion receipt for job-1 is invalid: receipt execution ledgers changed after verified completion; try: deadreckon verdict job-1"
        );
    }

    #[test]
    fn characterization_bad_authority_digest_error_is_exact() {
        let (fixture, receipt) = sealed_fixture();
        let mut tampered = receipt.clone();
        tampered.authority_sha256 = format!("sha256:{}", "1".repeat(64));
        atomic_write_json(&fixture.paths.job_receipt("job-1"), &tampered)
            .expect("tampered receipt");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("authority digest tamper refused");

        assert_eq!(
            error.to_string(),
            format!(
                "invalid input: completion receipt for job-1 is invalid: authority digest changed (expected sha256:{}, found {}); try: deadreckon verdict job-1",
                "1".repeat(64),
                receipt.authority_sha256
            )
        );
    }

    #[test]
    fn characterization_bad_semantic_judgment_digest_error_is_exact() {
        let (fixture, receipt) = sealed_fixture();
        let mut tampered = receipt.clone();
        tampered.semantic_judgment_sha256 = format!("sha256:{}", "2".repeat(64));
        atomic_write_json(&fixture.paths.job_receipt("job-1"), &tampered)
            .expect("tampered receipt");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("judgment digest tamper refused");

        assert_eq!(
            error.to_string(),
            format!(
                "invalid input: completion receipt for job-1 is invalid: semantic judgment digest changed (expected sha256:{}, found {}); try: deadreckon verdict job-1",
                "2".repeat(64),
                receipt.semantic_judgment_sha256
            )
        );
    }

    #[test]
    fn characterization_unachieved_judgment_error_is_exact() {
        let (fixture, receipt) = sealed_fixture();
        let judgment_path = fixture.state.run_root.join(super::SEMANTIC_JUDGMENT_JSON);
        let mut judgment = fixture.judgment.clone();
        judgment.decision = SemanticDecision::Revise;
        atomic_write_json(&judgment_path, &judgment).expect("rewrite judgment");
        // Keep the digest fact green so the failure lands on the judgment
        // content itself.
        let mut tampered = receipt;
        tampered.semantic_judgment_sha256 =
            sha256_file(&judgment_path).expect("new judgment digest");
        atomic_write_json(&fixture.paths.job_receipt("job-1"), &tampered)
            .expect("tampered receipt");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("unachieved judgment refused");

        assert_eq!(
            error.to_string(),
            "invalid input: completion receipt for job-1 is invalid: semantic judgment no longer records achieved for this job; try: deadreckon verdict job-1"
        );
    }

    // ---- G7: strict short-circuit and read-only properties ----

    #[test]
    fn strict_validation_short_circuits_at_the_first_failing_fact() {
        let (fixture, receipt) = sealed_fixture();
        let mut tampered = receipt;
        tampered.run_id = RunId("other-run".to_string());
        atomic_write_json(&fixture.paths.job_receipt("job-1"), &tampered)
            .expect("tampered receipt");

        // The strict (fail-fast) walk stops dead at the first failure: no
        // later checks execute, so a failure at fact 2 never reads gate-key
        // material or hashes the result tree.
        let strict = super::audit_completion_receipt_inner(&fixture.paths, &fixture.state, true);
        let names: Vec<&str> = strict.facts.iter().map(|fact| fact.name.as_str()).collect();
        assert_eq!(names, ["receipt_document", "receipt_identity"]);
        assert!(!strict.facts[1].pass);

        // The collecting audit keeps walking yet collapses to the same error.
        let full = super::audit_completion_receipt(&fixture.paths, &fixture.state);
        assert!(full.facts.len() > 2, "{:#?}", full.facts);
        assert_eq!(
            strict.into_result().expect_err("strict error").to_string(),
            full.into_result().expect_err("audit error").to_string()
        );
    }

    #[cfg(unix)]
    #[test]
    fn strict_failure_paths_never_read_the_gate_key() {
        use std::os::unix::fs::PermissionsExt;

        let (fixture, receipt) = sealed_fixture();
        let mut tampered = receipt;
        tampered.run_id = RunId("other-run".to_string());
        atomic_write_json(&fixture.paths.job_receipt("job-1"), &tampered)
            .expect("tampered receipt");
        let key_store = crate::gate::gate_key_path(&fixture.paths, "job-1")
            .parent()
            .expect("key store")
            .to_path_buf();
        fs::set_permissions(&key_store, fs::Permissions::from_mode(0o000)).expect("seal key store");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state);
        fs::set_permissions(&key_store, fs::Permissions::from_mode(0o700))
            .expect("restore key store");

        assert_eq!(
            error.expect_err("identity tamper refused").to_string(),
            "invalid input: completion receipt for job-1 is invalid: receipt identity does not match the requested run; try: deadreckon verdict job-1",
            "the pre-key failure must be decided without touching the key store"
        );
    }

    fn tree_snapshot(root: &std::path::Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
        let mut entries = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(read_dir) = fs::read_dir(&dir) else {
                continue;
            };
            for entry in read_dir {
                let entry = entry.expect("dir entry");
                let path = entry.path();
                let metadata = entry.metadata().expect("metadata");
                if metadata.is_dir() {
                    stack.push(path.clone());
                }
                entries.push((path, metadata.len(), metadata.modified().expect("mtime")));
            }
        }
        entries.sort();
        entries
    }

    #[test]
    fn audit_is_read_only_over_the_run_root_and_home() {
        let (fixture, receipt) = sealed_fixture();
        let mut tampered = receipt;
        tampered.attempt += 1;
        atomic_write_json(&fixture.paths.job_receipt("job-1"), &tampered)
            .expect("tampered receipt");
        let home = fixture.paths.home().to_path_buf();
        let before_home = tree_snapshot(&home);
        let before_run = tree_snapshot(&fixture.state.run_root);

        let audit = super::audit_completion_receipt(&fixture.paths, &fixture.state);
        assert!(!audit.passed());
        let strict = validate_completion_receipt(&fixture.paths, &fixture.state);
        assert!(strict.is_err());

        assert_eq!(tree_snapshot(&home), before_home, "home tree mutated");
        assert_eq!(
            tree_snapshot(&fixture.state.run_root),
            before_run,
            "run root mutated"
        );
    }

    #[cfg(unix)]
    #[test]
    fn audit_completes_gracefully_when_the_gate_key_store_is_unreadable() {
        use std::os::unix::fs::PermissionsExt;

        let (fixture, _receipt) = sealed_fixture();
        let key_store = crate::gate::gate_key_path(&fixture.paths, "job-1")
            .parent()
            .expect("key store")
            .to_path_buf();
        fs::set_permissions(&key_store, fs::Permissions::from_mode(0o000)).expect("seal key store");

        let audit = super::audit_completion_receipt(&fixture.paths, &fixture.state);
        fs::set_permissions(&key_store, fs::Permissions::from_mode(0o700))
            .expect("restore key store");

        assert!(!audit.passed());
        let last = audit.facts.last().expect("facts collected");
        assert_eq!(last.name, "receipt_signature");
        assert!(!last.pass, "unreadable key store fails the signature fact");
    }

    // ---- G7: fact-collecting audit ----

    #[test]
    fn audit_on_a_sealed_receipt_passes_every_fact_and_matches_strict() {
        let (fixture, receipt) = sealed_fixture();

        let audit = super::audit_completion_receipt(&fixture.paths, &fixture.state);

        assert!(audit.passed(), "{:#?}", audit.facts);
        assert!(
            audit.facts.iter().all(|fact| fact.pass),
            "{:#?}",
            audit.facts
        );
        let names: Vec<&str> = audit.facts.iter().map(|fact| fact.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "receipt_document",
                "receipt_identity",
                "receipt_provenance",
                "authority_document",
                "authority_inputs",
                "result_boundary",
                "sandbox_observation",
                "attempt_identity",
                "sandbox_observation_digest",
                "execution_evidence",
                "authority_digest",
                "launch_plan_digest",
                "deterministic_marker_digest",
                "semantic_judgment_digest",
                "judgment_achieved",
                "marker_signature",
                "result_projection_digest",
                "result_tree_digest",
                "receipt_signature",
            ]
        );
        assert_eq!(audit.into_result().expect("strict result"), receipt);
    }

    #[test]
    fn audit_keeps_collecting_after_a_failed_digest_and_reproduces_the_strict_error() {
        let (fixture, _receipt) = sealed_fixture();
        let marker_path = crate::marker_path_for_run_root(&fixture.state.run_root);
        let mut bytes = fs::read(&marker_path).expect("marker bytes");
        bytes.push(b'\n');
        fs::write(&marker_path, bytes).expect("tamper marker");

        let audit = super::audit_completion_receipt(&fixture.paths, &fixture.state);

        let fact = |name: &str| {
            audit
                .facts
                .iter()
                .find(|fact| fact.name == name)
                .unwrap_or_else(|| panic!("missing fact {name}"))
        };
        assert!(!fact("deterministic_marker_digest").pass);
        // Later independent facts are still collected instead of short-circuited.
        assert!(fact("result_tree_digest").pass);
        assert!(fact("receipt_signature").pass);
        assert!(!audit.passed());

        let strict = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("strict path refuses");
        assert_eq!(
            audit
                .into_result()
                .expect_err("audit collapses to the strict error")
                .to_string(),
            strict.to_string()
        );
    }

    #[test]
    fn audit_without_a_receipt_reports_a_single_failed_document_fact() {
        let fixture = fixture();

        let audit = super::audit_completion_receipt(&fixture.paths, &fixture.state);

        assert_eq!(audit.facts.len(), 1, "{:#?}", audit.facts);
        assert_eq!(audit.facts[0].name, "receipt_document");
        assert!(!audit.facts[0].pass);
        audit.into_result().expect_err("no receipt to validate");
    }
}
