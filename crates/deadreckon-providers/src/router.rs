use std::collections::BTreeMap;
use std::path::Path;

use crate::cli_claude_code::CliClaudeCodeProvider;
use crate::cli_codex::CliCodexProvider;
use crate::cli_generic::GenericCliProvider;
use crate::config::{
    apply_catalog_to_provider_entry, kind_from_name, merge_provider_entry,
    provider_entries_from_registry, read_config,
};
use crate::http::ProviderAdapter;
use crate::registry::{DescriptorKind, ModelCatalogOverride, ProviderRegistry};
use crate::smoke::ScriptedSmokeProvider;
use crate::{
    Provider, ProviderConfigFile, ProviderEntry, ProviderError, ProviderKind, ProviderRequest,
    ProviderResponse, ProviderRouteInfo, ProviderUsage, Result, SpendEstimate,
};

pub struct ProviderRouter {
    routes: Vec<Box<dyn Provider>>,
    context_windows: BTreeMap<String, Option<u32>>,
}

impl ProviderRouter {
    pub fn smoke() -> Self {
        Self {
            routes: vec![Box::new(ScriptedSmokeProvider::new())],
            context_windows: BTreeMap::new(),
        }
    }

    pub fn from_config_path(path: &Path, override_provider: Option<&str>) -> Result<Self> {
        Self::from_config_path_with_model(path, override_provider, None)
    }

    pub fn from_config_path_with_model(
        path: &Path,
        override_provider: Option<&str>,
        override_model: Option<&str>,
    ) -> Result<Self> {
        Self::from_config_path_with_model_and_catalog_override(
            path,
            override_provider,
            override_model,
            None,
        )
    }

    pub fn from_config_path_with_model_and_catalog_override(
        path: &Path,
        override_provider: Option<&str>,
        override_model: Option<&str>,
        catalog_override: Option<&ModelCatalogOverride>,
    ) -> Result<Self> {
        // REPORT.md: Provider Routing / BYOK keeps credentials in the user's
        // local config/env and tries the configured fallback chain.
        let config = read_config(path)?;
        let registry = path
            .parent()
            .map(ProviderRegistry::with_overrides)
            .transpose()?
            .unwrap_or(ProviderRegistry::builtin()?);
        Self::from_config_with_model_and_registry(
            config,
            override_provider,
            override_model,
            &registry,
            catalog_override,
        )
    }

    pub fn from_config(
        config: ProviderConfigFile,
        override_provider: Option<&str>,
    ) -> Result<Self> {
        Self::from_config_with_model(config, override_provider, None)
    }

    pub fn from_config_with_model(
        config: ProviderConfigFile,
        override_provider: Option<&str>,
        override_model: Option<&str>,
    ) -> Result<Self> {
        Self::from_config_with_model_and_catalog_override(
            config,
            override_provider,
            override_model,
            None,
        )
    }

    pub fn from_config_with_model_and_catalog_override(
        config: ProviderConfigFile,
        override_provider: Option<&str>,
        override_model: Option<&str>,
        catalog_override: Option<&ModelCatalogOverride>,
    ) -> Result<Self> {
        let registry = ProviderRegistry::builtin()?;
        Self::from_config_with_model_and_registry(
            config,
            override_provider,
            override_model,
            &registry,
            catalog_override,
        )
    }

    fn from_config_with_model_and_registry(
        config: ProviderConfigFile,
        override_provider: Option<&str>,
        override_model: Option<&str>,
        registry: &ProviderRegistry,
        catalog_override: Option<&ModelCatalogOverride>,
    ) -> Result<Self> {
        let mut providers = provider_entries_from_registry(registry);
        for (name, entry) in config.providers {
            if let Some(base) = providers.get_mut(&name) {
                merge_provider_entry(base, entry);
            } else {
                providers.insert(name, entry);
            }
        }

        let route_names = if let Some(provider) = override_provider {
            vec![provider.to_string()]
        } else {
            configured_route_names(config.default_provider, config.fallback)
        };

        let mut routes = Vec::new();
        let mut context_windows = BTreeMap::new();
        for name in route_names {
            let Some(mut entry) = providers.remove(&name) else {
                return Err(ProviderError::InvalidConfig(format!(
                    "unknown provider route {name}"
                )));
            };
            if let Some(model) = override_model {
                entry.model = Some(model.to_string());
            }
            let context_window =
                apply_catalog_to_provider_entry(&name, &mut entry, registry, catalog_override);
            let kind = entry.kind.unwrap_or_else(|| kind_from_name(&name));
            entry.kind = Some(kind.clone());
            routes.push(build_provider(name.clone(), kind, entry, registry)?);
            context_windows.insert(name, context_window);
        }

        Ok(Self {
            routes,
            context_windows,
        })
    }

    pub fn routes(&self) -> &[Box<dyn Provider>] {
        &self.routes
    }

    pub fn route_info(&self) -> Vec<ProviderRouteInfo> {
        self.routes
            .iter()
            .map(|route| ProviderRouteInfo {
                name: route.name().to_string(),
                kind: route.kind(),
                model: route.model().to_string(),
                has_credential: route.has_credential(),
            })
            .collect()
    }

    pub fn selected_route_info(&self) -> Option<ProviderRouteInfo> {
        let routes = self.route_info();
        routes
            .iter()
            .find(|route| route.has_credential)
            .cloned()
            .or_else(|| routes.into_iter().next())
    }

    pub fn context_window_for_route(&self, provider_name: Option<&str>) -> Option<u32> {
        let route_name = provider_name
            .and_then(|name| self.routes.iter().find(|route| route.name() == name))
            .or_else(|| self.routes.first())
            .map(|route| route.name())?;
        self.context_windows.get(route_name).copied().flatten()
    }

    pub async fn complete(&self, request: &ProviderRequest) -> Result<ProviderResponse> {
        let mut failures = Vec::new();
        for route in &self.routes {
            if !route.has_credential() {
                failures.push(format!("{}: missing credential", route.name()));
                continue;
            }
            match route.complete(request).await {
                Ok(response) => return Ok(response),
                Err(err) => failures.push(format!("{}: {err}", route.name())),
            }
        }
        Err(ProviderError::NoRoute(failures.join("; ")))
    }

    pub fn estimate_for_route(
        &self,
        provider_name: Option<&str>,
        usage: ProviderUsage,
    ) -> Result<SpendEstimate> {
        let route = provider_name
            .and_then(|name| self.routes.iter().find(|route| route.name() == name))
            .or_else(|| self.routes.first())
            .ok_or_else(|| ProviderError::NoRoute("empty provider route".to_string()))?;
        Ok(route.estimate_spend(usage))
    }
}

fn configured_route_names(
    default_provider: Option<String>,
    fallback: Option<Vec<String>>,
) -> Vec<String> {
    let mut route_names = Vec::new();
    if let Some(default_provider) = default_provider {
        route_names.push(default_provider);
    }
    if let Some(fallback) = fallback {
        for provider in fallback {
            if !route_names.iter().any(|route| route == &provider) {
                route_names.push(provider);
            }
        }
    }
    if route_names.is_empty() {
        route_names.extend([
            "cli:claude-code".to_string(),
            "cli:codex".to_string(),
            "anthropic".to_string(),
            "openai".to_string(),
            "openai-compatible".to_string(),
        ]);
    }
    route_names
}

fn build_provider(
    name: String,
    kind: ProviderKind,
    entry: ProviderEntry,
    registry: &ProviderRegistry,
) -> Result<Box<dyn Provider>> {
    match kind {
        ProviderKind::CliClaudeCode => Ok(Box::new(CliClaudeCodeProvider::new(name, entry))),
        ProviderKind::CliCodex => Ok(Box::new(CliCodexProvider::new(name, entry))),
        ProviderKind::ScriptedSmoke => Ok(Box::new(ScriptedSmokeProvider::new())),
        ProviderKind::Anthropic | ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => {
            Ok(Box::new(ProviderAdapter::new(name, kind, entry)))
        }
        ProviderKind::Generic(id) => {
            let descriptor = registry
                .get(&id)
                .or_else(|| registry.get(&name))
                .ok_or_else(|| {
                    ProviderError::InvalidConfig(format!("generic provider {id} has no descriptor"))
                })?;
            if descriptor.kind != DescriptorKind::Cli {
                return Err(ProviderError::InvalidConfig(format!(
                    "generic provider {id} is not a cli descriptor"
                )));
            }
            Ok(Box::new(GenericCliProvider::new(
                name,
                entry,
                descriptor.clone(),
            )?))
        }
    }
}
