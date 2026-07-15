// Wired into the codex/claude drivers across phases P5–P9; the `dead_code`
// allow is removed once every item has a caller.
#![allow(dead_code)]
//! Shared machinery for CLI agent wire contracts (Semaphore).
//!
//! Provider-neutral: the codex and claude mirrors each translate their JSONL
//! into the [`CliStreamEvent`] vocabulary defined here, and the shared driver
//! machinery consumes only that vocabulary. Nothing in this module branches on
//! codex/claude specifics, so a follow-up slice (Pennant) can drive the generic
//! fleet from descriptor TOML without a refactor.

/// One neutral fact extracted from a single JSONL stream line. A line may yield
/// several (a claude `result` line carries a session id, usage, and the answer
/// at once).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CliStreamEvent {
    /// The provider's conversation id (codex thread id / claude session id).
    Conversation(String),
    /// Token usage for the turn (+ optional reported cost, informational only).
    Usage(CliUsage),
    /// The final answer text carried in the structured result (claude only;
    /// codex answers arrive via `--output-last-message`).
    Answer(String),
    /// A tool/item row for the flight ledger.
    Tool(CliToolRow),
    /// A terminal failure reported by the stream.
    Failure(String),
    /// A recognized-but-uninteresting event (turn.started, assistant text …).
    Recognized,
    /// A JSON line whose `type` tag we do not model — preserved, never fatal.
    Unknown,
}

/// Token usage lifted from a provider's terminal event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CliUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Provider-reported cost in USD, informational only. Recorded in the turn
    /// trace detail; never moves `SpendEstimate` (subscription CLIs bill in
    /// time, not dollars).
    pub cost_usd: Option<f64>,
}

/// A tool-call row lifted from the stream for live flight ingestion.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CliToolRow {
    /// Provider item/tool id (`item_1`, `toolu_…`) — the live-ingestion key.
    pub id: String,
    pub tool_name: Option<String>,
    pub tool_category: Option<String>,
    pub summary: String,
    pub status: Option<String>,
    /// The raw JSONL line, stored verbatim in the flight ledger.
    pub raw: String,
}

/// Clamp a free-text tool summary to a bounded width for the flight ledger.
pub(crate) fn truncate_summary(text: &str) -> String {
    const MAX: usize = 200;
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(MAX).collect();
    out.push('…');
    out
}
