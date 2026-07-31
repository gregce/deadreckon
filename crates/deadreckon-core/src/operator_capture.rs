//! Trusted, append-only operator capture histories.
//!
//! Capture evidence is deliberately separate from Job lifecycle truth. The
//! trusted controller authenticates an immutable binding, every append, and a
//! final receipt with the protected per-Job gate key.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use deadreckon_protocol::{
    JobAuthority, JobId, OperatorCaptureBinding, OperatorCaptureCompletionLineage,
    OperatorCaptureEvent, OperatorCaptureEventKind, OperatorCaptureEventSequence,
    OperatorCapturePhase, OperatorCaptureProvenance, OperatorCaptureReceipt,
    OperatorCaptureSchemaVersion, OperatorCaptureSource, OperatorCaptureStatus,
};
use fs2::FileExt as _;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::gate::read_gate_key;
use crate::job::load_job;
use crate::paths::DeadreckonPaths;

pub const OPERATOR_CAPTURE_BINDING_JSON: &str = "binding.json";
pub const OPERATOR_CAPTURE_EVENTS_JSONL: &str = "capture-events.jsonl";
pub const OPERATOR_CAPTURE_RECEIPT_JSON: &str = "capture-receipt.json";
const OPERATOR_CAPTURE_HISTORY_HEAD_JSON: &str = "capture-history-head.json";

const OPERATOR_CAPTURE_LOCK: &str = ".control.lock";
const MAX_CAPTURE_HISTORY_BYTES: u64 = 64 * 1024 * 1024;
const MAX_CAPTURE_CONTROL_BYTES: u64 = 1024 * 1024;
const BINDING_MAGIC: &[u8] = b"deadreckon.operator-capture-binding.v1\0";
const EVENT_MAGIC: &[u8] = b"deadreckon.operator-capture-event.v1\0";
const HISTORY_HEAD_MAGIC: &[u8] = b"deadreckon.operator-capture-history-head.v1\0";
const RECEIPT_MAGIC: &[u8] = b"deadreckon.operator-capture-receipt.v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UncommittedTailPolicy {
    Reject,
    RecoverUnderSessionLock,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperatorCaptureHistoryHead {
    schema_version: OperatorCaptureSchemaVersion,
    job_id: JobId,
    session_id: String,
    binding_sha256: String,
    sequence: OperatorCaptureEventSequence,
    event_sha256: String,
    history_bytes: u64,
    signature: String,
}

#[derive(Debug)]
struct ValidatedHistory {
    history: OperatorCaptureHistory,
    head: Option<OperatorCaptureHistoryHead>,
    pending_head_index: Option<usize>,
}

/// Trusted input for one capture append. The caller must preserve the same
/// timestamp when retrying an event ID so exact duplicate bytes are possible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorCaptureEventDraft {
    pub event_id: String,
    pub causation_id: String,
    pub timestamp: DateTime<Utc>,
    pub phase: OperatorCapturePhase,
    pub kind: OperatorCaptureEventKind,
    pub provenance: OperatorCaptureProvenance,
    pub source: OperatorCaptureSource,
    pub subject: String,
    pub content_sha256: String,
    pub content_bytes: u64,
}

/// Authenticated parsed rows and their exact persisted bytes.
#[derive(Debug, Clone)]
pub struct OperatorCaptureHistory {
    events: Vec<OperatorCaptureEvent>,
    raw_lines: Vec<Vec<u8>>,
}

impl OperatorCaptureHistory {
    pub fn events(&self) -> &[OperatorCaptureEvent] {
        &self.events
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// Authenticate and atomically persist the immutable capture binding.
///
/// Repeating the same binding is idempotent. A different replacement for the
/// same Job and session is refused.
pub fn write_operator_capture_binding(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
) -> Result<OperatorCaptureBinding> {
    validate_binding_fields(paths, binding)?;
    let key = read_gate_key(paths, binding.job_id.as_ref())?;
    let mut sealed = binding.clone();
    sealed.schema_version = OperatorCaptureSchemaVersion::CURRENT;
    sealed.signature.clear();
    sealed.signature = sign_binding(&sealed, &key)?;
    let path = paths.operator_capture_binding(binding.job_id.as_ref(), &binding.session_id);

    if path.exists() {
        let existing =
            load_operator_capture_binding(paths, binding.job_id.as_ref(), &binding.session_id)?;
        return if existing == sealed {
            Ok(existing)
        } else {
            Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "immutable binding already exists with different bytes",
            ))
        };
    }

    match persist_new_json(&path, &sealed) {
        Ok(()) => Ok(sealed),
        Err(DeadreckonError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::AlreadyExists =>
        {
            let existing =
                load_operator_capture_binding(paths, binding.job_id.as_ref(), &binding.session_id)?;
            if existing == sealed {
                Ok(existing)
            } else {
                Err(capture_error(
                    binding.job_id.as_ref(),
                    &binding.session_id,
                    "immutable binding was concurrently created with different bytes",
                ))
            }
        }
        Err(error) => Err(error),
    }
}

/// Load and authenticate one immutable capture binding.
pub fn load_operator_capture_binding(
    paths: &DeadreckonPaths,
    job_id: &str,
    session_id: &str,
) -> Result<OperatorCaptureBinding> {
    let path = paths.operator_capture_binding(job_id, session_id);
    let raw = read_stable_regular_file(&path, job_id, session_id, "binding")?;
    let binding: OperatorCaptureBinding = serde_json::from_slice(&raw).with_json_path(&path)?;
    if binding.job_id.as_ref() != job_id || binding.session_id != session_id {
        return Err(capture_error(
            job_id,
            session_id,
            "binding contains a foreign Job or session identity",
        ));
    }
    validate_binding_fields(paths, &binding)?;
    let key = read_gate_key(paths, job_id)?;
    verify_binding_signature(&binding, &key)?;
    Ok(binding)
}

/// Append one authenticated capture fact under the session control lock.
pub fn append_operator_capture_event(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
    draft: &OperatorCaptureEventDraft,
) -> Result<OperatorCaptureEvent> {
    validate_draft(binding, draft)?;
    let session_dir = paths.operator_capture_dir(binding.job_id.as_ref(), &binding.session_id);
    fs::create_dir_all(&session_dir).with_path(&session_dir)?;
    let lock_path = session_dir.join(OPERATOR_CAPTURE_LOCK);
    let lock = open_control_lock(&lock_path)?;
    lock.lock_exclusive().with_path(&lock_path)?;

    let persisted =
        load_operator_capture_binding(paths, binding.job_id.as_ref(), &binding.session_id)?;
    if &persisted != binding {
        return Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "provided binding does not equal the authenticated persisted binding",
        ));
    }
    let key = read_gate_key(paths, binding.job_id.as_ref())?;
    let binding_sha256 = operator_capture_binding_sha256(binding)?;
    let events_path = paths.operator_capture_events(binding.job_id.as_ref(), &binding.session_id);
    let validated = read_and_validate_history_with_tail_policy(
        &events_path,
        binding,
        &binding_sha256,
        &key,
        UncommittedTailPolicy::RecoverUnderSessionLock,
    )?;
    if let Some(index) = validated.pending_head_index {
        let existing = &validated.history.events[index];
        let raw = &validated.history.raw_lines[index];
        let candidate = build_signed_event(
            binding,
            &binding_sha256,
            existing.sequence,
            existing.previous_event_sha256.clone(),
            draft,
            &key,
        )?;
        if encode_json(&events_path, &candidate)? != *raw {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "capture history has one committed row without a durable head; only an exact event retry may recover it",
            ));
        }
        let history_bytes = committed_history_bytes(&validated.history, index + 1, binding)?;
        persist_next_history_head(
            paths,
            binding,
            &binding_sha256,
            &key,
            validated.head.as_ref(),
            existing,
            raw,
            history_bytes,
        )?;
        sync_history_and_head(paths, binding)?;
        return Ok(existing.clone());
    }
    let prior_head = validated.head;
    let history = validated.history;

    if let Some((existing, raw)) = history
        .events
        .iter()
        .zip(&history.raw_lines)
        .find(|(event, _)| event.event_id == draft.event_id)
    {
        let candidate = build_signed_event(
            binding,
            &binding_sha256,
            existing.sequence,
            existing.previous_event_sha256.clone(),
            draft,
            &key,
        )?;
        let candidate_raw = encode_json(&events_path, &candidate)?;
        return if candidate_raw == *raw {
            sync_history_and_head(paths, binding)?;
            Ok(existing.clone())
        } else {
            Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                &format!(
                    "event id {} already exists with different bytes",
                    draft.event_id
                ),
            ))
        };
    }

    if paths
        .operator_capture_receipt(binding.job_id.as_ref(), &binding.session_id)
        .exists()
    {
        return Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "capture history is sealed and cannot accept another event",
        ));
    }
    validate_next_lifecycle(binding, &history, draft)?;
    let next = u64::try_from(history.events.len())
        .ok()
        .and_then(|count| count.checked_add(1))
        .and_then(OperatorCaptureEventSequence::new)
        .ok_or_else(|| {
            capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "capture event sequence overflowed",
            )
        })?;
    let previous_event_sha256 = history
        .raw_lines
        .last()
        .map(|line| sha256_bytes(line.as_slice()));
    let event = build_signed_event(
        binding,
        &binding_sha256,
        next,
        previous_event_sha256,
        draft,
        &key,
    )?;
    let raw = encode_json(&events_path, &event)?;
    let history_bytes = append_synced_json_line(&events_path, &event)?;
    persist_next_history_head(
        paths,
        binding,
        &binding_sha256,
        &key,
        prior_head.as_ref(),
        &event,
        &raw,
        history_bytes,
    )?;
    Ok(event)
}

/// Read and authenticate a complete capture history.
pub fn read_operator_capture_history(
    paths: &DeadreckonPaths,
    job_id: &str,
    session_id: &str,
) -> Result<OperatorCaptureHistory> {
    let binding = load_operator_capture_binding(paths, job_id, session_id)?;
    validate_operator_capture_history(paths, &binding)
}

/// Validate ordering, binding, hash chaining, and every event HMAC.
pub fn validate_operator_capture_history(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
) -> Result<OperatorCaptureHistory> {
    let persisted =
        load_operator_capture_binding(paths, binding.job_id.as_ref(), &binding.session_id)?;
    if persisted != *binding {
        return Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "provided binding does not equal the authenticated persisted binding",
        ));
    }
    let key = read_gate_key(paths, binding.job_id.as_ref())?;
    let binding_sha256 = operator_capture_binding_sha256(binding)?;
    read_and_validate_history(
        &paths.operator_capture_events(binding.job_id.as_ref(), &binding.session_id),
        binding,
        &binding_sha256,
        &key,
    )
}

/// Seal the current history and a result digest into an immutable receipt.
pub fn seal_operator_capture_receipt(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
    issued_at: DateTime<Utc>,
    result_sha256: &str,
    result_bytes: u64,
    status: OperatorCaptureStatus,
    completion_lineage: Option<OperatorCaptureCompletionLineage>,
) -> Result<OperatorCaptureReceipt> {
    if let Some(lineage) = &completion_lineage {
        validate_completion_lineage(lineage, binding.job_id.as_ref(), &binding.session_id)?;
    }
    require_sha256(
        result_sha256,
        binding.job_id.as_ref(),
        &binding.session_id,
        "result",
    )?;
    if status == OperatorCaptureStatus::Passed
        && (!binding.pass_capable || completion_lineage.is_none())
    {
        return Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "a passed receipt requires a pass-capable binding and completion lineage",
        ));
    }
    let session_dir = paths.operator_capture_dir(binding.job_id.as_ref(), &binding.session_id);
    fs::create_dir_all(&session_dir).with_path(&session_dir)?;
    let lock_path = session_dir.join(OPERATOR_CAPTURE_LOCK);
    let lock = open_control_lock(&lock_path)?;
    lock.lock_exclusive().with_path(&lock_path)?;
    let persisted =
        load_operator_capture_binding(paths, binding.job_id.as_ref(), &binding.session_id)?;
    if persisted != *binding {
        return Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "provided binding does not equal the authenticated persisted binding",
        ));
    }
    let key = read_gate_key(paths, binding.job_id.as_ref())?;
    let binding_sha256 = operator_capture_binding_sha256(binding)?;
    let events_path = paths.operator_capture_events(binding.job_id.as_ref(), &binding.session_id);
    let mut validated = read_and_validate_history_with_tail_policy(
        &events_path,
        binding,
        &binding_sha256,
        &key,
        UncommittedTailPolicy::RecoverUnderSessionLock,
    )?;
    if let Some(index) = validated.pending_head_index {
        let existing = &validated.history.events[index];
        let raw = &validated.history.raw_lines[index];
        if existing.kind != OperatorCaptureEventKind::ResultFinalized {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "capture history has one committed row without a durable head; seal may recover only its exact ResultFinalized retry",
            ));
        }
        let retry_draft = OperatorCaptureEventDraft {
            event_id: format!("finalize:{}", result_sha256.trim_start_matches("sha256:")),
            causation_id: format!("{status:?}"),
            timestamp: existing.timestamp,
            phase: OperatorCapturePhase::Finalized,
            kind: OperatorCaptureEventKind::ResultFinalized,
            provenance: OperatorCaptureProvenance::TrustedSupervisor,
            source: OperatorCaptureSource::ResultEnvelope,
            subject: "result".to_string(),
            content_sha256: result_sha256.to_string(),
            content_bytes: result_bytes,
        };
        let candidate = build_signed_event(
            binding,
            &binding_sha256,
            existing.sequence,
            existing.previous_event_sha256.clone(),
            &retry_draft,
            &key,
        )?;
        if encode_json(&events_path, &candidate)? != *raw {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "uncheckpointed ResultFinalized row does not match this exact seal retry",
            ));
        }
        let history_bytes = committed_history_bytes(&validated.history, index + 1, binding)?;
        let recovered = persist_next_history_head(
            paths,
            binding,
            &binding_sha256,
            &key,
            validated.head.as_ref(),
            existing,
            raw,
            history_bytes,
        )?;
        validated.head = Some(recovered);
        validated.pending_head_index = None;
        sync_history_and_head(paths, binding)?;
    }
    let mut history = validated.history;
    let history_head = validated.head;
    let receipt_path = paths.operator_capture_receipt(binding.job_id.as_ref(), &binding.session_id);
    if receipt_path.exists() {
        sync_history_and_head(paths, binding)?;
        let existing = validate_operator_capture_receipt(paths, binding)?;
        return if existing.result_sha256 == result_sha256
            && existing.result_bytes == result_bytes
            && existing.status == status
            && existing.completion_lineage == completion_lineage
        {
            Ok(existing)
        } else {
            Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "immutable capture receipt already exists for a different result or status",
            ))
        };
    }
    if status == OperatorCaptureStatus::Passed {
        validate_pass_coverage(binding, &history)?;
    }
    let final_event = history
        .events
        .last()
        .filter(|event| event.kind == OperatorCaptureEventKind::ResultFinalized);
    if let Some(final_event) = final_event {
        if final_event.content_sha256 != result_sha256
            || final_event.content_bytes != result_bytes
            || final_event.causation_id != format!("{status:?}")
        {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "existing ResultFinalized event belongs to a different result or status",
            ));
        }
        sync_history_and_head(paths, binding)?;
    } else {
        validate_next_lifecycle(
            binding,
            &history,
            &OperatorCaptureEventDraft {
                event_id: format!("finalize:{}", result_sha256.trim_start_matches("sha256:")),
                causation_id: format!("{status:?}"),
                timestamp: issued_at,
                phase: OperatorCapturePhase::Finalized,
                kind: OperatorCaptureEventKind::ResultFinalized,
                provenance: OperatorCaptureProvenance::TrustedSupervisor,
                source: OperatorCaptureSource::ResultEnvelope,
                subject: "result".to_string(),
                content_sha256: result_sha256.to_string(),
                content_bytes: result_bytes,
            },
        )?;
        let next = u64::try_from(history.events.len())
            .ok()
            .and_then(|count| count.checked_add(1))
            .and_then(OperatorCaptureEventSequence::new)
            .ok_or_else(|| {
                capture_error(
                    binding.job_id.as_ref(),
                    &binding.session_id,
                    "capture event sequence overflowed",
                )
            })?;
        let previous_event_sha256 = history
            .raw_lines
            .last()
            .map(|line| sha256_bytes(line.as_slice()));
        let draft = OperatorCaptureEventDraft {
            event_id: format!("finalize:{}", result_sha256.trim_start_matches("sha256:")),
            causation_id: format!("{status:?}"),
            timestamp: issued_at,
            phase: OperatorCapturePhase::Finalized,
            kind: OperatorCaptureEventKind::ResultFinalized,
            provenance: OperatorCaptureProvenance::TrustedSupervisor,
            source: OperatorCaptureSource::ResultEnvelope,
            subject: "result".to_string(),
            content_sha256: result_sha256.to_string(),
            content_bytes: result_bytes,
        };
        let event = build_signed_event(
            binding,
            &binding_sha256,
            next,
            previous_event_sha256,
            &draft,
            &key,
        )?;
        let raw = encode_json(&events_path, &event)?;
        let history_bytes = append_synced_json_line(&events_path, &event)?;
        persist_next_history_head(
            paths,
            binding,
            &binding_sha256,
            &key,
            history_head.as_ref(),
            &event,
            &raw,
            history_bytes,
        )?;
        history = read_and_validate_history(&events_path, binding, &binding_sha256, &key)?;
    }
    let final_raw = history.raw_lines.last().ok_or_else(|| {
        capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "cannot seal a receipt for an empty capture history",
        )
    })?;
    let event_count = u64::try_from(history.events.len()).map_err(|_| {
        capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "capture event count overflowed",
        )
    })?;
    let mut receipt = OperatorCaptureReceipt {
        schema_version: OperatorCaptureSchemaVersion::CURRENT,
        job_id: binding.job_id.clone(),
        session_id: binding.session_id.clone(),
        binding_sha256: operator_capture_binding_sha256(binding)?,
        issued_at: history
            .events
            .last()
            .map(|event| event.timestamp)
            .unwrap_or(issued_at),
        event_count,
        final_event_sha256: sha256_bytes(final_raw),
        result_sha256: result_sha256.to_string(),
        result_bytes,
        completion_lineage,
        status,
        signature: String::new(),
    };
    let key = read_gate_key(paths, binding.job_id.as_ref())?;
    receipt.signature = sign_receipt(&receipt, &key)?;
    match persist_new_json(&receipt_path, &receipt) {
        Ok(()) => Ok(receipt),
        Err(DeadreckonError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::AlreadyExists =>
        {
            let existing = validate_operator_capture_receipt(paths, binding)?;
            if existing == receipt {
                Ok(existing)
            } else {
                Err(capture_error(
                    binding.job_id.as_ref(),
                    &binding.session_id,
                    "capture receipt was concurrently created with different bytes",
                ))
            }
        }
        Err(error) => Err(error),
    }
}

/// Authenticate the receipt and re-bind it to the complete current history.
pub fn validate_operator_capture_receipt(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
) -> Result<OperatorCaptureReceipt> {
    let history = validate_operator_capture_history(paths, binding)?;
    let path = paths.operator_capture_receipt(binding.job_id.as_ref(), &binding.session_id);
    let raw = read_stable_regular_file(
        &path,
        binding.job_id.as_ref(),
        &binding.session_id,
        "capture receipt",
    )?;
    let receipt: OperatorCaptureReceipt = serde_json::from_slice(&raw).with_json_path(&path)?;
    let expected_count = u64::try_from(history.events.len()).map_err(|_| {
        capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "capture event count overflowed",
        )
    })?;
    let expected_final = history
        .raw_lines
        .last()
        .map(|line| sha256_bytes(line))
        .ok_or_else(|| {
            capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "capture receipt cannot validate an empty history",
            )
        })?;
    let finalized = history.events.last().ok_or_else(|| {
        capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "capture receipt cannot validate an empty history",
        )
    })?;
    if receipt.job_id != binding.job_id
        || receipt.session_id != binding.session_id
        || receipt.binding_sha256 != operator_capture_binding_sha256(binding)?
        || receipt.event_count != expected_count
        || receipt.final_event_sha256 != expected_final
        || finalized.kind != OperatorCaptureEventKind::ResultFinalized
        || finalized.source != OperatorCaptureSource::ResultEnvelope
        || finalized.content_sha256 != receipt.result_sha256
        || finalized.content_bytes != receipt.result_bytes
        || finalized.causation_id != format!("{:?}", receipt.status)
        || (receipt.status == OperatorCaptureStatus::Passed && receipt.completion_lineage.is_none())
        || (receipt.status != OperatorCaptureStatus::Passed && receipt.completion_lineage.is_some())
    {
        return Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "capture receipt does not match its binding and complete event history",
        ));
    }
    require_sha256(
        &receipt.result_sha256,
        binding.job_id.as_ref(),
        &binding.session_id,
        "receipt result",
    )?;
    if let Some(lineage) = &receipt.completion_lineage {
        validate_completion_lineage(lineage, binding.job_id.as_ref(), &binding.session_id)?;
    }
    let key = read_gate_key(paths, binding.job_id.as_ref())?;
    verify_receipt_signature(&receipt, &key)?;
    Ok(receipt)
}

fn validate_completion_lineage(
    lineage: &OperatorCaptureCompletionLineage,
    job_id: &str,
    session_id: &str,
) -> Result<()> {
    for (label, digest) in [
        (
            "completion receipt",
            lineage.completion_receipt_sha256.as_str(),
        ),
        ("completion authority", lineage.authority_sha256.as_str()),
        ("completion contract", lineage.contract_sha256.as_str()),
        (
            "completion effective policy",
            lineage.effective_policy_sha256.as_str(),
        ),
        (
            "completion launch plan",
            lineage.launch_plan_sha256.as_str(),
        ),
        (
            "completion source tree",
            lineage.source_tree_sha256.as_str(),
        ),
        (
            "completion result tree",
            lineage.result_tree_sha256.as_str(),
        ),
    ] {
        require_sha256(digest, job_id, session_id, label)?;
    }
    for (label, revision) in [
        (
            "completion source revision",
            lineage.source_revision.as_deref(),
        ),
        (
            "completion result revision",
            lineage.result_revision.as_deref(),
        ),
    ] {
        if let Some(revision) = revision
            && (revision.len() < 7
                || revision.len() > 40
                || !revision
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        {
            return Err(capture_error(
                job_id,
                session_id,
                &format!("{label} must be a 7-40 character lowercase Git object ID"),
            ));
        }
    }
    Ok(())
}

/// Digest of the exact signed binding represented in canonical struct order.
pub fn operator_capture_binding_sha256(binding: &OperatorCaptureBinding) -> Result<String> {
    Ok(sha256_bytes(&framed_json(
        BINDING_MAGIC,
        binding,
        Path::new(OPERATOR_CAPTURE_BINDING_JSON),
    )?))
}

fn validate_binding_fields(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
) -> Result<()> {
    let job_id = binding.job_id.as_ref();
    let session_id = binding.session_id.as_str();
    for (label, value) in [
        ("Job ID", job_id),
        ("session ID", session_id),
        ("trial ID", binding.trial_id.as_str()),
        ("source revision", binding.source_revision.as_str()),
        (
            "DeadReckon source revision",
            binding.deadreckon_source_revision.as_str(),
        ),
        ("DeadReckon version", binding.deadreckon_version.as_str()),
        ("declared backend", binding.declared_backend.as_str()),
        (
            "recorder interpreter",
            binding.recorder_interpreter.as_str(),
        ),
        ("capture binary path", binding.capture_binary.as_str()),
        ("DeadReckon binary path", binding.deadreckon_binary.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(capture_error(
                job_id,
                session_id,
                &format!("{label} must not be empty"),
            ));
        }
    }
    for (label, value) in [
        ("source tree", binding.source_tree_sha256.as_str()),
        ("manifest", binding.manifest_sha256.as_str()),
        ("result schema", binding.result_schema_sha256.as_str()),
        ("recorder", binding.recorder_sha256.as_str()),
        (
            "recorder interpreter",
            binding.recorder_interpreter_sha256.as_str(),
        ),
        ("capture binary", binding.capture_binary_sha256.as_str()),
        (
            "DeadReckon binary",
            binding.deadreckon_binary_sha256.as_str(),
        ),
        ("replay", binding.replay_sha256.as_str()),
    ] {
        require_sha256(value, job_id, session_id, label)?;
    }
    if binding.deadreckon_source_revision.len() != 40
        || !binding
            .deadreckon_source_revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(capture_error(
            job_id,
            session_id,
            "DeadReckon source revision must be a full 40-character lowercase Git object ID",
        ));
    }
    let mut routes = BTreeSet::new();
    for (role, role_routes) in &binding.provider_routes {
        if role.trim().is_empty() || role_routes.is_empty() {
            return Err(capture_error(
                job_id,
                session_id,
                "provider roles and route lists must be non-empty",
            ));
        }
        for route in role_routes {
            if route.trim().is_empty() || !routes.insert((role.as_str(), route.as_str())) {
                return Err(capture_error(
                    job_id,
                    session_id,
                    "provider routes must be non-empty and unique within each role",
                ));
            }
        }
    }
    let mut requirements = BTreeSet::new();
    for requirement in &binding.required_captures {
        if requirement.subject.trim().is_empty()
            || requirement.media_type.trim().is_empty()
            || matches!(
                requirement.source,
                OperatorCaptureSource::Binding
                    | OperatorCaptureSource::JobIntervention
                    | OperatorCaptureSource::JobCleanup
                    | OperatorCaptureSource::SandboxBoundaryObservation
                    | OperatorCaptureSource::CampaignIntervention
                    | OperatorCaptureSource::ResultEnvelope
                    | OperatorCaptureSource::ManualFile
            )
            || !requirements.insert((requirement.subject.as_str(), requirement.phase))
        {
            return Err(capture_error(
                job_id,
                session_id,
                "required capture subject/phase/source/media declarations are invalid or duplicated",
            ));
        }
    }
    if binding.pass_capable && binding.required_captures.is_empty() {
        return Err(capture_error(
            job_id,
            session_id,
            "pass-capable capture requires at least one manifest-bound capture subject",
        ));
    }
    if binding.pass_capable
        && binding
            .required_captures
            .iter()
            .any(|requirement| requirement.source == OperatorCaptureSource::UnavailableObjective)
    {
        return Err(capture_error(
            job_id,
            session_id,
            "pass-capable capture cannot require an unavailable objective source",
        ));
    }
    if binding.pass_capable
        && (!matches!(
            binding.declared_backend.as_str(),
            "sandbox-exec" | "bwrap" | "docker"
        ) || binding.provider_routes.is_empty())
    {
        return Err(capture_error(
            job_id,
            session_id,
            "pass-capable capture requires sandbox-exec, bwrap, or docker and a concrete provider route",
        ));
    }

    let job = load_job(paths, job_id)?;
    if job.job_id != binding.job_id || job.shape != binding.declared_shape {
        return Err(capture_error(
            job_id,
            session_id,
            "binding Job identity or declared shape does not match immutable job.json",
        ));
    }
    let authority_path = paths.job_authority(job_id);
    let authority_raw =
        read_stable_regular_file(&authority_path, job_id, session_id, "Job authority")?;
    let authority: JobAuthority =
        serde_json::from_slice(&authority_raw).with_json_path(&authority_path)?;
    if authority.job_id != binding.job_id
        || authority.source_tree_sha256 != binding.source_tree_sha256
        || authority.source_revision.as_deref() != Some(binding.source_revision.as_str())
    {
        return Err(capture_error(
            job_id,
            session_id,
            "binding source identity does not match immutable Job authority",
        ));
    }
    Ok(())
}

fn validate_draft(
    binding: &OperatorCaptureBinding,
    draft: &OperatorCaptureEventDraft,
) -> Result<()> {
    for (label, value) in [
        ("event ID", draft.event_id.as_str()),
        ("causation ID", draft.causation_id.as_str()),
        ("subject", draft.subject.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                &format!("{label} must not be empty"),
            ));
        }
    }
    require_sha256(
        &draft.content_sha256,
        binding.job_id.as_ref(),
        &binding.session_id,
        "event content",
    )?;
    let manual = draft.provenance == OperatorCaptureProvenance::OperatorAttested
        || draft.source == OperatorCaptureSource::ManualFile
        || draft.kind == OperatorCaptureEventKind::OperatorAttestation;
    if manual
        && !(draft.provenance == OperatorCaptureProvenance::OperatorAttested
            && draft.source == OperatorCaptureSource::ManualFile
            && draft.kind == OperatorCaptureEventKind::OperatorAttestation)
    {
        return Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "manual capture must use operator_attested provenance, manual_file source, and operator_attestation kind together",
        ));
    }
    validate_source_claim(
        binding.job_id.as_ref(),
        &binding.session_id,
        draft.source,
        draft.phase,
        draft.kind,
        draft.provenance,
    )?;
    Ok(())
}

fn build_signed_event(
    binding: &OperatorCaptureBinding,
    binding_sha256: &str,
    sequence: OperatorCaptureEventSequence,
    previous_event_sha256: Option<String>,
    draft: &OperatorCaptureEventDraft,
    key: &[u8],
) -> Result<OperatorCaptureEvent> {
    let mut event = OperatorCaptureEvent {
        schema_version: OperatorCaptureSchemaVersion::CURRENT,
        job_id: binding.job_id.clone(),
        session_id: binding.session_id.clone(),
        binding_sha256: binding_sha256.to_string(),
        sequence,
        event_id: draft.event_id.clone(),
        causation_id: draft.causation_id.clone(),
        timestamp: draft.timestamp,
        phase: draft.phase,
        kind: draft.kind,
        provenance: draft.provenance,
        source: draft.source,
        subject: draft.subject.clone(),
        content_sha256: draft.content_sha256.clone(),
        content_bytes: draft.content_bytes,
        previous_event_sha256,
        signature: String::new(),
    };
    event.signature = sign_event(&event, key)?;
    Ok(event)
}

fn validate_pass_coverage(
    binding: &OperatorCaptureBinding,
    history: &OperatorCaptureHistory,
) -> Result<()> {
    for requirement in &binding.required_captures {
        let covered = history.events.iter().any(|event| {
            event.subject == requirement.subject
                && event.phase == requirement.phase
                && event.source == requirement.source
                && event.provenance != OperatorCaptureProvenance::OperatorAttested
                && event.source != OperatorCaptureSource::ManualFile
        });
        if !covered {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                &format!(
                    "passed receipt requires objective subject {} in phase {:?}",
                    requirement.subject, requirement.phase
                ),
            ));
        }
    }
    let expected_intervention_source = match binding.trial_id.as_str() {
        "live_provider_worker_kill"
        | "live_provider_supervisor_restart"
        | "live_provider_network_loss"
        | "machine_reboot"
        | "live_provider_parent_repair" => OperatorCaptureSource::JobIntervention,
        "live_campaign_interruption_recovery" => OperatorCaptureSource::CampaignIntervention,
        "cross_provider_gate_attack"
        | "linux_bubblewrap_gate_boundary"
        | "docker_gate_boundary" => OperatorCaptureSource::SandboxBoundaryObservation,
        _ => {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "passed receipt requires a known trial-specific intervention policy",
            ));
        }
    };
    for (label, kind, source) in [
        (
            "intervention",
            OperatorCaptureEventKind::InterventionRecorded,
            expected_intervention_source,
        ),
        (
            "cleanup",
            OperatorCaptureEventKind::CleanupRecorded,
            OperatorCaptureSource::JobCleanup,
        ),
    ] {
        let covered = history.events.iter().any(|event| {
            event.kind == kind
                && event.source == source
                && event.provenance != OperatorCaptureProvenance::OperatorAttested
                && event.source != OperatorCaptureSource::ManualFile
        });
        if !covered {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                &format!("passed receipt requires an authenticated objective {label} event"),
            ));
        }
    }
    Ok(())
}

fn validate_source_claim(
    job_id: &str,
    session_id: &str,
    source: OperatorCaptureSource,
    phase: OperatorCapturePhase,
    kind: OperatorCaptureEventKind,
    provenance: OperatorCaptureProvenance,
) -> Result<()> {
    let valid = match source {
        OperatorCaptureSource::Binding => {
            phase == OperatorCapturePhase::Prepared
                && kind == OperatorCaptureEventKind::SessionPrepared
                && provenance == OperatorCaptureProvenance::TrustedSupervisor
        }
        OperatorCaptureSource::JobIntervention => {
            phase == OperatorCapturePhase::Intervention
                && kind == OperatorCaptureEventKind::InterventionRecorded
                && provenance == OperatorCaptureProvenance::TrustedSupervisor
        }
        OperatorCaptureSource::SandboxBoundaryObservation
        | OperatorCaptureSource::CampaignIntervention => {
            phase == OperatorCapturePhase::Intervention
                && kind == OperatorCaptureEventKind::InterventionRecorded
                && provenance == OperatorCaptureProvenance::TrustedSupervisor
        }
        OperatorCaptureSource::JobCleanup => {
            phase == OperatorCapturePhase::Cleanup
                && kind == OperatorCaptureEventKind::CleanupRecorded
                && provenance == OperatorCaptureProvenance::TrustedSupervisor
        }
        OperatorCaptureSource::ManualFile => {
            kind == OperatorCaptureEventKind::OperatorAttestation
                && provenance == OperatorCaptureProvenance::OperatorAttested
                && phase != OperatorCapturePhase::Prepared
                && phase != OperatorCapturePhase::Finalized
        }
        OperatorCaptureSource::UnavailableObjective => false,
        OperatorCaptureSource::ResultEnvelope => {
            phase == OperatorCapturePhase::Finalized
                && kind == OperatorCaptureEventKind::ResultFinalized
                && provenance == OperatorCaptureProvenance::TrustedSupervisor
        }
        OperatorCaptureSource::JobView
        | OperatorCaptureSource::JobReport
        | OperatorCaptureSource::Receipt
        | OperatorCaptureSource::Doctor => {
            matches!(
                phase,
                OperatorCapturePhase::Before | OperatorCapturePhase::After
            ) && kind == OperatorCaptureEventKind::EvidenceCaptured
                && provenance == OperatorCaptureProvenance::PublicDeadreckon
        }
        OperatorCaptureSource::HostBootId | OperatorCaptureSource::SupervisorServiceStatus => {
            matches!(
                phase,
                OperatorCapturePhase::Before | OperatorCapturePhase::After
            ) && kind == OperatorCaptureEventKind::EvidenceCaptured
                && provenance == OperatorCaptureProvenance::AuthoritativeHost
        }
        OperatorCaptureSource::JobEvents
        | OperatorCaptureSource::Job
        | OperatorCaptureSource::Authority
        | OperatorCaptureSource::LaunchPlan
        | OperatorCaptureSource::Lease
        | OperatorCaptureSource::SupervisedChild
        | OperatorCaptureSource::SemanticJudgment
        | OperatorCaptureSource::ParentRepairManifest
        | OperatorCaptureSource::ParentRepairCandidate
        | OperatorCaptureSource::ParentArtifact
        | OperatorCaptureSource::ParentEvents
        | OperatorCaptureSource::Campaign
        | OperatorCaptureSource::CampaignEvents
        | OperatorCaptureSource::ActivePlan
        | OperatorCaptureSource::ActivePlanEvents => {
            matches!(
                phase,
                OperatorCapturePhase::Before | OperatorCapturePhase::After
            ) && kind == OperatorCaptureEventKind::EvidenceCaptured
                && provenance == OperatorCaptureProvenance::TrustedSupervisor
        }
    };
    if valid {
        Ok(())
    } else {
        Err(capture_error(
            job_id,
            session_id,
            "capture source cannot mint the requested phase, kind, or provenance",
        ))
    }
}

fn validate_next_lifecycle(
    binding: &OperatorCaptureBinding,
    history: &OperatorCaptureHistory,
    draft: &OperatorCaptureEventDraft,
) -> Result<()> {
    match history.events.last() {
        None => {
            if draft.phase != OperatorCapturePhase::Prepared
                || draft.kind != OperatorCaptureEventKind::SessionPrepared
            {
                return Err(capture_error(
                    binding.job_id.as_ref(),
                    &binding.session_id,
                    "the first capture event must be SessionPrepared",
                ));
            }
        }
        Some(previous) => {
            if draft.phase < previous.phase {
                return Err(capture_error(
                    binding.job_id.as_ref(),
                    &binding.session_id,
                    "capture phases cannot move backward",
                ));
            }
            if draft.kind == OperatorCaptureEventKind::SessionPrepared {
                return Err(capture_error(
                    binding.job_id.as_ref(),
                    &binding.session_id,
                    "SessionPrepared may appear exactly once",
                ));
            }
            if previous.kind == OperatorCaptureEventKind::ResultFinalized {
                return Err(capture_error(
                    binding.job_id.as_ref(),
                    &binding.session_id,
                    "ResultFinalized must be the final capture event",
                ));
            }
        }
    }
    Ok(())
}

fn read_and_validate_history(
    path: &Path,
    binding: &OperatorCaptureBinding,
    binding_sha256: &str,
    key: &[u8],
) -> Result<OperatorCaptureHistory> {
    let validated = read_and_validate_history_with_tail_policy(
        path,
        binding,
        binding_sha256,
        key,
        UncommittedTailPolicy::Reject,
    )?;
    if validated.pending_head_index.is_some() {
        return Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "capture history has a committed row that is not anchored by its durable history head",
        ));
    }
    Ok(validated.history)
}

fn read_and_validate_history_with_tail_policy(
    path: &Path,
    binding: &OperatorCaptureBinding,
    binding_sha256: &str,
    key: &[u8],
    tail_policy: UncommittedTailPolicy,
) -> Result<ValidatedHistory> {
    let head_path = capture_history_head_path_from_events(path)?;
    let head = load_history_head(&head_path, binding, binding_sha256, key)?;
    let history = read_raw_history(
        path,
        binding.job_id.as_ref(),
        &binding.session_id,
        tail_policy,
        head.as_ref().map(|value| value.history_bytes),
    )?;
    let mut expected_sequence = 1_u64;
    let mut expected_previous = None;
    let mut event_ids = BTreeSet::new();
    for (event, raw) in history.events.iter().zip(&history.raw_lines) {
        if encode_json(path, event)? != *raw {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                &format!(
                    "capture event {} is not stored in canonical wire form",
                    event.event_id
                ),
            ));
        }
        if event.job_id != binding.job_id
            || event.session_id != binding.session_id
            || event.binding_sha256 != binding_sha256
        {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "capture history contains a foreign binding",
            ));
        }
        if event.sequence.get() != expected_sequence {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                &format!(
                    "expected capture event sequence {expected_sequence}, found {}",
                    event.sequence.get()
                ),
            ));
        }
        if event.previous_event_sha256 != expected_previous {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                &format!("capture event {} breaks the hash chain", event.event_id),
            ));
        }
        if !event_ids.insert(event.event_id.as_str()) {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                &format!("capture event id {} is duplicated", event.event_id),
            ));
        }
        require_sha256(
            &event.content_sha256,
            binding.job_id.as_ref(),
            &binding.session_id,
            "event content",
        )?;
        validate_source_claim(
            binding.job_id.as_ref(),
            &binding.session_id,
            event.source,
            event.phase,
            event.kind,
            event.provenance,
        )?;
        verify_event_signature(event, key)?;
        expected_previous = Some(sha256_bytes(raw));
        expected_sequence = expected_sequence.checked_add(1).ok_or_else(|| {
            capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "capture event sequence overflowed",
            )
        })?;
    }
    validate_persisted_lifecycle(binding, &history.events)?;
    let pending_head_index =
        validate_history_head(binding, binding_sha256, &history, head.as_ref())?;
    Ok(ValidatedHistory {
        history,
        head,
        pending_head_index,
    })
}

fn validate_persisted_lifecycle(
    binding: &OperatorCaptureBinding,
    events: &[OperatorCaptureEvent],
) -> Result<()> {
    let Some(first) = events.first() else {
        return Ok(());
    };
    if first.phase != OperatorCapturePhase::Prepared
        || first.kind != OperatorCaptureEventKind::SessionPrepared
        || events
            .iter()
            .filter(|event| event.kind == OperatorCaptureEventKind::SessionPrepared)
            .count()
            != 1
    {
        return Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "capture history must start with exactly one SessionPrepared event",
        ));
    }
    for pair in events.windows(2) {
        if pair[1].phase < pair[0].phase {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "capture history phases move backward",
            ));
        }
        if pair[0].kind == OperatorCaptureEventKind::ResultFinalized {
            return Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "ResultFinalized is not the final capture event",
            ));
        }
    }
    Ok(())
}

fn capture_history_head_path(paths: &DeadreckonPaths, binding: &OperatorCaptureBinding) -> PathBuf {
    paths
        .operator_capture_dir(binding.job_id.as_ref(), &binding.session_id)
        .join(OPERATOR_CAPTURE_HISTORY_HEAD_JSON)
}

fn capture_history_head_path_from_events(events_path: &Path) -> Result<PathBuf> {
    let parent = events_path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "capture history path has no parent: {}",
            events_path.display()
        ))
    })?;
    Ok(parent.join(OPERATOR_CAPTURE_HISTORY_HEAD_JSON))
}

fn committed_history_bytes(
    history: &OperatorCaptureHistory,
    count: usize,
    binding: &OperatorCaptureBinding,
) -> Result<u64> {
    history
        .raw_lines
        .iter()
        .take(count)
        .try_fold(0_u64, |total, raw| {
            let row = u64::try_from(raw.len())
                .ok()
                .and_then(|length| length.checked_add(1))
                .ok_or_else(|| {
                    capture_error(
                        binding.job_id.as_ref(),
                        &binding.session_id,
                        "capture history row length overflowed",
                    )
                })?;
            total.checked_add(row).ok_or_else(|| {
                capture_error(
                    binding.job_id.as_ref(),
                    &binding.session_id,
                    "capture history length overflowed",
                )
            })
        })
}

fn load_history_head(
    path: &Path,
    binding: &OperatorCaptureBinding,
    binding_sha256: &str,
    key: &[u8],
) -> Result<Option<OperatorCaptureHistoryHead>> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    let raw = read_stable_regular_file_with_limit(
        path,
        binding.job_id.as_ref(),
        &binding.session_id,
        "capture history head",
        MAX_CAPTURE_CONTROL_BYTES,
    )?;
    let head: OperatorCaptureHistoryHead = serde_json::from_slice(&raw).with_json_path(path)?;
    if head.job_id != binding.job_id
        || head.session_id != binding.session_id
        || head.binding_sha256 != binding_sha256
        || head.history_bytes == 0
    {
        return Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "capture history head does not match its binding",
        ));
    }
    require_sha256(
        &head.event_sha256,
        binding.job_id.as_ref(),
        &binding.session_id,
        "capture history head event",
    )?;
    verify_history_head_signature(&head, key)?;
    Ok(Some(head))
}

fn validate_history_head(
    binding: &OperatorCaptureBinding,
    binding_sha256: &str,
    history: &OperatorCaptureHistory,
    head: Option<&OperatorCaptureHistoryHead>,
) -> Result<Option<usize>> {
    let Some(head) = head else {
        return match history.events.len() {
            0 => Ok(None),
            1 => Ok(Some(0)),
            _ => Err(capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "nonempty capture history has no authenticated durable head",
            )),
        };
    };
    if head.binding_sha256 != binding_sha256 {
        return Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "capture history head has a foreign binding digest",
        ));
    }
    let anchored_count = usize::try_from(head.sequence.get()).map_err(|_| {
        capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "capture history head sequence overflowed",
        )
    })?;
    if anchored_count == 0 || anchored_count > history.events.len() {
        return Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "capture history is shorter than its authenticated durable head",
        ));
    }
    let anchored_bytes = committed_history_bytes(history, anchored_count, binding)?;
    let anchored_raw = &history.raw_lines[anchored_count - 1];
    let anchored_event = &history.events[anchored_count - 1];
    if head.history_bytes != anchored_bytes
        || head.event_sha256 != sha256_bytes(anchored_raw)
        || head.sequence != anchored_event.sequence
    {
        return Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "capture history does not match its authenticated durable head",
        ));
    }
    match history.events.len().saturating_sub(anchored_count) {
        0 => Ok(None),
        1 => Ok(Some(anchored_count)),
        _ => Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "capture history is more than one committed row ahead of its durable head",
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn persist_next_history_head(
    paths: &DeadreckonPaths,
    binding: &OperatorCaptureBinding,
    binding_sha256: &str,
    key: &[u8],
    previous: Option<&OperatorCaptureHistoryHead>,
    event: &OperatorCaptureEvent,
    raw: &[u8],
    history_bytes: u64,
) -> Result<OperatorCaptureHistoryHead> {
    let expected_sequence = previous.map_or(1_u64, |head| head.sequence.get().saturating_add(1));
    let expected_bytes = previous
        .map_or(0_u64, |head| head.history_bytes)
        .checked_add(u64::try_from(raw.len()).unwrap_or(u64::MAX))
        .and_then(|length| length.checked_add(1))
        .ok_or_else(|| {
            capture_error(
                binding.job_id.as_ref(),
                &binding.session_id,
                "capture history head length overflowed",
            )
        })?;
    if event.sequence.get() != expected_sequence || history_bytes != expected_bytes {
        return Err(capture_error(
            binding.job_id.as_ref(),
            &binding.session_id,
            "capture history head update is not the next monotonic row",
        ));
    }
    let mut head = OperatorCaptureHistoryHead {
        schema_version: OperatorCaptureSchemaVersion::CURRENT,
        job_id: binding.job_id.clone(),
        session_id: binding.session_id.clone(),
        binding_sha256: binding_sha256.to_string(),
        sequence: event.sequence,
        event_sha256: sha256_bytes(raw),
        history_bytes,
        signature: String::new(),
    };
    head.signature = sign_history_head(&head, key)?;
    persist_replacing_json(&capture_history_head_path(paths, binding), &head)?;
    Ok(head)
}

fn sync_history_and_head(paths: &DeadreckonPaths, binding: &OperatorCaptureBinding) -> Result<()> {
    let events = paths.operator_capture_events(binding.job_id.as_ref(), &binding.session_id);
    let head = capture_history_head_path(paths, binding);
    sync_stable_regular_file(
        &events,
        binding.job_id.as_ref(),
        &binding.session_id,
        "capture history",
        MAX_CAPTURE_HISTORY_BYTES,
    )?;
    sync_stable_regular_file(
        &head,
        binding.job_id.as_ref(),
        &binding.session_id,
        "capture history head",
        MAX_CAPTURE_CONTROL_BYTES,
    )?;
    sync_parent(events.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput("capture history path has no parent".to_string())
    })?)
}

fn read_raw_history(
    path: &Path,
    job_id: &str,
    session_id: &str,
    tail_policy: UncommittedTailPolicy,
    anchored_bytes: Option<u64>,
) -> Result<OperatorCaptureHistory> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OperatorCaptureHistory {
                events: Vec::new(),
                raw_lines: Vec::new(),
            });
        }
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if !before.file_type().is_file() || before.file_type().is_symlink() {
        return Err(capture_error(
            job_id,
            session_id,
            "capture history is not a regular non-symlink file",
        ));
    }
    if before.len() > MAX_CAPTURE_HISTORY_BYTES {
        return Err(capture_error(
            job_id,
            session_id,
            "capture history exceeds the trusted read size bound",
        ));
    }

    let mut options = OpenOptions::new();
    options.read(true);
    if tail_policy == UncommittedTailPolicy::RecoverUnderSessionLock {
        options.write(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).with_path(path)?;
    let opened = file.metadata().with_path(path)?;
    if !opened.file_type().is_file() || opened.len() > MAX_CAPTURE_HISTORY_BYTES {
        return Err(capture_error(
            job_id,
            session_id,
            "capture history exceeds the trusted read size bound or is not a regular file",
        ));
    }
    let mut raw = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(MAX_CAPTURE_HISTORY_BYTES + 1)
        .read_to_end(&mut raw)
        .with_path(path)?;
    if u64::try_from(raw.len()).unwrap_or(u64::MAX) > MAX_CAPTURE_HISTORY_BYTES {
        return Err(capture_error(
            job_id,
            session_id,
            "capture history exceeds the trusted read size bound",
        ));
    }
    let after = file.metadata().with_path(path)?;
    let post_path = fs::symlink_metadata(path).with_path(path)?;
    if !stable_metadata_matches(&before, &opened)
        || !stable_metadata_matches(&opened, &after)
        || !stable_metadata_matches(&after, &post_path)
        || u64::try_from(raw.len()).ok() != Some(after.len())
    {
        return Err(capture_error(
            job_id,
            session_id,
            "capture history changed while its trusted bytes were read",
        ));
    }

    if !raw.is_empty() && !raw.ends_with(b"\n") {
        if tail_policy == UncommittedTailPolicy::Reject {
            return Err(capture_error(
                job_id,
                session_id,
                "capture history has an uncommitted torn final row",
            ));
        }
        let committed_len = raw
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        let committed_len_u64 = u64::try_from(committed_len).map_err(|_| {
            capture_error(
                job_id,
                session_id,
                "committed capture history length overflowed",
            )
        })?;
        if anchored_bytes.is_some_and(|minimum| committed_len_u64 < minimum) {
            return Err(capture_error(
                job_id,
                session_id,
                "capture history is shorter than its authenticated durable head",
            ));
        }
        file.set_len(committed_len_u64).with_path(path)?;
        file.sync_all().with_path(path)?;
        let truncated = file.metadata().with_path(path)?;
        let post_truncate_path = fs::symlink_metadata(path).with_path(path)?;
        if truncated.len() != committed_len_u64
            || !stable_metadata_matches(&truncated, &post_truncate_path)
        {
            return Err(capture_error(
                job_id,
                session_id,
                "capture history path changed while recovering its uncommitted tail",
            ));
        }
        raw.truncate(committed_len);
    }
    let mut events = Vec::new();
    let mut raw_lines = Vec::new();
    for (index, line) in raw
        .split(|byte| *byte == b'\n')
        .take_while(|line| !line.is_empty())
        .enumerate()
    {
        let event = serde_json::from_slice::<OperatorCaptureEvent>(line).map_err(|source| {
            capture_error(
                job_id,
                session_id,
                &format!(
                    "capture history is corrupt at {} row {}: {source}",
                    path.display(),
                    index + 1
                ),
            )
        })?;
        events.push(event);
        raw_lines.push(line.to_vec());
    }
    let expected_rows = raw.split(|byte| *byte == b'\n').count().saturating_sub(1);
    if events.len() != expected_rows {
        return Err(capture_error(
            job_id,
            session_id,
            "capture history contains an empty completed row",
        ));
    }
    Ok(OperatorCaptureHistory { events, raw_lines })
}

fn sign_binding(binding: &OperatorCaptureBinding, key: &[u8]) -> Result<String> {
    let mut unsigned = binding.clone();
    unsigned.signature.clear();
    sign_bytes(
        key,
        &framed_json(
            BINDING_MAGIC,
            &unsigned,
            Path::new(OPERATOR_CAPTURE_BINDING_JSON),
        )?,
        binding.job_id.as_ref(),
        &binding.session_id,
    )
}

fn verify_binding_signature(binding: &OperatorCaptureBinding, key: &[u8]) -> Result<()> {
    let mut unsigned = binding.clone();
    unsigned.signature.clear();
    verify_bytes(
        key,
        &framed_json(
            BINDING_MAGIC,
            &unsigned,
            Path::new(OPERATOR_CAPTURE_BINDING_JSON),
        )?,
        &binding.signature,
        binding.job_id.as_ref(),
        &binding.session_id,
        "binding",
    )
}

fn sign_event(event: &OperatorCaptureEvent, key: &[u8]) -> Result<String> {
    let mut unsigned = event.clone();
    unsigned.signature.clear();
    sign_bytes(
        key,
        &framed_json(
            EVENT_MAGIC,
            &unsigned,
            Path::new(OPERATOR_CAPTURE_EVENTS_JSONL),
        )?,
        event.job_id.as_ref(),
        &event.session_id,
    )
}

fn verify_event_signature(event: &OperatorCaptureEvent, key: &[u8]) -> Result<()> {
    let mut unsigned = event.clone();
    unsigned.signature.clear();
    verify_bytes(
        key,
        &framed_json(
            EVENT_MAGIC,
            &unsigned,
            Path::new(OPERATOR_CAPTURE_EVENTS_JSONL),
        )?,
        &event.signature,
        event.job_id.as_ref(),
        &event.session_id,
        "event",
    )
}

fn sign_history_head(head: &OperatorCaptureHistoryHead, key: &[u8]) -> Result<String> {
    let mut unsigned = head.clone();
    unsigned.signature.clear();
    sign_bytes(
        key,
        &framed_json(
            HISTORY_HEAD_MAGIC,
            &unsigned,
            Path::new(OPERATOR_CAPTURE_HISTORY_HEAD_JSON),
        )?,
        head.job_id.as_ref(),
        &head.session_id,
    )
}

fn verify_history_head_signature(head: &OperatorCaptureHistoryHead, key: &[u8]) -> Result<()> {
    let mut unsigned = head.clone();
    unsigned.signature.clear();
    verify_bytes(
        key,
        &framed_json(
            HISTORY_HEAD_MAGIC,
            &unsigned,
            Path::new(OPERATOR_CAPTURE_HISTORY_HEAD_JSON),
        )?,
        &head.signature,
        head.job_id.as_ref(),
        &head.session_id,
        "capture history head",
    )
}

fn sign_receipt(receipt: &OperatorCaptureReceipt, key: &[u8]) -> Result<String> {
    let mut unsigned = receipt.clone();
    unsigned.signature.clear();
    sign_bytes(
        key,
        &framed_json(
            RECEIPT_MAGIC,
            &unsigned,
            Path::new(OPERATOR_CAPTURE_RECEIPT_JSON),
        )?,
        receipt.job_id.as_ref(),
        &receipt.session_id,
    )
}

fn verify_receipt_signature(receipt: &OperatorCaptureReceipt, key: &[u8]) -> Result<()> {
    let mut unsigned = receipt.clone();
    unsigned.signature.clear();
    verify_bytes(
        key,
        &framed_json(
            RECEIPT_MAGIC,
            &unsigned,
            Path::new(OPERATOR_CAPTURE_RECEIPT_JSON),
        )?,
        &receipt.signature,
        receipt.job_id.as_ref(),
        &receipt.session_id,
        "receipt",
    )
}

fn sign_bytes(key: &[u8], bytes: &[u8], job_id: &str, session_id: &str) -> Result<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
        capture_error(
            job_id,
            session_id,
            "HMAC-SHA-256 refused the protected capture key",
        )
    })?;
    mac.update(bytes);
    Ok(hex_encode(&mac.finalize().into_bytes()))
}

fn verify_bytes(
    key: &[u8],
    bytes: &[u8],
    signature: &str,
    job_id: &str,
    session_id: &str,
    label: &str,
) -> Result<()> {
    let signature = hex_decode(signature).map_err(|reason| {
        capture_error(
            job_id,
            session_id,
            &format!("{label} signature is invalid hex: {reason}"),
        )
    })?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
        capture_error(
            job_id,
            session_id,
            "HMAC-SHA-256 refused the protected capture key",
        )
    })?;
    mac.update(bytes);
    mac.verify_slice(&signature).map_err(|_| {
        capture_error(
            job_id,
            session_id,
            &format!("{label} signature verification failed"),
        )
    })
}

fn framed_json<T: Serialize>(magic: &[u8], value: &T, path: &Path) -> Result<Vec<u8>> {
    let encoded = encode_json(path, value)?;
    let len = u64::try_from(encoded.len()).map_err(|_| {
        DeadreckonError::InvalidInput("operator capture artifact is too large".to_string())
    })?;
    let mut bytes = magic.to_vec();
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(&encoded);
    Ok(bytes)
}

fn encode_json<T: Serialize>(path: &Path, value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|source| DeadreckonError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn append_synced_json_line(path: &Path, value: &impl Serialize) -> Result<u64> {
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!("path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent).with_path(parent)?;
    let payload = encode_json(path, value)?;
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > MAX_CAPTURE_HISTORY_BYTES
            {
                return Err(DeadreckonError::InvalidInput(format!(
                    "capture history append target is unsafe: {}",
                    path.display()
                )));
            }
            Some(metadata)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => None,
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).with_path(path)?;
    let opened = file.metadata().with_path(path)?;
    if let Some(before) = &before
        && !stable_metadata_matches(before, &opened)
    {
        return Err(DeadreckonError::InvalidInput(
            "capture history path changed before append".to_string(),
        ));
    }
    let row_bytes = u64::try_from(payload.len())
        .ok()
        .and_then(|length| length.checked_add(1))
        .unwrap_or(u64::MAX);
    if !opened.file_type().is_file()
        || opened.len().saturating_add(row_bytes) > MAX_CAPTURE_HISTORY_BYTES
    {
        return Err(DeadreckonError::InvalidInput(
            "capture history exceeds its trusted size bound".to_string(),
        ));
    }
    file.write_all(&payload).with_path(path)?;
    file.sync_all().with_path(path)?;
    file.write_all(b"\n").with_path(path)?;
    file.sync_all().with_path(path)?;
    let post = fs::symlink_metadata(path).with_path(path)?;
    let committed = file.metadata().with_path(path)?;
    let expected_len = opened.len().checked_add(row_bytes).ok_or_else(|| {
        DeadreckonError::InvalidInput("capture history length overflowed".to_string())
    })?;
    if committed.len() != expected_len || !stable_metadata_matches(&committed, &post) {
        return Err(DeadreckonError::InvalidInput(
            "capture history path changed during append".to_string(),
        ));
    }
    if before.is_none() {
        sync_parent(parent)?;
    }
    Ok(committed.len())
}

fn persist_new_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!("path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent).with_path(parent)?;
    let mut temp = NamedTempFile::new_in(parent).with_path(parent)?;
    serde_json::to_writer_pretty(&mut temp, value).with_json_path(path)?;
    temp.write_all(b"\n").with_path(path)?;
    temp.as_file_mut().sync_all().with_path(path)?;
    match temp.persist_noclobber(path) {
        Ok(file) => {
            file.sync_all().with_path(path)?;
            sync_parent(parent)?;
            Ok(())
        }
        Err(error) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source: error.error,
        }),
    }
}

fn persist_replacing_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!("path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent).with_path(parent)?;
    let mut temp = NamedTempFile::new_in(parent).with_path(parent)?;
    serde_json::to_writer_pretty(&mut temp, value).with_json_path(path)?;
    temp.write_all(b"\n").with_path(path)?;
    temp.as_file_mut().sync_all().with_path(path)?;
    match temp.persist(path) {
        Ok(file) => {
            file.sync_all().with_path(path)?;
            sync_parent(parent)
        }
        Err(error) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source: error.error,
        }),
    }
}

fn read_stable_regular_file(
    path: &Path,
    job_id: &str,
    session_id: &str,
    label: &str,
) -> Result<Vec<u8>> {
    read_stable_regular_file_with_limit(path, job_id, session_id, label, 256 * 1024 * 1024)
}

fn read_stable_regular_file_with_limit(
    path: &Path,
    job_id: &str,
    session_id: &str,
    label: &str,
    max_bytes: u64,
) -> Result<Vec<u8>> {
    let before = fs::symlink_metadata(path).with_path(path)?;
    if !before.file_type().is_file() || before.file_type().is_symlink() {
        return Err(capture_error(
            job_id,
            session_id,
            &format!("{label} is not a regular non-symlink file"),
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(path).with_path(path)?;
    let opened = file.metadata().with_path(path)?;
    if !opened.file_type().is_file() || opened.len() > max_bytes {
        return Err(capture_error(
            job_id,
            session_id,
            &format!("{label} is not a regular file or exceeds the trusted read size bound"),
        ));
    }
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .with_path(path)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
        return Err(capture_error(
            job_id,
            session_id,
            &format!("{label} exceeds the trusted read size bound"),
        ));
    }
    let after = file.metadata().with_path(path)?;
    let post_path = fs::symlink_metadata(path).with_path(path)?;
    if !stable_metadata_matches(&before, &opened)
        || !stable_metadata_matches(&opened, &after)
        || !stable_metadata_matches(&after, &post_path)
        || u64::try_from(bytes.len()).ok() != Some(after.len())
    {
        return Err(capture_error(
            job_id,
            session_id,
            &format!("{label} changed while its trusted bytes were read"),
        ));
    }
    Ok(bytes)
}

fn sync_stable_regular_file(
    path: &Path,
    job_id: &str,
    session_id: &str,
    label: &str,
    max_bytes: u64,
) -> Result<()> {
    let before = fs::symlink_metadata(path).with_path(path)?;
    if !before.file_type().is_file() || before.file_type().is_symlink() || before.len() > max_bytes
    {
        return Err(capture_error(
            job_id,
            session_id,
            &format!("{label} is not a safe bounded regular file"),
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let file = options.open(path).with_path(path)?;
    let opened = file.metadata().with_path(path)?;
    if !opened.file_type().is_file()
        || opened.len() > max_bytes
        || !stable_metadata_matches(&before, &opened)
    {
        return Err(capture_error(
            job_id,
            session_id,
            &format!("{label} changed before durability sync"),
        ));
    }
    file.sync_all().with_path(path)?;
    let after = file.metadata().with_path(path)?;
    let post = fs::symlink_metadata(path).with_path(path)?;
    if !stable_metadata_matches(&opened, &after) || !stable_metadata_matches(&after, &post) {
        return Err(capture_error(
            job_id,
            session_id,
            &format!("{label} changed during durability sync"),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn stable_metadata_matches(left: &fs::Metadata, right: &fs::Metadata) -> bool {
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

#[cfg(windows)]
fn stable_metadata_matches(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;

    left.file_type().is_file()
        && right.file_type().is_file()
        && left.file_attributes() == right.file_attributes()
        && left.creation_time() == right.creation_time()
        && left.last_write_time() == right.last_write_time()
        && left.file_size() == right.file_size()
}

#[cfg(not(any(unix, windows)))]
fn stable_metadata_matches(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.file_type().is_file()
        && right.file_type().is_file()
        && left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
}

fn open_control_lock(path: &Path) -> Result<File> {
    const MAX_CONTROL_LOCK_BYTES: u64 = 1024 * 1024;
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!("lock path has no parent: {}", path.display()))
    })?;
    let existed = path.exists();
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let file = options.open(path).with_path(path)?;
    let metadata = file.metadata().with_path(path)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_CONTROL_LOCK_BYTES {
        return Err(DeadreckonError::InvalidInput(format!(
            "control lock is unsafe: {}",
            path.display()
        )));
    }
    let post = fs::symlink_metadata(path).with_path(path)?;
    if !stable_metadata_matches(&metadata, &post) {
        return Err(DeadreckonError::InvalidInput(
            "control lock path changed while opening".to_string(),
        ));
    }
    if !existed {
        file.sync_all().with_path(path)?;
        sync_parent(parent)?;
    }
    Ok(file)
}

#[cfg(unix)]
fn sync_parent(parent: &Path) -> Result<()> {
    File::open(parent)
        .with_path(parent)?
        .sync_all()
        .with_path(parent)
}

#[cfg(not(unix))]
fn sync_parent(_parent: &Path) -> Result<()> {
    Ok(())
}

fn require_sha256(value: &str, job_id: &str, session_id: &str, label: &str) -> Result<()> {
    let digest = value.strip_prefix("sha256:").filter(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if digest.is_some() {
        Ok(())
    } else {
        Err(capture_error(
            job_id,
            session_id,
            &format!("{label} digest is not sha256:<64 lowercase hex characters>"),
        ))
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
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
        .map(|pair| {
            let high = hex_nibble(pair[0])?;
            let low = hex_nibble(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(byte: u8) -> std::result::Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(format!("invalid hex byte {byte}")),
    }
}

fn capture_error(job_id: &str, session_id: &str, detail: &str) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!(
        "operator capture {job_id}/{session_id} is invalid: {detail}"
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use chrono::{TimeZone as _, Utc};
    use deadreckon_protocol::{
        AuthorityAcceptedBy, Job, JobAuthority, JobExecutionPolicy, JobId, JobPolicy,
        JobSchemaVersion, JobShape, OperatorCaptureCompletionLineage, OperatorCaptureRequirement,
        RunId, SemanticJudgeMode,
    };
    use serde_json::Value;
    use tempfile::TempDir;

    use super::{
        OperatorCaptureBinding, OperatorCaptureEventDraft, OperatorCaptureEventKind,
        OperatorCaptureEventSequence, OperatorCapturePhase, OperatorCaptureProvenance,
        OperatorCaptureSchemaVersion, OperatorCaptureSource, OperatorCaptureStatus,
        append_operator_capture_event, append_synced_json_line, build_signed_event,
        load_operator_capture_binding, operator_capture_binding_sha256,
        read_operator_capture_history, seal_operator_capture_receipt,
        validate_operator_capture_receipt, write_operator_capture_binding,
    };
    use crate::gate::{read_gate_key, write_gate_key};
    use crate::job::write_job;
    use crate::paths::DeadreckonPaths;
    use crate::state::atomic_write_json;

    fn digest(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    fn completion_lineage() -> OperatorCaptureCompletionLineage {
        OperatorCaptureCompletionLineage {
            completion_receipt_sha256: digest('1'),
            authority_sha256: digest('2'),
            contract_sha256: digest('3'),
            effective_policy_sha256: digest('4'),
            launch_plan_sha256: digest('5'),
            source_tree_sha256: digest('6'),
            source_revision: Some("0123456789abcdef".to_string()),
            result_tree_sha256: digest('7'),
            result_revision: Some("fedcba9876543210".to_string()),
        }
    }

    fn fixture(session_id: &str) -> (TempDir, DeadreckonPaths, OperatorCaptureBinding) {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("state"));
        let job_id = JobId::from("job-1");
        let created_at = Utc
            .with_ymd_and_hms(2026, 7, 30, 12, 0, 0)
            .single()
            .expect("timestamp");
        let job = Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job_id.clone(),
            scope: "test".to_string(),
            goal: "prove trusted captures".to_string(),
            shape: JobShape::Single,
            created_at,
            source_cwd: temp.path().join("source"),
            launch_plan_sha256: digest('1'),
            authority_sha256: digest('2'),
            policy: JobPolicy {
                max_spend_usd: 1.0,
                max_wall_seconds: 60,
                max_attempts: 1,
                deadline: None,
                semantic_judge: SemanticJudgeMode::Required,
                execution: Some(JobExecutionPolicy::workspace_only("strict")),
            },
        };
        write_job(&paths, &job).expect("job");
        let authority = JobAuthority {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job_id.clone(),
            run_id: RunId::from("job-1"),
            approved_at: created_at,
            accepted_by: AuthorityAcceptedBy::Operator,
            goal_sha256: digest('3'),
            contract_sha256: digest('4'),
            effective_policy_sha256: digest('5'),
            launch_plan_sha256: digest('1'),
            source_tree_sha256: digest('6'),
            source_revision: Some("0123456789abcdef".to_string()),
            sandbox_requested: "strict".to_string(),
            semantic_judge_mode: SemanticJudgeMode::Required,
        };
        atomic_write_json(&paths.job_authority("job-1"), &authority).expect("authority");
        write_gate_key(&paths, "job-1", &[7_u8; 32]).expect("gate key");
        let binding = OperatorCaptureBinding {
            schema_version: OperatorCaptureSchemaVersion::CURRENT,
            job_id,
            session_id: session_id.to_string(),
            trial_id: "live_provider_worker_kill".to_string(),
            created_at,
            source_revision: "0123456789abcdef".to_string(),
            source_tree_sha256: digest('6'),
            deadreckon_source_revision: "0123456789abcdef0123456789abcdef01234567".to_string(),
            manifest_sha256: digest('7'),
            result_schema_sha256: digest('8'),
            recorder_sha256: digest('9'),
            recorder_interpreter: "/usr/bin/python3".to_string(),
            recorder_interpreter_sha256: digest('d'),
            capture_binary: "/usr/local/bin/dr-capture".to_string(),
            capture_binary_sha256: digest('a'),
            deadreckon_binary: "/usr/local/bin/deadreckon".to_string(),
            deadreckon_binary_sha256: digest('b'),
            deadreckon_version: "0.7.0".to_string(),
            declared_shape: JobShape::Single,
            declared_backend: "sandbox-exec".to_string(),
            provider_routes: std::collections::BTreeMap::from([
                ("worker".to_string(), vec!["cli:codex".to_string()]),
                (
                    "independent_judge".to_string(),
                    vec!["cli:judge".to_string()],
                ),
            ]),
            replay_sha256: digest('c'),
            pass_capable: true,
            required_captures: vec![OperatorCaptureRequirement {
                subject: "before".to_string(),
                phase: OperatorCapturePhase::Before,
                source: OperatorCaptureSource::JobView,
                media_type: "application/json".to_string(),
            }],
            signature: String::new(),
        };
        (temp, paths, binding)
    }

    fn draft(id: &str, subject: &str, second: u32) -> OperatorCaptureEventDraft {
        OperatorCaptureEventDraft {
            event_id: id.to_string(),
            causation_id: "operator-step-1".to_string(),
            timestamp: Utc
                .with_ymd_and_hms(2026, 7, 30, 12, 0, second)
                .single()
                .expect("timestamp"),
            phase: if subject == "after" {
                OperatorCapturePhase::After
            } else {
                OperatorCapturePhase::Before
            },
            kind: OperatorCaptureEventKind::EvidenceCaptured,
            provenance: OperatorCaptureProvenance::PublicDeadreckon,
            source: OperatorCaptureSource::JobView,
            subject: subject.to_string(),
            content_sha256: digest('c'),
            content_bytes: 42,
        }
    }

    fn lifecycle_draft(
        id: &str,
        phase: OperatorCapturePhase,
        kind: OperatorCaptureEventKind,
        second: u32,
    ) -> OperatorCaptureEventDraft {
        OperatorCaptureEventDraft {
            event_id: id.to_string(),
            causation_id: "operator-step-1".to_string(),
            timestamp: Utc
                .with_ymd_and_hms(2026, 7, 30, 12, 0, second)
                .single()
                .expect("timestamp"),
            phase,
            kind,
            provenance: OperatorCaptureProvenance::TrustedSupervisor,
            source: match kind {
                OperatorCaptureEventKind::InterventionRecorded => {
                    OperatorCaptureSource::JobIntervention
                }
                OperatorCaptureEventKind::CleanupRecorded => OperatorCaptureSource::JobCleanup,
                _ => OperatorCaptureSource::JobEvents,
            },
            subject: format!("{phase:?}").to_lowercase(),
            content_sha256: digest('d'),
            content_bytes: 21,
        }
    }

    fn prepared_binding(
        paths: &DeadreckonPaths,
        unsigned: &OperatorCaptureBinding,
    ) -> OperatorCaptureBinding {
        let binding = write_operator_capture_binding(paths, unsigned).expect("binding");
        let encoded = serde_json::to_vec(&binding).expect("binding json");
        append_operator_capture_event(
            paths,
            &binding,
            &OperatorCaptureEventDraft {
                event_id: "session-prepared".to_string(),
                causation_id: "prepare".to_string(),
                timestamp: binding.created_at,
                phase: OperatorCapturePhase::Prepared,
                kind: OperatorCaptureEventKind::SessionPrepared,
                provenance: OperatorCaptureProvenance::TrustedSupervisor,
                source: OperatorCaptureSource::Binding,
                subject: "binding".to_string(),
                content_sha256: super::sha256_bytes(&encoded),
                content_bytes: u64::try_from(encoded.len()).expect("binding size"),
            },
        )
        .expect("prepared event");
        binding
    }

    fn persist_uncheckpointed_event(
        paths: &DeadreckonPaths,
        binding: &OperatorCaptureBinding,
        event_draft: &OperatorCaptureEventDraft,
        commit_newline: bool,
    ) -> (deadreckon_protocol::OperatorCaptureEvent, Vec<u8>) {
        let history =
            read_operator_capture_history(paths, binding.job_id.as_ref(), &binding.session_id)
                .expect("anchored history");
        let key = read_gate_key(paths, binding.job_id.as_ref()).expect("key");
        let binding_sha256 = operator_capture_binding_sha256(binding).expect("binding digest");
        let sequence = OperatorCaptureEventSequence::new(
            u64::try_from(history.events.len()).expect("event count") + 1,
        )
        .expect("sequence");
        let previous = history.raw_lines.last().map(|raw| super::sha256_bytes(raw));
        let event = build_signed_event(
            binding,
            &binding_sha256,
            sequence,
            previous,
            event_draft,
            &key,
        )
        .expect("signed event");
        let raw = serde_json::to_vec(&event).expect("event json");
        let path = paths.operator_capture_events(binding.job_id.as_ref(), &binding.session_id);
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("history");
        std::io::Write::write_all(&mut file, &raw).expect("event payload");
        file.sync_all().expect("payload sync");
        if commit_newline {
            std::io::Write::write_all(&mut file, b"\n").expect("commit newline");
            file.sync_all().expect("commit sync");
        }
        (event, raw)
    }

    #[test]
    fn binding_events_and_receipt_form_one_authenticated_chain() {
        let (_temp, paths, unsigned) = fixture("session-1");
        let binding = prepared_binding(&paths, &unsigned);
        assert!(!binding.signature.is_empty());
        let first = append_operator_capture_event(&paths, &binding, &draft("event-1", "before", 1))
            .expect("first event");
        append_operator_capture_event(
            &paths,
            &binding,
            &lifecycle_draft(
                "event-3",
                OperatorCapturePhase::Intervention,
                OperatorCaptureEventKind::InterventionRecorded,
                3,
            ),
        )
        .expect("objective intervention");
        let second = append_operator_capture_event(&paths, &binding, &draft("event-2", "after", 2))
            .expect("second event");
        append_operator_capture_event(
            &paths,
            &binding,
            &lifecycle_draft(
                "event-4",
                OperatorCapturePhase::Cleanup,
                OperatorCaptureEventKind::CleanupRecorded,
                4,
            ),
        )
        .expect("objective cleanup");
        assert_eq!(first.sequence.get(), 2);
        assert_eq!(second.sequence.get(), 4);
        assert!(second.previous_event_sha256.is_some());

        let duplicate =
            append_operator_capture_event(&paths, &binding, &draft("event-1", "before", 1))
                .expect("exact duplicate");
        assert_eq!(duplicate, first);
        assert_eq!(
            read_operator_capture_history(&paths, "job-1", "session-1")
                .expect("history")
                .events()
                .len(),
            5
        );
        let error =
            append_operator_capture_event(&paths, &binding, &draft("event-1", "changed", 1))
                .expect_err("conflicting duplicate");
        assert!(error.to_string().contains("different bytes"));

        let issued_at = Utc
            .with_ymd_and_hms(2026, 7, 30, 12, 1, 0)
            .single()
            .expect("timestamp");
        let receipt = seal_operator_capture_receipt(
            &paths,
            &binding,
            issued_at,
            &digest('d'),
            42,
            OperatorCaptureStatus::Passed,
            Some(completion_lineage()),
        )
        .expect("receipt");
        let retried_receipt = seal_operator_capture_receipt(
            &paths,
            &binding,
            issued_at + chrono::Duration::seconds(30),
            &digest('d'),
            42,
            OperatorCaptureStatus::Passed,
            Some(completion_lineage()),
        )
        .expect("idempotent receipt retry");
        assert_eq!(retried_receipt, receipt);
        let mut invalid_lineage = completion_lineage();
        invalid_lineage.contract_sha256 = "not-a-digest".to_string();
        let invalid = seal_operator_capture_receipt(
            &paths,
            &binding,
            issued_at,
            &digest('d'),
            42,
            OperatorCaptureStatus::Passed,
            Some(invalid_lineage),
        )
        .expect_err("invalid completion lineage");
        assert!(invalid.to_string().contains("completion contract digest"));
        assert_eq!(receipt.event_count, 6);
        let finalized = read_operator_capture_history(&paths, "job-1", "session-1")
            .expect("finalized history")
            .events()
            .last()
            .cloned()
            .expect("finalized event");
        assert_eq!(finalized.kind, OperatorCaptureEventKind::ResultFinalized);
        assert_eq!(finalized.source, OperatorCaptureSource::ResultEnvelope);
        assert_eq!(finalized.content_sha256, digest('d'));
        assert_eq!(
            validate_operator_capture_receipt(&paths, &binding)
                .expect("valid receipt")
                .status,
            OperatorCaptureStatus::Passed
        );
        let sealed_duplicate =
            append_operator_capture_event(&paths, &binding, &draft("event-1", "before", 1))
                .expect("sealed exact duplicate");
        assert_eq!(sealed_duplicate, first);
        let error = append_operator_capture_event(&paths, &binding, &draft("event-5", "late", 5))
            .expect_err("sealed history");
        assert!(error.to_string().contains("sealed"));
        let error = seal_operator_capture_receipt(
            &paths,
            &binding,
            issued_at,
            &digest('e'),
            42,
            OperatorCaptureStatus::Passed,
            Some(completion_lineage()),
        )
        .expect_err("different result cannot reuse receipt");
        assert!(error.to_string().contains("different result or status"));
    }

    #[test]
    fn binding_is_immutable_authenticated_and_stored_outside_workspaces() {
        let (_temp, paths, unsigned) = fixture("../../session");
        let binding = prepared_binding(&paths, &unsigned);
        let path = paths.operator_capture_binding("job-1", "../../session");
        assert!(path.starts_with(paths.operator_captures_dir()));
        assert!(!path.starts_with(paths.job_dir("job-1")));

        let mut replacement = binding.clone();
        replacement.manifest_sha256 = digest('e');
        replacement.signature.clear();
        let error = write_operator_capture_binding(&paths, &replacement)
            .expect_err("changed replacement refused");
        assert!(error.to_string().contains("different bytes"));

        let mut raw: Value =
            serde_json::from_slice(&fs::read(&path).expect("binding bytes")).expect("binding json");
        raw["manifest_sha256"] = Value::String(digest('f'));
        fs::write(
            &path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&raw).expect("binding json")
            ),
        )
        .expect("tamper binding");
        let error = load_operator_capture_binding(&paths, "job-1", "../../session")
            .expect_err("tampered binding");
        assert!(error.to_string().contains("signature verification failed"));
    }

    #[test]
    fn event_signature_chain_and_torn_rows_fail_closed() {
        let (_temp, paths, unsigned) = fixture("session-2");
        let binding = prepared_binding(&paths, &unsigned);
        append_operator_capture_event(&paths, &binding, &draft("event-1", "before", 1))
            .expect("event");
        append_operator_capture_event(
            &paths,
            &binding,
            &lifecycle_draft(
                "event-2",
                OperatorCapturePhase::Intervention,
                OperatorCaptureEventKind::InterventionRecorded,
                2,
            ),
        )
        .expect("intervention");
        append_operator_capture_event(
            &paths,
            &binding,
            &lifecycle_draft(
                "event-3",
                OperatorCapturePhase::Cleanup,
                OperatorCaptureEventKind::CleanupRecorded,
                3,
            ),
        )
        .expect("cleanup");
        let path = paths.operator_capture_events("job-1", "session-2");
        let raw = fs::read_to_string(&path).expect("events");
        let mut events = raw
            .lines()
            .map(|line| {
                serde_json::from_str::<deadreckon_protocol::OperatorCaptureEvent>(line)
                    .expect("event json")
            })
            .collect::<Vec<_>>();
        events[1].subject = "tampered".to_string();
        fs::write(
            &path,
            format!(
                "{}\n",
                events
                    .iter()
                    .map(|event| serde_json::to_string(event).expect("event json"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        )
        .expect("tamper event");
        let error = read_operator_capture_history(&paths, "job-1", "session-2")
            .expect_err("signature mutation");
        assert!(error.to_string().contains("signature verification failed"));

        fs::write(&path, b"{").expect("torn row");
        let error =
            read_operator_capture_history(&paths, "job-1", "session-2").expect_err("torn row");
        assert!(error.to_string().contains("torn final row"));
    }

    #[cfg(unix)]
    #[test]
    fn history_reader_refuses_symlink_targets() {
        use std::os::unix::fs::symlink;

        let (temp, paths, unsigned) = fixture("history-symlink");
        let _binding = prepared_binding(&paths, &unsigned);
        let path = paths.operator_capture_events("job-1", "history-symlink");
        let target = temp.path().join("redirected-history.jsonl");
        fs::rename(&path, &target).expect("move history target");
        symlink(&target, &path).expect("history symlink");

        let error = read_operator_capture_history(&paths, "job-1", "history-symlink")
            .expect_err("symlink history must be refused");

        assert!(error.to_string().contains("regular non-symlink"));
    }

    #[test]
    fn history_reader_refuses_oversized_files_before_allocating_them() {
        let (_temp, paths, unsigned) = fixture("history-oversized");
        let _binding = prepared_binding(&paths, &unsigned);
        let path = paths.operator_capture_events("job-1", "history-oversized");
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("history")
            .set_len(super::MAX_CAPTURE_HISTORY_BYTES + 1)
            .expect("sparse oversized history");

        let error = read_operator_capture_history(&paths, "job-1", "history-oversized")
            .expect_err("oversized history must be refused");

        assert!(error.to_string().contains("trusted read size bound"));
    }

    #[test]
    fn locked_append_recovers_only_the_uncommitted_torn_tail() {
        let (_temp, paths, unsigned) = fixture("history-torn-recovery");
        let binding = prepared_binding(&paths, &unsigned);
        let path = paths.operator_capture_events("job-1", "history-torn-recovery");
        let committed = fs::read(&path).expect("committed history");
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("history append");
        std::io::Write::write_all(&mut file, br#"{"schema_version":"#).expect("partial event");
        file.sync_all().expect("partial event sync");

        let error = read_operator_capture_history(&paths, "job-1", "history-torn-recovery")
            .expect_err("unlocked reads must not repair history");
        assert!(error.to_string().contains("uncommitted torn final row"));
        assert!(fs::read(&path).expect("still torn").len() > committed.len());

        let appended =
            append_operator_capture_event(&paths, &binding, &draft("event-1", "before", 1))
                .expect("locked retry repairs and appends");
        assert_eq!(appended.sequence.get(), 2);
        let recovered = fs::read(&path).expect("recovered history");
        assert!(recovered.starts_with(&committed));
        assert!(recovered.ends_with(b"\n"));
        assert_eq!(
            read_operator_capture_history(&paths, "job-1", "history-torn-recovery")
                .expect("authenticated recovery")
                .events()
                .len(),
            2
        );
    }

    #[test]
    fn crash_before_commit_newline_replays_the_same_event_idempotently() {
        let (_temp, paths, unsigned) = fixture("history-newline-crash");
        let binding = prepared_binding(&paths, &unsigned);
        let event_draft = draft("event-1", "before", 1);
        let path = paths.operator_capture_events("job-1", "history-newline-crash");
        let committed_prefix = fs::read(&path).expect("committed prefix");
        let (pending, raw) = persist_uncheckpointed_event(&paths, &binding, &event_draft, false);
        assert!(
            fs::read(&path)
                .expect("payload-only history")
                .ends_with(&raw)
        );

        let retried = append_operator_capture_event(&paths, &binding, &event_draft)
            .expect("retry commits the same event");

        assert_eq!(retried, pending);
        let recovered = fs::read(&path).expect("recovered bytes");
        assert!(recovered.starts_with(&committed_prefix));
        assert_eq!(
            recovered.len(),
            committed_prefix.len() + raw.len() + 1,
            "payload-only tail is truncated before the exact row is replayed"
        );
        assert!(recovered.ends_with(b"\n"));
    }

    #[test]
    fn committed_row_without_head_requires_its_exact_retry() {
        let (_temp, paths, unsigned) = fixture("history-pending-head");
        let binding = prepared_binding(&paths, &unsigned);
        let event_draft = draft("event-1", "before", 1);
        let path = paths.operator_capture_events("job-1", "history-pending-head");
        let (pending, _) = persist_uncheckpointed_event(&paths, &binding, &event_draft, true);
        let pending_bytes = fs::read(&path).expect("pending history");

        let error = read_operator_capture_history(&paths, "job-1", "history-pending-head")
            .expect_err("uncheckpointed row is not public history");
        assert!(error.to_string().contains("durable history head"));
        let error =
            append_operator_capture_event(&paths, &binding, &draft("different-event", "before", 2))
                .expect_err("different append cannot adopt pending row");
        assert!(error.to_string().contains("only an exact event retry"));
        assert_eq!(
            fs::read(&path).expect("unchanged pending history"),
            pending_bytes
        );

        let recovered = append_operator_capture_event(&paths, &binding, &event_draft)
            .expect("exact retry anchors pending row");
        assert_eq!(recovered, pending);
        assert_eq!(
            fs::read(&path).expect("no duplicate row"),
            pending_bytes,
            "head recovery must not append the row twice"
        );
        assert_eq!(
            read_operator_capture_history(&paths, "job-1", "history-pending-head")
                .expect("anchored history")
                .events()
                .len(),
            2
        );
    }

    #[cfg(unix)]
    #[test]
    fn exact_duplicate_retry_must_resync_the_durable_head() {
        use std::os::unix::fs::PermissionsExt as _;

        let (_temp, paths, unsigned) = fixture("history-duplicate-sync");
        let binding = prepared_binding(&paths, &unsigned);
        let event_draft = draft("event-1", "before", 1);
        let first =
            append_operator_capture_event(&paths, &binding, &event_draft).expect("first append");
        let head_path = super::capture_history_head_path(&paths, &binding);
        fs::set_permissions(&head_path, fs::Permissions::from_mode(0o400)).expect("read-only head");

        append_operator_capture_event(&paths, &binding, &event_draft)
            .expect_err("duplicate cannot report durable success when head cannot be resynced");

        fs::set_permissions(&head_path, fs::Permissions::from_mode(0o600)).expect("writable head");
        let duplicate = append_operator_capture_event(&paths, &binding, &event_draft)
            .expect("duplicate resync");
        assert_eq!(duplicate, first);
    }

    #[test]
    fn valid_committed_suffix_truncation_is_rejected_by_head() {
        let (_temp, paths, unsigned) = fixture("history-truncated-suffix");
        let binding = prepared_binding(&paths, &unsigned);
        append_operator_capture_event(&paths, &binding, &draft("event-1", "before", 1))
            .expect("second committed row");
        let path = paths.operator_capture_events("job-1", "history-truncated-suffix");
        let raw = fs::read(&path).expect("history");
        let first_row_len = raw
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .expect("first commit newline");
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("history")
            .set_len(u64::try_from(first_row_len).expect("first row length"))
            .expect("truncate signed suffix");

        let error = read_operator_capture_history(&paths, "job-1", "history-truncated-suffix")
            .expect_err("head detects suffix truncation");
        assert!(error.to_string().contains("shorter than"));
        let error = append_operator_capture_event(&paths, &binding, &draft("event-2", "before", 2))
            .expect_err("append cannot continue truncated history");
        assert!(error.to_string().contains("shorter than"));
    }

    #[test]
    fn missing_forged_and_rolled_back_heads_fail_closed() {
        let (_temp, paths, unsigned) = fixture("history-head-attacks");
        let binding = prepared_binding(&paths, &unsigned);
        let head_path = super::capture_history_head_path(&paths, &binding);
        let first_head = fs::read(&head_path).expect("first head");
        append_operator_capture_event(&paths, &binding, &draft("event-1", "before", 1))
            .expect("second row");
        append_operator_capture_event(&paths, &binding, &draft("event-2", "before", 2))
            .expect("third row");
        let current_head = fs::read(&head_path).expect("current head");

        fs::write(&head_path, &first_head).expect("roll back signed head");
        let error = read_operator_capture_history(&paths, "job-1", "history-head-attacks")
            .expect_err("rolled-back head");
        assert!(
            error
                .to_string()
                .contains("more than one committed row ahead")
        );

        fs::write(&head_path, &current_head).expect("restore head");
        let mut forged: Value = serde_json::from_slice(&current_head).expect("head json");
        forged["history_bytes"] = Value::from(1_u64);
        fs::write(
            &head_path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&forged).expect("forged head")
            ),
        )
        .expect("forge head");
        let error = read_operator_capture_history(&paths, "job-1", "history-head-attacks")
            .expect_err("forged head");
        assert!(error.to_string().contains("signature verification failed"));

        fs::remove_file(&head_path).expect("remove head");
        let error = read_operator_capture_history(&paths, "job-1", "history-head-attacks")
            .expect_err("missing nonempty head");
        assert!(error.to_string().contains("no authenticated durable head"));
    }

    #[test]
    fn seal_exact_retry_recovers_final_row_before_head_publication() {
        let (_temp, paths, unsigned) = fixture("history-final-head-crash");
        let binding = prepared_binding(&paths, &unsigned);
        let issued_at = Utc
            .with_ymd_and_hms(2026, 7, 30, 12, 1, 0)
            .single()
            .expect("timestamp");
        let final_draft = OperatorCaptureEventDraft {
            event_id: format!("finalize:{}", digest('d').trim_start_matches("sha256:")),
            causation_id: format!("{:?}", OperatorCaptureStatus::Failed),
            timestamp: issued_at,
            phase: OperatorCapturePhase::Finalized,
            kind: OperatorCaptureEventKind::ResultFinalized,
            provenance: OperatorCaptureProvenance::TrustedSupervisor,
            source: OperatorCaptureSource::ResultEnvelope,
            subject: "result".to_string(),
            content_sha256: digest('d'),
            content_bytes: 42,
        };
        persist_uncheckpointed_event(&paths, &binding, &final_draft, true);

        let receipt = seal_operator_capture_receipt(
            &paths,
            &binding,
            issued_at + chrono::Duration::seconds(1),
            &digest('d'),
            42,
            OperatorCaptureStatus::Failed,
            None,
        )
        .expect("exact seal retry anchors row before receipt");

        assert_eq!(receipt.status, OperatorCaptureStatus::Failed);
        validate_operator_capture_receipt(&paths, &binding).expect("durable receipt chain");
    }

    #[cfg(unix)]
    #[test]
    fn stable_reader_refuses_device_handles() {
        let error = super::read_stable_regular_file(
            Path::new("/dev/null"),
            "job-1",
            "device-reader",
            "capture artifact",
        )
        .expect_err("device is not a stable regular file");
        assert!(error.to_string().contains("regular non-symlink"));
    }

    #[test]
    fn locked_seal_recovers_an_uncommitted_tail_before_finalizing() {
        let (_temp, paths, unsigned) = fixture("history-seal-recovery");
        let binding = prepared_binding(&paths, &unsigned);
        let path = paths.operator_capture_events("job-1", "history-seal-recovery");
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("history append");
        std::io::Write::write_all(&mut file, br#"{"schema_version":"#).expect("partial event");
        file.sync_all().expect("partial event sync");
        let issued_at = Utc
            .with_ymd_and_hms(2026, 7, 30, 12, 1, 0)
            .single()
            .expect("timestamp");

        let receipt = seal_operator_capture_receipt(
            &paths,
            &binding,
            issued_at,
            &digest('d'),
            42,
            OperatorCaptureStatus::Failed,
            None,
        )
        .expect("locked seal repairs and finalizes");

        assert_eq!(receipt.status, OperatorCaptureStatus::Failed);
        assert_eq!(
            read_operator_capture_history(&paths, "job-1", "history-seal-recovery")
                .expect("authenticated finalized history")
                .events()
                .len(),
            2
        );
        assert!(fs::read(&path).expect("finalized history").ends_with(b"\n"));
    }

    #[test]
    fn locked_recovery_never_discards_an_invalid_committed_row() {
        let (_temp, paths, unsigned) = fixture("history-invalid-committed");
        let binding = prepared_binding(&paths, &unsigned);
        let path = paths.operator_capture_events("job-1", "history-invalid-committed");
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("history append");
        std::io::Write::write_all(&mut file, b"{\"schema_version\":1}\n")
            .expect("invalid committed row");
        file.sync_all().expect("invalid row sync");
        let corrupted = fs::read(&path).expect("corrupted history");

        let error = append_operator_capture_event(
            &paths,
            &binding,
            &draft("event-after-corruption", "before", 2),
        )
        .expect_err("committed invalid row must fail closed");

        assert!(error.to_string().contains("capture history is corrupt"));
        assert_eq!(
            fs::read(&path).expect("invalid row retained"),
            corrupted,
            "recovery may truncate only bytes after the final commit newline"
        );
    }

    #[test]
    fn receipt_signature_and_history_head_are_revalidated() {
        let (_temp, paths, unsigned) = fixture("session-3");
        let binding = prepared_binding(&paths, &unsigned);
        append_operator_capture_event(&paths, &binding, &draft("event-1", "before", 1))
            .expect("event");
        let issued_at = Utc
            .with_ymd_and_hms(2026, 7, 30, 12, 1, 0)
            .single()
            .expect("timestamp");
        seal_operator_capture_receipt(
            &paths,
            &binding,
            issued_at,
            &digest('d'),
            42,
            OperatorCaptureStatus::Failed,
            None,
        )
        .expect("receipt");
        let path = paths.operator_capture_receipt("job-1", "session-3");
        let mut receipt: Value =
            serde_json::from_slice(&fs::read(&path).expect("receipt")).expect("receipt json");
        receipt["result_sha256"] = Value::String(digest('e'));
        fs::write(
            &path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&receipt).expect("receipt json")
            ),
        )
        .expect("tamper receipt");
        let error = validate_operator_capture_receipt(&paths, &binding)
            .expect_err("receipt signature mutation");
        assert!(
            error.to_string().contains("signature verification failed")
                || error
                    .to_string()
                    .contains("does not match its binding and complete event history")
        );
    }

    #[test]
    fn gaps_foreign_bindings_and_broken_chains_are_rejected() {
        let (_temp, paths, unsigned) = fixture("gap");
        let binding = write_operator_capture_binding(&paths, &unsigned).expect("binding");
        let key = read_gate_key(&paths, "job-1").expect("key");
        let binding_sha256 = operator_capture_binding_sha256(&binding).expect("binding digest");
        let gap = build_signed_event(
            &binding,
            &binding_sha256,
            OperatorCaptureEventSequence::new(2).expect("sequence"),
            None,
            &draft("event-2", "gap", 2),
            &key,
        )
        .expect("signed gap");
        append_synced_json_line(&paths.operator_capture_events("job-1", "gap"), &gap)
            .expect("append gap");
        let error =
            read_operator_capture_history(&paths, "job-1", "gap").expect_err("sequence gap");
        assert!(
            error
                .to_string()
                .contains("expected capture event sequence 1")
        );

        let (_temp, paths, unsigned) = fixture("foreign");
        let binding = prepared_binding(&paths, &unsigned);
        let key = read_gate_key(&paths, "job-1").expect("key");
        let mut foreign = build_signed_event(
            &binding,
            &digest('e'),
            OperatorCaptureEventSequence::new(1).expect("sequence"),
            None,
            &draft("event-1", "foreign", 1),
            &key,
        )
        .expect("foreign event");
        foreign.signature = super::sign_event(&foreign, &key).expect("foreign signature");
        append_synced_json_line(&paths.operator_capture_events("job-1", "foreign"), &foreign)
            .expect("append foreign");
        let error =
            read_operator_capture_history(&paths, "job-1", "foreign").expect_err("foreign binding");
        assert!(error.to_string().contains("foreign binding"));

        let (_temp, paths, unsigned) = fixture("chain");
        let binding = prepared_binding(&paths, &unsigned);
        append_operator_capture_event(&paths, &binding, &draft("event-1", "before", 1))
            .expect("first");
        append_operator_capture_event(&paths, &binding, &draft("event-2", "after", 2))
            .expect("second");
        let path = paths.operator_capture_events("job-1", "chain");
        let raw = fs::read_to_string(&path).expect("history");
        let mut events = raw
            .lines()
            .map(|line| {
                serde_json::from_str::<deadreckon_protocol::OperatorCaptureEvent>(line)
                    .expect("event")
            })
            .collect::<Vec<_>>();
        events[2].previous_event_sha256 = Some(digest('f'));
        let key = read_gate_key(&paths, "job-1").expect("key");
        events[2].signature = super::sign_event(&events[2], &key).expect("resign");
        fs::write(
            &path,
            format!(
                "{}\n{}\n{}\n",
                serde_json::to_string(&events[0]).expect("first"),
                serde_json::to_string(&events[1]).expect("second"),
                serde_json::to_string(&events[2]).expect("third")
            ),
        )
        .expect("rewrite chain");
        let error =
            read_operator_capture_history(&paths, "job-1", "chain").expect_err("broken chain");
        assert!(error.to_string().contains("breaks the hash chain"));
    }

    #[test]
    fn closed_source_mapping_refuses_relabelled_objective_events() {
        let (_temp, paths, unsigned) = fixture("source-map");
        let binding = prepared_binding(&paths, &unsigned);
        let mut relabelled = draft("forged-intervention", "before", 1);
        relabelled.phase = OperatorCapturePhase::Intervention;
        relabelled.kind = OperatorCaptureEventKind::InterventionRecorded;
        let error = append_operator_capture_event(&paths, &binding, &relabelled)
            .expect_err("JobView cannot mint intervention");
        assert!(
            error
                .to_string()
                .contains("source cannot mint the requested phase")
        );
    }

    #[test]
    fn manual_attestation_cannot_satisfy_pass_coverage() {
        let (_temp, paths, unsigned) = fixture("manual-coverage");
        let binding = prepared_binding(&paths, &unsigned);
        let mut manual = draft("manual-before", "before", 1);
        manual.kind = OperatorCaptureEventKind::OperatorAttestation;
        manual.provenance = OperatorCaptureProvenance::OperatorAttested;
        manual.source = OperatorCaptureSource::ManualFile;
        append_operator_capture_event(&paths, &binding, &manual).expect("manual fact");
        append_operator_capture_event(
            &paths,
            &binding,
            &lifecycle_draft(
                "intervention",
                OperatorCapturePhase::Intervention,
                OperatorCaptureEventKind::InterventionRecorded,
                2,
            ),
        )
        .expect("intervention");
        append_operator_capture_event(
            &paths,
            &binding,
            &lifecycle_draft(
                "cleanup",
                OperatorCapturePhase::Cleanup,
                OperatorCaptureEventKind::CleanupRecorded,
                3,
            ),
        )
        .expect("cleanup");
        let error = seal_operator_capture_receipt(
            &paths,
            &binding,
            Utc.with_ymd_and_hms(2026, 7, 30, 12, 1, 0)
                .single()
                .expect("timestamp"),
            &digest('d'),
            42,
            OperatorCaptureStatus::Passed,
            Some(completion_lineage()),
        )
        .expect_err("manual evidence cannot pass");
        assert!(
            error
                .to_string()
                .contains("requires objective subject before")
        );
    }
}
