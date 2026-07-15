// Wired into the claude driver across phases P5–P9; the `dead_code` allow is
// removed once every item has a caller.
#![allow(dead_code)]
//! Claude Code `-p --output-format stream-json` wire contract mirror +
//! capability probe.
//!
//! Claude Code has no local source checkout: the contract is grounded by
//! probing the installed binary (`claude -p --help`) and by fixtures recorded
//! from real invocations (see `tests/fixtures/semaphore/`). Parsing is tolerant:
//! unknown `type` tags degrade to [`CliStreamEvent::Unknown`] and never abort a
//! turn.

use std::collections::HashMap;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// Feature set detected from `claude --help`. Absent flags disable the
/// corresponding behavior with a caveat rather than erroring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClaudeCapabilities {
    pub stream_json: bool,
    pub resume: bool,
    /// `--json-schema` structured output. The probe is forward-ready; wiring it
    /// into the driver is a follow-up slice (Semaphore only records the bit).
    pub json_schema: bool,
}

impl ClaudeCapabilities {
    pub(crate) fn none() -> Self {
        Self {
            stream_json: false,
            resume: false,
            json_schema: false,
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
    }
}

/// Probe `claude --help` once per binary path and cache the result. A binary
/// that cannot be executed reports [`ClaudeCapabilities::none`].
pub(crate) fn probe_claude_capabilities(binary: &str) -> ClaudeCapabilities {
    static CACHE: OnceLock<Mutex<HashMap<String, ClaudeCapabilities>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock()
        && let Some(found) = guard.get(binary)
    {
        return *found;
    }
    let caps = Command::new(binary)
        .arg("--help")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            let text = String::from_utf8_lossy(&output.stdout);
            parse_claude_capabilities(&text)
        })
        .unwrap_or_else(ClaudeCapabilities::none);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(binary.to_string(), caps);
    }
    caps
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
}
