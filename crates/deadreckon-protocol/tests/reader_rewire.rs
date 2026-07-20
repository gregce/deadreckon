use std::fs;
use std::path::Path;

#[test]
fn run_view_builds_from_protocol_types_only() {
    let run_view = Path::new(env!("CARGO_MANIFEST_DIR")).join("../deadreckon-core/src/run_view.rs");
    let source = fs::read_to_string(&run_view).expect("read run_view.rs");

    assert!(
        source.contains(
            "use deadreckon_protocol::{RunEvent, RunEventKind, SpendRecord, TraceRecord};"
        ),
        "RunView must import persisted line types directly from deadreckon-protocol"
    );
    for local_definition in [
        "struct RunEvent",
        "enum RunEventKind",
        "struct SpendRecord",
        "struct TraceRecord",
        "struct FlightEvent",
        "struct NarrativeSnapshotRef",
    ] {
        assert!(
            !source.contains(local_definition),
            "RunView must not define protocol wire type {local_definition}"
        );
    }
}
