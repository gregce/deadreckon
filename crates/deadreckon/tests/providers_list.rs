#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::redundant_clone
)]

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

#[test]
fn providers_list_default_shows_configured_only() {
    let temp = repo_tempdir();
    write_config(
        temp.path(),
        r#"
default_provider = "cli:codex"
fallback = ["anthropic"]

[providers."cli:codex"]
kind = "cli-codex"

[providers.anthropic]
kind = "anthropic"
"#,
    );

    let output = deadreckon(temp.path())
        .args(["providers", "list"])
        .env("PATH", temp.path().join("empty-bin"))
        .output()
        .expect("providers list");

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cli:codex"));
    assert!(stdout.contains("anthropic"));
    assert!(!stdout.contains("openai-compatible"));
}

#[test]
fn providers_list_all_includes_built_ins_not_in_config() {
    let temp = repo_tempdir();
    write_config(
        temp.path(),
        r#"
default_provider = "cli:codex"

[providers."cli:codex"]
kind = "cli-codex"
"#,
    );

    let output = deadreckon(temp.path())
        .args(["providers", "list", "--all"])
        .env("PATH", temp.path().join("empty-bin"))
        .output()
        .expect("providers list all");

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cli:codex"));
    assert!(stdout.contains("cli:gemini"));
    assert!(stdout.contains("cli:opencode"));
    assert!(stdout.contains("cli:copilot"));
    assert!(stdout.contains("cli:pi"));
    assert!(stdout.contains("openai-compatible"));
    assert!(stdout.contains("smoke"));
}

#[test]
fn providers_list_all_includes_copilot_and_pi() {
    let temp = repo_tempdir();
    write_config(
        temp.path(),
        r#"
default_provider = "cli:codex"

[providers."cli:codex"]
kind = "cli-codex"
"#,
    );

    let output = deadreckon(temp.path())
        .args(["providers", "list", "--all"])
        .env("PATH", temp.path().join("empty-bin"))
        .output()
        .expect("providers list all");

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cli:copilot"), "{stdout}");
    assert!(stdout.contains("cli:pi"), "{stdout}");
}

#[test]
fn providers_list_models_includes_aliases() {
    let temp = repo_tempdir();

    let output = deadreckon(temp.path())
        .args(["providers", "list", "--all", "--models"])
        .env("PATH", temp.path().join("empty-bin"))
        .output()
        .expect("providers list models");

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("gpt-4o-mini"));
    assert!(stdout.contains("aliases=4o-mini"));
    assert!(stdout.contains("claude-sonnet-4-5"));
}

#[test]
fn providers_list_full_emits_exact_paths_no_truncation() {
    let temp = repo_tempdir();
    let exact_path = "/tmp/deadreckon/provider-registry/very/long/path/to/codex";
    write_config(
        temp.path(),
        r#"
default_provider = "cli:codex"

[providers."cli:codex"]
kind = "cli-codex"
"#,
    );
    write_provider_override(
        temp.path(),
        "codex.toml",
        &format!(
            r#"
id = "cli:codex"
default_binary = "{exact_path}"
"#
        ),
    );

    let output = deadreckon(temp.path())
        .args(["providers", "list", "--full"])
        .env("PATH", temp.path().join("empty-bin"))
        .output()
        .expect("providers list full");

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(exact_path), "{stdout}");
}

fn repo_tempdir() -> TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.test-tmp");
    fs::create_dir_all(&root).expect("test tmp root");
    let temp = TempDir::new_in(&root).expect("tempdir");
    fs::create_dir_all(temp.path().join("empty-bin")).expect("empty bin");
    temp
}

fn deadreckon(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command.env("DEADRECKON_HOME", home).env("NO_COLOR", "1");
    command
}

fn write_config(home: &Path, body: &str) {
    fs::create_dir_all(home).expect("home");
    fs::write(home.join("config.toml"), body).expect("write config");
}

fn write_provider_override(home: &Path, name: &str, body: &str) {
    let dir = home.join("providers.d");
    fs::create_dir_all(&dir).expect("providers.d");
    fs::write(dir.join(name), body).expect("write provider override");
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
