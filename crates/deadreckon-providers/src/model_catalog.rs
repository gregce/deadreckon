use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::registry::{ModelEntry, ProviderDescriptor};

/// Where the selectable models for a provider came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelCatalogSource {
    /// Models reported by provider-owned local state maintained by its CLI.
    ProviderCache,
    /// Models declared by the built-in or user-overridden provider descriptor.
    Descriptor,
}

impl ModelCatalogSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::ProviderCache => "provider cache",
            Self::Descriptor => "provider descriptor",
        }
    }
}

/// A provider-scoped model catalog suitable for an interactive picker.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedModelCatalog {
    pub models: Vec<ModelEntry>,
    pub source: ModelCatalogSource,
}

/// Resolve models for one provider route.
///
/// Provider-owned discovery wins when it is available and parseable. The
/// route's descriptor remains the fail-safe catalog for every provider,
/// including user-defined provider descriptors. Discovery failures never
/// borrow models from another provider.
pub fn resolve_model_catalog(
    descriptor: &ProviderDescriptor,
    user_home: &Path,
) -> ResolvedModelCatalog {
    if matches!(descriptor.id.as_str(), "cli:codex" | "cli:codex-server")
        && let Some(models) = codex_cached_models(descriptor, user_home)
    {
        return ResolvedModelCatalog {
            models,
            source: ModelCatalogSource::ProviderCache,
        };
    }

    ResolvedModelCatalog {
        models: descriptor.model_catalog.clone(),
        source: ModelCatalogSource::Descriptor,
    }
}

#[derive(Debug, Deserialize)]
struct CodexModelsCache {
    #[serde(default)]
    models: Vec<CodexCachedModel>,
}

#[derive(Debug, Deserialize)]
struct CodexCachedModel {
    slug: String,
    #[serde(default)]
    context_window: Option<u32>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    priority: Option<u32>,
}

fn codex_cached_models(
    descriptor: &ProviderDescriptor,
    user_home: &Path,
) -> Option<Vec<ModelEntry>> {
    let raw = fs::read_to_string(user_home.join(".codex/models_cache.json")).ok()?;
    let mut cached = serde_json::from_str::<CodexModelsCache>(&raw).ok()?.models;
    cached.retain(|model| {
        !model.slug.trim().is_empty() && model.visibility.as_deref() != Some("hide")
    });
    cached.sort_by_key(|model| model.priority.unwrap_or(u32::MAX));
    if cached.is_empty() {
        return None;
    }

    let mut models = descriptor
        .model_catalog
        .iter()
        .filter(|entry| entry.id == "provider default")
        .cloned()
        .collect::<Vec<_>>();
    let mut seen = models
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<BTreeSet<_>>();
    for model in cached {
        if seen.insert(model.slug.clone()) {
            models.push(ModelEntry {
                id: model.slug,
                context_window: model.context_window,
                input_per_million: None,
                output_per_million: None,
                aliases: Vec::new(),
                recommended: false,
            });
        }
    }
    (!models.is_empty()).then_some(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ProviderRegistry;

    #[test]
    fn codex_uses_visible_provider_cache_models_in_priority_order() {
        let temp = tempfile::tempdir().expect("tempdir");
        let codex_home = temp.path().join(".codex");
        fs::create_dir_all(&codex_home).expect("codex dir");
        fs::write(
            codex_home.join("models_cache.json"),
            r#"{
              "models": [
                {"slug":"gpt-new-small","visibility":"list","priority":2,"context_window":1000},
                {"slug":"internal","visibility":"hide","priority":0,"context_window":1000},
                {"slug":"gpt-new-large","visibility":"list","priority":1,"context_window":2000}
              ]
            }"#,
        )
        .expect("cache");
        let registry = ProviderRegistry::builtin().expect("registry");
        let descriptor = registry.get("cli:codex").expect("codex");

        let catalog = resolve_model_catalog(descriptor, temp.path());

        assert_eq!(catalog.source, ModelCatalogSource::ProviderCache);
        assert_eq!(
            catalog
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["provider default", "gpt-new-large", "gpt-new-small"]
        );
    }

    #[test]
    fn every_other_provider_uses_its_own_descriptor_catalog() {
        let temp = tempfile::tempdir().expect("tempdir");
        let registry = ProviderRegistry::builtin().expect("registry");
        for descriptor in registry.iter().filter(|item| item.id != "cli:codex") {
            let catalog = resolve_model_catalog(descriptor, temp.path());
            assert_eq!(catalog.source, ModelCatalogSource::Descriptor);
            assert_eq!(
                catalog.models, descriptor.model_catalog,
                "{}",
                descriptor.id
            );
        }
    }

    #[test]
    fn malformed_codex_cache_falls_back_to_descriptor() {
        let temp = tempfile::tempdir().expect("tempdir");
        let codex_home = temp.path().join(".codex");
        fs::create_dir_all(&codex_home).expect("codex dir");
        fs::write(codex_home.join("models_cache.json"), "not json").expect("cache");
        let registry = ProviderRegistry::builtin().expect("registry");
        let descriptor = registry.get("cli:codex").expect("codex");

        let catalog = resolve_model_catalog(descriptor, temp.path());

        assert_eq!(catalog.source, ModelCatalogSource::Descriptor);
        assert_eq!(catalog.models, descriptor.model_catalog);
    }
}
