use chrono::{TimeZone, Utc};
use deadreckon_protocol::{RunEvent, RunEventKind, SpendRecord, TraceRecord};

const EVENTS: &str = include_str!("fixtures/pre-keel-run/events.jsonl");
const SPEND: &str = include_str!("fixtures/pre-keel-run/spend.jsonl");
const TRACES: &str = include_str!("fixtures/pre-keel-run/traces.jsonl");

#[test]
fn run_event_wire_bytes_unchanged_after_move() {
    let event = RunEvent {
        timestamp: Utc
            .with_ymd_and_hms(2026, 7, 1, 12, 0, 0)
            .single()
            .expect("fixture timestamp"),
        run_id: "pre-keel-run".to_string(),
        event: RunEventKind::ToolCallResult {
            turn: 1,
            tool_call_id: "tool-1".to_string(),
            status: "ok".to_string(),
            preview: "wrote src/lib.rs".to_string(),
        },
    };

    assert_eq!(
        format!("{}\n", serde_json::to_string(&event).expect("event json")),
        EVENTS
    );
}

#[test]
fn pre_keel_fixture_events_parse_identically() {
    for line in EVENTS.lines() {
        let event: RunEvent = serde_json::from_str(line).expect("pre-Keel event parses");
        assert_eq!(
            serde_json::to_string(&event).expect("event json"),
            line,
            "pre-Keel event bytes changed"
        );
    }
}

#[test]
fn spend_and_trace_wire_bytes_unchanged_after_move() {
    assert_fixture_roundtrips::<SpendRecord>(SPEND, "spend");
    assert_fixture_roundtrips::<TraceRecord>(TRACES, "trace");
}

#[test]
fn unknown_fields_still_tolerated() {
    let spend = r#"{"timestamp":"2026-07-01T12:00:01Z","turn":1,"provider":"cli:codex","model":"gpt-5.2-codex","input_tokens":120,"output_tokens":45,"cost_usd":0.01,"total_cost_usd":0.01,"cap_usd":1.0,"subscription":false,"estimated":true,"kind":"loop","future_field":{"nested":true}}"#;
    let trace = r#"{"timestamp":"2026-07-01T12:00:02Z","run_id":"pre-keel-run","turn":1,"event":"provider.completed","latency_ms":2500,"detail":{"status":"ok"},"future_field":[1,2,3]}"#;

    serde_json::from_str::<SpendRecord>(spend).expect("spend ignores unknown fields");
    serde_json::from_str::<TraceRecord>(trace).expect("trace ignores unknown fields");
}

fn assert_fixture_roundtrips<T>(fixture: &str, label: &str)
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    for line in fixture.lines() {
        let record: T = serde_json::from_str(line).unwrap_or_else(|error| {
            panic!("pre-Keel {label} parses: {error}");
        });
        assert_eq!(
            serde_json::to_string(&record).expect("record json"),
            line,
            "pre-Keel {label} bytes changed"
        );
    }
}
