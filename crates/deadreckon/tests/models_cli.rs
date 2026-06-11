#![allow(clippy::expect_used)]

//! `deadreckon models` — the catalog surface for picking a model before a
//! run/chain/orchestrate/campaign. Lists per-provider catalogs with the
//! recommended entry and the configured default marked.

use std::fs;

use deadreckon_core::DeadreckonPaths;
use serde_json::Value;

mod common;

use common::{assert_success, deadreckon, prepend_fake_cli_to_path, repo_tempdir, stdout};

#[test]
fn models_lists_catalog_for_explicit_provider_with_recommended_marker() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .args(["models", "cli:claude-code"])
        .output()
        .expect("models");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("provider default"), "{out}");
    assert!(out.contains("sonnet"), "{out}");
    assert!(out.contains("recommended"), "{out}");
    assert!(out.contains("deadreckon config model"), "{out}");
}

#[test]
fn models_json_is_additive_and_names_configured_default() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(
        paths.config_path(),
        "default_provider = \"cli:codex\"\n\n[defaults]\nmodel = \"gpt-5.1-codex\"\n",
    )
    .expect("config");

    let output = deadreckon(&paths)
        .args(["models", "cli:codex", "--json"])
        .output()
        .expect("models json");

    assert_success(&output);
    let value: Value = serde_json::from_str(&stdout(&output)).expect("json");
    assert_eq!(value["provider"], "cli:codex");
    assert_eq!(value["configured_default"], "gpt-5.1-codex");
    let models = value["models"].as_array().expect("models array");
    assert!(models.len() >= 2, "{models:?}");
    let default_entry = models
        .iter()
        .find(|m| m["id"] == "gpt-5.1-codex")
        .expect("configured model present");
    assert_eq!(default_entry["default"], true);
    assert!(
        models.iter().any(|m| m["recommended"] == true),
        "{models:?}"
    );
}

#[test]
fn models_unknown_provider_refuses_with_providers_list_try_line() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .args(["models", "cli:not-a-provider"])
        .output()
        .expect("models");

    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(err.contains("deadreckon providers list --all"), "{err}");
}

#[test]
fn models_without_provider_lists_every_credentialed_route() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let output = deadreckon(&paths)
        .args(["models", "--all"])
        .output()
        .expect("models all");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("cli:claude-code"), "{out}");
    assert!(out.contains("anthropic"), "{out}");
}

#[test]
fn cli_run_preview_defaults_to_a_ten_hour_wall_cap() {
    let temp = repo_tempdir();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let path_env = prepend_fake_cli_to_path(&temp, "codex");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .env("PATH", &path_env)
        .env("DEADRECKON_AUTH_PROBE", "0")
        .args([
            "run",
            "wall default preview",
            "--provider",
            "cli:codex",
            "--fresh",
            "--preview",
        ])
        .output()
        .expect("preview");

    let text = format!(
        "{}{}",
        stdout(&output),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("10h"),
        "subscription CLI runs default to a ten-hour wall cap: {text}"
    );
    assert!(!text.contains("1h\n"), "{text}");
}

#[test]
fn run_preview_names_resolved_model_for_cli_provider() {
    let temp = repo_tempdir();
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo).expect("repo");
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let path_env = prepend_fake_cli_to_path(&temp, "codex");

    let output = deadreckon(&paths)
        .current_dir(&repo)
        .env("PATH", &path_env)
        .env("DEADRECKON_AUTH_PROBE", "0")
        .args([
            "run",
            "preview model echo",
            "--provider",
            "cli:codex",
            "--fresh",
            "--preview",
            "--model",
            "preview-mx",
        ])
        .output()
        .expect("preview");

    let text = format!(
        "{}{}",
        stdout(&output),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(text.contains("preview-mx"), "{text}");
}
