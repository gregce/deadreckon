//! Codex `exec --json` wire contract mirror + capability probe.
//!
//! The wire shapes mirror `codex-rs/exec/src/exec_events.rs` but are never
//! linked against it — the contract is what the installed binary emits,
//! feature-detected at runtime from `codex exec --help`. Parsing is tolerant:
//! unknown `type` tags degrade to [`CliStreamEvent::Unknown`] and never abort a
//! turn.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use crate::cli_contract::{CliStreamEvent, CliToolRow, CliUsage, truncate_summary};

/// Feature set detected from `codex exec --help`. Absent flags disable the
/// corresponding behavior with a caveat rather than erroring, so binaries that
/// predate the structured flags keep working.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CodexCapabilities {
    pub json: bool,
    pub output_last_message: bool,
    pub output_schema: bool,
    pub resume: bool,
    pub ephemeral: bool,
    pub ignore_user_config: bool,
    pub ignore_rules: bool,
    pub strict_config: bool,
    pub disable_features: bool,
    pub structured_text_features: bool,
    /// Bitset into [`STRUCTURED_TEXT_DISABLED_FEATURES`] for lifecycle-active
    /// tool surfaces. Deprecated or removed features that are already false
    /// are intentionally omitted from argv instead of provoking strict-config
    /// diagnostics.
    pub structured_text_disable_mask: u32,
}

impl CodexCapabilities {
    /// Conservative default for a binary we could not probe: assume none of the
    /// structured flags exist, so the driver degrades to raw-stdout behavior.
    pub(crate) fn none() -> Self {
        Self {
            json: false,
            output_last_message: false,
            output_schema: false,
            resume: false,
            ephemeral: false,
            ignore_user_config: false,
            ignore_rules: false,
            strict_config: false,
            disable_features: false,
            structured_text_features: false,
            structured_text_disable_mask: 0,
        }
    }
}

/// Parse a `codex exec --help` payload into a capability set. Pure and
/// help-text-driven so it is unit-testable without invoking the binary.
pub(crate) fn parse_codex_capabilities(help: &str) -> CodexCapabilities {
    parse_codex_capabilities_with_features(help, "")
}

/// Tool-bearing Codex surfaces that a schema-only request must explicitly
/// disable. The capability probe proves every name before argv construction;
/// unknown flags are never guessed into a trusted request.
pub(crate) const STRUCTURED_TEXT_DISABLED_FEATURES: &[&str] = &[
    "apps",
    "browser_use",
    "browser_use_external",
    "browser_use_full_cdp_access",
    "code_mode",
    "code_mode_host",
    "computer_use",
    "enable_mcp_apps",
    "in_app_browser",
    "multi_agent",
    "plugins",
    "shell_tool",
    "standalone_web_search",
    "unified_exec",
    "web_search_request",
];

pub(crate) fn parse_codex_capabilities_with_features(
    help: &str,
    feature_listing: &str,
) -> CodexCapabilities {
    let feature_rows = feature_listing
        .lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let name = *fields.first()?;
            let enabled = fields.last()?.parse::<bool>().ok()?;
            let lifecycle = fields.get(1..fields.len().saturating_sub(1))?.join(" ");
            Some((name, (lifecycle, enabled)))
        })
        .collect::<HashMap<_, _>>();
    let mut structured_text_disable_mask = 0_u32;
    let mut structured_text_features = true;
    for (index, name) in STRUCTURED_TEXT_DISABLED_FEATURES.iter().enumerate() {
        match feature_rows.get(name) {
            Some((lifecycle, enabled)) if lifecycle == "removed" || lifecycle == "deprecated" => {
                // A retired feature is safe only when it is already off. Do
                // not pass `--disable`: current Codex reports that use as a
                // configuration error even though the turn may continue.
                structured_text_features &= !enabled;
            }
            Some(_) => structured_text_disable_mask |= 1_u32 << index,
            None => structured_text_features = false,
        }
    }
    CodexCapabilities {
        json: help.contains("--json"),
        output_last_message: help.contains("--output-last-message"),
        output_schema: help.contains("--output-schema"),
        // `resume` is a subcommand of `codex exec`, listed under Commands.
        resume: help.contains("resume"),
        ephemeral: help.contains("--ephemeral"),
        ignore_user_config: help.contains("--ignore-user-config"),
        ignore_rules: help.contains("--ignore-rules"),
        strict_config: help.contains("--strict-config"),
        disable_features: help.contains("--disable"),
        structured_text_features,
        structured_text_disable_mask,
    }
}

pub(crate) fn structured_text_features_to_disable(
    capabilities: CodexCapabilities,
) -> impl Iterator<Item = &'static str> {
    STRUCTURED_TEXT_DISABLED_FEATURES
        .iter()
        .enumerate()
        .filter(move |(index, _)| capabilities.structured_text_disable_mask & (1_u32 << index) != 0)
        .map(|(_, name)| *name)
}

// ---------------------------------------------------------------------------
// Event mirror (wire per codex-rs exec_events.rs; tolerant by construction).
// ---------------------------------------------------------------------------

/// A codex `exec --json` JSONL event. Mirrors codex-rs exec_events.rs but is
/// tolerant: unknown `type` tags land in [`CodexThreadEvent::Unknown`] and each
/// item payload stays a `Value` for the flight ledger.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
enum CodexThreadEvent {
    #[serde(rename = "thread.started")]
    ThreadStarted { thread_id: String },
    #[serde(rename = "turn.started")]
    TurnStarted {},
    #[serde(rename = "turn.completed")]
    TurnCompleted { usage: CodexUsage },
    #[serde(rename = "turn.failed")]
    TurnFailed {
        #[serde(default)]
        error: Option<CodexErrorBody>,
    },
    #[serde(rename = "item.started")]
    ItemStarted { item: Value },
    #[serde(rename = "item.updated")]
    ItemUpdated { item: Value },
    #[serde(rename = "item.completed")]
    ItemCompleted { item: Value },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
struct CodexUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Debug, Clone, Deserialize)]
struct CodexErrorBody {
    #[serde(default)]
    message: Option<String>,
}

/// Parse one codex JSONL line into neutral stream events. Returns `None` when
/// the line is not JSON at all (garbage), so the shared loop can count and skip
/// it. A well-formed line with an unmodeled `type` yields `[Unknown]`.
pub(crate) fn parse_codex_line(line: &str) -> Option<Vec<CliStreamEvent>> {
    let event: CodexThreadEvent = serde_json::from_str(line).ok()?;
    Some(match event {
        CodexThreadEvent::ThreadStarted { thread_id } => {
            vec![CliStreamEvent::Conversation(thread_id)]
        }
        CodexThreadEvent::TurnCompleted { usage } => vec![CliStreamEvent::Usage(CliUsage {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cost_usd: None,
        })],
        CodexThreadEvent::TurnFailed { error } => vec![CliStreamEvent::Failure(
            error
                .and_then(|body| body.message)
                .unwrap_or_else(|| "codex turn failed".to_string()),
        )],
        CodexThreadEvent::Error { message } => vec![CliStreamEvent::Failure(message)],
        CodexThreadEvent::ItemStarted { item }
        | CodexThreadEvent::ItemUpdated { item }
        | CodexThreadEvent::ItemCompleted { item } => vec![codex_item_event(&item, line)],
        CodexThreadEvent::TurnStarted {} => vec![CliStreamEvent::Recognized],
        CodexThreadEvent::Unknown => vec![CliStreamEvent::Unknown],
    })
}

/// Map a codex thread item to a flight tool row, or [`CliStreamEvent::Recognized`]
/// for narrative items (agent_message, reasoning) that are not tool calls.
fn codex_item_event(item: &Value, raw: &str) -> CliStreamEvent {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
    if item_type == "agent_message" {
        return item
            .get("text")
            .and_then(Value::as_str)
            .map(|text| CliStreamEvent::Answer(text.to_string()))
            .unwrap_or(CliStreamEvent::Recognized);
    }
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(item_type)
        .to_string();
    let (category, summary) = match item_type {
        "command_execution" => (
            "shell",
            item.get("command")
                .and_then(Value::as_str)
                .unwrap_or("command")
                .to_string(),
        ),
        "file_change" => ("edit", codex_file_change_summary(item)),
        "mcp_tool_call" => (
            "mcp",
            item.get("tool")
                .and_then(Value::as_str)
                .or_else(|| item.get("server").and_then(Value::as_str))
                .unwrap_or("mcp tool")
                .to_string(),
        ),
        "web_search" => (
            "search",
            item.get("query")
                .and_then(Value::as_str)
                .unwrap_or("web search")
                .to_string(),
        ),
        "collab_tool_call" => (
            "collab",
            item.get("tool")
                .and_then(Value::as_str)
                .unwrap_or("collab tool")
                .to_string(),
        ),
        // reasoning, todo_list, error, and anything else are not tool calls:
        // recognized, but not appended to the flight ledger here.
        _ => return CliStreamEvent::Recognized,
    };
    CliStreamEvent::Tool(CliToolRow {
        id,
        tool_name: Some(item_type.to_string()),
        tool_category: Some(category.to_string()),
        summary: truncate_summary(&summary),
        status: item
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string),
        raw: raw.to_string(),
    })
}

fn codex_file_change_summary(item: &Value) -> String {
    let paths: Vec<&str> = item
        .get("changes")
        .and_then(Value::as_array)
        .map(|changes| {
            changes
                .iter()
                .filter_map(|change| change.get("path").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    if paths.is_empty() {
        "file change".to_string()
    } else {
        format!("edit {}", paths.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_HELP: &str = r#"Run Codex non-interactively

Usage: codex exec [OPTIONS] [PROMPT]
       codex exec [OPTIONS] <COMMAND> [ARGS]

Commands:
  resume  Resume a previous session by id or pick the most recent with --last

Options:
      --output-schema <FILE>
      --json
  -o, --output-last-message <FILE>
"#;

    #[test]
    fn codex_probe_detects_json_and_resume_flags() {
        let caps = parse_codex_capabilities(REAL_HELP);
        assert!(caps.json, "--json flag detected");
        assert!(caps.resume, "resume subcommand detected");
        assert!(caps.output_last_message, "--output-last-message detected");
        assert!(caps.output_schema, "--output-schema detected");
    }

    #[test]
    fn absent_flags_disable_features_not_error() {
        // An old binary whose help mentions none of the structured flags.
        let ancient = "Run Codex non-interactively\n\nUsage: codex exec [PROMPT]\n";
        let caps = parse_codex_capabilities(ancient);
        assert!(!caps.json);
        assert!(!caps.resume);
        assert!(!caps.output_last_message);
        assert!(!caps.output_schema);
        assert_eq!(caps, CodexCapabilities::none());
    }

    #[test]
    fn deprecated_disabled_features_are_proven_but_not_passed_back() {
        let help = format!(
            "{REAL_HELP}\n      --ephemeral\n      --ignore-user-config\n      --ignore-rules\n      --strict-config\n      --disable <FEATURE>\n"
        );
        let listing = STRUCTURED_TEXT_DISABLED_FEATURES
            .iter()
            .map(|name| {
                if *name == "web_search_request" {
                    format!("{name} deprecated false")
                } else {
                    format!("{name} stable true")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let capabilities = parse_codex_capabilities_with_features(&help, &listing);
        assert!(capabilities.structured_text_features);
        let disabled = structured_text_features_to_disable(capabilities).collect::<Vec<_>>();
        assert!(!disabled.contains(&"web_search_request"));
        assert!(disabled.contains(&"shell_tool"));
    }

    // Recorded from the real `codex exec --json` binary (0.144.1) —
    // tests/fixtures/semaphore/codex-tool.jsonl.
    const CODEX_TOOL_STREAM: &str = include_str!("../tests/fixtures/semaphore/codex-tool.jsonl");

    #[test]
    fn codex_events_parse_started_completed_and_items() {
        let mut conversation = None;
        let mut usage = None;
        let mut tools = Vec::new();
        for line in CODEX_TOOL_STREAM.lines().filter(|l| !l.trim().is_empty()) {
            for event in parse_codex_line(line).expect("real codex line parses") {
                match event {
                    CliStreamEvent::Conversation(id) => conversation = Some(id),
                    CliStreamEvent::Usage(u) => usage = Some(u),
                    CliStreamEvent::Tool(row) => tools.push(row),
                    _ => {}
                }
            }
        }
        assert_eq!(
            conversation.as_deref(),
            Some("019f67a0-0915-7fb3-9dba-9e0f8992df7f"),
            "thread.started.thread_id extracted"
        );
        let usage = usage.expect("turn.completed.usage extracted");
        assert_eq!(usage.input_tokens, 32981);
        assert_eq!(usage.output_tokens, 111);
        assert_eq!(usage.cost_usd, None);
        // The command_execution item (started + completed collapse to one row).
        let shell = tools
            .iter()
            .find(|row| row.tool_category.as_deref() == Some("shell"))
            .expect("command_execution tool row");
        assert!(shell.summary.contains("echo hello-from-codex"));
        assert_eq!(shell.tool_name.as_deref(), Some("command_execution"));
    }

    #[test]
    fn codex_unknown_event_parses_as_unknown_not_error() {
        let events =
            parse_codex_line(r#"{"type":"turn.aborted","reason":"user"}"#).expect("valid json");
        assert_eq!(events, vec![CliStreamEvent::Unknown]);
    }

    #[test]
    fn codex_agent_message_is_the_structured_answer() {
        let events = parse_codex_line(
            r#"{"type":"item.completed","item":{"id":"item_7","type":"agent_message","text":"{\"decision\":\"achieved\"}"}}"#,
        )
        .expect("valid codex agent message");
        assert_eq!(
            events,
            vec![CliStreamEvent::Answer(
                r#"{"decision":"achieved"}"#.to_string()
            )]
        );
    }

    #[test]
    fn garbage_line_is_skipped_and_counted() {
        // The real codex fixture leads with a human notice line, not JSON.
        assert!(parse_codex_line("Reading additional input from stdin...").is_none());
        assert!(parse_codex_line("not json at all { [").is_none());
    }
}
