use std::collections::BTreeMap;
use std::env;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

pub mod cli_claude_code;
pub mod cli_codex;
mod cli_common;

use cli_claude_code::CliClaudeCodeProvider;
use cli_codex::CliCodexProvider;
use deadreckon_sandbox::SandboxBackend;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const DEFAULT_CONFIG_PATH: &str = "/Users/gdc/.deadreckon/config.toml";

pub type ProviderFuture<'a> = Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("TOML error at {path}: {source}")]
    Toml {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("provider route has no credential: {0}")]
    MissingCredential(String),
    #[error("no provider route succeeded: {0}")]
    NoRoute(String),
    #[error("HTTP provider error for {provider}: {detail}")]
    Http { provider: String, detail: String },
    #[error("CLI provider error for {provider}: {detail}")]
    Cli { provider: String, detail: String },
    #[error("invalid provider configuration: {0}")]
    InvalidConfig(String),
}

pub type Result<T> = std::result::Result<T, ProviderError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    OpenAiCompatible,
    CliClaudeCode,
    CliCodex,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpendEstimate {
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    #[serde(default)]
    pub subscription: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_time_seconds: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub prompt: String,
    pub max_output_tokens: u32,
    pub cwd: Option<PathBuf>,
    pub output_path: Option<PathBuf>,
    pub sandbox_backend: Option<SandboxBackend>,
    pub pid_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderResponse {
    pub provider: String,
    pub model: String,
    pub content: String,
    pub usage: ProviderUsage,
    pub spend: SpendEstimate,
    #[serde(default)]
    pub trace: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfigFile {
    pub default_provider: Option<String>,
    pub fallback: Option<Vec<String>>,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderEntry {
    pub kind: Option<ProviderKind>,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub input_cost_per_million: Option<f64>,
    pub output_cost_per_million: Option<f64>,
    pub binary: Option<String>,
    #[serde(default)]
    pub extra_args: Vec<String>,
}

pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn kind(&self) -> ProviderKind;
    fn model(&self) -> &str;
    fn has_credential(&self) -> bool;
    fn estimate_spend(&self, usage: ProviderUsage) -> SpendEstimate;
    fn complete<'a>(&'a self, request: &'a ProviderRequest) -> ProviderFuture<'a>;
}

#[derive(Clone)]
pub struct ProviderAdapter {
    name: String,
    kind: ProviderKind,
    base_url: String,
    model: String,
    api_key: Option<String>,
    input_cost_per_million: f64,
    output_cost_per_million: f64,
    client: reqwest::Client,
}

impl ProviderAdapter {
    pub fn new(name: impl Into<String>, kind: ProviderKind, entry: ProviderEntry) -> Self {
        let name = name.into();
        let api_key = entry
            .api_key
            .or_else(|| entry.api_key_env.as_deref().and_then(env_value));
        let base_url = entry.base_url.unwrap_or_else(|| default_base_url(kind));
        let model = entry.model.unwrap_or_else(|| default_model(kind));
        Self {
            name,
            kind,
            base_url,
            model,
            api_key,
            input_cost_per_million: entry.input_cost_per_million.unwrap_or(0.0),
            output_cost_per_million: entry.output_cost_per_million.unwrap_or(0.0),
            client: reqwest::Client::new(),
        }
    }

    fn headers(&self) -> Result<HeaderMap> {
        let Some(api_key) = self.api_key.as_deref() else {
            return Err(ProviderError::MissingCredential(self.name.clone()));
        };
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        match self.kind {
            ProviderKind::Anthropic => {
                let key = HeaderValue::from_str(api_key).map_err(|err| {
                    ProviderError::InvalidConfig(format!("invalid Anthropic API key header: {err}"))
                })?;
                headers.insert("x-api-key", key);
                headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
            }
            ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => {
                let bearer =
                    HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| {
                        ProviderError::InvalidConfig(format!("invalid bearer token header: {err}"))
                    })?;
                headers.insert(AUTHORIZATION, bearer);
            }
            ProviderKind::CliClaudeCode | ProviderKind::CliCodex => {}
        }
        Ok(headers)
    }

    fn endpoint(&self) -> String {
        match self.kind {
            ProviderKind::Anthropic => {
                format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
            }
            ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => {
                format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
            }
            ProviderKind::CliClaudeCode | ProviderKind::CliCodex => {
                unreachable!("CLI providers do not use HTTP endpoints")
            }
        }
    }

    fn payload(&self, request: &ProviderRequest) -> Value {
        match self.kind {
            ProviderKind::Anthropic => json!({
                "model": self.model,
                "max_tokens": request.max_output_tokens,
                "messages": [{"role": "user", "content": request.prompt}],
            }),
            ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => json!({
                "model": self.model,
                "messages": [{"role": "user", "content": request.prompt}],
                "max_tokens": request.max_output_tokens,
                "stream": false,
            }),
            ProviderKind::CliClaudeCode | ProviderKind::CliCodex => {
                unreachable!("CLI providers do not use HTTP payloads")
            }
        }
    }

    async fn send(&self, request: &ProviderRequest) -> Result<ProviderResponse> {
        let headers = self.headers()?;
        let response = self
            .client
            .post(self.endpoint())
            .headers(headers)
            .json(&self.payload(request))
            .send()
            .await
            .map_err(|err| ProviderError::Http {
                provider: self.name.clone(),
                detail: err.to_string(),
            })?;
        let status = response.status();
        let body = response.text().await.map_err(|err| ProviderError::Http {
            provider: self.name.clone(),
            detail: err.to_string(),
        })?;
        if !status.is_success() {
            return Err(ProviderError::Http {
                provider: self.name.clone(),
                detail: format!("HTTP {status}: {}", trim_for_error(&body)),
            });
        }
        self.parse_response(&body)
    }

    fn parse_response(&self, body: &str) -> Result<ProviderResponse> {
        let value: Value = serde_json::from_str(body).map_err(|err| ProviderError::Http {
            provider: self.name.clone(),
            detail: err.to_string(),
        })?;
        let (content, usage) = match self.kind {
            ProviderKind::Anthropic => parse_anthropic_response(&value),
            ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => parse_openai_response(&value),
            ProviderKind::CliClaudeCode | ProviderKind::CliCodex => {
                unreachable!("CLI providers do not parse HTTP responses")
            }
        }?;
        let spend = self.estimate_spend(usage.clone());
        Ok(ProviderResponse {
            provider: self.name.clone(),
            model: self.model.clone(),
            content,
            usage,
            spend,
            trace: json!({"kind": "http_llm"}),
        })
    }
}

impl Provider for ProviderAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn has_credential(&self) -> bool {
        self.api_key.as_deref().is_some_and(|key| !key.is_empty())
    }

    fn estimate_spend(&self, usage: ProviderUsage) -> SpendEstimate {
        let input = usage.input_tokens as f64 / 1_000_000.0 * self.input_cost_per_million;
        let output = usage.output_tokens as f64 / 1_000_000.0 * self.output_cost_per_million;
        SpendEstimate {
            provider: self.name.clone(),
            model: self.model.clone(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cost_usd: round_usd(input + output),
            subscription: false,
            wall_time_seconds: None,
        }
    }

    fn complete<'a>(&'a self, request: &'a ProviderRequest) -> ProviderFuture<'a> {
        Box::pin(async move { self.send(request).await })
    }
}

pub struct ProviderRouter {
    routes: Vec<Box<dyn Provider>>,
}

impl ProviderRouter {
    pub fn from_config_path(path: &Path, override_provider: Option<&str>) -> Result<Self> {
        // REPORT.md: Provider Routing / BYOK keeps credentials in the user's
        // local config/env and tries the configured fallback chain.
        let config = read_config(path)?;
        Self::from_config(config, override_provider)
    }

    pub fn from_config(
        config: ProviderConfigFile,
        override_provider: Option<&str>,
    ) -> Result<Self> {
        let mut providers = builtin_entries();
        for (name, entry) in config.providers {
            providers.insert(name, entry);
        }

        let route_names = if let Some(provider) = override_provider {
            vec![provider.to_string()]
        } else if let Some(fallback) = config.fallback {
            fallback
        } else if let Some(default_provider) = config.default_provider {
            vec![default_provider]
        } else {
            vec![
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

pub fn read_config(path: &Path) -> Result<ProviderConfigFile> {
    match std::fs::read_to_string(path) {
        Ok(raw) => toml::from_str(&raw).map_err(|source| ProviderError::Toml {
            path: path.display().to_string(),
            source,
        }),
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

fn builtin_entries() -> BTreeMap<String, ProviderEntry> {
    BTreeMap::from([
        (
            "cli:claude-code".to_string(),
            ProviderEntry {
                kind: Some(ProviderKind::CliClaudeCode),
                api_key: None,
                api_key_env: None,
                base_url: None,
                model: Some("cli:claude-code".to_string()),
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
                model: Some("cli:codex".to_string()),
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

fn build_provider(name: String, kind: ProviderKind, entry: ProviderEntry) -> Box<dyn Provider> {
    match kind {
        ProviderKind::CliClaudeCode => Box::new(CliClaudeCodeProvider::new(name, entry)),
        ProviderKind::CliCodex => Box::new(CliCodexProvider::new(name, entry)),
        ProviderKind::Anthropic | ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => {
            Box::new(ProviderAdapter::new(name, kind, entry))
        }
    }
}

fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn default_base_url(kind: ProviderKind) -> String {
    match kind {
        ProviderKind::Anthropic => "https://api.anthropic.com".to_string(),
        ProviderKind::OpenAi => "https://api.openai.com/v1".to_string(),
        ProviderKind::OpenAiCompatible => "http://127.0.0.1:11434/v1".to_string(),
        ProviderKind::CliClaudeCode | ProviderKind::CliCodex => String::new(),
    }
}

fn default_model(kind: ProviderKind) -> String {
    match kind {
        ProviderKind::Anthropic => "claude-sonnet-4-5".to_string(),
        ProviderKind::OpenAi => "gpt-5.1-codex".to_string(),
        ProviderKind::OpenAiCompatible => "local-model".to_string(),
        ProviderKind::CliClaudeCode => "cli:claude-code".to_string(),
        ProviderKind::CliCodex => "cli:codex".to_string(),
    }
}

fn kind_from_name(name: &str) -> Option<ProviderKind> {
    match name {
        "anthropic" => Some(ProviderKind::Anthropic),
        "openai" => Some(ProviderKind::OpenAi),
        "openai-compatible" | "openrouter" | "llama-cpp" => Some(ProviderKind::OpenAiCompatible),
        "cli:claude-code" | "cli-claude-code" => Some(ProviderKind::CliClaudeCode),
        "cli:codex" | "cli-codex" => Some(ProviderKind::CliCodex),
        _ => None,
    }
}

fn parse_openai_response(value: &Value) -> Result<(String, ProviderUsage)> {
    let content = value
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let usage = ProviderUsage {
        input_tokens: value
            .pointer("/usage/prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: value
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    };
    Ok((content, usage))
}

fn parse_anthropic_response(value: &Value) -> Result<(String, ProviderUsage)> {
    let content = value
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let usage = ProviderUsage {
        input_tokens: value
            .pointer("/usage/input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        output_tokens: value
            .pointer("/usage/output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    };
    Ok((content, usage))
}

fn trim_for_error(body: &str) -> String {
    const MAX: usize = 240;
    if body.len() <= MAX {
        body.to_string()
    } else {
        format!("{}...", &body[..MAX])
    }
}

fn round_usd(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{
        Provider, ProviderAdapter, ProviderConfigFile, ProviderEntry, ProviderKind, ProviderRouter,
        ProviderUsage, read_config,
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
            })
            .await
            .expect_err("missing credentials");
        assert!(err.to_string().contains("missing credential"));
    }
}
