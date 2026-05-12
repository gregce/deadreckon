use std::path::Path;

use crate::cli_claude_code::CliClaudeCodeProvider;
use crate::cli_codex::CliCodexProvider;
use crate::config::{builtin_entries, kind_from_name, merge_provider_entry, read_config};
use crate::http::ProviderAdapter;
use crate::smoke::ScriptedSmokeProvider;
use crate::{
    Provider, ProviderConfigFile, ProviderEntry, ProviderError, ProviderKind, ProviderRequest,
    ProviderResponse, ProviderRouteInfo, ProviderUsage, Result, SpendEstimate,
};

pub struct ProviderRouter {
    routes: Vec<Box<dyn Provider>>,
}

impl ProviderRouter {
    pub fn smoke() -> Self {
        Self {
            routes: vec![Box::new(ScriptedSmokeProvider::new())],
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
        // REPORT.md: Provider Routing / BYOK keeps credentials in the user's
        // local config/env and tries the configured fallback chain.
        let config = read_config(path)?;
        Self::from_config_with_model(config, override_provider, override_model)
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
        let mut providers = builtin_entries();
        for (name, entry) in config.providers {
            if let Some(base) = providers.get_mut(&name) {
                merge_provider_entry(base, entry);
            } else {
                providers.insert(name, entry);
            }
        }

        let route_names = if let Some(provider) = override_provider {
            vec![provider.to_string()]
        } else if let Some(fallback) = config.fallback {
            fallback
        } else if let Some(default_provider) = config.default_provider {
            vec![default_provider]
        } else {
            vec![
                "cli:claude-code".to_string(),
                "cli:codex".to_string(),
                "anthropic".to_string(),
                "openai".to_string(),
                "openai-compatible".to_string(),
            ]
        };

        let mut routes = Vec::new();
        for name in route_names {
            let Some(mut entry) = providers.remove(&name) else {
                return Err(ProviderError::InvalidConfig(format!(
                    "unknown provider route {name}"
                )));
            };
            if let Some(model) = override_model {
                entry.model = Some(model.to_string());
            }
            let kind = entry
                .kind
                .or_else(|| kind_from_name(&name))
                .ok_or_else(|| ProviderError::InvalidConfig(format!("missing kind for {name}")))?;
            entry.kind = Some(kind);
            routes.push(build_provider(name, kind, entry));
        }

        Ok(Self { routes })
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

fn build_provider(name: String, kind: ProviderKind, entry: ProviderEntry) -> Box<dyn Provider> {
    match kind {
        ProviderKind::CliClaudeCode => Box::new(CliClaudeCodeProvider::new(name, entry)),
        ProviderKind::CliCodex => Box::new(CliCodexProvider::new(name, entry)),
        ProviderKind::ScriptedSmoke => Box::new(ScriptedSmokeProvider::new()),
        ProviderKind::Anthropic | ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => {
            Box::new(ProviderAdapter::new(name, kind, entry))
        }
    }
}
