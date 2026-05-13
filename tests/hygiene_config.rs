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

#[test]
fn rustfmt_toml_pins_imports_granularity_item() {
    let text = fs::read_to_string(workspace_root().join("rustfmt.toml")).expect("read rustfmt.toml");
    assert!(text.contains("edition = \"2024\""));
    assert!(text.contains("imports_granularity = \"Item\""));
    assert!(text.contains("group_imports = \"StdExternalCrate\""));
    assert!(text.contains("reorder_imports = true"));
}

#[test]
fn rustfmt_check_clean() {
    let output = Command::new("cargo")
        .args(["fmt", "--check"])
        .current_dir(workspace_root())
        .output()
        .expect("run cargo fmt --check");
    assert!(
        output.status.success(),
        "rustfmt must be clean\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn format_commit_touches_only_whitespace_and_imports() {
    let root = workspace_root();
    let commit = git_stdout(
        &root,
        &[
            "log",
            "--format=%H",
            "--grep=^style: apply rustfmt with imports_granularity=Item$",
            "-1",
        ],
    );
    assert!(!commit.trim().is_empty(), "missing dedicated rustfmt commit");
    let files = git_stdout(
        &root,
        &["show", "--name-only", "--format=", commit.trim()],
    );
    let files = files
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    assert!(
        files
            .iter()
            .all(|path| *path == "rustfmt.toml" || path.ends_with(".rs")),
        "format commit touched non-format files: {files:?}"
    );
    for file in files.iter().filter(|path| path.ends_with(".rs")) {
        assert_rs_identifier_set_unchanged(&root, commit.trim(), file);
    }
}

fn assert_lint_level(lint: &str, level: &str) {
    let text = fs::read_to_string(workspace_root().join("Cargo.toml")).expect("read Cargo.toml");
    let needle = format!("{lint} = \"{level}\"");
    assert!(text.contains(&needle), "missing `{needle}`");
}

fn assert_rs_identifier_set_unchanged(root: &PathBuf, commit: &str, file: &str) {
    let before = git_stdout(root, &["show", &format!("{commit}^:{file}")]);
    let after = git_stdout(root, &["show", &format!("{commit}:{file}")]);
    assert_eq!(
        identifier_tokens(&before),
        identifier_tokens(&after),
        "format commit changed Rust identifiers in {file}"
    );
}

fn identifier_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens.sort();
    tokens
}

fn git_stdout(root: &PathBuf, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("git stdout utf8")
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
