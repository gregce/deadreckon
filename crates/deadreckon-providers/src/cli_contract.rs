//! Shared machinery for CLI agent wire contracts (Semaphore).
//!
//! Provider-neutral: the codex and claude mirrors each translate their JSONL
//! into the [`CliStreamEvent`] vocabulary defined here, and the shared driver
//! machinery consumes only that vocabulary. Nothing in this module branches on
//! codex/claude specifics, so a follow-up slice (Pennant) can drive the generic
//! fleet from descriptor TOML without a refactor.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ProviderError;
use crate::registry::{ContractDialect, ContractSection};

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

type EventMirror = fn(&str) -> Option<Vec<CliStreamEvent>>;

/// One provider-neutral entry into Semaphore's parse/degrade/flight
/// machinery. Codex and Claude construct it with their richer event mirrors;
/// generic providers construct it from descriptor TOML.
#[derive(Clone)]
pub(crate) struct ProviderContract {
    parser: ContractParser,
}

#[derive(Clone)]
enum ContractParser {
    EventMirror(EventMirror),
    Descriptor(Box<ContractSection>),
}

type ContractProbeCache = Mutex<HashMap<(String, String), Option<String>>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContractError {
    pub field: &'static str,
    pub detail: String,
}

impl std::fmt::Display for ContractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.detail)
    }
}

impl ProviderContract {
    pub(crate) fn from_descriptor(section: &ContractSection) -> Result<Self, ContractError> {
        section.validate().map_err(|error| ContractError {
            field: error.field,
            detail: error.detail,
        })?;
        Ok(Self {
            parser: ContractParser::Descriptor(Box::new(section.clone())),
        })
    }

    pub(crate) fn from_event_mirror(parser: EventMirror) -> Self {
        Self {
            parser: ContractParser::EventMirror(parser),
        }
    }

    pub(crate) fn parse(&self, stdout: &str) -> DescriptorParse {
        match &self.parser {
            ContractParser::EventMirror(parser) => DescriptorParse {
                parsed: parse_stream(stdout, *parser),
                missing_fields: Vec::new(),
            },
            ContractParser::Descriptor(section) => extract_descriptor_output(section, stdout),
        }
    }

    #[allow(dead_code)] // Generic driver wiring lands in Pennant P4.
    pub(crate) fn descriptor(&self) -> Option<&ContractSection> {
        match &self.parser {
            ContractParser::Descriptor(section) => Some(section),
            ContractParser::EventMirror(_) => None,
        }
    }
}

/// Result of applying the optional descriptor probe expectation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContractProbe {
    pub active: bool,
    pub caveat: Option<String>,
}

pub(crate) fn evaluate_contract_probe(
    binary: &str,
    contract: &ProviderContract,
    help: Option<&str>,
) -> ContractProbe {
    let Some(section) = contract.descriptor() else {
        return ContractProbe {
            active: true,
            caveat: None,
        };
    };
    let Some(expected) = section.probe_substring.as_deref() else {
        return ContractProbe {
            active: true,
            caveat: None,
        };
    };
    if help.is_some_and(|help| help.contains(expected)) {
        ContractProbe {
            active: true,
            caveat: None,
        }
    } else {
        ContractProbe {
            active: false,
            caveat: Some(format!(
                "installed {binary} predates its contract; upgrade to enable token accounting"
            )),
        }
    }
}

/// Probe `binary --help` once per binary/expectation pair. Failure to execute
/// or a missing marker disables the contract with a caveat, never an error.
#[allow(dead_code)] // Generic driver wiring lands in Pennant P4.
pub(crate) fn probe_descriptor_contract(
    binary: &str,
    contract: &ProviderContract,
) -> ContractProbe {
    let expected = contract
        .descriptor()
        .and_then(|section| section.probe_substring.as_deref());
    let Some(expected) = expected else {
        return ContractProbe {
            active: true,
            caveat: None,
        };
    };
    static CACHE: OnceLock<ContractProbeCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (binary.to_string(), expected.to_string());
    if let Ok(guard) = cache.lock()
        && let Some(found) = guard.get(&key)
    {
        return evaluate_contract_probe(binary, contract, found.as_deref());
    }
    let help = Command::new(binary)
        .arg("--help")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            format!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, help.clone());
    }
    evaluate_contract_probe(binary, contract, help.as_deref())
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

// ---------------------------------------------------------------------------
// Tolerant stream fold + degraded detection (driver machinery).
// ---------------------------------------------------------------------------

/// Result of folding a provider's stdout through its line parser.
#[derive(Debug, Clone, Default)]
pub(crate) struct ParsedStream {
    pub conversation_id: Option<String>,
    pub usage: Option<CliUsage>,
    pub answer: Option<String>,
    pub tool_rows: Vec<CliToolRow>,
    pub failure: Option<String>,
    pub unknown_lines: u64,
    pub garbage_lines: u64,
    pub structured_events: u64,
}

impl ParsedStream {
    /// Nothing structured could be read — an old binary predating the JSONL
    /// flags, or output that is not the contract at all. The driver falls back
    /// to raw stdout with a caveat instead of failing the turn.
    pub(crate) fn degraded(&self) -> bool {
        self.structured_events == 0
    }
}

/// Fold a provider's stdout through its per-line parser. `parse_line` returns
/// `None` for a line that is not JSON at all (counted as garbage, skipped);
/// `Some(events)` for a parsed line (possibly `[Unknown]`).
pub(crate) fn parse_stream(
    stdout: &str,
    parse_line: impl Fn(&str) -> Option<Vec<CliStreamEvent>>,
) -> ParsedStream {
    let mut parsed = ParsedStream::default();
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Some(events) = parse_line(line) else {
            parsed.garbage_lines += 1;
            continue;
        };
        for event in events {
            match event {
                CliStreamEvent::Conversation(id) => {
                    parsed.conversation_id = Some(id);
                    parsed.structured_events += 1;
                }
                CliStreamEvent::Usage(u) => {
                    parsed.usage = Some(u);
                    parsed.structured_events += 1;
                }
                CliStreamEvent::Answer(t) => {
                    parsed.answer = Some(t);
                    parsed.structured_events += 1;
                }
                CliStreamEvent::Tool(row) => {
                    upsert_tool_row(&mut parsed.tool_rows, row);
                    parsed.structured_events += 1;
                }
                CliStreamEvent::Failure(m) => {
                    parsed.failure = Some(m);
                    parsed.structured_events += 1;
                }
                CliStreamEvent::Recognized => {
                    parsed.structured_events += 1;
                }
                CliStreamEvent::Unknown => {
                    parsed.unknown_lines += 1;
                }
            }
        }
    }
    parsed
}

// ---------------------------------------------------------------------------
// Descriptor-declared JSON-pointer extraction (Pennant).
// ---------------------------------------------------------------------------

/// The shared stream facts plus declared pointers that did not resolve. A
/// missing pointer disables only that capability and becomes a caveat at the
/// driver boundary; it is never a parse error.
#[derive(Debug, Clone, Default)]
pub(crate) struct DescriptorParse {
    pub parsed: ParsedStream,
    #[allow(dead_code)] // Surfaced as generic-driver caveats in Pennant P4.
    pub missing_fields: Vec<&'static str>,
}

/// Extract a descriptor contract from stdout. JSONL terminal facts use
/// last-resolution-wins, except the conversation id, which is deliberately
/// pinned to the first resolution so later status rows cannot switch sessions.
pub(crate) fn extract_descriptor_output(
    contract: &ContractSection,
    stdout: &str,
) -> DescriptorParse {
    let mut state = DescriptorExtractionState::default();
    match contract.dialect {
        ContractDialect::JsonLines => {
            for (index, raw) in stdout.lines().enumerate() {
                if raw.trim().is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(raw) {
                    Ok(value) => {
                        state.parsed.structured_events += 1;
                        extract_descriptor_value(contract, &value, Some(raw), index, &mut state);
                    }
                    Err(_) => state.parsed.garbage_lines += 1,
                }
            }
        }
        ContractDialect::JsonDocument => match serde_json::from_str::<Value>(stdout) {
            Ok(value) => {
                state.parsed.structured_events = 1;
                extract_descriptor_value(contract, &value, None, 0, &mut state);
            }
            Err(_) => state.parsed.garbage_lines = u64::from(!stdout.trim().is_empty()),
        },
    }
    state.finish(contract)
}

#[derive(Debug, Default)]
struct DescriptorExtractionState {
    parsed: ParsedStream,
    usage_input: Option<u64>,
    usage_output: Option<u64>,
    cost: Option<f64>,
    seen_conversation: bool,
    seen_usage_input: bool,
    seen_usage_output: bool,
    seen_cost: bool,
    seen_answer: bool,
    seen_error_flag: bool,
    seen_error_message: bool,
}

impl DescriptorExtractionState {
    fn finish(mut self, contract: &ContractSection) -> DescriptorParse {
        if self.seen_usage_input || self.seen_usage_output || self.seen_cost {
            self.parsed.usage = Some(CliUsage {
                input_tokens: self.usage_input.unwrap_or(0),
                output_tokens: self.usage_output.unwrap_or(0),
                cost_usd: self.cost,
            });
        }
        let mut missing = Vec::new();
        for (declared, seen, field) in [
            (
                contract.conversation_id_path.is_some(),
                self.seen_conversation,
                "conversation_id_path",
            ),
            (
                contract.usage_input_path.is_some(),
                self.seen_usage_input,
                "usage_input_path",
            ),
            (
                contract.usage_output_path.is_some(),
                self.seen_usage_output,
                "usage_output_path",
            ),
            (contract.cost_path.is_some(), self.seen_cost, "cost_path"),
            (
                contract.answer_path.is_some(),
                self.seen_answer,
                "answer_path",
            ),
            (
                contract.error_flag_path.is_some(),
                self.seen_error_flag,
                "error_flag_path",
            ),
            (
                contract.error_message_path.is_some(),
                self.seen_error_message,
                "error_message_path",
            ),
        ] {
            if declared && !seen {
                missing.push(field);
            }
        }
        DescriptorParse {
            parsed: self.parsed,
            missing_fields: missing,
        }
    }
}

fn extract_descriptor_value(
    contract: &ContractSection,
    value: &Value,
    raw_line: Option<&str>,
    line_index: usize,
    state: &mut DescriptorExtractionState,
) {
    if !state.seen_conversation
        && let Some(found) = pointer_value(value, contract.conversation_id_path.as_deref())
    {
        state.seen_conversation = true;
        state.parsed.conversation_id = scalar_text(found);
    }
    if let Some(found) = pointer_value(value, contract.usage_input_path.as_deref()) {
        state.seen_usage_input = true;
        state.usage_input = token_count(found);
    }
    if let Some(found) = pointer_value(value, contract.usage_output_path.as_deref()) {
        state.seen_usage_output = true;
        state.usage_output = token_count(found);
    }
    if let Some(found) = pointer_value(value, contract.cost_path.as_deref()) {
        state.seen_cost = true;
        state.cost = number(found);
    }
    if let Some(found) = pointer_value(value, contract.answer_path.as_deref()) {
        state.seen_answer = true;
        state.parsed.answer = scalar_text(found);
    }

    let error_message = pointer_value(value, contract.error_message_path.as_deref());
    if let Some(found) = error_message {
        state.seen_error_message = true;
        if let Some(text) = scalar_text(found) {
            // Retain the last message in case the flag resolves on another
            // terminal row.
            state.parsed.failure = Some(text);
        }
    }
    if let Some(found) = pointer_value(value, contract.error_flag_path.as_deref()) {
        state.seen_error_flag = true;
        if truthy(found) {
            if state.parsed.failure.is_none() {
                state.parsed.failure = Some("provider reported an error".to_string());
            }
        } else {
            state.parsed.failure = None;
        }
    }

    if let Some(raw) = raw_line {
        for pointer in &contract.flight_event_paths {
            let Some(selected) = value.pointer(pointer) else {
                continue;
            };
            // A selector may point at a nested tool-request object (Copilot)
            // or at a scalar marker on the root event (Pi). Prefer the nested
            // object when it carries fields of its own; otherwise retain the
            // root event so sibling tool metadata remains visible.
            let event = if selected.is_object() {
                selected
            } else {
                value
            };
            let row = descriptor_flight_row(event, raw, line_index);
            upsert_tool_row(&mut state.parsed.tool_rows, row);
        }
    }
}

fn pointer_value<'a>(value: &'a Value, pointer: Option<&str>) -> Option<&'a Value> {
    pointer.and_then(|pointer| value.pointer(pointer))
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        other => serde_json::to_string(other).ok(),
    }
}

fn token_count(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::String(value) => !value.is_empty() && !value.eq_ignore_ascii_case("false"),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn descriptor_flight_row(value: &Value, raw: &str, line_index: usize) -> CliToolRow {
    let id = [
        "/tool_call_id",
        "/toolCallId",
        "/data/toolCallId",
        "/item/id",
        "/id",
        "/data/id",
    ]
    .iter()
    .find_map(|pointer| value.pointer(pointer).and_then(scalar_text))
    .unwrap_or_else(|| format!("descriptor:{:016x}", fnv1a(raw.as_bytes())));
    let tool_name = [
        "/tool_name",
        "/toolName",
        "/data/toolName",
        "/name",
        "/item/type",
    ]
    .iter()
    .find_map(|pointer| value.pointer(pointer).and_then(scalar_text));
    let tool_category = ["/tool_category", "/toolType", "/data/toolType", "/category"]
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(scalar_text));
    let summary = [
        "/summary",
        "/intentionSummary",
        "/message",
        "/text",
        "/content",
        "/arguments",
        "/args",
        "/result",
        "/data/result/content",
        "/data/arguments",
        "/data/partialOutput",
        "/data/inputDelta",
    ]
    .iter()
    .find_map(|pointer| value.pointer(pointer).and_then(scalar_text))
    .unwrap_or_else(|| format!("structured provider event {}", line_index + 1));
    let status = ["/status", "/state", "/type"]
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(scalar_text));
    CliToolRow {
        id,
        tool_name,
        tool_category,
        summary: truncate_summary(&summary),
        status,
        raw: raw.to_string(),
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// A later row for the same item id supersedes an earlier one (an
/// `item.completed` replaces the `item.started` it upgraded from), so the
/// ledger keeps one row per logical tool call in terminal state.
fn upsert_tool_row(rows: &mut Vec<CliToolRow>, row: CliToolRow) {
    if let Some(existing) = rows.iter_mut().find(|existing| existing.id == row.id) {
        let mut terminal = row;
        terminal.tool_name = terminal.tool_name.or_else(|| existing.tool_name.clone());
        terminal.tool_category = terminal
            .tool_category
            .or_else(|| existing.tool_category.clone());
        terminal.status = terminal.status.or_else(|| existing.status.clone());
        *existing = terminal;
    } else {
        rows.push(row);
    }
}

/// A tool-call row lifted from the live stream, carried in the response trace
/// (`trace.flight_rows`) for the runtime to ingest into the flight ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderFlightRow {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_category: Option<String>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub raw: String,
}

pub(crate) fn flight_rows_from(parsed: &ParsedStream) -> Vec<ProviderFlightRow> {
    parsed
        .tool_rows
        .iter()
        .map(|row| ProviderFlightRow {
            id: row.id.clone(),
            tool_name: row.tool_name.clone(),
            tool_category: row.tool_category.clone(),
            summary: row.summary.clone(),
            status: row.status.clone(),
            raw: row.raw.clone(),
        })
        .collect()
}

/// True when a nonzero-exit resume output looks like the conversation is gone
/// (as opposed to a transient error), so the driver retries once fresh.
pub(crate) fn session_not_found(output: &crate::cli_common::CliOutput) -> bool {
    let haystack = format!("{}\n{}", output.stdout, output.stderr).to_ascii_lowercase();
    [
        "session not found",
        "no such session",
        "unknown session",
        "conversation not found",
        "could not find session",
        "no rollout",
        "not found",
    ]
    .iter()
    .any(|marker| haystack.contains(marker))
}

/// Append a `{code, message}` caveat to the response trace. Degraded contracts,
/// session resets, and unsupported schema requests all surface this way instead
/// of failing the turn.
pub(crate) fn add_caveat(trace: &mut Value, code: &str, message: &str) {
    let caveat = serde_json::json!({ "code": code, "message": message });
    match trace.get_mut("caveats").and_then(Value::as_array_mut) {
        Some(arr) => arr.push(caveat),
        None => {
            if let Some(obj) = trace.as_object_mut() {
                obj.insert("caveats".to_string(), Value::Array(vec![caveat]));
            }
        }
    }
}

/// Write the request's JSON Schema to a file for codex `--output-schema`.
pub(crate) async fn write_schema_file(
    dir: &Path,
    schema: &Value,
) -> Result<PathBuf, ProviderError> {
    let path = dir.join("provider-output-schema.json");
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| ProviderError::Io {
                path: parent.display().to_string(),
                source,
            })?;
    }
    let body = serde_json::to_string_pretty(schema).map_err(|source| ProviderError::Io {
        path: path.display().to_string(),
        source: std::io::Error::other(source),
    })?;
    tokio::fs::write(&path, body)
        .await
        .map_err(|source| ProviderError::Io {
            path: path.display().to_string(),
            source,
        })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).single().expect("ts")
    }

    fn pointer_contract(dialect: ContractDialect) -> ContractSection {
        ContractSection {
            stream_args: vec!["--json".to_string()],
            dialect,
            conversation_id_path: Some("/session/id".to_string()),
            usage_input_path: Some("/usage/input".to_string()),
            usage_output_path: Some("/usage/output".to_string()),
            answer_path: Some("/answer".to_string()),
            ..ContractSection::default()
        }
    }

    #[test]
    fn descriptor_contract_flows_through_semaphore_machinery() {
        let section = pointer_contract(ContractDialect::JsonDocument);
        let contract = ProviderContract::from_descriptor(&section).expect("contract");
        let extracted = contract.parse(
            r#"{"session":{"id":"shared-1"},"usage":{"input":21,"output":22},"answer":"shared machinery"}"#,
        );
        assert_eq!(extracted.parsed.answer.as_deref(), Some("shared machinery"));
        assert_eq!(extracted.parsed.usage.expect("usage").input_tokens, 21);

        // A bespoke mirror enters through the same parse facade.
        let bespoke = ProviderContract::from_event_mirror(|line| {
            serde_json::from_str::<Value>(line)
                .ok()
                .map(|_| vec![CliStreamEvent::Recognized])
        });
        assert_eq!(
            bespoke
                .parse("{\"type\":\"event\"}\n")
                .parsed
                .structured_events,
            1
        );
    }

    #[test]
    fn probe_substring_miss_disables_contract_with_caveat() {
        let mut section = pointer_contract(ContractDialect::JsonDocument);
        section.probe_substring = Some("--structured".to_string());
        let contract = ProviderContract::from_descriptor(&section).expect("contract");
        let probe = evaluate_contract_probe("example-cli", &contract, Some("--plain only"));
        assert!(!probe.active);
        assert_eq!(
            probe.caveat.as_deref(),
            Some("installed example-cli predates its contract; upgrade to enable token accounting")
        );
    }

    #[test]
    fn pointers_extract_from_json_lines_last_wins() {
        let contract = pointer_contract(ContractDialect::JsonLines);
        let output = concat!(
            "{\"session\":{\"id\":\"s-1\"},\"usage\":{\"input\":3,\"output\":4},\"answer\":\"draft\"}\n",
            "{\"session\":{\"id\":\"s-2\"},\"usage\":{\"input\":7,\"output\":8},\"answer\":\"final\"}\n"
        );
        let extracted = extract_descriptor_output(&contract, output);
        assert_eq!(extracted.parsed.answer.as_deref(), Some("final"));
        assert_eq!(
            extracted.parsed.usage,
            Some(CliUsage {
                input_tokens: 7,
                output_tokens: 8,
                cost_usd: None,
            })
        );
        assert!(extracted.missing_fields.is_empty());
    }

    #[test]
    fn conversation_id_takes_first_resolution() {
        let contract = pointer_contract(ContractDialect::JsonLines);
        let output = concat!(
            "{\"session\":{\"id\":\"first\"}}\n",
            "{\"session\":{\"id\":\"later\"}}\n"
        );
        let extracted = extract_descriptor_output(&contract, output);
        assert_eq!(extracted.parsed.conversation_id.as_deref(), Some("first"));
    }

    #[test]
    fn document_dialect_extracts_from_single_json() {
        let contract = pointer_contract(ContractDialect::JsonDocument);
        let extracted = extract_descriptor_output(
            &contract,
            r#"{"session":{"id":"doc-1"},"usage":{"input":11,"output":12},"answer":"document answer"}"#,
        );
        assert_eq!(extracted.parsed.conversation_id.as_deref(), Some("doc-1"));
        assert_eq!(extracted.parsed.answer.as_deref(), Some("document answer"));
        assert_eq!(extracted.parsed.usage.expect("usage").output_tokens, 12);
    }

    #[test]
    fn missing_pointer_is_capability_caveat_not_error() {
        let contract = pointer_contract(ContractDialect::JsonDocument);
        let extracted = extract_descriptor_output(&contract, r#"{"answer":"still works"}"#);
        assert_eq!(extracted.parsed.answer.as_deref(), Some("still works"));
        assert!(extracted.parsed.failure.is_none());
        assert_eq!(
            extracted.missing_fields,
            [
                "conversation_id_path",
                "usage_input_path",
                "usage_output_path"
            ]
        );
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
