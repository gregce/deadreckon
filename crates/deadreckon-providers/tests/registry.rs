use std::fs;

use deadreckon_providers::ProviderKind;
use deadreckon_providers::registry::ContractDialect;
use deadreckon_providers::registry::IngestCwdMatch;
use deadreckon_providers::registry::IngestStorage;
use deadreckon_providers::registry::ProviderDescriptor;
use deadreckon_providers::registry::ProviderRegistry;
use deadreckon_providers::registry::parse_custom_command;
use deadreckon_providers::registry::parse_descriptor;
use tempfile::TempDir;

#[test]
fn registry_builtin_lists_cli_ingest_providers() {
    let registry = ProviderRegistry::builtin().expect("builtin registry");
    for id in [
        "anthropic",
        "openai",
        "openai-compatible",
        "smoke",
        "cli:claude-code",
        "cli:codex",
        "cli:gemini",
        "cli:opencode",
        "cli:copilot",
        "cli:pi",
    ] {
        assert!(registry.get(id).is_some(), "{id} missing from registry");
    }
}

#[test]
fn registry_builtin_lists_copilot_and_pi() {
    let registry = ProviderRegistry::builtin().expect("builtin registry");

    assert!(registry.get("cli:copilot").is_some());
    assert!(registry.get("cli:pi").is_some());
}

#[test]
fn gemini_contract_or_documented_gap() {
    let registry = ProviderRegistry::builtin().expect("builtin registry");
    let gemini = registry.get("cli:gemini").expect("gemini descriptor");
    let source = include_str!("../descriptors/cli-gemini.toml");

    assert!(gemini.contract.is_none());
    assert_eq!(gemini.exec_template.args_template, ["-p", "{prompt}"]);
    assert!(source.contains("Gemini CLI 0.42.0"));
    assert!(source.contains("IneligibleTierError/UNSUPPORTED_CLIENT"));
    assert!(source.contains("before any structured event is emitted"));
}

#[test]
fn opencode_contract_or_documented_gap() {
    let registry = ProviderRegistry::builtin().expect("builtin registry");
    let opencode = registry.get("cli:opencode").expect("opencode descriptor");
    let source = include_str!("../descriptors/cli-opencode.toml");
    let events = include_str!("fixtures/pennant/opencode-structured-gap.jsonl")
        .lines()
        .map(serde_json::from_str::<serde_json::Value>)
        .collect::<Result<Vec<_>, _>>()
        .expect("OpenCode gap fixture");

    assert!(opencode.contract.is_none());
    assert_eq!(opencode.exec_template.args_template, ["run", "{prompt}"]);
    assert!(source.contains("OpenCode CLI 0.15.5"));
    assert!(source.contains("text(answer), error, then text(null)"));
    assert!(source.contains("richer event mirror is a V1 escalation"));
    assert_eq!(events[1]["part"]["text"], "OPENCODE_FIXTURE_OK");
    assert_eq!(events[2]["type"], "error");
    assert!(events[3]["part"]["text"].is_null());
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
fn descriptor_ingest_round_trips_for_codex_and_claude() {
    let registry = ProviderRegistry::builtin().expect("builtin registry");
    let codex = registry.get("cli:codex").expect("codex descriptor");
    let codex_ingest = codex.ingest.as_ref().expect("codex ingest");
    assert_eq!(codex_ingest.id_prefix.as_deref(), Some("codex:"));
    assert_eq!(codex_ingest.env_var.as_deref(), Some("CODEX_SESSIONS_DIR"));
    assert_eq!(codex_ingest.schema, "codex-cli");
    assert_eq!(codex_ingest.cwd_match, IngestCwdMatch::SessionMeta);
    assert_eq!(codex_ingest.storage, Some(IngestStorage::Jsonl));

    let claude = registry
        .get("cli:claude-code")
        .expect("claude-code descriptor");
    let claude_ingest = claude.ingest.as_ref().expect("claude ingest");
    assert_eq!(
        claude_ingest.env_var.as_deref(),
        Some("CLAUDE_PROJECTS_DIR")
    );
    assert_eq!(claude_ingest.schema, "claude-code");
    assert_eq!(claude_ingest.cwd_match, IngestCwdMatch::ClaudeProjectDir);
    assert_eq!(claude_ingest.storage, Some(IngestStorage::Jsonl));

    let encoded = toml::to_string(codex).expect("serialize codex descriptor");
    let decoded: ProviderDescriptor = toml::from_str(&encoded).expect("decode codex descriptor");
    assert_eq!(decoded.ingest, codex.ingest);
}

#[test]
fn descriptor_ingest_round_trips_for_copilot_and_pi() {
    let registry = ProviderRegistry::builtin().expect("builtin registry");
    let copilot = registry.get("cli:copilot").expect("copilot descriptor");
    let copilot_ingest = copilot.ingest.as_ref().expect("copilot ingest");
    assert_eq!(copilot.default_binary.as_deref(), Some("copilot"));
    assert_eq!(copilot_ingest.id_prefix.as_deref(), Some("copilot:"));
    assert_eq!(copilot_ingest.env_var.as_deref(), Some("COPILOT_DIR"));
    assert_eq!(copilot_ingest.schema, "copilot-cli");
    assert_eq!(copilot_ingest.cwd_match, IngestCwdMatch::JsonPointer);
    assert_eq!(
        copilot_ingest.cwd_match_path.as_deref(),
        Some("data.context.cwd")
    );
    assert_eq!(copilot_ingest.storage, Some(IngestStorage::Jsonl));
    assert!(copilot_ingest.live_contract);

    let pi = registry.get("cli:pi").expect("pi descriptor");
    let pi_ingest = pi.ingest.as_ref().expect("pi ingest");
    assert_eq!(pi.default_binary.as_deref(), Some("pi"));
    assert_eq!(pi_ingest.id_prefix.as_deref(), Some("pi:"));
    assert_eq!(
        pi_ingest.env_var.as_deref(),
        Some("PI_CODING_AGENT_SESSION_DIR")
    );
    assert_eq!(pi_ingest.schema, "pi");
    assert_eq!(pi_ingest.cwd_match, IngestCwdMatch::TopLevel);
    assert_eq!(pi_ingest.storage, Some(IngestStorage::Jsonl));
    assert!(pi_ingest.live_contract);

    let encoded = toml::to_string(copilot).expect("serialize copilot descriptor");
    let decoded: ProviderDescriptor = toml::from_str(&encoded).expect("decode copilot descriptor");
    assert_eq!(decoded.ingest, copilot.ingest);
}

#[test]
fn descriptor_models_and_install_hints_cover_copilot_and_pi() {
    let registry = ProviderRegistry::builtin().expect("builtin registry");
    let copilot = registry.get("cli:copilot").expect("copilot descriptor");
    let copilot_version = copilot
        .version_probe
        .as_ref()
        .expect("copilot version probe");
    assert_eq!(copilot_version.args, ["--version"]);
    assert_eq!(
        copilot_version.expect_substring.as_deref(),
        Some("GitHub Copilot CLI")
    );
    assert_eq!(copilot.exec_template.model_arg.as_deref(), Some("--model"));
    assert!(
        copilot
            .install_hint
            .try_lines
            .iter()
            .any(|line| line.contains("@github/copilot"))
    );
    assert!(
        copilot
            .model_catalog
            .iter()
            .any(|entry| entry.aliases.iter().any(|alias| alias == "copilot-default"))
    );

    let pi = registry.get("cli:pi").expect("pi descriptor");
    assert_eq!(pi.exec_template.model_arg.as_deref(), Some("--model"));
    assert!(
        pi.install_hint
            .try_lines
            .iter()
            .any(|line| line.contains("@earendil-works/pi-coding-agent"))
    );
    assert!(
        pi.model_catalog
            .iter()
            .any(|entry| entry.aliases.iter().any(|alias| alias == "pi-default"))
    );
}

#[test]
fn descriptor_without_ingest_still_loads() {
    let descriptor = parse_descriptor(
        r#"
id = "cli:minimal"
display_name = "Minimal CLI"
kind = "cli"
default_binary = "minimal"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["run", "{prompt}"]
"#,
        "test:minimal",
    )
    .expect("parse descriptor without ingest");

    assert!(descriptor.ingest.is_none());
}

#[test]
fn contract_section_parses_full_and_minimal_forms() {
    let full = parse_descriptor(
        r#"
id = "cli:contract-full"
display_name = "Contract Full"
kind = "cli"
default_binary = "contract-full"

[exec_template]
args_template = ["{prompt}"]

[contract]
stream_args = ["--json"]
dialect = "json-lines"
conversation_id_path = "/session_id"
usage_input_path = "/usage/input"
usage_output_path = "/usage/output"
cost_path = "/cost"
answer_path = "/answer"
error_flag_path = "/is_error"
error_message_path = "/error/message"
flight_event_paths = ["/type"]
resume_args = ["--session", "{conversation_id}"]
probe_substring = "--json"
"#,
        "test:contract-full",
    )
    .expect("full contract");
    let contract = full.contract.expect("contract");
    assert_eq!(contract.dialect, ContractDialect::JsonLines);
    assert_eq!(contract.answer_path.as_deref(), Some("/answer"));
    assert_eq!(contract.resume_args, ["--session", "{conversation_id}"]);

    let minimal = parse_descriptor(
        r#"
id = "cli:contract-minimal"
display_name = "Contract Minimal"
kind = "cli"
default_binary = "contract-minimal"

[exec_template]
args_template = ["{prompt}"]

[contract]
stream_args = ["--json"]
"#,
        "test:contract-minimal",
    )
    .expect("minimal contract");
    assert_eq!(
        minimal.contract.expect("contract").dialect,
        ContractDialect::JsonLines
    );
}

#[test]
fn malformed_contract_warns_and_provider_stays_usable() {
    let descriptor = parse_descriptor(
        r#"
id = "cli:contract-bad"
display_name = "Contract Bad"
kind = "cli"
default_binary = "contract-bad"

[exec_template]
args_template = ["run", "{prompt}"]

[contract]
stream_args = []
answer_path = "/answer"
"#,
        "test:contract-bad",
    )
    .expect("provider remains usable");
    assert!(descriptor.contract.is_none());
    assert_eq!(descriptor.exec_template.args_template, ["run", "{prompt}"]);
    assert!(descriptor.warnings[0].contains("field stream_args"));
    assert!(descriptor.warnings[0].contains("deadreckon providers check cli:contract-bad"));
}

#[test]
fn document_dialect_rejects_flight_selectors() {
    let descriptor = parse_descriptor(
        r#"
id = "cli:contract-document"
display_name = "Contract Document"
kind = "cli"
default_binary = "contract-document"

[exec_template]
args_template = ["{prompt}"]

[contract]
stream_args = ["--json"]
dialect = "json-document"
flight_event_paths = ["/type"]
"#,
        "test:contract-document",
    )
    .expect("provider remains usable");
    assert!(descriptor.contract.is_none());
    assert!(descriptor.warnings[0].contains("field flight_event_paths"));
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
fn provider_override_can_replace_ingest_roots_without_losing_exec_template() {
    let temp = TempDir::new().expect("tempdir");
    let dir = temp.path().join("providers.d");
    fs::create_dir_all(&dir).expect("providers.d");
    fs::write(
        dir.join("codex.toml"),
        r#"
id = "cli:codex"

[ingest]
default_dirs = ["~/custom-codex/sessions"]
schema = "codex-cli"
cwd_match = "session-meta"
storage = "jsonl"
"#,
    )
    .expect("write override");

    let registry = ProviderRegistry::with_overrides(temp.path()).expect("registry");
    let descriptor = registry.get("cli:codex").expect("codex descriptor");
    let ingest = descriptor.ingest.as_ref().expect("codex ingest");
    assert_eq!(
        ingest.default_dirs,
        [std::path::PathBuf::from("~/custom-codex/sessions")]
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

#[test]
fn provider_kind_generic_variant_round_trips_serde() {
    let value = ProviderKind::Generic("cli:cursor-agent".to_string());
    let encoded = serde_json::to_string(&value).expect("serialize generic kind");
    assert_eq!(encoded, "\"cli:cursor-agent\"");
    let decoded: ProviderKind = serde_json::from_str(&encoded).expect("deserialize generic kind");
    assert_eq!(decoded, value);
}
