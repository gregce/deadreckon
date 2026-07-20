use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[test]
fn protocol_crate_has_no_internal_dependencies() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(manifest_dir.join("Cargo.toml")).expect("manifest");
    let dependencies = dependency_names(&manifest);
    let expected = ["chrono", "schemars", "serde", "serde_json", "thiserror"]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();

    assert_eq!(dependencies, expected);
    assert!(
        dependencies
            .iter()
            .all(|dependency| !dependency.starts_with("deadreckon-"))
    );
    assert!(!manifest.contains("tokio"));
}

fn dependency_names(manifest: &str) -> BTreeSet<String> {
    let mut in_dependencies = false;
    let mut names = BTreeSet::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_dependencies = line == "[dependencies]";
            continue;
        }
        if in_dependencies && let Some((name, _)) = line.split_once('=') {
            names.insert(
                name.trim()
                    .strip_suffix(".workspace")
                    .unwrap_or_else(|| name.trim())
                    .to_string(),
            );
        }
    }
    names
}

#[test]
fn id_newtypes_serialize_transparently() {
    use deadreckon_protocol::{PlanId, RunId, TurnId};

    let run = RunId::from("run-1");
    let plan = PlanId::from("plan-1");
    let turn = TurnId::from("turn-1");

    assert_eq!(serde_json::to_string(&run).expect("run json"), r#""run-1""#);
    assert_eq!(
        serde_json::to_string(&plan).expect("plan json"),
        r#""plan-1""#
    );
    assert_eq!(
        serde_json::to_string(&turn).expect("turn json"),
        r#""turn-1""#
    );
    assert_eq!(
        serde_json::from_str::<RunId>(r#""run-1""#).expect("run parse"),
        run
    );
}
