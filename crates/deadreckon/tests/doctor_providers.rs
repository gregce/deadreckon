#![allow(clippy::expect_used)]

use std::fs;
use std::path::Path;

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

fn write_config(paths: &DeadreckonPaths, config: &str) {
    fs::write(paths.config_path(), config).expect("config");
}

fn run_doctor(paths: &DeadreckonPaths, path: &Path) -> String {
    let output = deadreckon(paths)
        .env("PATH", path)
        .arg("doctor")
        .output()
        .expect("doctor");
    assert_success(&output);
    stdout(&output)
}

#[cfg(unix)]
fn write_fake_cli(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("fake cli");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod fake cli");
}

#[test]
fn doctor_and_runtime_resolve_every_builtin_cli_to_the_same_binary() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");

    let mut config = String::from("default_provider = \"cli:pi\"\n\n");
    for (name, _) in CLI_PROVIDERS {
        config.push_str(&format!(
            "[providers.\"{name}\"]\nkind = \"{name}\"\nextra_args = []\n\n"
        ));
    }
    write_config(&paths, &config);

    let mut path = None;
    for (_, binary) in CLI_PROVIDERS {
        path = Some(prepend_fake_cli_to_path(&temp, binary));
    }
    let output = run_doctor(&paths, Path::new(&path.expect("path")));

    for (name, binary) in CLI_PROVIDERS {
        assert!(
            output.contains(&format!(
                "provider {name} kind=cli: passed - CLI binary {binary} found"
            )),
            "doctor did not bind {name} to {binary}:\n{output}"
        );
    }
}

#[test]
fn doctor_uses_the_config_binary_override_that_runtime_constructs() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    let binary = temp.path().join("pi-operator-override");
    write_fake_cli(&binary, "exit 0");
    write_config(
        &paths,
        &format!(
            r#"
default_provider = "cli:pi"

[providers."cli:pi"]
kind = "cli:pi"
binary = "{}"
extra_args = []
"#,
            binary.display()
        ),
    );

    let output = run_doctor(
        &paths,
        Path::new(&std::env::var("PATH").unwrap_or_default()),
    );

    assert!(
        output.contains(&format!("CLI binary {} found", binary.display())),
        "{output}"
    );
}

#[test]
fn doctor_rejects_a_descriptorless_custom_cli_that_runtime_cannot_construct() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    let binary = temp.path().join("my-tool");
    write_fake_cli(&binary, "exit 0");
    write_config(
        &paths,
        &format!(
            r#"
default_provider = "cli:my-tool"

[providers."cli:my-tool"]
kind = "cli:my-tool"
binary = "{}"
extra_args = []
"#,
            binary.display()
        ),
    );

    let output = run_doctor(
        &paths,
        Path::new(&std::env::var("PATH").unwrap_or_default()),
    );

    assert!(
        output.contains("generic provider cli:my-tool has no descriptor"),
        "{output}"
    );
    assert!(
        !output.contains("provider cli:my-tool kind=cli: passed"),
        "doctor must not claim a route is usable when runtime rejects it:\n{output}"
    );
}

#[test]
fn providers_list_probes_the_configured_runtime_binary_override() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    let binary = temp.path().join("pi-probe-override");
    write_fake_cli(
        &binary,
        "printf '%s\\n' 'pi - AI coding assistant with read, bash, edit, write tools'",
    );
    write_config(
        &paths,
        &format!(
            r#"
default_provider = "cli:pi"

[providers."cli:pi"]
kind = "cli:pi"
binary = "{}"
extra_args = []
"#,
            binary.display()
        ),
    );

    let output = deadreckon(&paths)
        .args(["providers", "list", "--json"])
        .output()
        .expect("providers list");
    assert_success(&output);
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("providers JSON");
    let providers = payload["providers"].as_array().expect("providers array");
    let pi = providers
        .iter()
        .find(|provider| provider["id"] == "cli:pi")
        .expect("pi provider");

    assert_eq!(pi["status"], "ok");
    assert_eq!(pi["location"], binary.display().to_string());
}

#[test]
fn doctor_live_runs_a_real_bounded_worker_turn_instead_of_claiming_static_readiness() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    fs::create_dir_all(paths.home()).expect("home");
    let binary = temp.path().join("pi-live-override");
    write_fake_cli(
        &binary,
        r#"if [ "$1" = "--help" ]; then
  printf '%s\n' 'pi - AI coding assistant with read, bash, edit, write tools Options: --mode --session'
  exit 0
fi
printf '%s\n' '{"id":"session-live","assistantMessageEvent":{"content":"DEADRECKON_PROVIDER_OK"},"message":{"usage":{"input":1,"output":1}}}'"#,
    );
    write_config(
        &paths,
        &format!(
            r#"
default_provider = "cli:pi"

[providers."cli:pi"]
kind = "cli:pi"
binary = "{}"
extra_args = []
"#,
            binary.display()
        ),
    );

    let static_output = run_doctor(
        &paths,
        Path::new(&std::env::var("PATH").unwrap_or_default()),
    );
    assert!(
        static_output.contains("live model turn not tested"),
        "{static_output}"
    );

    let live = deadreckon(&paths)
        .args(["doctor", "--live"])
        .output()
        .expect("live doctor");
    assert_success(&live);
    let output = stdout(&live);
    assert!(output.contains("live worker ping ok"), "{output}");
    assert!(output.contains("model provider default"), "{output}");

    let help = deadreckon(&paths)
        .args(["doctor", "--help"])
        .output()
        .expect("doctor help");
    assert_success(&help);
    assert!(stdout(&help).contains("--live"));
}
