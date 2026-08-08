#![allow(clippy::expect_used)]

//! `deadreckon config` — the CONFIG machine surface (FULL-DRIVE gap B2).
//!
//! `show --json` is the complete effective configuration in one envelope,
//! `set`/`unset --json` are validated round-trips, and API keys obey the
//! secret discipline: they enter through stdin (`set-key`), never argv, and
//! a stored key never appears in any output byte of any config surface.

use std::fs;
use std::io::Write as _;
use std::process::{Output, Stdio};

use deadreckon_core::DeadreckonPaths;
use serde_json::Value;

mod common;

use common::{assert_success, deadreckon, repo_tempdir, stderr, stdout};

/// A recognizable secret: the redaction tests assert these exact bytes
/// never surface anywhere.
const SECRET: &str = "sk-live-REDACTION-PROOF-BYTES";

fn write_config(paths: &DeadreckonPaths, contents: &str) {
    fs::create_dir_all(paths.home()).expect("home");
    fs::write(paths.config_path(), contents).expect("config");
}

fn combined(output: &Output) -> String {
    format!("{}{}", stdout(output), stderr(output))
}

fn parse_stdout(output: &Output) -> Value {
    serde_json::from_str(&stdout(output)).expect("one JSON object on stdout")
}

fn configured_home(paths: &DeadreckonPaths) {
    write_config(
        paths,
        &format!(
            r#"default_provider = "anthropic"
fallback = ["cli:codex"]

[defaults]
provider = "anthropic"
max_spend = 25.0

[providers.anthropic]
api_key = "{SECRET}"
model = "claude-test-model"

[custom_section]
knob = 3
"#
        ),
    );
}

#[test]
fn config_show_json_is_the_complete_effective_configuration_with_secrets_redacted() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    configured_home(&paths);

    let output = deadreckon(&paths)
        .args(["config", "show", "--json"])
        .output()
        .expect("config show");

    assert_success(&output);
    // The redaction rule is byte-level: the stored key appears nowhere in
    // any output stream.
    assert!(!combined(&output).contains(SECRET), "{}", combined(&output));

    let value = parse_stdout(&output);
    assert_eq!(value["kind"], "config");
    assert_eq!(value["id"], "show");
    assert_eq!(value["status"], "completed");
    assert_eq!(value["action"], "show");
    assert_eq!(
        value["config_path"],
        paths.config_path().display().to_string().as_str()
    );
    assert_eq!(value["config_exists"], true);

    // Set-vs-default provenance for every registered setting.
    let settings = value["settings"].as_object().expect("settings object");
    assert_eq!(settings["defaults.max_spend"]["value"], 25.0);
    assert_eq!(settings["defaults.max_spend"]["source"], "set");
    assert_eq!(settings["defaults.provider"]["value"], "anthropic");
    assert_eq!(settings["defaults.provider"]["source"], "set");
    assert_eq!(settings["defaults.sandbox"]["value"], "auto");
    assert_eq!(settings["defaults.sandbox"]["source"], "default");
    assert_eq!(settings["defaults.cli_max_wall_seconds"]["value"], 36_000.0);
    assert_eq!(
        settings["defaults.cli_max_wall_seconds"]["source"],
        "default"
    );

    // Provider entries carry every non-secret field; the key slot reads only
    // the marker.
    assert_eq!(value["providers"]["anthropic"]["api_key"], "configured");
    assert_eq!(
        value["providers"]["anthropic"]["model"],
        "claude-test-model"
    );
    assert_eq!(value["fallback"][0], "cli:codex");

    // The raw document rides along complete (custom tables included) and
    // redacted.
    assert_eq!(value["file"]["custom_section"]["knob"], 3);
    assert_eq!(
        value["file"]["providers"]["anthropic"]["api_key"],
        "configured"
    );
    assert_eq!(value["file"]["default_provider"], "anthropic");

    // The shared G1 scaffold from the one surface object.
    assert_eq!(value["verdict"]["kind"], "completed");
    assert!(value["primary_action"].is_string(), "{value}");
}

#[test]
fn config_show_prose_stays_operator_friendly_and_redacted() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));
    configured_home(&paths);

    let output = deadreckon(&paths)
        .args(["config", "show"])
        .output()
        .expect("config show prose");

    assert_success(&output);
    let out = stdout(&output);
    assert!(out.contains("completed config show"), "{out}");
    assert!(out.contains("Explanation"), "{out}");
    assert!(out.contains("providers.anthropic"), "{out}");
    assert!(out.contains("api_key configured"), "{out}");
    assert!(!combined(&output).contains(SECRET), "{}", combined(&output));
}

#[test]
fn config_set_and_unset_round_trip_with_envelopes() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let set = deadreckon(&paths)
        .args(["config", "set", "defaults.max_spend", "25", "--json"])
        .output()
        .expect("config set");
    assert_success(&set);
    let set_value = parse_stdout(&set);
    assert_eq!(set_value["kind"], "config");
    assert_eq!(set_value["id"], "defaults.max_spend");
    assert_eq!(set_value["action"], "set");
    assert_eq!(set_value["key"], "defaults.max_spend");
    assert_eq!(set_value["value"], 25);
    assert_eq!(set_value["previous"], Value::Null);

    let get = deadreckon(&paths)
        .args(["config", "get", "defaults.max_spend"])
        .output()
        .expect("config get");
    assert_success(&get);
    assert_eq!(stdout(&get).trim(), "25");

    let unset = deadreckon(&paths)
        .args(["config", "unset", "defaults.max_spend", "--json"])
        .output()
        .expect("config unset");
    assert_success(&unset);
    let unset_value = parse_stdout(&unset);
    assert_eq!(unset_value["kind"], "config");
    assert_eq!(unset_value["action"], "unset");
    assert_eq!(unset_value["removed"], true);
    assert_eq!(unset_value["status"], "completed");

    // Unsetting an absent key is an honest no-op, not an error.
    let again = deadreckon(&paths)
        .args(["config", "unset", "defaults.max_spend", "--json"])
        .output()
        .expect("config unset again");
    assert_success(&again);
    let again_value = parse_stdout(&again);
    assert_eq!(again_value["removed"], false);
    assert_eq!(again_value["status"], "no-op");

    // And show reports the key back at its built-in default.
    let show = deadreckon(&paths)
        .args(["config", "show", "--json"])
        .output()
        .expect("config show");
    assert_success(&show);
    let show_value = parse_stdout(&show);
    assert_eq!(
        show_value["settings"]["defaults.max_spend"]["source"],
        "default"
    );
    assert_eq!(show_value["settings"]["defaults.max_spend"]["value"], 10.0);
}

#[test]
fn config_set_refuses_unknown_and_secret_keys_with_typed_envelopes() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    // A typo inside a validated namespace refuses and lists the valid keys.
    let unknown = deadreckon(&paths)
        .args(["config", "set", "defaults.max_spendd", "5", "--json"])
        .output()
        .expect("config set unknown");
    assert!(!unknown.status.success());
    let unknown_value = parse_stdout(&unknown);
    assert_eq!(unknown_value["kind"], "error");
    assert_eq!(unknown_value["verb"], "config");
    let message = unknown_value["message"].as_str().expect("message");
    assert!(message.contains("unknown config key"), "{message}");
    assert!(message.contains("defaults.max_spend"), "{message}");
    assert!(
        !unknown_value["try_lines"]
            .as_array()
            .expect("try")
            .is_empty(),
        "{unknown_value}"
    );

    // The secret slot refuses toward the stdin surface and never echoes the
    // attempted value.
    let secret = deadreckon(&paths)
        .args([
            "config",
            "set",
            "providers.anthropic.api_key",
            "sk-argv-attempt",
            "--json",
        ])
        .output()
        .expect("config set secret");
    assert!(!secret.status.success());
    let secret_value = parse_stdout(&secret);
    assert_eq!(secret_value["kind"], "error");
    assert!(
        secret_value["try_lines"]
            .as_array()
            .expect("try")
            .iter()
            .any(|line| line == "deadreckon config set-key anthropic"),
        "{secret_value}"
    );
    assert!(
        !combined(&secret).contains("sk-argv-attempt"),
        "{}",
        combined(&secret)
    );

    // Value shapes are typed per key.
    let not_a_number = deadreckon(&paths)
        .args(["config", "set", "defaults.max_spend", "nope", "--json"])
        .output()
        .expect("config set bad number");
    assert!(!not_a_number.status.success());
    let not_a_number_value = parse_stdout(&not_a_number);
    assert!(
        not_a_number_value["message"]
            .as_str()
            .expect("message")
            .contains("greater than zero"),
        "{not_a_number_value}"
    );

    let bad_backend = deadreckon(&paths)
        .args(["config", "set", "defaults.sandbox", "flying", "--json"])
        .output()
        .expect("config set bad backend");
    assert!(!bad_backend.status.success());
    assert!(
        parse_stdout(&bad_backend)["message"]
            .as_str()
            .expect("message")
            .contains("sandbox-exec"),
        "{}",
        stdout(&bad_backend)
    );
}

#[test]
fn config_set_key_reads_stdin_and_never_echoes_the_key() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    let mut child = deadreckon(&paths)
        .args(["config", "set-key", "anthropic", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn set-key");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(format!("{SECRET}\n").as_bytes())
        .expect("write key");
    let output = child.wait_with_output().expect("set-key output");

    assert_success(&output);
    assert!(!combined(&output).contains(SECRET), "{}", combined(&output));
    let value = parse_stdout(&output);
    assert_eq!(value["kind"], "config");
    assert_eq!(value["action"], "set-key");
    assert_eq!(value["provider"], "anthropic");
    assert_eq!(value["stored"], true);
    assert_eq!(value["keychain_or_file"], "file");

    // The key landed in the config file...
    let config = fs::read_to_string(paths.config_path()).expect("config");
    assert!(config.contains(SECRET), "{config}");

    // ...and still never surfaces through show.
    let show = deadreckon(&paths)
        .args(["config", "show", "--json"])
        .output()
        .expect("config show");
    assert_success(&show);
    assert!(!combined(&show).contains(SECRET), "{}", combined(&show));
    assert_eq!(
        parse_stdout(&show)["providers"]["anthropic"]["api_key"],
        "configured"
    );

    let unset = deadreckon(&paths)
        .args(["config", "unset-key", "anthropic", "--json"])
        .output()
        .expect("unset-key");
    assert_success(&unset);
    let unset_value = parse_stdout(&unset);
    assert_eq!(unset_value["action"], "unset-key");
    assert_eq!(unset_value["removed"], true);
    assert_eq!(unset_value["keychain_or_file"], "file");
    let config = fs::read_to_string(paths.config_path()).expect("config");
    assert!(!config.contains(SECRET), "{config}");

    // Removing again is an honest no-op.
    let again = deadreckon(&paths)
        .args(["config", "unset-key", "anthropic", "--json"])
        .output()
        .expect("unset-key again");
    assert_success(&again);
    let again_value = parse_stdout(&again);
    assert_eq!(again_value["removed"], false);
    assert_eq!(again_value["status"], "no-op");
}

#[test]
fn config_set_key_refuses_empty_stdin_and_keyless_routes() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    // No key on stdin is a typed refusal, not an empty write.
    let empty = deadreckon(&paths)
        .args(["config", "set-key", "anthropic", "--json"])
        .stdin(Stdio::null())
        .output()
        .expect("set-key empty stdin");
    assert!(!empty.status.success());
    let empty_value = parse_stdout(&empty);
    assert_eq!(empty_value["kind"], "error");
    assert!(
        empty_value["message"]
            .as_str()
            .expect("message")
            .contains("no API key arrived on stdin"),
        "{empty_value}"
    );

    // A subscription CLI route takes no API key.
    let mut child = deadreckon(&paths)
        .args(["config", "set-key", "cli:codex", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn set-key cli route");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"irrelevant\n")
        .expect("write");
    let cli_route = child.wait_with_output().expect("set-key cli output");
    assert!(!cli_route.status.success());
    assert!(
        parse_stdout(&cli_route)["message"]
            .as_str()
            .expect("message")
            .contains("CLI login"),
        "{}",
        stdout(&cli_route)
    );

    // An unknown route lists the API-key-capable routes.
    let unknown = deadreckon(&paths)
        .args(["config", "set-key", "nope-route", "--json"])
        .stdin(Stdio::null())
        .output()
        .expect("set-key unknown");
    assert!(!unknown.status.success());
    let unknown_value = parse_stdout(&unknown);
    let message = unknown_value["message"].as_str().expect("message");
    assert!(message.contains("unknown provider nope-route"), "{message}");
    assert!(message.contains("anthropic"), "{message}");
}

#[test]
fn config_free_form_keys_outside_validated_namespaces_still_round_trip() {
    let temp = repo_tempdir();
    let paths = DeadreckonPaths::from_home(temp.path().join("home"));

    // `[seams]`-style namespaces stay free-form TOML, exactly as before the
    // validated surface landed.
    let set = deadreckon(&paths)
        .args([
            "config",
            "set",
            "seams.policy.worker",
            "policy-cmd",
            "--json",
        ])
        .output()
        .expect("config set free-form");
    assert_success(&set);
    assert_eq!(parse_stdout(&set)["value"], "policy-cmd");

    let get = deadreckon(&paths)
        .args(["config", "get", "seams.policy.worker"])
        .output()
        .expect("config get free-form");
    assert_success(&get);
    assert_eq!(stdout(&get).trim(), "policy-cmd");
}
