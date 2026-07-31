#![allow(clippy::expect_used)]

use std::fs;

use deadreckon_core::DeadreckonPaths;

mod common;

use common::{assert_success, deadreckon, prepend_fake_cli_to_path, repo_tempdir, stdout};

const CLI_PROVIDERS: &[(&str, &str)] = &[
    ("cli:claude-code", "claude"),
    ("cli:codex", "codex"),
    ("cli:codex-server", "codex"),
    ("cli:gemini", "gemini"),
    ("cli:opencode", "opencode"),
    ("cli:copilot", "copilot"),
    ("cli:pi", "pi"),
];

fn pi_config() -> String {
    r#"
default_provider = "cli:pi"

[providers."cli:pi"]
kind = "cli:pi"
extra_args = []
"#
    .to_string()
}

fn write_config(paths: &DeadreckonPaths, config: &str) {
    fs::write(paths.config_path(), config).expect("config");
}

fn run_doctor(paths: &DeadreckonPaths, path: &std::path::Path) -> String {
    let output = deadreckon(paths)
        .env("PATH", path)
        .arg("doctor")
        .output()
        .expect("doctor");
    assert_success(&output);
    stdout(&output)
}

#[test]
fn doctor_reports_correct_binary_for_pi() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    write_config(&paths, &pi_config());

    // A fake `pi` on PATH: the doctor must probe it, not a hardcoded codex.
    let path = prepend_fake_cli_to_path(&temp, "pi");
    let stdout = run_doctor(&paths, std::path::Path::new(&path));

    assert!(stdout.contains("CLI binary pi found"), "{stdout}");
    assert!(
        !stdout.contains("CLI binary codex"),
        "pi must not be reported as the codex binary:\n{stdout}"
    );
}

#[test]
fn doctor_reports_pi_missing_without_the_binary() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    write_config(&paths, &pi_config());

    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin).expect("empty bin");
    let stdout = run_doctor(&paths, &empty_bin);

    assert!(stdout.contains("CLI binary pi missing"), "{stdout}");
    assert!(
        !stdout.contains("CLI binary codex missing"),
        "pi must not be reported as a missing codex:\n{stdout}"
    );
}

#[test]
fn doctor_lists_all_cli_providers_with_correct_binaries() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");

    let mut config = String::from("default_provider = \"cli:codex\"\n\n");
    for (name, _) in CLI_PROVIDERS {
        config.push_str(&format!(
            "[providers.\"{name}\"]\nkind = \"{name}\"\nextra_args = []\n\n"
        ));
    }
    write_config(&paths, &config);

    // Every built-in CLI binary lands in the same fake-bin dir; the last
    // returned PATH includes them all.
    let mut path = None;
    for (_, binary) in CLI_PROVIDERS {
        path = Some(prepend_fake_cli_to_path(&temp, binary));
    }
    let stdout = run_doctor(&paths, std::path::Path::new(&path.expect("path")));

    for (name, binary) in CLI_PROVIDERS {
        assert!(
            stdout.contains(&format!("CLI binary {binary} found")),
            "provider {name} should resolve to its default binary {binary}:\n{stdout}"
        );
    }
    assert!(
        !stdout.contains("CLI binary codex missing"),
        "no provider may be reported against the wrong binary:\n{stdout}"
    );
}

#[test]
fn doctor_checks_codex_binary_unchanged() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    write_config(
        &paths,
        r#"
default_provider = "cli:codex"

[providers."cli:codex"]
kind = "cli-codex"
extra_args = []
"#,
    );

    let path = prepend_fake_cli_to_path(&temp, "codex");
    let stdout = run_doctor(&paths, std::path::Path::new(&path));

    assert!(stdout.contains("CLI binary codex found"), "{stdout}");
}

#[test]
fn doctor_handles_unknown_provider_name() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    write_config(
        &paths,
        r#"
default_provider = "my-tool"

[providers."my-tool"]
kind = "cli:my-tool"
extra_args = []
"#,
    );

    let empty_bin = temp.path().join("empty-bin");
    fs::create_dir_all(&empty_bin).expect("empty bin");
    let stdout = run_doctor(&paths, &empty_bin);

    // No registry descriptor: the binary name is derived from the provider
    // name ("my-tool"), and the check must not panic.
    assert!(stdout.contains("CLI binary my-tool missing"), "{stdout}");
}
