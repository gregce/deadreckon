use std::fs;

use deadreckon_providers::registry::ProviderDescriptor;
use deadreckon_providers::registry::ProviderRegistry;
use deadreckon_providers::registry::parse_custom_command;
use tempfile::TempDir;

#[test]
fn registry_builtin_lists_six_existing_providers() {
    let registry = ProviderRegistry::builtin().expect("builtin registry");
    for id in [
        "anthropic",
        "openai",
        "openai-compatible",
        "smoke",
        "cli:claude-code",
        "cli:codex",
    ] {
        assert!(registry.get(id).is_some(), "{id} missing from registry");
    }
}

#[test]
fn descriptor_toml_round_trips_serde() {
    let registry = ProviderRegistry::builtin().expect("builtin registry");
    let descriptor = registry.get("anthropic").expect("anthropic descriptor");
    let encoded = toml::to_string(descriptor).expect("serialize descriptor");
    let decoded: ProviderDescriptor = toml::from_str(&encoded).expect("deserialize descriptor");
    assert_eq!(descriptor, &decoded);
}

#[test]
fn provider_overrides_d_files_load_in_lexical_order() {
    let temp = TempDir::new().expect("tempdir");
    let dir = temp.path().join("providers.d");
    fs::create_dir_all(&dir).expect("providers.d");
    fs::write(
        dir.join("01-codex.toml"),
        r#"
id = "cli:codex"
default_binary = "/tmp/first-codex"
"#,
    )
    .expect("write first override");
    fs::write(
        dir.join("02-codex.toml"),
        r#"
id = "cli:codex"
default_binary = "/tmp/second-codex"
"#,
    )
    .expect("write second override");

    let registry = ProviderRegistry::with_overrides(temp.path()).expect("registry");
    let descriptor = registry.get("cli:codex").expect("codex descriptor");
    assert_eq!(
        descriptor.default_binary.as_deref(),
        Some("/tmp/second-codex")
    );
}

#[test]
fn override_file_can_extend_built_in_default_binary() {
    let temp = TempDir::new().expect("tempdir");
    let dir = temp.path().join("providers.d");
    fs::create_dir_all(&dir).expect("providers.d");
    fs::write(
        dir.join("codex.toml"),
        r#"
id = "cli:codex"
default_binary = "/opt/deadreckon/codex"
"#,
    )
    .expect("write override");

    let registry = ProviderRegistry::with_overrides(temp.path()).expect("registry");
    let descriptor = registry.get("cli:codex").expect("codex descriptor");
    assert_eq!(
        descriptor.default_binary.as_deref(),
        Some("/opt/deadreckon/codex")
    );
    assert_eq!(
        descriptor.exec_template.model_arg.as_deref(),
        Some("--model")
    );
}

#[test]
fn override_file_can_register_brand_new_id() {
    let temp = TempDir::new().expect("tempdir");
    let dir = temp.path().join("providers.d");
    fs::create_dir_all(&dir).expect("providers.d");
    fs::write(
        dir.join("local-test.toml"),
        r#"
id = "cli:local-test"
display_name = "Local Test CLI"
kind = "cli"
default_binary = "local-test"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["run", "{prompt}"]

[install_hint]
url = "https://example.invalid/local-test"
try_lines = ["install local-test"]
"#,
    )
    .expect("write override");

    let registry = ProviderRegistry::with_overrides(temp.path()).expect("registry");
    let descriptor = registry
        .get("cli:local-test")
        .expect("new descriptor registered");
    assert_eq!(descriptor.display_name, "Local Test CLI");
    assert_eq!(descriptor.exec_template.args_template, ["run", "{prompt}"]);
}

#[test]
fn parse_custom_command_handles_quoted_paths() {
    let (binary, args) =
        parse_custom_command(r#""/Applications/My Tools/codex" --config "a b.toml""#)
            .expect("parse command");
    assert_eq!(binary, "/Applications/My Tools/codex");
    assert_eq!(args, ["--config", "a b.toml"]);
}

#[test]
fn parse_custom_command_handles_escaped_chars() {
    let (binary, args) =
        parse_custom_command(r#"claude --msg "It\'s \"working\"""#).expect("parse command");
    assert_eq!(binary, "claude");
    assert_eq!(args, ["--msg", "It's \"working\""]);
}
