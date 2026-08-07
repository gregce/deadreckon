//! Typed vocabulary for `notify.jsonl` lines.
//!
//! `notify.jsonl` is a per-run, append-only, display-only ledger. It carries
//! two row shapes: the historical notification delivery attempt and the typed
//! operator-attention signal a desktop companion turns into a real user
//! notification. Rows confer no authority — acceptance is proven only by the
//! signed marker and validated receipt — and the binary never consumes them.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Version of the operator-attention row shape.
pub const NOTIFY_EVENT_SCHEMA_VERSION: u32 = 1;

/// Literal `kind` discriminator of an operator-attention row. Delivery
/// attempt rows carry no `kind` field at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperatorAttentionKind {
    OperatorAttention,
}

/// Why the operator's attention is requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OperatorAttentionReason {
    /// A two-key completion receipt was sealed: the result is verified and
    /// waits for the operator to promote it.
    VerifiedAwaitingPromote,
    /// The run loop stopped at a spend or wall-time cap.
    PausedAtCap,
    /// The supervisor classified the Job as blocked.
    Blocked,
    /// The supervisor classified the Job as failed.
    Failed,
    /// The supervisor classified the Job as cancelled.
    Cancelled,
    /// The Job waits for operator review before it can continue.
    WaitingInput,
}

/// One display-only operator-attention signal.
///
/// The row is a rendering aid for external observers (see `docs/TAILING.md`):
/// it never carries authority, and deadreckon itself never reads it back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OperatorAttentionEvent {
    pub schema_version: u32,
    pub kind: OperatorAttentionKind,
    pub reason: OperatorAttentionReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub at: DateTime<Utc>,
    /// One operator-readable line in glossary user words.
    pub summary: String,
    /// Real CLI commands the operator can run next.
    pub next_actions: Vec<String>,
}

/// Delivery outcome vocabulary of the historical attempt rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NotifyDeliveryTransition {
    Accepted,
    Paused,
    Failed,
}

/// One notification delivery attempt — the historical `notify.jsonl` shape.
/// Rows record *attempts*, including failures (`ok: false` with a `detail`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NotifyDeliveryAttempt {
    pub ts: DateTime<Utc>,
    pub transition: NotifyDeliveryTransition,
    pub channel: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// One line of `notify.jsonl`: either row shape, distinguished by the
/// presence of the `kind` field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum NotifyEvent {
    OperatorAttention(OperatorAttentionEvent),
    DeliveryAttempt(NotifyDeliveryAttempt),
}
