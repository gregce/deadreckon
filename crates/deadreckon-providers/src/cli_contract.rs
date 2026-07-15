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

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::ProviderError;

pub(crate) const PROVIDER_ID_CODEX: &str = "cli:codex";
pub(crate) const PROVIDER_ID_CLAUDE: &str = "cli:claude-code";

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

// ---------------------------------------------------------------------------
// provider-session.json — per-run conversation id (files, not PipelineState).
// ---------------------------------------------------------------------------

pub(crate) const SESSION_FILE_NAME: &str = "provider-session.json";

/// The per-run conversation record. Provider-scoped: a run whose provider
/// changes mid-life (rescue) ignores a session recorded by a different provider
/// name. Written atomically (temp file + rename). Absent file = first turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ProviderSession {
    pub schema: u32,
    pub provider: String,
    pub conversation_id: String,
    pub created_at: DateTime<Utc>,
    pub last_turn_at: DateTime<Utc>,
    #[serde(default)]
    pub resume_failures: u32,
}

impl ProviderSession {
    pub(crate) fn new(provider: &str, conversation_id: &str, now: DateTime<Utc>) -> Self {
        Self {
            schema: 1,
            provider: provider.to_string(),
            conversation_id: conversation_id.to_string(),
            created_at: now,
            last_turn_at: now,
            resume_failures: 0,
        }
    }

    fn path(session_dir: &Path) -> PathBuf {
        session_dir.join(SESSION_FILE_NAME)
    }

    /// A session resumes only when no prior resume has failed; `resume_failures
    /// >= 1` forces a fresh conversation next turn.
    pub(crate) fn can_resume(&self) -> bool {
        self.resume_failures == 0
    }

    pub(crate) fn touch(&mut self, now: DateTime<Utc>) {
        self.last_turn_at = now;
    }

    pub(crate) fn mark_resume_failure(&mut self, now: DateTime<Utc>) {
        self.resume_failures = self.resume_failures.saturating_add(1);
        self.last_turn_at = now;
    }

    /// Read the session for `provider` from `session_dir`. Returns `None` when
    /// the file is absent (first turn), unreadable, malformed, or recorded by a
    /// different provider — each of which means "start fresh", never an error.
    pub(crate) fn read(session_dir: &Path, provider: &str) -> Option<Self> {
        let raw = std::fs::read_to_string(Self::path(session_dir)).ok()?;
        let session: ProviderSession = serde_json::from_str(&raw).ok()?;
        (session.provider == provider).then_some(session)
    }

    /// Write the session atomically (temp file + rename).
    pub(crate) fn write(&self, session_dir: &Path) -> Result<(), ProviderError> {
        let path = Self::path(session_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ProviderError::Io {
                path: parent.display().to_string(),
                source,
            })?;
        }
        let body = serde_json::to_string_pretty(self).map_err(|source| ProviderError::Io {
            path: path.display().to_string(),
            source: std::io::Error::other(source),
        })?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, body).map_err(|source| ProviderError::Io {
            path: tmp.display().to_string(),
            source,
        })?;
        std::fs::rename(&tmp, &path).map_err(|source| ProviderError::Io {
            path: path.display().to_string(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).single().expect("ts")
    }

    #[test]
    fn session_file_roundtrips_and_scopes_by_provider() {
        let dir = tempfile::tempdir().expect("tempdir");
        let session = ProviderSession::new("cli:codex", "thread-abc", ts(1_000));
        session.write(dir.path()).expect("write");

        let read_back = ProviderSession::read(dir.path(), "cli:codex").expect("same provider");
        assert_eq!(read_back, session);

        // Provider-scoped: a different provider ignores the recorded session.
        assert!(ProviderSession::read(dir.path(), "cli:claude-code").is_none());
        // Absent dir = first turn, not an error.
        let empty = tempfile::tempdir().expect("tempdir2");
        assert!(ProviderSession::read(empty.path(), "cli:codex").is_none());
    }

    #[test]
    fn resume_failure_marks_session_for_fresh_conversation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut session = ProviderSession::new("cli:codex", "thread-abc", ts(1_000));
        assert!(session.can_resume(), "fresh session resumes");
        session.mark_resume_failure(ts(2_000));
        session.write(dir.path()).expect("write");

        let read_back = ProviderSession::read(dir.path(), "cli:codex").expect("read");
        assert_eq!(read_back.resume_failures, 1);
        assert!(
            !read_back.can_resume(),
            "a failed resume forces a fresh conversation next turn"
        );
    }
}
