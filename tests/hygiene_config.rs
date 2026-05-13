use std::fs;
use std::path::PathBuf;
use std::process::Command;

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
fn lint_table_denies_unwrap_used() {
    assert_lint_level("unwrap_used", "deny");
}

#[test]
fn lint_table_denies_expect_used() {
    assert_lint_level("expect_used", "deny");
}

#[test]
fn lint_table_denies_await_holding_lock() {
    assert_lint_level("await_holding_lock", "deny");
}

#[test]
fn clippy_runs_clean_under_deny_warnings() {
    let output = Command::new("cargo")
        .args(["clippy", "--workspace", "--", "-D", "warnings"])
        .current_dir(workspace_root())
        .output()
        .expect("run cargo clippy --workspace -- -D warnings");
    assert!(
        output.status.success(),
        "clippy must be clean under -D warnings\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_lint_level(lint: &str, level: &str) {
    let text = fs::read_to_string(workspace_root().join("Cargo.toml")).expect("read Cargo.toml");
    let needle = format!("{lint} = \"{level}\"");
    assert!(text.contains(&needle), "missing `{needle}`");
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
