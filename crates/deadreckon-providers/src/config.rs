use std::collections::BTreeMap;
use std::env;
use std::path::Path;

use crate::registry::{
    DescriptorKind, ModelCatalogOverride, ModelEntry, ProviderDescriptor, ProviderRegistry,
};
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

#[cfg(test)]
pub(crate) fn builtin_entries() -> Result<BTreeMap<String, ProviderEntry>> {
    Ok(provider_entries_from_registry(&ProviderRegistry::builtin()?))
}

pub(crate) fn provider_entries_from_registry(
    registry: &ProviderRegistry,
) -> BTreeMap<String, ProviderEntry> {
    registry
        .iter()
        .map(|descriptor| {
            (
                descriptor.id.clone(),
                provider_entry_from_descriptor(descriptor),
            )
        })
        .collect()
}

pub(crate) fn provider_entry_from_descriptor(descriptor: &ProviderDescriptor) -> ProviderEntry {
    let default_model = if descriptor.kind == DescriptorKind::Cli {
        None
    } else if descriptor.id == "openai-compatible" {
        env::var("OPENAI_COMPATIBLE_MODEL")
            .ok()
            .or_else(|| descriptor.default_model.clone())
    } else {
        descriptor.default_model.clone()
    };
    let default_endpoint = if descriptor.id == "openai-compatible" {
        env::var("OPENAI_COMPATIBLE_BASE_URL")
            .ok()
            .or_else(|| descriptor.default_endpoint.clone())
    } else {
        descriptor.default_endpoint.clone()
    };
    let default_model_pricing = default_model
        .as_deref()
        .and_then(|model| {
            descriptor
                .model_catalog
                .iter()
                .find(|entry| entry.id == model)
        })
        .or_else(|| descriptor.model_catalog.first());
    ProviderEntry {
        kind: Some(kind_from_name(&descriptor.id)),
        api_key: None,
        api_key_env: descriptor.auth.env_var.clone(),
        base_url: default_endpoint,
        model: default_model,
        input_cost_per_million: default_model_pricing.and_then(|entry| entry.input_per_million),
        output_cost_per_million: default_model_pricing.and_then(|entry| entry.output_per_million),
        binary: descriptor.default_binary.clone(),
        extra_args: Vec::new(),
    }
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

pub(crate) fn apply_catalog_to_provider_entry(
    provider_name: &str,
    entry: &mut ProviderEntry,
    registry: &ProviderRegistry,
    catalog_override: Option<&ModelCatalogOverride>,
) -> Option<u32> {
    let model = entry.model.as_deref()?;
    let descriptor_entry = descriptor_for_entry(provider_name, entry, registry)
        .and_then(|descriptor| model_entry_for_model(&descriptor.model_catalog, model));
    let selected = catalog_override
        .and_then(|catalog| catalog.entry_for_model(model))
        .or(descriptor_entry);
    if let Some(model_entry) = selected {
        if let Some(input) = model_entry.input_per_million {
            entry.input_cost_per_million = Some(input);
        }
        if let Some(output) = model_entry.output_per_million {
            entry.output_cost_per_million = Some(output);
        }
        model_entry.context_window
    } else {
        None
    }
}

fn descriptor_for_entry<'a>(
    provider_name: &str,
    entry: &ProviderEntry,
    registry: &'a ProviderRegistry,
) -> Option<&'a ProviderDescriptor> {
    let descriptor_id = match entry.kind.as_ref() {
        Some(ProviderKind::Anthropic) => Some("anthropic"),
        Some(ProviderKind::OpenAi) => Some("openai"),
        Some(ProviderKind::OpenAiCompatible) => Some("openai-compatible"),
        Some(ProviderKind::CliClaudeCode) => Some("cli:claude-code"),
        Some(ProviderKind::CliCodex) => Some("cli:codex"),
        Some(ProviderKind::ScriptedSmoke) => Some("smoke"),
        Some(ProviderKind::Generic(id)) => Some(id.as_str()),
        None => None,
    };
    descriptor_id
        .and_then(|id| registry.get(id))
        .or_else(|| registry.get(provider_name))
}

fn model_entry_for_model<'a>(catalog: &'a [ModelEntry], model: &str) -> Option<&'a ModelEntry> {
    catalog
        .iter()
        .find(|entry| entry.id == model || entry.aliases.iter().any(|alias| alias == model))
}

pub(crate) fn kind_from_name(name: &str) -> ProviderKind {
    match name {
        "anthropic" => ProviderKind::Anthropic,
        "openai" => ProviderKind::OpenAi,
        "openai-compatible" | "openrouter" | "llama-cpp" => ProviderKind::OpenAiCompatible,
        "cli:claude-code" | "cli-claude-code" => ProviderKind::CliClaudeCode,
        "cli:codex" | "cli-codex" => ProviderKind::CliCodex,
        "smoke" => ProviderKind::ScriptedSmoke,
        _ => ProviderKind::Generic(name.to_string()),
    }
}
