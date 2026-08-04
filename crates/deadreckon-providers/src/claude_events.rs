//! Claude Code `-p --output-format stream-json` wire contract mirror +
//! capability probe.
//!
//! Claude Code has no local source checkout: the contract is grounded by
//! probing the installed binary (`claude -p --help`) and by fixtures recorded
//! from real invocations (see `tests/fixtures/semaphore/`). Parsing is tolerant:
//! unknown `type` tags degrade to [`CliStreamEvent::Unknown`] and never abort a
//! turn.

use serde::Deserialize;
use serde_json::Value;

use crate::cli_contract::{CliStreamEvent, CliToolRow, CliUsage, truncate_summary};

/// Feature set detected from `claude --help`. Absent flags disable the
/// corresponding behavior with a caveat rather than erroring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClaudeCapabilities {
    pub stream_json: bool,
    pub resume: bool,
    /// `--json-schema` structured output. The probe is forward-ready; wiring it
    /// into the driver is a follow-up slice (Semaphore only records the bit).
    pub json_schema: bool,
    pub schema_only_posture: bool,
}

impl ClaudeCapabilities {
    pub(crate) fn none() -> Self {
        Self {
            stream_json: false,
            resume: false,
            json_schema: false,
            schema_only_posture: false,
        }
    }
}

/// Parse a `claude --help` payload into a capability set. Pure and
/// help-text-driven so it is unit-testable without invoking the binary.
pub(crate) fn parse_claude_capabilities(help: &str) -> ClaudeCapabilities {
    ClaudeCapabilities {
        // `--output-format` lists "stream-json" among its choices.
        stream_json: help.contains("--output-format") && help.contains("stream-json"),
        resume: help.contains("--resume"),
        json_schema: help.contains("--json-schema"),
        schema_only_posture: [
            "--safe-mode",
            "--tools",
            "--strict-mcp-config",
            "--mcp-config",
            "--setting-sources",
        ]
        .iter()
        .all(|flag| help.contains(flag)),
    }
}

// ---------------------------------------------------------------------------
// Event mirror (wire per fixtures recorded from the real binary; tolerant).
// ---------------------------------------------------------------------------

/// A claude `-p --output-format stream-json` JSONL event. Only session id,
/// usage, cost, result text, and is_error are read structurally; content blocks
/// stay `Value` for the flight ledger. Unknown `type` tags (e.g.
/// `rate_limit_event`) land in [`ClaudeStreamEvent::Unknown`].
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClaudeStreamEvent {
    System {
        #[serde(default)]
        session_id: Option<String>,
    },
    Assistant {
        message: Value,
    },
    // Tool-result echoes — recognized so they are not Unknown; their content is
    // already captured from the matching assistant `tool_use`, so no fields are
    // read structurally.
    User,
    Result {
        #[serde(default)]
        result: Option<String>,
        #[serde(default)]
        structured_output: Option<Value>,
        #[serde(default)]
        usage: Option<Value>,
        #[serde(default)]
        total_cost_usd: Option<f64>,
        #[serde(default)]
        session_id: Option<String>,
        #[serde(default)]
        is_error: Option<bool>,
    },
    #[serde(other)]
    Unknown,
}

/// Parse one claude JSONL line into neutral stream events. `None` for a line
/// that is not JSON at all (garbage). A `result` line yields several facts at
/// once (session id, usage, answer, and a failure when `is_error`).
pub(crate) fn parse_claude_line(line: &str) -> Option<Vec<CliStreamEvent>> {
    let event: ClaudeStreamEvent = serde_json::from_str(line).ok()?;
    Some(match event {
        ClaudeStreamEvent::System { session_id } => match session_id {
            Some(id) => vec![CliStreamEvent::Conversation(id)],
            None => vec![CliStreamEvent::Recognized],
        },
        ClaudeStreamEvent::Assistant { message } => claude_content_events(&message, line),
        // Tool results echo rows already lifted from the assistant tool_use;
        // recognized, not re-appended.
        ClaudeStreamEvent::User => vec![CliStreamEvent::Recognized],
        ClaudeStreamEvent::Result {
            result,
            structured_output,
            usage,
            total_cost_usd,
            session_id,
            is_error,
        } => {
            let mut events = Vec::new();
            if let Some(id) = session_id {
                events.push(CliStreamEvent::Conversation(id));
            }
            events.push(CliStreamEvent::Usage(CliUsage {
                input_tokens: claude_usage_field(usage.as_ref(), "input_tokens"),
                output_tokens: claude_usage_field(usage.as_ref(), "output_tokens"),
                cost_usd: total_cost_usd,
            }));
            if let Some(value) = structured_output {
                events.push(CliStreamEvent::Answer(value.to_string()));
            } else if let Some(text) = result {
                events.push(CliStreamEvent::Answer(text));
            }
            if is_error == Some(true) {
                events.push(CliStreamEvent::Failure(
                    "claude reported is_error".to_string(),
                ));
            }
            events
        }
        ClaudeStreamEvent::Unknown => vec![CliStreamEvent::Unknown],
    })
}

fn claude_usage_field(usage: Option<&Value>, key: &str) -> u64 {
    usage
        .and_then(|u| u.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// Lift `tool_use` blocks from an assistant message into flight tool rows; text
/// blocks are recognized narrative.
fn claude_content_events(message: &Value, raw: &str) -> Vec<CliStreamEvent> {
    let Some(blocks) = message.get("content").and_then(Value::as_array) else {
        return vec![CliStreamEvent::Recognized];
    };
    let mut events = Vec::new();
    for block in blocks {
        if block.get("type").and_then(Value::as_str) == Some("tool_use") {
            let name = block.get("name").and_then(Value::as_str);
            events.push(CliStreamEvent::Tool(CliToolRow {
                id: block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("tool_use")
                    .to_string(),
                tool_name: name.map(str::to_string),
                tool_category: name.map(claude_tool_category),
                summary: claude_tool_summary(name, block.get("input")),
                status: None,
                raw: raw.to_string(),
            }));
        }
    }
    if events.is_empty() {
        events.push(CliStreamEvent::Recognized);
    }
    events
}

fn claude_tool_category(name: &str) -> String {
    match name {
        "Bash" => "shell",
        "Edit" | "Write" | "NotebookEdit" | "MultiEdit" => "edit",
        "Read" | "Glob" | "Grep" => "read",
        "WebFetch" | "WebSearch" => "search",
        "Task" => "subagent",
        _ => "tool",
    }
    .to_string()
}

fn claude_tool_summary(name: Option<&str>, input: Option<&Value>) -> String {
    let label = name.unwrap_or("tool");
    let detail = input.and_then(|input| {
        input
            .get("command")
            .or_else(|| input.get("file_path"))
            .or_else(|| input.get("path"))
            .or_else(|| input.get("pattern"))
            .or_else(|| input.get("description"))
            .and_then(Value::as_str)
    });
    match detail {
        Some(detail) => truncate_summary(&format!("{label}: {detail}")),
        None => label.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_HELP: &str = r#"Claude Code - starts an interactive session by default, use -p/--print

  --json-schema <schema>                JSON Schema for structured output
  --output-format <format>              Output format (only works with --print):
                                        "text" (default), "json" (single
                                        result), or "stream-json" (realtime
                                        streaming) (choices: "text", "json",
                                        "stream-json")
  -r, --resume [sessionId]              Resume a conversation
  -p, --print                           Print response and exit
"#;

    #[test]
    fn claude_probe_detects_stream_json_and_resume() {
        let caps = parse_claude_capabilities(REAL_HELP);
        assert!(caps.stream_json, "stream-json output format detected");
        assert!(caps.resume, "--resume detected");
        assert!(caps.json_schema, "--json-schema detected (forward-ready)");
    }

    #[test]
    fn claude_absent_flags_disable_features_not_error() {
        let ancient = "Claude Code\n\n  -p, --print   Print response and exit\n";
        let caps = parse_claude_capabilities(ancient);
        assert!(!caps.stream_json);
        assert!(!caps.resume);
        assert!(!caps.json_schema);
        assert_eq!(caps, ClaudeCapabilities::none());
    }

    // Recorded from the real `claude -p --output-format stream-json` binary
    // (2.1.210) — tests/fixtures/semaphore/claude-*.jsonl.
    const CLAUDE_SIMPLE_STREAM: &str =
        include_str!("../tests/fixtures/semaphore/claude-simple.jsonl");
    const CLAUDE_TOOL_STREAM: &str = include_str!("../tests/fixtures/semaphore/claude-tool.jsonl");

    #[allow(clippy::type_complexity)]
    fn fold(
        stream: &str,
    ) -> (
        Option<String>,
        Option<CliUsage>,
        Option<String>,
        Vec<CliToolRow>,
        bool,
    ) {
        let mut conversation = None;
        let mut usage = None;
        let mut answer = None;
        let mut tools = Vec::new();
        let mut failure = false;
        for line in stream.lines().filter(|l| !l.trim().is_empty()) {
            for event in parse_claude_line(line).expect("real claude line parses") {
                match event {
                    CliStreamEvent::Conversation(id) => conversation = Some(id),
                    CliStreamEvent::Usage(u) => usage = Some(u),
                    CliStreamEvent::Answer(a) => answer = Some(a),
                    CliStreamEvent::Tool(row) => tools.push(row),
                    CliStreamEvent::Failure(_) => failure = true,
                    _ => {}
                }
            }
        }
        (conversation, usage, answer, tools, failure)
    }

    #[test]
    fn claude_events_parse_init_assistant_and_result() {
        let (conversation, _usage, answer, tools, failure) = fold(CLAUDE_TOOL_STREAM);
        assert_eq!(
            conversation.as_deref(),
            Some("ace2c86d-18b6-4b25-a36d-4a37c4430586"),
            "session id from system(init)/result"
        );
        assert_eq!(answer.as_deref(), Some("done"), "answer is result.result");
        assert!(!failure, "successful run is not a failure");
        let bash = tools
            .iter()
            .find(|row| row.tool_name.as_deref() == Some("Bash"))
            .expect("Bash tool_use row");
        assert_eq!(bash.tool_category.as_deref(), Some("shell"));
        assert!(bash.summary.contains("echo hello-from-claude"));
        assert!(bash.id.starts_with("toolu_"));
    }

    #[test]
    fn claude_result_carries_usage_cost_and_session() {
        let (conversation, usage, answer, _tools, _failure) = fold(CLAUDE_SIMPLE_STREAM);
        assert_eq!(
            conversation.as_deref(),
            Some("a99c262a-49ec-4653-9801-14a86fe09228")
        );
        assert_eq!(answer.as_deref(), Some("pong"));
        let usage = usage.expect("result.usage");
        assert_eq!(usage.input_tokens, 2);
        assert_eq!(usage.output_tokens, 4);
        // Reported cost is carried for the trace detail — never for spend.
        assert_eq!(usage.cost_usd, Some(0.131228));
    }

    #[test]
    fn claude_unknown_event_parses_as_unknown_not_error() {
        // rate_limit_event is a real event type the mirror does not model.
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#;
        assert_eq!(
            parse_claude_line(line).expect("valid json"),
            vec![CliStreamEvent::Unknown]
        );
        assert!(parse_claude_line("not json at all").is_none());
    }
}
