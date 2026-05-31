#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

//! Provider routing and adapters for deadreckon.

mod cli_common;
mod cli_generic;
mod config;
mod error;
mod http;
mod router;
mod smoke;
mod types;

pub mod cli_claude_code;
pub mod cli_codex;
pub mod registry;
pub mod taxonomy;

pub use config::{DEFAULT_CONFIG_PATH, read_config};
pub use error::{ProviderError, Result};
pub use http::ProviderAdapter;
pub use registry::{ModelCatalogOverride, ModelEntry};
pub use router::{ModelContextWindowSource, ProviderRouter};
pub use types::{
    Provider, ProviderConfigFile, ProviderEntry, ProviderFuture, ProviderKind, ProviderRequest,
    ProviderResponse, ProviderRouteInfo, ProviderUsage, SpendEstimate,
};

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::config::builtin_entries;
    use super::{
        ModelCatalogOverride, ModelEntry, Provider, ProviderAdapter, ProviderConfigFile,
        ProviderEntry, ProviderKind, ProviderRouter, ProviderUsage, read_config,
    };

    #[test]
    fn config_parses_fallback_routes() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("config.toml");
        fs::write(
            &path,
            r#"
fallback = ["openai-compatible", "anthropic"]

[providers.openai-compatible]
kind = "open-ai-compatible"
base_url = "http://127.0.0.1:8080/v1"
model = "local"
api_key = "test"
"#,
        )
        .expect("write config");

        let config = read_config(&path).expect("parse");
        let router = ProviderRouter::from_config(config, None).expect("router");
        assert_eq!(router.routes().len(), 2);
        assert_eq!(router.routes()[0].kind(), ProviderKind::OpenAiCompatible);
        assert!(router.routes()[0].has_credential());
    }

    #[test]
    fn partial_provider_entry_merges_with_builtin_defaults() {
        let router = ProviderRouter::from_config(
            ProviderConfigFile {
                default_provider: None,
                fallback: Some(vec!["openai".to_string()]),
                providers: [(
                    "openai".to_string(),
                    ProviderEntry {
                        kind: None,
                        api_key: None,
                        api_key_env: None,
                        base_url: None,
                        model: Some("custom-openai-model".to_string()),
                        input_cost_per_million: None,
                        output_cost_per_million: None,
                        binary: None,
                        extra_args: Vec::new(),
                    },
                )]
                .into_iter()
                .collect(),
            },
            None,
        )
        .expect("router");

        let route = router.selected_route_info().expect("route");
        assert_eq!(route.name, "openai");
        assert_eq!(route.model, "custom-openai-model");
        let spend = router
            .estimate_for_route(
                Some("openai"),
                ProviderUsage {
                    input_tokens: 1_000_000,
                    output_tokens: 1_000_000,
                },
            )
            .expect("spend");
        assert_eq!(spend.cost_usd, 11.25);
    }

    #[test]
    fn migrated_cli_codex_uses_descriptor_default_binary() {
        let entries = builtin_entries().expect("descriptor builtins");
        let codex = entries.get("cli:codex").expect("codex provider entry");
        assert_eq!(codex.binary.as_deref(), Some("codex"));
        assert_eq!(codex.model.as_deref(), None);
        assert_eq!(codex.kind, Some(ProviderKind::CliCodex));
    }

    #[test]
    fn migrated_anthropic_uses_descriptor_default_endpoint() {
        let entries = builtin_entries().expect("descriptor builtins");
        let anthropic = entries.get("anthropic").expect("anthropic provider entry");
        assert_eq!(
            anthropic.base_url.as_deref(),
            Some("https://api.anthropic.com")
        );
        assert_eq!(anthropic.model.as_deref(), Some("claude-sonnet-4-5"));
        assert_eq!(anthropic.api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn existing_provider_router_tests_pass_post_migration() {
        let router = ProviderRouter::from_config(
            ProviderConfigFile {
                default_provider: None,
                fallback: None,
                providers: Default::default(),
            },
            None,
        )
        .expect("router");
        let names = router
            .route_info()
            .into_iter()
            .map(|route| route.name)
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "cli:claude-code",
                "cli:codex",
                "anthropic",
                "openai",
                "openai-compatible"
            ]
        );
    }

    #[test]
    fn default_provider_leads_fallback_routes() {
        let router = ProviderRouter::from_config(
            ProviderConfigFile {
                default_provider: Some("anthropic".to_string()),
                fallback: Some(vec!["openai".to_string(), "anthropic".to_string()]),
                providers: [
                    (
                        "openai".to_string(),
                        ProviderEntry {
                            kind: None,
                            api_key: Some("openai-key".to_string()),
                            api_key_env: None,
                            base_url: None,
                            model: None,
                            input_cost_per_million: None,
                            output_cost_per_million: None,
                            binary: None,
                            extra_args: Vec::new(),
                        },
                    ),
                    (
                        "anthropic".to_string(),
                        ProviderEntry {
                            kind: None,
                            api_key: Some("anthropic-key".to_string()),
                            api_key_env: None,
                            base_url: None,
                            model: None,
                            input_cost_per_million: None,
                            output_cost_per_million: None,
                            binary: None,
                            extra_args: Vec::new(),
                        },
                    ),
                ]
                .into_iter()
                .collect(),
            },
            None,
        )
        .expect("router");

        let routes = router.route_info();
        assert_eq!(routes[0].name, "anthropic");
        assert_eq!(routes[1].name, "openai");
        assert_eq!(
            router.selected_route_info().map(|route| route.name),
            Some("anthropic".to_string())
        );
    }

    #[test]
    fn defaults_provider_table_is_read_as_default_provider() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("config.toml");
        fs::write(
            &path,
            r#"
fallback = ["openai"]

[defaults]
provider = "anthropic"

[providers.openai]
api_key = "openai-key"

[providers.anthropic]
api_key = "anthropic-key"
"#,
        )
        .expect("write config");

        let config = read_config(&path).expect("parse");
        assert_eq!(config.default_provider.as_deref(), Some("anthropic"));
        let router = ProviderRouter::from_config(config, None).expect("router");
        assert_eq!(
            router.selected_route_info().map(|route| route.name),
            Some("anthropic".to_string())
        );
    }

    #[test]
    fn spend_estimate_uses_per_million_rates() {
        let adapter = ProviderAdapter::new(
            "openai",
            ProviderKind::OpenAi,
            ProviderEntry {
                kind: Some(ProviderKind::OpenAi),
                api_key: Some("key".to_string()),
                api_key_env: None,
                base_url: None,
                model: Some("model".to_string()),
                input_cost_per_million: Some(2.0),
                output_cost_per_million: Some(8.0),
                binary: None,
                extra_args: Vec::new(),
            },
        );

        let spend = adapter.estimate_spend(ProviderUsage {
            input_tokens: 1_000,
            output_tokens: 2_000,
        });
        assert_eq!(spend.cost_usd, 0.018);
    }

    #[test]
    fn catalog_seam_overrides_context_window_and_pricing() {
        let catalog = ModelCatalogOverride::from_models(vec![ModelEntry {
            id: "custom-model".to_string(),
            context_window: Some(123_456),
            input_per_million: Some(2.0),
            output_per_million: Some(3.0),
            aliases: vec!["custom-alias".to_string()],
        }])
        .expect("catalog");
        let router = ProviderRouter::from_config_with_model_and_catalog_override(
            ProviderConfigFile {
                default_provider: Some("openai".to_string()),
                fallback: None,
                providers: Default::default(),
            },
            None,
            Some("custom-alias"),
            Some(&catalog),
        )
        .expect("router");

        assert_eq!(
            router.context_window_for_route(Some("openai")),
            Some(123_456)
        );
        assert_eq!(
            router
                .context_window_for_route_with_source(Some("openai"))
                .map(|(_, source)| source.as_str()),
            Some("seam")
        );
        let spend = router
            .estimate_for_route(
                Some("openai"),
                ProviderUsage {
                    input_tokens: 1_000_000,
                    output_tokens: 1_000_000,
                },
            )
            .expect("spend");
        assert_eq!(spend.cost_usd, 5.0);
    }

    #[test]
    fn catalog_seam_malformed_falls_back_to_builtin() {
        let malformed = serde_json::json!({
            "models": [
                { "context_window": 1, "input_per_million": 99.0 }
            ]
        });
        assert!(ModelCatalogOverride::from_value(malformed).is_err());

        let router = ProviderRouter::from_config_with_model_and_catalog_override(
            ProviderConfigFile {
                default_provider: Some("openai".to_string()),
                fallback: None,
                providers: Default::default(),
            },
            None,
            None,
            None,
        )
        .expect("router");

        assert_eq!(
            router.context_window_for_route(Some("openai")),
            Some(400_000)
        );
        assert_eq!(
            router
                .context_window_for_route_with_source(Some("openai"))
                .map(|(_, source)| source.as_str()),
            Some("catalog")
        );
        let spend = router
            .estimate_for_route(
                Some("openai"),
                ProviderUsage {
                    input_tokens: 1_000_000,
                    output_tokens: 1_000_000,
                },
            )
            .expect("spend");
        assert_eq!(spend.cost_usd, 11.25);
    }

    #[test]
    fn unconfigured_catalog_uses_builtin_model_entry_list() {
        let router = ProviderRouter::from_config(
            ProviderConfigFile {
                default_provider: Some("anthropic".to_string()),
                fallback: None,
                providers: Default::default(),
            },
            None,
        )
        .expect("router");

        assert_eq!(
            router.context_window_for_route(Some("anthropic")),
            Some(200_000)
        );
        let spend = router
            .estimate_for_route(
                Some("anthropic"),
                ProviderUsage {
                    input_tokens: 1_000_000,
                    output_tokens: 1_000_000,
                },
            )
            .expect("spend");
        assert_eq!(spend.cost_usd, 18.0);
    }

    #[tokio::test]
    async fn router_reports_missing_credentials_without_calling_network() {
        let router = ProviderRouter::from_config(
            ProviderConfigFile {
                default_provider: None,
                fallback: Some(vec!["openai".to_string()]),
                providers: Default::default(),
            },
            None,
        )
        .expect("router");
        let err = router
            .complete(&super::ProviderRequest {
                prompt: "hello".to_string(),
                max_output_tokens: 16,
                cwd: None,
                output_path: None,
                sandbox_backend: None,
                pid_file: None,
                cancellation_token: None,
            })
            .await
            .expect_err("missing credentials");
        assert!(err.to_string().contains("missing credential"));
    }

    #[tokio::test]
    async fn smoke_router_is_scripted_and_keyless() {
        let router = ProviderRouter::smoke();
        let response = router
            .complete(&super::ProviderRequest {
                prompt: "tiny hello rust".to_string(),
                max_output_tokens: 16,
                cwd: None,
                output_path: None,
                sandbox_backend: None,
                pid_file: None,
                cancellation_token: None,
            })
            .await
            .expect("smoke response");
        assert_eq!(response.provider, "smoke");
        assert_eq!(response.trace["kind"], "scripted_smoke");
        assert!(response.content.contains("\"action\":\"bash\""));
    }
}
