use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use deadreckon_protocol::{JobEventSequence, JobSchemaVersion, StopReason, all_schemas};

#[test]
fn job_event_schema_is_checked() {
    let generated = all_schemas()
        .into_iter()
        .find_map(|(name, schema)| (name == "job-event").then_some(schema))
        .expect("job-event belongs to the checked protocol schema set");
    let checked_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("docs/schemas/job-event.schema.json");
    let checked = fs::read_to_string(&checked_path)
        .unwrap_or_else(|error| panic!("{}: {error}", checked_path.display()));
    let mut rendered = serde_json::to_string_pretty(&generated).expect("render job-event schema");
    rendered.push('\n');

    assert_eq!(checked, rendered, "checked job-event schema drifted");
}

#[test]
fn job_event_sequence_starts_at_one() {
    assert!(
        serde_json::from_str::<JobEventSequence>("0").is_err(),
        "sequence zero must be rejected at the wire boundary"
    );
    let first =
        serde_json::from_str::<JobEventSequence>("1").expect("sequence one is the first event");
    assert_eq!(first.get(), 1);
    assert_eq!(serde_json::to_string(&first).expect("sequence json"), "1");
}

#[test]
fn job_schema_version_is_checked() {
    let current =
        serde_json::from_str::<JobSchemaVersion>("1").expect("current schema version parses");
    assert_eq!(current, JobSchemaVersion::CURRENT);
    assert_eq!(current.get(), 1);
    assert!(
        serde_json::from_str::<JobSchemaVersion>("2").is_err(),
        "unknown schema meaning must not be interpreted as v1"
    );
}

#[test]
fn job_stop_reasons_are_distinct() {
    let encoded = StopReason::ALL
        .into_iter()
        .map(|reason| serde_json::to_string(&reason).expect("stop reason json"))
        .collect::<BTreeSet<_>>();

    assert_eq!(encoded.len(), StopReason::ALL.len());
    assert_eq!(
        encoded,
        [
            "\"attempt_limit\"",
            "\"cancel_requested\"",
            "\"corrupt_history\"",
            "\"deadline\"",
            "\"deterministic_revise\"",
            "\"fatal_gate\"",
            "\"fatal_provider\"",
            "\"legacy_unknown\"",
            "\"lost_containment\"",
            "\"operator_input_required\"",
            "\"semantic_revise\"",
            "\"semantic_unavailable\"",
            "\"semantic_uncertain\"",
            "\"spend_cap\"",
            "\"transient_provider\"",
            "\"verified\"",
            "\"wall_cap\"",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    );
}
