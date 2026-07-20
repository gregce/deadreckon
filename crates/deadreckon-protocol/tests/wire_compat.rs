use chrono::{TimeZone, Utc};
use deadreckon_protocol::{RunEvent, RunEventKind};

const EVENTS: &str = include_str!("fixtures/pre-keel-run/events.jsonl");

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
