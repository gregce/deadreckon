#![allow(
    clippy::expect_used,
    clippy::needless_pass_by_value,
    clippy::redundant_clone
)]

use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

#[test]
fn detect_lists_every_registered_provider() {
    let temp = repo_tempdir();
    let output = deadreckon(temp.path())
        .arg("detect")
        .env("PATH", temp.path().join("empty-bin"))
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_COMPATIBLE_API_KEY")
        .output()
        .expect("detect");

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    for id in [
        "anthropic",
        "openai",
        "openai-compatible",
        "smoke",
        "cli:claude-code",
        "cli:codex",
        "cli:gemini",
        "cli:opencode",
    ] {
        assert!(
            stdout.contains(id),
            "{id} missing from detect output:\n{stdout}"
        );
    }
}

#[test]
fn detect_lists_new_cli_descriptors_with_install_hints() {
    let temp = repo_tempdir();
    let output = deadreckon(temp.path())
        .arg("detect")
        .env("PATH", temp.path().join("empty-bin"))
        .output()
        .expect("detect");

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cli:gemini"), "{stdout}");
    assert!(
        stdout.contains("npm install -g @google/gemini-cli"),
        "{stdout}"
    );
    assert!(stdout.contains("cli:opencode"), "{stdout}");
    assert!(
        stdout.contains("curl -fsSL https://opencode.ai/install | bash"),
        "{stdout}"
    );
}

#[test]
fn init_yes_prefers_registry_cli_binary_order() {
    let temp = repo_tempdir();
    let bin = temp.path().join("bin");
    write_fake_binary(&bin, "a-first", "a-first 1.0.0");
    write_fake_binary(&bin, "z-second", "z-second 1.0.0");
    write_provider_override(
        temp.path(),
        "a-first.toml",
        r#"
id = "cli:a-first"
display_name = "A First CLI"
kind = "cli"
default_binary = "a-first"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["run", "{prompt}"]
"#,
    );
    write_provider_override(
        temp.path(),
        "z-second.toml",
        r#"
id = "cli:z-second"
display_name = "Z Second CLI"
kind = "cli"
default_binary = "z-second"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["run", "{prompt}"]
"#,
    );

    let output = deadreckon(temp.path())
        .args(["init", "--no-confirm", "--no-completion"])
        .env("PATH", &bin)
        .output()
        .expect("init");

    assert_success(&output);
    let config = fs::read_to_string(temp.path().join("config.toml")).expect("config");
    assert!(
        config.contains("default_provider = \"cli:a-first\""),
        "{config}"
    );
    assert!(
        config.contains("doc_provider = \"cli:a-first\""),
        "{config}"
    );
}

#[test]
fn detect_marks_cli_codex_ok_when_fake_binary_in_path() {
    let temp = repo_tempdir();
    let bin = temp.path().join("bin");
    write_fake_binary(&bin, "codex", "codex-cli 9.9.9");

    let output = deadreckon(temp.path())
        .arg("detect")
        .arg("cli:codex")
        .env("PATH", &bin)
        .output()
        .expect("detect codex");

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("cli:codex"));
    assert!(stdout.contains("ready"));
    assert!(stdout.contains("codex-cli 9.9.9"));
}

#[test]
fn detect_marks_anthropic_missing_credential_when_env_unset() {
    let temp = repo_tempdir();

    let output = deadreckon(temp.path())
        .arg("detect")
        .arg("anthropic")
        .env_remove("ANTHROPIC_API_KEY")
        .output()
        .expect("detect anthropic");

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ANTHROPIC_API_KEY missing"));
    assert!(stdout.contains("try:"));
    assert!(stdout.contains("export ANTHROPIC_API_KEY"));
}

#[test]
fn detect_marks_cli_provider_version_mismatch_with_min_known_good() {
    let temp = repo_tempdir();
    let bin = temp.path().join("bin");
    write_fake_binary(&bin, "codex", "codex-cli 0.1.0");
    write_provider_override(
        temp.path(),
        "codex.toml",
        r#"
id = "cli:codex"

[version_probe]
args = ["--version"]
min_known_good = "99.0.0"
"#,
    );

    let output = deadreckon(temp.path())
        .arg("detect")
        .arg("cli:codex")
        .env("PATH", &bin)
        .output()
        .expect("detect codex");

    assert_success(&output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("version below known-good 99.0.0"));
    assert!(stdout.contains("try:"));
    assert!(stdout.contains("npm i -g @openai/codex"));
}

#[test]
fn detect_json_output_matches_schema() {
    let temp = repo_tempdir();

    let output = deadreckon(temp.path())
        .arg("detect")
        .arg("--json")
        .env("PATH", temp.path().join("empty-bin"))
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .env_remove("OPENAI_COMPATIBLE_API_KEY")
        .output()
        .expect("detect json");

    assert_success(&output);
    let json: Value = serde_json::from_slice(&output.stdout).expect("json output");
    let providers = json["providers"].as_array().expect("providers array");
    let codex = providers
        .iter()
        .find(|provider| provider["id"] == "cli:codex")
        .expect("codex provider");
    assert_eq!(codex["kind"], "cli");
    assert_eq!(codex["status"], "failed");
    assert!(codex["try_lines"].is_array());
}

#[test]
fn detect_ping_flag_required_for_http_endpoint_probe() {
    let temp = repo_tempdir();
    write_provider_override(
        temp.path(),
        "local-test.toml",
        r#"
id = "http:test"
display_name = "HTTP Test"
kind = "http"
default_endpoint = "http://127.0.0.1:9"

[auth]
kind = "none"

[install_hint]
try_lines = ["start the local test server"]
"#,
    );

    let no_ping = deadreckon(temp.path())
        .args(["detect", "http:test", "--json"])
        .output()
        .expect("detect no ping");
    assert_success(&no_ping);
    let json: Value = serde_json::from_slice(&no_ping.stdout).expect("json no ping");
    let provider = &json["providers"][0];
    assert_eq!(provider["status"], "ok");
    assert_eq!(provider["version"], "ping skipped");

    let ping = deadreckon(temp.path())
        .args(["detect", "http:test", "--json", "--ping"])
        .output()
        .expect("detect ping");
    assert_success(&ping);
    let json: Value = serde_json::from_slice(&ping.stdout).expect("json ping");
    let provider = &json["providers"][0];
    assert_eq!(provider["status"], "failed");
    assert_eq!(provider["error_kind"], "endpoint_unreachable");
}

fn repo_tempdir() -> TempDir {
    let root = Path::new("/Users/gdc/deadreckon/.test-tmp");
    fs::create_dir_all(root).expect("test tmp root");
    let temp = TempDir::new_in(root).expect("tempdir");
    fs::create_dir_all(temp.path().join("empty-bin")).expect("empty bin");
    temp
}

fn deadreckon(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_deadreckon"));
    command.env("DEADRECKON_HOME", home).env("NO_COLOR", "1");
    command
}

fn write_provider_override(home: &Path, name: &str, body: &str) {
    let dir = home.join("providers.d");
    fs::create_dir_all(&dir).expect("providers.d");
    fs::write(dir.join(name), body).expect("write provider override");
}

fn write_fake_binary(dir: &Path, name: &str, version: &str) {
    fs::create_dir_all(dir).expect("bin dir");
    let path = dir.join(name);
    fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{}'\n",
            version.replace('\'', "'\\''")
        ),
    )
    .expect("fake binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod");
    }
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
