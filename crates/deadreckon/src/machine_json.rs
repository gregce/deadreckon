//! Gap G1: machine result and error envelopes for the state-changing verbs.
//!
//! `finish`, `apply`, `kill`, `steer`, `extend`, `fork`, `merge`,
//! `materialize`, and `abandon` mutate durable state but historically spoke
//! only prose. When the operator passes `--json`, the dispatch arms this
//! module with the verb name; terminal surfaces then emit one shared envelope
//! object on stdout instead of prose, and `main()` converts every fail-closed
//! refusal into a machine refusal envelope with the same exit code. The armed
//! flag is process-global (the same pattern as `ui::set_plain_output`) so the
//! prose path stays byte-identical whenever `--json` was not given.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use deadreckon::verdict_surface::VerdictSurface;

static ARMED_VERB: OnceLock<&'static str> = OnceLock::new();
static NDJSON_STREAM: AtomicBool = AtomicBool::new(false);

/// Record at dispatch time that this invocation's subcommand carried
/// `--json`. Arming is one-shot per process, matching one CLI invocation.
/// Arming also forces plain rendering: a machine caller may be attached to a
/// PTY, and no TUI/spinner/ANSI-capable surface may pollute the single
/// JSON-object stdout stream. (Without `--json` this never runs, so prose
/// and `--plain` behavior stay byte-identical.)
pub(crate) fn arm(verb: &'static str, json: bool) {
    if json {
        crate::ui::set_plain_output(true);
        let _ = ARMED_VERB.set(verb);
    }
}

/// Arm a verb whose armed stdout is one NDJSON stream (`follow`). Every
/// machine envelope the process later prints — including `main()`'s
/// fail-closed refusal envelope — must then be serialized compactly so it is
/// itself one valid NDJSON line: a mid-stream refusal lands after valid
/// stream records on the same fd, and a line-oriented decoder must be able to
/// parse it (docs/TAILING.md).
pub(crate) fn arm_ndjson(verb: &'static str, json: bool) {
    if json {
        NDJSON_STREAM.store(true, Ordering::SeqCst);
    }
    arm(verb, json);
}

/// True when the armed verb's stdout is an NDJSON stream, so envelopes must
/// stay single-line.
pub(crate) fn ndjson_stream() -> bool {
    NDJSON_STREAM.load(Ordering::SeqCst)
}

/// The verb that was armed with `--json`, if any. Terminal surfaces branch on
/// this: `Some(verb)` emits the machine envelope, `None` keeps the exact
/// historical prose.
pub(crate) fn active() -> Option<&'static str> {
    ARMED_VERB.get().copied()
}

/// The shared success envelope: the same `{kind,id,status,next_actions,
/// try_lines}` scaffold the inspection surfaces emit, verb-specific outcome
/// facts at the top level, and the full `VerdictSurface` projection via
/// `add_to_json` so prose and JSON come from one surface object.
pub(crate) fn success_envelope(
    verb: &str,
    id: &str,
    surface: &VerdictSurface,
    facts: Value,
) -> Value {
    let mut next_actions = vec![surface.primary_action.command.clone()];
    next_actions.extend(
        surface
            .secondary_actions
            .iter()
            .map(|action| action.command.clone()),
    );
    let mut value = json!({
        "kind": verb,
        "id": id,
        "status": surface.kind.as_str(),
        "next_actions": next_actions,
        "try_lines": Vec::<String>::new(),
    });
    if let (Value::Object(envelope), Value::Object(facts)) = (&mut value, facts) {
        for (key, fact) in facts {
            envelope.entry(key).or_insert(fact);
        }
    }
    surface.add_to_json(value)
}

/// Print the shared success envelope for the armed verb as the one JSON
/// object on stdout.
pub(crate) fn emit_success(
    verb: &str,
    id: &str,
    surface: &VerdictSurface,
    facts: Value,
) -> serde_json::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&success_envelope(verb, id, surface, facts))?
    );
    Ok(())
}

/// The machine refusal envelope. `deadreckon_core::user_error` packs
/// `"\ntry: <hint>"` lines into the message; every such line is lifted out of
/// the message into `try_lines` so machine consumers get one field for
/// recovery actions. `extra_try` carries the same `error_hint` line the prose
/// path prints, deduplicated against the lifted lines.
pub(crate) fn error_envelope(verb: &str, code: i32, message: &str, extra_try: &str) -> Value {
    let mut message_lines = Vec::new();
    let mut try_lines: Vec<String> = Vec::new();
    for line in message.lines() {
        if let Some(hint) = line.trim_start().strip_prefix("try: ") {
            try_lines.push(hint.trim().to_string());
        } else {
            message_lines.push(line);
        }
    }
    let extra = extra_try.trim();
    let extra = extra.strip_prefix("try:").map_or(extra, str::trim);
    if !extra.is_empty() && !try_lines.iter().any(|line| line == extra) {
        try_lines.push(extra.to_string());
    }
    json!({
        "kind": "error",
        "code": code,
        "verb": verb,
        "message": message_lines.join("\n").trim_end().to_string(),
        "try_lines": try_lines,
    })
}

#[cfg(test)]
mod tests {
    use deadreckon::verdict_surface::{ExplanationPanel, VerdictKind, VerdictSurface};

    use super::*;

    fn fixture_surface() -> VerdictSurface {
        VerdictSurface::must_new(
            VerdictKind::Completed,
            "steer",
            Some("abc12345"),
            ExplanationPanel::new(
                "what happened",
                "why this verdict",
                [("run".to_string(), "abc12345".to_string())],
            ),
            [("Recommended", "deadreckon attach abc12345")],
            [("Secondary", "deadreckon status abc12345")],
        )
    }

    #[test]
    fn success_envelope_carries_the_shared_scaffold_and_verb_facts() {
        let value = success_envelope(
            "steer",
            "abc12345",
            &fixture_surface(),
            json!({ "inbox_seq": 0, "queued_at": "2026-08-07T00:00:00Z" }),
        );

        assert_eq!(value["kind"], "steer");
        assert_eq!(value["id"], "abc12345");
        assert_eq!(value["status"], "completed");
        assert_eq!(value["inbox_seq"], 0);
        assert_eq!(value["queued_at"], "2026-08-07T00:00:00Z");
        assert_eq!(value["next_actions"][0], "deadreckon attach abc12345");
        assert_eq!(value["next_actions"][1], "deadreckon status abc12345");
        assert_eq!(value["try_lines"], json!([]));
        // The one-surface-object rule: prose and JSON share the same
        // VerdictSurface, projected through add_to_json.
        assert_eq!(value["primary_action"], "deadreckon attach abc12345");
        assert_eq!(value["verdict"]["kind"], "completed");
    }

    #[test]
    fn success_envelope_facts_cannot_overwrite_the_scaffold() {
        let value = success_envelope(
            "kill",
            "abc12345",
            &fixture_surface(),
            json!({ "kind": "spoofed", "signal": "SIGTERM" }),
        );

        assert_eq!(value["kind"], "kill");
        assert_eq!(value["signal"], "SIGTERM");
    }

    #[test]
    fn error_envelope_lifts_packed_try_lines_out_of_the_message() {
        // The exact shape user_error produces.
        let message = "run abc12345 is executing and cannot accept steering\ntry: deadreckon extend abc12345 \"goal\"";
        let value = error_envelope("steer", 1, message, "");

        assert_eq!(value["kind"], "error");
        assert_eq!(value["code"], 1);
        assert_eq!(value["verb"], "steer");
        assert_eq!(
            value["message"],
            "run abc12345 is executing and cannot accept steering"
        );
        assert_eq!(
            value["try_lines"],
            json!(["deadreckon extend abc12345 \"goal\""])
        );
    }

    #[test]
    fn error_envelope_preserves_the_exit_code_and_deduplicates_hints() {
        let value = error_envelope(
            "finish",
            2,
            "no completed run found",
            "try: deadreckon list",
        );

        assert_eq!(value["code"], 2);
        assert_eq!(value["try_lines"], json!(["deadreckon list"]));

        let duplicated = error_envelope(
            "finish",
            1,
            "refused\ntry: deadreckon list",
            "deadreckon list",
        );
        assert_eq!(duplicated["try_lines"], json!(["deadreckon list"]));
    }

    #[test]
    fn error_envelope_survives_a_multi_line_surface_message() {
        // CliError::Surface carries a full rendered card; the envelope must
        // still be one valid JSON object with the text intact.
        let message = "blocked steer abc12345\n\nExplanation\nline one\nline two";
        let value = error_envelope("steer", 1, message, "");
        assert_eq!(value["message"], message);
        assert_eq!(value["try_lines"], json!([]));
    }
}
