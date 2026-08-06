#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::redundant_clone
)]

use std::fs;
use std::path::Path;

use serde_json::Value;

mod common;

use common::{
    assert_success_with_labels as assert_success, deadreckon_home_no_color as deadreckon,
    repo_tempdir_with_empty_bin as repo_tempdir,
};

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
    assert!(stdout.contains("blocked providers configured"), "{stdout}");
    assert!(stdout.contains("Explanation"), "{stdout}");
    assert!(stdout.contains("Evidence"), "{stdout}");
    assert_eq!(stdout.matches("\nRecommended\n").count(), 1, "{stdout}");
    assert!(
        stdout.contains("Recommended\ndeadreckon detect cli:codex"),
        "{stdout}"
    );
    assert!(!stdout.contains("hint:"), "{stdout}");
    assert!(!stdout.contains("try:"), "{stdout}");
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
fn providers_list_json_adds_verdict_and_primary_action() {
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
        .args(["providers", "list", "--json"])
        .env("PATH", temp.path().join("empty-bin"))
        .output()
        .expect("providers list json");

    assert_success(&output);
    let json: Value = serde_json::from_slice(&output.stdout).expect("json");
    assert!(json["providers"].is_array());
    assert!(json["verdict"].is_object());
    assert!(json["primary_action"].is_string());
    assert_eq!(
        json["primary_action"],
        json["verdict"]["recommended_command"]
    );
    assert_eq!(json["next_actions"][0], json["primary_action"]);
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
fn providers_listing_marks_contract_bearing_routes() {
    let temp = repo_tempdir();

    let plain = deadreckon(temp.path())
        .args(["providers", "list", "--all"])
        .env("PATH", temp.path().join("empty-bin"))
        .output()
        .expect("providers list contracts");
    assert_success(&plain);
    let stdout = String::from_utf8_lossy(&plain.stdout);
    let pi = stdout
        .lines()
        .find(|line| line.contains("cli:pi"))
        .expect("Pi row");
    let copilot = stdout
        .lines()
        .find(|line| line.contains("cli:copilot"))
        .expect("Copilot row");
    let gemini = stdout
        .lines()
        .find(|line| line.contains("cli:gemini"))
        .expect("Gemini row");
    assert!(pi.contains("contract=yes"), "{pi}");
    assert!(pi.contains("review=external"), "{pi}");
    assert!(copilot.contains("contract=yes"), "{copilot}");
    assert!(copilot.contains("review=external"), "{copilot}");
    assert!(gemini.contains("contract=no"), "{gemini}");
    assert!(gemini.contains("review=external"), "{gemini}");

    let json_output = deadreckon(temp.path())
        .args(["providers", "list", "--all", "--json"])
        .env("PATH", temp.path().join("empty-bin"))
        .output()
        .expect("providers list contracts json");
    assert_success(&json_output);
    let json: Value = serde_json::from_slice(&json_output.stdout).expect("json");
    let providers = json["providers"].as_array().expect("providers");
    let contract = |id: &str| {
        providers
            .iter()
            .find(|provider| provider["id"] == id)
            .and_then(|provider| provider["contract"].as_bool())
    };
    assert_eq!(contract("cli:pi"), Some(true));
    assert_eq!(contract("cli:copilot"), Some(true));
    assert_eq!(contract("cli:gemini"), Some(false));
    assert_eq!(contract("cli:opencode"), Some(true));
    let review = |id: &str| {
        providers
            .iter()
            .find(|provider| provider["id"] == id)
            .and_then(|provider| provider["schema_only_review"].as_bool())
    };
    assert_eq!(review("cli:copilot"), Some(false));
    assert_eq!(review("cli:gemini"), Some(false));
    assert_eq!(review("cli:opencode"), Some(false));
    assert_eq!(review("cli:pi"), Some(false));
    assert_eq!(review("cli:codex"), Some(true));
}

#[test]
fn providers_list_surfaces_malformed_contract_warning() {
    let temp = repo_tempdir();
    write_provider_override(
        temp.path(),
        "pi.toml",
        r#"
id = "cli:pi"

[contract]
stream_args = []
dialect = "json-lines"
"#,
    );

    let output = deadreckon(temp.path())
        .args(["providers", "list", "--all"])
        .env("PATH", temp.path().join("empty-bin"))
        .output()
        .expect("providers list malformed contract");
    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("malformed [contract] field stream_args"),
        "{stdout}"
    );
    assert!(
        stdout.contains("try: deadreckon providers check cli:pi"),
        "{stdout}"
    );

    let check = deadreckon(temp.path())
        .args(["providers", "check", "cli:pi"])
        .env("PATH", temp.path().join("empty-bin"))
        .output()
        .expect("providers check malformed contract");
    assert_success(&check);
    let check_stdout = String::from_utf8_lossy(&check.stdout);
    assert!(
        check_stdout.contains("malformed [contract] field stream_args"),
        "{check_stdout}"
    );
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

fn write_config(home: &Path, body: &str) {
    fs::create_dir_all(home).expect("home");
    fs::write(home.join("config.toml"), body).expect("write config");
}

fn write_provider_override(home: &Path, name: &str, body: &str) {
    let dir = home.join("providers.d");
    fs::create_dir_all(&dir).expect("providers.d");
    fs::write(dir.join(name), body).expect("write provider override");
}
