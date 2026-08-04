use std::collections::BTreeMap;
use std::path::Path;

use crate::cli_claude_code::CliClaudeCodeProvider;
use crate::cli_codex::CliCodexProvider;
use crate::cli_generic::GenericCliProvider;
use crate::codex_app_server::CliCodexServerProvider;
use crate::config::{
    CatalogEntrySource, apply_catalog_to_provider_entry, kind_from_name, merge_provider_entry,
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
    context_window_sources: BTreeMap<String, Option<ModelContextWindowSource>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelContextWindowSource {
    Catalog,
    Seam,
}

impl ModelContextWindowSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Catalog => "catalog",
            Self::Seam => "seam",
        }
    }
}

impl ProviderRouter {
    pub fn smoke() -> Self {
        Self {
            routes: vec![Box::new(ScriptedSmokeProvider::new())],
            context_windows: BTreeMap::new(),
            context_window_sources: BTreeMap::new(),
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
        let mut context_window_sources = BTreeMap::new();
        for name in route_names {
            let Some(mut entry) = providers.remove(&name) else {
                return Err(ProviderError::InvalidConfig(format!(
                    "unknown provider route {name}"
                )));
            };
            if let Some(model) = override_model {
                entry.model = Some(model.to_string());
            }
            let catalog_entry =
                apply_catalog_to_provider_entry(&name, &mut entry, registry, catalog_override);
            let context_window = catalog_entry.and_then(|entry| entry.context_window);
            let context_window_source = catalog_entry.and_then(|entry| {
                entry.context_window.map(|_| match entry.source {
                    CatalogEntrySource::Catalog => ModelContextWindowSource::Catalog,
                    CatalogEntrySource::Seam => ModelContextWindowSource::Seam,
                })
            });
            let kind = entry.kind.unwrap_or_else(|| kind_from_name(&name));
            entry.kind = Some(kind.clone());
            routes.push(build_provider(name.clone(), kind, entry, registry)?);
            context_windows.insert(name.clone(), context_window);
            context_window_sources.insert(name, context_window_source);
        }

        Ok(Self {
            routes,
            context_windows,
            context_window_sources,
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

    pub fn context_window_for_route_with_source(
        &self,
        provider_name: Option<&str>,
    ) -> Option<(u32, ModelContextWindowSource)> {
        let route_name = provider_name
            .and_then(|name| self.routes.iter().find(|route| route.name() == name))
            .or_else(|| self.routes.first())
            .map(|route| route.name())?;
        let window = self.context_windows.get(route_name).copied().flatten()?;
        let source = self
            .context_window_sources
            .get(route_name)
            .copied()
            .flatten()?;
        Some((window, source))
    }

    pub async fn complete(&self, request: &ProviderRequest) -> Result<ProviderResponse> {
        let mut failures = Vec::new();
        let mut attempted = 0_usize;
        let mut last_error = None;
        for route in &self.routes {
            if !route.has_credential() {
                failures.push(format!("{}: missing credential", route.name()));
                continue;
            }
            attempted += 1;
            match route.complete(request).await {
                Ok(response) => return Ok(response),
                Err(err) => {
                    if err.stops_routing() {
                        return Err(err);
                    }
                    failures.push(format!("{}: {err}", route.name()));
                    last_error = Some(err);
                }
            }
        }
        // A single attempted route surfaces its typed error so callers keep
        // the retryability signal (429/5xx/transport) instead of an opaque
        // NoRoute wrapper; multiple attempts mean fallthrough already
        // happened and the aggregate is final.
        if attempted == 1
            && let Some(err) = last_error
        {
            return Err(err);
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
        ProviderKind::Generic(id) if id == "cli:codex-server" || name == "cli:codex-server" => {
            Ok(Box::new(CliCodexServerProvider::new(name, entry)))
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct CountingProvider {
        name: &'static str,
        calls: Arc<AtomicUsize>,
        terminal: bool,
    }

    impl Provider for CountingProvider {
        fn name(&self) -> &str {
            self.name
        }

        fn kind(&self) -> ProviderKind {
            ProviderKind::ScriptedSmoke
        }

        fn model(&self) -> &str {
            "test"
        }

        fn has_credential(&self) -> bool {
            true
        }

        fn estimate_spend(&self, usage: ProviderUsage) -> SpendEstimate {
            SpendEstimate {
                provider: self.name.to_string(),
                model: "test".to_string(),
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cost_usd: 0.0,
                subscription: false,
                wall_time_seconds: None,
            }
        }

        fn complete<'a>(&'a self, _request: &'a ProviderRequest) -> crate::ProviderFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if self.terminal {
                    Err(ProviderError::Cancelled {
                        provider: self.name.to_string(),
                        detail: "deadline cancelled the route".to_string(),
                    })
                } else {
                    Err(ProviderError::NoRoute("fallback attempted".to_string()))
                }
            })
        }
    }

    #[tokio::test]
    async fn cancellation_never_launches_fallback_route() {
        let first_calls = Arc::new(AtomicUsize::new(0));
        let fallback_calls = Arc::new(AtomicUsize::new(0));
        let router = ProviderRouter {
            routes: vec![
                Box::new(CountingProvider {
                    name: "first",
                    calls: first_calls.clone(),
                    terminal: true,
                }),
                Box::new(CountingProvider {
                    name: "fallback",
                    calls: fallback_calls.clone(),
                    terminal: false,
                }),
            ],
            context_windows: BTreeMap::new(),
            context_window_sources: BTreeMap::new(),
        };

        let error = router
            .complete(&ProviderRequest::default())
            .await
            .expect_err("cancelled route");

        assert!(matches!(error, ProviderError::Cancelled { .. }));
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fallback_calls.load(Ordering::SeqCst), 0);
    }
}
