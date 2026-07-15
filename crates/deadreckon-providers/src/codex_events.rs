// Wired into the codex driver across phases P5–P9; the `dead_code` allow is
// removed once every item has a caller.
#![allow(dead_code)]
//! Codex `exec --json` wire contract mirror + capability probe.
//!
//! The wire shapes mirror `codex-rs/exec/src/exec_events.rs` but are never
//! linked against it — the contract is what the installed binary emits,
//! feature-detected at runtime from `codex exec --help`. Parsing is tolerant:
//! unknown `type` tags degrade to [`CliStreamEvent::Unknown`] and never abort a
//! turn.

use std::collections::HashMap;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// Feature set detected from `codex exec --help`. Absent flags disable the
/// corresponding behavior with a caveat rather than erroring, so binaries that
/// predate the structured flags keep working.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CodexCapabilities {
    pub json: bool,
    pub output_last_message: bool,
    pub output_schema: bool,
    pub resume: bool,
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
        }
    }
}

/// Parse a `codex exec --help` payload into a capability set. Pure and
/// help-text-driven so it is unit-testable without invoking the binary.
pub(crate) fn parse_codex_capabilities(help: &str) -> CodexCapabilities {
    CodexCapabilities {
        json: help.contains("--json"),
        output_last_message: help.contains("--output-last-message"),
        output_schema: help.contains("--output-schema"),
        // `resume` is a subcommand of `codex exec`, listed under Commands.
        resume: help.contains("resume"),
    }
}

/// Probe `codex exec --help` once per binary path and cache the result. A
/// binary that cannot be executed reports [`CodexCapabilities::none`].
pub(crate) fn probe_codex_capabilities(binary: &str) -> CodexCapabilities {
    static CACHE: OnceLock<Mutex<HashMap<String, CodexCapabilities>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock()
        && let Some(found) = guard.get(binary)
    {
        return *found;
    }
    let caps = Command::new(binary)
        .args(["exec", "--help"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            let text = String::from_utf8_lossy(&output.stdout);
            parse_codex_capabilities(&text)
        })
        .unwrap_or_else(CodexCapabilities::none);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(binary.to_string(), caps);
    }
    caps
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
}
