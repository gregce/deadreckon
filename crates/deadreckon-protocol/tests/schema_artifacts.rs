use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use deadreckon_protocol::all_schemas;
use schemars::schema::RootSchema;
use serde_json::Value;

const UPDATE_COMMAND: &str = "DEADRECKON_UPDATE_SCHEMAS=1 cargo test -p deadreckon-protocol";

fn schemas_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("docs/schemas")
}

fn render_schema(schema: &RootSchema) -> String {
    let mut rendered = serde_json::to_string_pretty(schema).unwrap();
    rendered.push('\n');
    rendered
}

fn drift_message(detail: &str) -> String {
    format!("{detail}\n\nregenerate checked schemas with:\n  {UPDATE_COMMAND}")
}

#[test]
fn schema_drift_failure_names_update_command() {
    let message = drift_message("schema drifted");
    assert!(message.contains(UPDATE_COMMAND), "{message}");

    let readme = fs::read_to_string(schemas_dir().join("README.md"))
        .expect("docs/schemas/README.md must explain checked schema regeneration");
    assert!(readme.contains(UPDATE_COMMAND), "{readme}");
}

#[test]
fn generated_schemas_match_checked_in_files() {
    let schemas = all_schemas();
    let expected_names = schemas
        .iter()
        .map(|(name, _)| format!("{name}.schema.json"))
        .collect::<BTreeSet<_>>();
    let schemas_dir = schemas_dir();

    if std::env::var_os("DEADRECKON_UPDATE_SCHEMAS").as_deref() == Some("1".as_ref()) {
        fs::create_dir_all(&schemas_dir).unwrap();
        for (name, schema) in &schemas {
            fs::write(
                schemas_dir.join(format!("{name}.schema.json")),
                render_schema(schema),
            )
            .unwrap();
        }
    }

    let actual_names = fs::read_dir(&schemas_dir)
        .unwrap_or_else(|error| panic!("{}", drift_message(&error.to_string())))
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".schema.json"))
        .collect::<BTreeSet<_>>();

    assert_eq!(
        actual_names,
        expected_names,
        "{}",
        drift_message("checked schema file set differs from the protocol vocabulary")
    );

    for (name, schema) in schemas {
        let path = schemas_dir.join(format!("{name}.schema.json"));
        let checked_in = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}", drift_message(&error.to_string())));
        assert_eq!(
            checked_in,
            render_schema(&schema),
            "{}",
            drift_message(&format!("schema drifted: {}", path.display()))
        );
    }
}

#[test]
fn schema_covers_every_ledger_item_variant() {
    let ledger_schema = all_schemas()
        .into_iter()
        .find_map(|(name, schema)| (name == "ledger-item").then_some(schema))
        .expect("ledger-item schema must be generated");
    let schema = serde_json::to_value(ledger_schema).unwrap();
    let mut singleton_strings = BTreeSet::new();
    collect_singleton_enum_strings(&schema, &mut singleton_strings);

    for kind in [
        "event",
        "spend",
        "trace",
        "flight",
        "narrative_snapshot_ref",
    ] {
        assert!(
            singleton_strings.contains(kind),
            "ledger-item schema omits canonical kind {kind:?}"
        );
    }
}

fn collect_singleton_enum_strings(value: &Value, found: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(values) = object.get("enum").and_then(Value::as_array)
                && let [Value::String(value)] = values.as_slice()
            {
                found.insert(value.clone());
            }
            for child in object.values() {
                collect_singleton_enum_strings(child, found);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_singleton_enum_strings(child, found);
            }
        }
        _ => {}
    }
}
