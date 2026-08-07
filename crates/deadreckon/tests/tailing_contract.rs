//! Conformance test for the external tailing contract (docs/TAILING.md).
//!
//! Simulates an external observer that only sees bytes on disk: an
//! offset-based tail with torn-line retry, strict-sequence verification on
//! `job-events.jsonl`, and schema conformance of emitted Job events against
//! the checked `docs/schemas/job-event.schema.json`.

#![allow(clippy::expect_used)]

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use deadreckon_core::{DeadreckonPaths, append_job_event, read_job_history};
use deadreckon_protocol::{JobEvent, JobEventKind, JobEventSequence, JobId, JobSchemaVersion};
use serde_json::{Value, json};
use tempfile::TempDir;

const JOB_ID: &str = "job-tail";

fn event(sequence: u64, event_id: &str, kind: JobEventKind) -> JobEvent {
    JobEvent {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: JobId(JOB_ID.to_string()),
        sequence: JobEventSequence::new(sequence).expect("nonzero sequence"),
        event_id: event_id.to_string(),
        causation_id: event_id.to_string(),
        timestamp: Utc::now(),
        lease_epoch: 0,
        kind,
        detail: json!({ "note": event_id }),
    }
}

/// The recommended reader from docs/TAILING.md: keep a byte offset, read the
/// new bytes, split at the last newline, parse only complete lines, and
/// retain an unterminated final line as a torn append for the next poll.
struct ContractTail {
    path: PathBuf,
    offset: u64,
    pending: Vec<u8>,
}

impl ContractTail {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            pending: Vec::new(),
        }
    }

    fn poll(&mut self) -> Vec<Value> {
        let Ok(metadata) = fs::metadata(&self.path) else {
            return Vec::new();
        };
        if metadata.len() < self.offset {
            // Restart (blessed only for acceptance-progress.jsonl): re-read
            // from the top and discard torn bytes from the previous identity.
            self.offset = 0;
            self.pending.clear();
        }
        let mut file = fs::File::open(&self.path).expect("open blessed file");
        file.seek(SeekFrom::Start(self.offset)).expect("seek");
        let mut fresh = Vec::new();
        file.read_to_end(&mut fresh).expect("read tail bytes");
        self.offset += fresh.len() as u64;
        let mut buffer = std::mem::take(&mut self.pending);
        buffer.extend_from_slice(&fresh);
        let complete_len = buffer
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1);
        self.pending = buffer[complete_len..].to_vec();
        buffer[..complete_len]
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                serde_json::from_slice(line)
                    .expect("every newline-terminated line must be one complete JSON object")
            })
            .collect()
    }
}

/// Strict-sequence verification for `job-events.jsonl` rows, as the contract
/// tells external readers to run it: `sequence` must continue exactly from
/// `expected` with no gaps.
fn sequence_gap(rows: &[Value], mut expected: u64) -> Option<String> {
    for row in rows {
        let sequence = row["sequence"].as_u64().expect("sequence field");
        if sequence != expected {
            return Some(format!("expected sequence {expected}, found {sequence}"));
        }
        expected += 1;
    }
    None
}

#[test]
fn torn_final_line_is_ignored_then_read_after_completion() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path());
    append_job_event(&paths, &event(1, "created", JobEventKind::Created)).expect("created");
    append_job_event(&paths, &event(2, "queued", JobEventKind::Queued)).expect("queued");

    let events_path = paths.job_events(JOB_ID);
    let mut tail = ContractTail::new(events_path.clone());
    let rows = tail.poll();
    assert_eq!(rows.len(), 2);
    assert!(sequence_gap(&rows, 1).is_none());

    // Simulate racing an in-flight append: the writer's bytes are on disk but
    // the terminating newline is not yet.
    let third = event(3, "attempt-started", JobEventKind::AttemptStarted);
    let line = serde_json::to_vec(&third).expect("serialize third event");
    let mut file = OpenOptions::new()
        .append(true)
        .open(&events_path)
        .expect("open for torn append");
    file.write_all(&line).expect("write torn prefix");
    file.sync_all().expect("sync torn prefix");

    let rows = tail.poll();
    assert!(rows.is_empty(), "a torn final line must not be parsed");
    assert!(
        !tail.pending.is_empty(),
        "torn bytes are retained for retry"
    );

    // deadreckon's own reader treats the same bytes the same way: the torn
    // row is ignored and surfaced as a caveat, never parsed or guessed at.
    let history = read_job_history(&events_path).expect("history under torn tail");
    assert_eq!(history.events().len(), 2);
    assert_eq!(history.caveats.len(), 1);

    let mut file = OpenOptions::new()
        .append(true)
        .open(&events_path)
        .expect("open to complete append");
    file.write_all(b"\n").expect("complete the append");
    file.sync_all().expect("sync completion");

    let rows = tail.poll();
    assert_eq!(rows.len(), 1, "the completed line is read on retry");
    assert_eq!(
        rows[0],
        serde_json::to_value(&third).expect("third as JSON")
    );
    assert!(sequence_gap(&rows, 3).is_none());
    assert!(tail.pending.is_empty());
}

#[test]
fn strict_sequence_gap_is_detected_and_never_written() {
    // Reader side: a fabricated gap on disk must be flagged, not smoothed over.
    let temp = TempDir::new().expect("tempdir");
    let gap_path = temp.path().join("job-events.jsonl");
    let first = serde_json::to_string(&event(1, "created", JobEventKind::Created))
        .expect("serialize first");
    let skipped =
        serde_json::to_string(&event(3, "skipped", JobEventKind::Queued)).expect("serialize gap");
    fs::write(&gap_path, format!("{first}\n{skipped}\n")).expect("write gapped history");
    let mut tail = ContractTail::new(gap_path);
    let rows = tail.poll();
    let gap = sequence_gap(&rows, 1).expect("gap must be detected");
    assert_eq!(gap, "expected sequence 2, found 3");

    // Writer side: the blessed writer refuses to create that gap at all.
    let paths = DeadreckonPaths::from_home(temp.path());
    append_job_event(&paths, &event(1, "created", JobEventKind::Created)).expect("created");
    let error = append_job_event(&paths, &event(3, "skipped", JobEventKind::Queued))
        .expect_err("out-of-order append must be refused")
        .to_string();
    assert!(
        error.contains("append expected sequence 2, received 3"),
        "{error}"
    );
}

#[test]
fn emitted_job_events_conform_to_checked_schema() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path());
    let mut approved = event(2, "approved", JobEventKind::ContractApproved);
    approved.detail = json!({ "contract_sha256": "sha256:contract" });
    append_job_event(&paths, &event(1, "created", JobEventKind::Created)).expect("created");
    append_job_event(&paths, &approved).expect("approved");
    append_job_event(&paths, &event(3, "queued", JobEventKind::Queued)).expect("queued");

    let schema: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("docs/schemas/job-event.schema.json"),
        )
        .expect("checked job-event schema must exist"),
    )
    .expect("checked job-event schema must be JSON");

    let mut tail = ContractTail::new(paths.job_events(JOB_ID));
    let rows = tail.poll();
    assert_eq!(rows.len(), 3);
    assert!(sequence_gap(&rows, 1).is_none());
    for row in &rows {
        if let Err(error) = json_matches_schema(row, &schema, &schema, "$") {
            panic!("emitted job event does not match checked schema: {error}");
        }
    }

    // The validator has teeth: dropping a required field must fail.
    let mut invalid = rows[0].clone();
    invalid
        .as_object_mut()
        .expect("job event JSON object")
        .remove("kind");
    assert!(
        json_matches_schema(&invalid, &schema, &schema, "$").is_err(),
        "schema validator must reject a job event missing a required field"
    );
}

#[test]
fn emitted_notify_lines_conform_to_checked_schema_and_append_only() {
    let temp = TempDir::new().expect("tempdir");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    let cwd = temp.path().join("repo");
    fs::create_dir_all(&cwd).expect("repo");
    let state = deadreckon_core::create_run(
        &paths,
        deadreckon_core::RunOptions {
            goal: "tail the notify ledger".to_string(),
            cwd,
            sandbox: "none".to_string(),
            provider: Some("smoke".to_string()),
            skill_name: "default-coding".to_string(),
            max_spend_usd: Some(1.0),
            max_wall_seconds: None,
            run_id: None,
            codebase: None,
        },
    )
    .expect("run");

    let schema: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../..")
                .join("docs/schemas/notify-event.schema.json"),
        )
        .expect("checked notify-event schema must exist"),
    )
    .expect("checked notify-event schema must be JSON");

    let mut tail = ContractTail::new(deadreckon::notify::notify_jsonl_path(&state));

    // The historical row shape: one delivery attempt from the CLI writer.
    deadreckon::notify::append_notify_attempt(
        &state,
        &deadreckon::notify::NotifyAttempt {
            ts: Utc::now(),
            transition: deadreckon::notify::NotifyTransition::Paused,
            channel: "native".to_string(),
            ok: false,
            detail: Some("osascript exited with 1".to_string()),
        },
    )
    .expect("delivery attempt row");
    let attempts = tail.poll();
    assert_eq!(attempts.len(), 1);

    // The typed operator-attention shape from the owning-process emitters.
    deadreckon_core::append_operator_attention(
        &state.run_root,
        &deadreckon_core::verified_awaiting_promote_event("job-tail", &state.run_id, &state.scope),
    )
    .expect("verified attention row");
    deadreckon_core::append_operator_attention(
        &state.run_root,
        &deadreckon_core::paused_at_cap_event(
            &state.run_id,
            &state.scope,
            Some("spend cap $1.00 reached"),
        ),
    )
    .expect("paused attention row");

    // Append-only: the retained offset stays valid and sees only new rows.
    let attention = tail.poll();
    assert_eq!(attention.len(), 2);
    assert_eq!(attention[0]["kind"], json!("operator_attention"));
    assert_eq!(attention[0]["reason"], json!("verified_awaiting_promote"));
    assert_eq!(attention[1]["reason"], json!("paused_at_cap"));

    for row in attempts.iter().chain(attention.iter()) {
        if let Err(error) = json_matches_schema(row, &schema, &schema, "$") {
            panic!("emitted notify line does not match checked schema: {error}");
        }
    }

    // The validator has teeth: a reason outside the schema vocabulary and a
    // truncated attempt row must both fail.
    let mut unknown_reason = attention[0].clone();
    unknown_reason["reason"] = json!("totally_new_reason");
    assert!(
        json_matches_schema(&unknown_reason, &schema, &schema, "$").is_err(),
        "schema validator must reject an unknown operator-attention reason"
    );
    let mut truncated_attempt = attempts[0].clone();
    truncated_attempt
        .as_object_mut()
        .expect("attempt row object")
        .remove("transition");
    assert!(
        json_matches_schema(&truncated_attempt, &schema, &schema, "$").is_err(),
        "schema validator must reject an attempt row missing a required field"
    );
}

// The minimal draft-07 walker used by the report schema test
// (`report_json_validates_against_generated_schema` in
// crates/deadreckon/src/commands/report.rs), applied here to the checked
// job-event schema from an external reader's viewpoint.
fn json_matches_schema(
    instance: &Value,
    schema: &Value,
    root: &Value,
    path: &str,
) -> Result<(), String> {
    if let Some(allowed) = schema.as_bool() {
        return allowed
            .then_some(())
            .ok_or_else(|| format!("{path}: rejected by false schema"));
    }
    let object = schema
        .as_object()
        .ok_or_else(|| format!("{path}: schema is not an object"))?;

    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        let pointer = reference
            .strip_prefix('#')
            .ok_or_else(|| format!("{path}: unsupported external schema reference {reference}"))?;
        let target = root
            .pointer(pointer)
            .ok_or_else(|| format!("{path}: unresolved schema reference {reference}"))?;
        return json_matches_schema(instance, target, root, path);
    }

    if let Some(values) = object.get("enum").and_then(Value::as_array)
        && !values.contains(instance)
    {
        return Err(format!("{path}: value is outside schema enum"));
    }

    validate_subschemas(instance, object, root, path)?;

    if let Some(expected) = object.get("type")
        && !schema_type_matches(instance, expected)
    {
        return Err(format!(
            "{path}: expected schema type {expected}, got {instance}"
        ));
    }

    if let Some(minimum) = object.get("minimum").and_then(Value::as_f64)
        && instance.as_f64().is_some_and(|number| number < minimum)
    {
        return Err(format!("{path}: number is below schema minimum {minimum}"));
    }

    if let Some(instance_object) = instance.as_object() {
        if let Some(required) = object.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                if !instance_object.contains_key(key) {
                    return Err(format!("{path}: missing required property {key:?}"));
                }
            }
        }
        let properties = object.get("properties").and_then(Value::as_object);
        for (key, value) in instance_object {
            let child_path = format!("{path}.{key}");
            if let Some(child_schema) = properties.and_then(|values| values.get(key)) {
                json_matches_schema(value, child_schema, root, &child_path)?;
            } else if let Some(additional) = object.get("additionalProperties") {
                json_matches_schema(value, additional, root, &child_path)?;
            }
        }
    }

    if let Some(instance_array) = instance.as_array()
        && let Some(items) = object.get("items")
    {
        if let Some(tuple) = items.as_array() {
            for (index, (value, child_schema)) in
                instance_array.iter().zip(tuple.iter()).enumerate()
            {
                json_matches_schema(value, child_schema, root, &format!("{path}[{index}]"))?;
            }
        } else {
            for (index, value) in instance_array.iter().enumerate() {
                json_matches_schema(value, items, root, &format!("{path}[{index}]"))?;
            }
        }
    }

    Ok(())
}

fn validate_subschemas(
    instance: &Value,
    schema: &serde_json::Map<String, Value>,
    root: &Value,
    path: &str,
) -> Result<(), String> {
    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
        for child in all_of {
            json_matches_schema(instance, child, root, path)?;
        }
    }
    for keyword in ["anyOf", "oneOf"] {
        let Some(children) = schema.get(keyword).and_then(Value::as_array) else {
            continue;
        };
        let matches = children
            .iter()
            .filter(|child| json_matches_schema(instance, child, root, path).is_ok())
            .count();
        let valid = if keyword == "oneOf" {
            matches == 1
        } else {
            matches >= 1
        };
        if !valid {
            return Err(format!(
                "{path}: matched {matches} branches of schema keyword {keyword}"
            ));
        }
    }
    if let Some(not_schema) = schema.get("not")
        && json_matches_schema(instance, not_schema, root, path).is_ok()
    {
        return Err(format!("{path}: matched forbidden schema"));
    }
    Ok(())
}

fn schema_type_matches(instance: &Value, expected: &Value) -> bool {
    match expected {
        Value::String(kind) => instance_matches_type(instance, kind),
        Value::Array(kinds) => kinds.iter().any(|kind| {
            kind.as_str()
                .is_some_and(|kind| instance_matches_type(instance, kind))
        }),
        _ => false,
    }
}

fn instance_matches_type(instance: &Value, kind: &str) -> bool {
    match kind {
        "null" => instance.is_null(),
        "boolean" => instance.is_boolean(),
        "object" => instance.is_object(),
        "array" => instance.is_array(),
        "number" => instance.is_number(),
        "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
        "string" => instance.is_string(),
        _ => false,
    }
}
