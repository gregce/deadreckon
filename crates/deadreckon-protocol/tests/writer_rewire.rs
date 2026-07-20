#![allow(clippy::expect_used)]

use std::fs;
use std::path::Path;

#[test]
fn no_ledger_line_type_defined_outside_protocol_crate() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("protocol crate lives under the workspace crates directory");
    let crates = workspace.join("crates");
    let mut violations = Vec::new();

    visit_rust_sources(&crates, &mut |path, source| {
        if path.starts_with(env!("CARGO_MANIFEST_DIR")) {
            return;
        }

        for declaration in [
            "struct RunEvent {",
            "enum RunEventKind {",
            "struct SpendRecord {",
            "struct TraceRecord {",
            "struct FlightEvent {",
            "enum FlightEventKind {",
            "struct FlightUsage {",
            "struct NarrativeSnapshotRef {",
        ] {
            if source.contains(declaration) {
                violations.push(format!("{} defines {}", path.display(), declaration.trim()));
            }
        }

        if source.contains("pub use deadreckon_protocol") {
            violations.push(format!(
                "{} publicly re-exports protocol ledger types",
                path.display()
            ));
        }
    });

    let core_root = fs::read_to_string(crates.join("deadreckon-core/src/lib.rs"))
        .expect("read deadreckon-core root");
    for reexport in [
        "SpendRecord",
        "TraceRecord",
        "RunEvent",
        "RunEventKind",
        "FlightEvent",
        "FlightEventKind",
        "FlightUsage",
        "NarrativeSnapshotRef",
    ] {
        if core_root
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
            .any(|identifier| identifier == reexport)
        {
            violations.push(format!(
                "deadreckon-core/src/lib.rs publicly re-exports {reexport}"
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "deadreckon-protocol must be the sole public owner of ledger line types:\n{}",
        violations.join("\n")
    );
}

fn visit_rust_sources(root: &Path, visit: &mut impl FnMut(&Path, &str)) {
    for entry in fs::read_dir(root).expect("read crates directory") {
        let entry = entry.expect("read directory entry");
        let path = entry.path();
        if path.is_dir() {
            visit_rust_sources(&path, visit);
        } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
            let source = fs::read_to_string(&path).expect("read Rust source");
            visit(&path, &source);
        }
    }
}
