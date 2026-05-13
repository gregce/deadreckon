use std::fs;
use std::path::PathBuf;

const CRATE_MANIFESTS: &[&str] = &[
    "crates/deadreckon/Cargo.toml",
    "crates/deadreckon-core/Cargo.toml",
    "crates/deadreckon-providers/Cargo.toml",
    "crates/deadreckon-runtime/Cargo.toml",
    "crates/deadreckon-sandbox/Cargo.toml",
];

#[test]
fn every_crate_inherits_workspace_lints() {
    let root = workspace_root();
    for manifest in CRATE_MANIFESTS {
        let text = fs::read_to_string(root.join(manifest)).expect("read crate Cargo.toml");
        assert!(
            text.contains("[lints]\nworkspace = true"),
            "{manifest} must inherit [workspace.lints]"
        );
    }
}

#[test]
fn clippy_toml_allows_unwrap_in_tests() {
    let text = fs::read_to_string(workspace_root().join("clippy.toml")).expect("read clippy.toml");
    assert!(text.contains("allow-unwrap-in-tests = true"));
    assert!(text.contains("allow-expect-in-tests = true"));
    assert!(text.contains("allow-dbg-in-tests = true"));
    assert!(text.contains("large-error-threshold = 256"));
}

#[test]
fn clippy_warn_snapshot_present() {
    let path = workspace_root().join("tests/.clippy-warn-snapshot");
    let text = fs::read_to_string(&path).expect("read clippy warn snapshot");
    assert!(
        !text.trim().is_empty(),
        "{} should record the P2 warn-only clippy output",
        path.display()
    );
}

fn workspace_root() -> PathBuf {
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if dir.join("crates").is_dir() && dir.join("Cargo.toml").is_file() {
            return dir;
        }
        assert!(dir.pop(), "could not locate workspace root");
    }
}
