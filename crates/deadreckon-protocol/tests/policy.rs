use deadreckon_protocol::{
    FlightEvent, LedgerFile, LedgerItem, NarrativeSnapshotRef, RunEvent, SpendRecord, TraceRecord,
    is_persisted, ledger_file_for, redact_for_persistence,
};
use serde::Serialize;
use std::path::PathBuf;

const EVENTS: &str = include_str!("fixtures/pre-keel-run/events.jsonl");
const SPEND: &str = include_str!("fixtures/pre-keel-run/spend.jsonl");
const TRACES: &str = include_str!("fixtures/pre-keel-run/traces.jsonl");
const FLIGHT_EVENTS: &str = include_str!("fixtures/pre-keel-run/flight-events.jsonl");

#[test]
fn policy_reproduces_current_persistence_decisions() {
    let cases = [
        (
            LedgerItem::Event(parse(EVENTS)),
            LedgerFile::Events,
            EVENTS.trim_end(),
        ),
        (
            LedgerItem::Spend(parse(SPEND)),
            LedgerFile::Spend,
            SPEND.trim_end(),
        ),
        (
            LedgerItem::Trace(parse(TRACES)),
            LedgerFile::Traces,
            TRACES.trim_end(),
        ),
        (
            LedgerItem::Flight(parse(FLIGHT_EVENTS)),
            LedgerFile::FlightEvents,
            FLIGHT_EVENTS.trim_end(),
        ),
    ];

    for (item, file, expected_line) in cases {
        assert!(is_persisted(&item));
        assert_eq!(ledger_file_for(&item), Some(file));
        let redacted = redact_for_persistence(item);
        assert_eq!(bare_json(&redacted), expected_line);
    }

    let narrative = LedgerItem::NarrativeSnapshotRef(NarrativeSnapshotRef {
        snapshot_id: "snapshot-1".to_string(),
        path: PathBuf::from("narrative/snapshots.jsonl"),
    });
    assert!(is_persisted(&narrative));
    assert_eq!(
        ledger_file_for(&narrative),
        Some(LedgerFile::NarrativeSnapshots)
    );
    assert!(!is_persisted(&LedgerItem::Unknown));
    assert_eq!(ledger_file_for(&LedgerItem::Unknown), None);
}

#[test]
fn gate_nonce_redaction_lives_in_policy() {
    let mut trace: TraceRecord = parse(TRACES);
    trace.detail = serde_json::json!({
        "gate_nonce": "secret-a",
        "nested": {
            "gate/nonce": "secret-b",
            "safe": "visible",
        },
        "list": [{"gate-nonce": "secret-c"}],
    });

    let LedgerItem::Trace(redacted) = redact_for_persistence(LedgerItem::Trace(trace.clone()))
    else {
        panic!("trace remains a trace");
    };
    assert_eq!(redacted.detail["gate_nonce"], "[REDACTED]");
    assert_eq!(redacted.detail["nested"]["gate/nonce"], "[REDACTED]");
    assert_eq!(redacted.detail["list"][0]["gate-nonce"], "[REDACTED]");
    assert_eq!(redacted.detail["nested"]["safe"], "visible");

    assert_eq!(
        serde_json::to_value(trace).expect("original trace"),
        serde_json::json!({
            "timestamp": "2026-07-01T12:00:02Z",
            "run_id": "pre-keel-run",
            "turn": 1,
            "event": "provider.completed",
            "latency_ms": 2500,
            "detail": {
                "gate_nonce": "secret-a",
                "nested": {"gate/nonce": "secret-b", "safe": "visible"},
                "list": [{"gate-nonce": "secret-c"}],
            },
        })
    );
}

fn parse<T>(fixture: &str) -> T
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(fixture.trim_end()).expect("fixture parses")
}

fn bare_json(item: &LedgerItem) -> String {
    fn json<T: Serialize>(value: &T) -> String {
        serde_json::to_string(value).expect("bare line json")
    }

    match item {
        LedgerItem::Event(value) => json::<RunEvent>(value),
        LedgerItem::Spend(value) => json::<SpendRecord>(value),
        LedgerItem::Trace(value) => json::<TraceRecord>(value),
        LedgerItem::Flight(value) => json::<FlightEvent>(value),
        LedgerItem::NarrativeSnapshotRef(value) => json(value),
        LedgerItem::Unknown => panic!("unknown is not persisted"),
    }
}
