use std::collections::BTreeMap;
use std::env;
use std::path::Path;

use crate::{ProviderConfigFile, ProviderEntry, ProviderError, ProviderKind, Result};

pub const DEFAULT_CONFIG_PATH: &str = "/Users/gdc/.deadreckon/config.toml";

pub fn read_config(path: &Path) -> Result<ProviderConfigFile> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let mut config: ProviderConfigFile =
                toml::from_str(&raw).map_err(|source| ProviderError::Toml {
                    path: path.display().to_string(),
                    source,
                })?;
            if config.default_provider.is_none() {
                config.default_provider = defaults_provider(&raw);
            }
            Ok(config)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(ProviderConfigFile {
            default_provider: None,
            fallback: None,
            providers: BTreeMap::new(),
        }),
        Err(source) => Err(ProviderError::Io {
            path: path.display().to_string(),
            source,
        }),
    }
}

fn defaults_provider(raw: &str) -> Option<String> {
    let root: toml::Value = toml::from_str(raw).ok()?;
    root.as_table()?
        .get("defaults")?
        .as_table()?
        .get("provider")?
        .as_str()
        .map(ToString::to_string)
}

pub(crate) fn builtin_entries() -> BTreeMap<String, ProviderEntry> {
    BTreeMap::from([
        (
            "smoke".to_string(),
            ProviderEntry {
                kind: Some(ProviderKind::ScriptedSmoke),
                api_key: None,
                api_key_env: None,
                base_url: None,
                model: Some("local-scripted-smoke".to_string()),
                input_cost_per_million: Some(0.0),
                output_cost_per_million: Some(0.0),
                binary: None,
                extra_args: Vec::new(),
            },
        ),
        (
            "cli:claude-code".to_string(),
            ProviderEntry {
                kind: Some(ProviderKind::CliClaudeCode),
                api_key: None,
                api_key_env: None,
                base_url: None,
                model: None,
                input_cost_per_million: Some(0.0),
                output_cost_per_million: Some(0.0),
                binary: Some("claude".to_string()),
                extra_args: Vec::new(),
            },
        ),
        (
            "cli:codex".to_string(),
            ProviderEntry {
                kind: Some(ProviderKind::CliCodex),
                api_key: None,
                api_key_env: None,
                base_url: None,
                model: None,
                input_cost_per_million: Some(0.0),
                output_cost_per_million: Some(0.0),
                binary: Some("codex".to_string()),
                extra_args: Vec::new(),
            },
        ),
        (
            "anthropic".to_string(),
            ProviderEntry {
                kind: Some(ProviderKind::Anthropic),
                api_key: None,
                api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
                base_url: Some("https://api.anthropic.com".to_string()),
                model: Some("claude-sonnet-4-5".to_string()),
                input_cost_per_million: Some(3.0),
                output_cost_per_million: Some(15.0),
                binary: None,
                extra_args: Vec::new(),
            },
        ),
        (
            "openai".to_string(),
            ProviderEntry {
                kind: Some(ProviderKind::OpenAi),
                api_key: None,
                api_key_env: Some("OPENAI_API_KEY".to_string()),
                base_url: Some("https://api.openai.com/v1".to_string()),
                model: Some("gpt-5.1-codex".to_string()),
                input_cost_per_million: Some(1.25),
                output_cost_per_million: Some(10.0),
                binary: None,
                extra_args: Vec::new(),
            },
        ),
        (
            "openai-compatible".to_string(),
            ProviderEntry {
                kind: Some(ProviderKind::OpenAiCompatible),
                api_key: None,
                api_key_env: Some("OPENAI_COMPATIBLE_API_KEY".to_string()),
                base_url: env::var("OPENAI_COMPATIBLE_BASE_URL").ok(),
                model: env::var("OPENAI_COMPATIBLE_MODEL").ok(),
                input_cost_per_million: Some(0.0),
                output_cost_per_million: Some(0.0),
                binary: None,
                extra_args: Vec::new(),
            },
        ),
    ])
}

pub(crate) fn merge_provider_entry(base: &mut ProviderEntry, entry: ProviderEntry) {
    if entry.kind.is_some() {
        base.kind = entry.kind;
    }
    if entry.api_key.is_some() {
        base.api_key = entry.api_key;
    }
    if entry.api_key_env.is_some() {
        base.api_key_env = entry.api_key_env;
    }
    if entry.base_url.is_some() {
        base.base_url = entry.base_url;
    }
    if entry.model.is_some() {
        base.model = entry.model;
    }
    if entry.input_cost_per_million.is_some() {
        base.input_cost_per_million = entry.input_cost_per_million;
    }
    if entry.output_cost_per_million.is_some() {
        base.output_cost_per_million = entry.output_cost_per_million;
    }
    if entry.binary.is_some() {
        base.binary = entry.binary;
    }
    if !entry.extra_args.is_empty() {
        base.extra_args = entry.extra_args;
    }
}

pub(crate) fn kind_from_name(name: &str) -> Option<ProviderKind> {
    match name {
        "anthropic" => Some(ProviderKind::Anthropic),
        "openai" => Some(ProviderKind::OpenAi),
        "openai-compatible" | "openrouter" | "llama-cpp" => Some(ProviderKind::OpenAiCompatible),
        "cli:claude-code" | "cli-claude-code" => Some(ProviderKind::CliClaudeCode),
        "cli:codex" | "cli-codex" => Some(ProviderKind::CliCodex),
        "smoke" => Some(ProviderKind::ScriptedSmoke),
        _ => None,
    }
}
