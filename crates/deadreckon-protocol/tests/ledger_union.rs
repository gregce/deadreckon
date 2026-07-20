use std::path::PathBuf;

use deadreckon_protocol::{
    EventLine, FlightEvent, FlightLine, LedgerFile, LedgerItem, NarrativeSnapshotRef,
    NarrativeSnapshotRefLine, RunEvent, SpendLine, SpendRecord, TraceLine, TraceRecord,
};

const EVENTS: &str = include_str!("fixtures/pre-keel-run/events.jsonl");
const SPEND: &str = include_str!("fixtures/pre-keel-run/spend.jsonl");
const TRACES: &str = include_str!("fixtures/pre-keel-run/traces.jsonl");
const FLIGHT_EVENTS: &str = include_str!("fixtures/pre-keel-run/flight-events.jsonl");

#[test]
fn ledger_item_tags_every_kind_snake_case() {
    let (event, spend, trace, flight, narrative) = fixture_items();
    let items = [
        (LedgerItem::Event(event.clone()), "event"),
        (LedgerItem::Spend(spend.clone()), "spend"),
        (LedgerItem::Trace(trace.clone()), "trace"),
        (LedgerItem::Flight(flight.clone()), "flight"),
        (
            LedgerItem::NarrativeSnapshotRef(narrative.clone()),
            "narrative_snapshot_ref",
        ),
    ];

    for (item, expected) in items {
        let value = serde_json::to_value(item).expect("ledger item json");
        assert_eq!(value["kind"], expected);
        assert!(value.get("value").is_some(), "{expected} lost its payload");
    }

    let alias: LedgerItem = serde_json::from_value(serde_json::json!({
        "kind": "run_event",
        "value": event,
    }))
    .expect("event alias parses");
    assert!(matches!(alias, LedgerItem::Event(_)));

    assert_eq!(
        serde_json::to_string(&EventLine(event)).expect("event line"),
        EVENTS.trim_end()
    );
    assert_eq!(
        serde_json::to_string(&SpendLine(spend)).expect("spend line"),
        SPEND.trim_end()
    );
    assert_eq!(
        serde_json::to_string(&TraceLine(trace)).expect("trace line"),
        TRACES.trim_end()
    );
    assert_eq!(
        serde_json::to_string(&FlightLine(flight)).expect("flight line"),
        FLIGHT_EVENTS.trim_end()
    );
    let narrative_value =
        serde_json::to_value(NarrativeSnapshotRefLine(narrative)).expect("narrative ref line");
    assert_eq!(narrative_value["snapshot_id"], "snapshot-1");
}

#[test]
fn unknown_kind_parses_as_unknown_not_error() {
    let item: LedgerItem = serde_json::from_str(
        r#"{"kind":"future_ledger_kind","value":{"schema":99,"payload":"kept safe"}}"#,
    )
    .expect("unknown ledger kind degrades");
    assert!(matches!(item, LedgerItem::Unknown));
}

#[test]
fn ledger_file_mapping_is_total_over_persisted_kinds() {
    let (event, spend, trace, flight, narrative) = fixture_items();
    let cases = [
        (LedgerItem::Event(event), LedgerFile::Events),
        (LedgerItem::Spend(spend), LedgerFile::Spend),
        (LedgerItem::Trace(trace), LedgerFile::Traces),
        (LedgerItem::Flight(flight), LedgerFile::FlightEvents),
        (
            LedgerItem::NarrativeSnapshotRef(narrative),
            LedgerFile::NarrativeSnapshots,
        ),
    ];

    for (item, expected) in cases {
        assert_eq!(LedgerFile::for_item(&item), Some(expected));
    }
    assert_eq!(LedgerFile::for_item(&LedgerItem::Unknown), None);

    assert_eq!(LedgerFile::Events.relative_path(), "events.jsonl");
    assert_eq!(LedgerFile::Spend.relative_path(), "spend.jsonl");
    assert_eq!(LedgerFile::Traces.relative_path(), "traces.jsonl");
    assert_eq!(
        LedgerFile::FlightEvents.relative_path(),
        "flight-events.jsonl"
    );
    assert_eq!(
        LedgerFile::NarrativeSnapshots.relative_path(),
        "narrative/snapshots.jsonl"
    );
}

fn fixture_items() -> (
    RunEvent,
    SpendRecord,
    TraceRecord,
    FlightEvent,
    NarrativeSnapshotRef,
) {
    (
        serde_json::from_str(EVENTS.trim_end()).expect("event fixture"),
        serde_json::from_str(SPEND.trim_end()).expect("spend fixture"),
        serde_json::from_str(TRACES.trim_end()).expect("trace fixture"),
        serde_json::from_str(FLIGHT_EVENTS.trim_end()).expect("flight fixture"),
        NarrativeSnapshotRef {
            snapshot_id: "snapshot-1".to_string(),
            path: PathBuf::from("narrative/snapshots.jsonl"),
        },
    )
}
