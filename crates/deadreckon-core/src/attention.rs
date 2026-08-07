//! Display-only operator-attention signals appended to `notify.jsonl`.
//!
//! Each emitter is the process that owns the underlying fact: the sealing
//! path appends `verified_awaiting_promote` when the two-key completion
//! receipt lands, the run-loop process appends `paused_at_cap` when it stops
//! at a cap, and the supervisor appends `blocked`/`failed`/`cancelled`/
//! `waiting_input` when it classifies a Job. Rows are a rendering aid for
//! external observers (docs/TAILING.md): they carry no authority and the
//! binary never reads them back. Appends are best-effort — a failed append
//! must never fail the transition that produced the fact.

use std::path::{Path, PathBuf};

use chrono::Utc;
use deadreckon_protocol::{
    JobOutcome, NOTIFY_EVENT_SCHEMA_VERSION, OperatorAttentionEvent, OperatorAttentionKind,
    OperatorAttentionReason, StopReason,
};

use crate::Result;
use crate::glossary::{NOUN_VERIFIED_RUN, PHRASE_VERIFIED_BY_DR_GATE};

pub const NOTIFY_JSONL: &str = "notify.jsonl";

pub fn notify_jsonl_path(run_root: &Path) -> PathBuf {
    run_root.join(NOTIFY_JSONL)
}

/// Append one operator-attention row to the run's `notify.jsonl`.
pub fn append_operator_attention(run_root: &Path, event: &OperatorAttentionEvent) -> Result<()> {
    crate::state::append_json_line(&notify_jsonl_path(run_root), event)
}

fn attention_event(
    reason: OperatorAttentionReason,
    job_id: Option<&str>,
    run_id: Option<&str>,
    scope: Option<&str>,
    summary: String,
    next_actions: Vec<String>,
) -> OperatorAttentionEvent {
    OperatorAttentionEvent {
        schema_version: NOTIFY_EVENT_SCHEMA_VERSION,
        kind: OperatorAttentionKind::OperatorAttention,
        reason,
        job_id: job_id.map(ToString::to_string),
        run_id: run_id.map(ToString::to_string),
        scope: scope.map(ToString::to_string),
        at: Utc::now(),
        summary,
        next_actions,
    }
}

/// The two-key completion receipt was sealed: the result is verified and
/// waits for the operator to promote it.
pub fn verified_awaiting_promote_event(
    job_id: &str,
    run_id: &str,
    scope: &str,
) -> OperatorAttentionEvent {
    attention_event(
        OperatorAttentionReason::VerifiedAwaitingPromote,
        Some(job_id),
        Some(run_id),
        Some(scope),
        format!(
            "{NOUN_VERIFIED_RUN} {run_id} is {PHRASE_VERIFIED_BY_DR_GATE} and waiting for you to promote it"
        ),
        vec![
            format!("deadreckon finish {run_id}"),
            format!("deadreckon verdict {run_id} --receipt"),
        ],
    )
}

/// The run loop stopped at a spend or wall-time cap.
pub fn paused_at_cap_event(
    run_id: &str,
    scope: &str,
    pause_reason: Option<&str>,
) -> OperatorAttentionEvent {
    let summary = match pause_reason {
        Some(reason) => format!("run {run_id} paused at cap: {reason}"),
        None => format!("run {run_id} paused at cap"),
    };
    attention_event(
        OperatorAttentionReason::PausedAtCap,
        None,
        Some(run_id),
        Some(scope),
        summary,
        vec![
            format!("deadreckon resume {run_id}"),
            format!("deadreckon show {run_id}"),
        ],
    )
}

/// Operator-attention row for a supervisor terminal classification, or
/// `None` when the classification asks nothing of the operator here
/// (`Verified` is announced by the sealing path, `BudgetExhausted` by the
/// run loop's own `paused_at_cap`).
pub fn supervisor_classified_event(
    job_id: &str,
    run_id: Option<&str>,
    scope: &str,
    outcome: JobOutcome,
    stop_reason: Option<StopReason>,
) -> Option<OperatorAttentionEvent> {
    let detail = stop_reason.map(stop_reason_phrase);
    let with_detail = |lead: String| match detail {
        Some(detail) => format!("{lead}: {detail}"),
        None => lead,
    };
    let (reason, summary, next_actions) = match outcome {
        JobOutcome::Blocked => (
            OperatorAttentionReason::Blocked,
            with_detail(format!("job {job_id} is blocked")),
            vec![format!("deadreckon status {job_id}")],
        ),
        JobOutcome::Failed => (
            OperatorAttentionReason::Failed,
            with_detail(format!("job {job_id} failed")),
            vec![
                format!("deadreckon status {job_id}"),
                format!("deadreckon extend {job_id} '<your follow-up goal>'"),
            ],
        ),
        JobOutcome::Cancelled => (
            OperatorAttentionReason::Cancelled,
            format!("job {job_id} was cancelled"),
            vec![format!("deadreckon status {job_id}")],
        ),
        JobOutcome::NeedsReview => (
            OperatorAttentionReason::WaitingInput,
            with_detail(format!("job {job_id} needs your review")),
            vec![
                format!("deadreckon verdict {job_id}"),
                format!("deadreckon extend {job_id} '<your follow-up goal>'"),
            ],
        ),
        JobOutcome::Verified
        | JobOutcome::BudgetExhausted
        | JobOutcome::DeadlineReached
        | JobOutcome::RetryExhausted => return None,
    };
    Some(attention_event(
        reason,
        Some(job_id),
        run_id,
        Some(scope),
        summary,
        next_actions,
    ))
}

fn stop_reason_phrase(reason: StopReason) -> &'static str {
    match reason {
        StopReason::Verified => "verified",
        StopReason::SemanticRevise => "the semantic judge asked for revision",
        StopReason::SemanticUncertain => "the semantic judge was uncertain",
        StopReason::SemanticUnavailable => "the semantic judge was unavailable",
        StopReason::OperatorInputRequired => "waiting for operator input",
        StopReason::SpendCap => "the spend cap was reached",
        StopReason::WallCap => "the wall-time cap was reached",
        StopReason::Deadline => "the deadline was reached",
        StopReason::AttemptLimit => "the attempt limit was reached",
        StopReason::CancelRequested => "cancellation was requested",
        StopReason::DeterministicRevise => "the done contract asked for revision",
        StopReason::TransientProvider => "the provider failed transiently",
        StopReason::FatalProvider => "the provider failed",
        StopReason::FatalGate => "the completion gate failed",
        StopReason::LostContainment => "process containment was lost",
        StopReason::SupervisorFailure => "the supervisor failed",
        StopReason::CorruptHistory => "the job history is corrupt",
        StopReason::LegacyUnknown => "the stop reason is unknown",
    }
}
