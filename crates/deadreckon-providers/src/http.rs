use std::env;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

use crate::{
    Provider, ProviderEntry, ProviderError, ProviderFuture, ProviderKind, ProviderRequest,
    ProviderResponse, ProviderUsage, Result, SpendEstimate, validate_openai_strict_output_schema,
};

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
        let base_url = entry.base_url.unwrap_or_else(|| default_base_url(&kind));
        let model = entry.model.unwrap_or_else(|| default_model(&kind));
        Self {
            name,
            kind,
            base_url,
            model,
            api_key,
            input_cost_per_million: entry.input_cost_per_million.unwrap_or(0.0),
            output_cost_per_million: entry.output_cost_per_million.unwrap_or(0.0),
            // A stalled connection must never hang an unattended run: bound
            // connect and total request time. Long completions stream within
            // the request window; the turn loop's wall budget is the outer cap.
            client: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(30))
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    fn headers(&self) -> Result<HeaderMap> {
        let Some(api_key) = self.api_key.as_deref() else {
            return Err(ProviderError::MissingCredential(self.name.clone()));
        };
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        match &self.kind {
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
            ProviderKind::CliClaudeCode
            | ProviderKind::CliCodex
            | ProviderKind::ScriptedSmoke
            | ProviderKind::Generic(_) => {}
        }
        Ok(headers)
    }

    fn endpoint(&self) -> String {
        match &self.kind {
            ProviderKind::Anthropic => {
                format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
            }
            ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => {
                format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
            }
            ProviderKind::CliClaudeCode
            | ProviderKind::CliCodex
            | ProviderKind::ScriptedSmoke
            | ProviderKind::Generic(_) => {
                unreachable!("CLI providers do not use HTTP endpoints")
            }
        }
    }

    fn payload(&self, request: &ProviderRequest) -> Value {
        match &self.kind {
            ProviderKind::Anthropic => {
                let mut payload = json!({
                    "model": self.model,
                    "max_tokens": request.max_output_tokens,
                    "messages": [{"role": "user", "content": request.prompt}],
                });
                if let Some(schema) = request.output_schema.as_ref() {
                    payload["output_config"] = json!({
                        "format": {"type": "json_schema", "schema": schema}
                    });
                }
                payload
            }
            ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => {
                let mut payload = json!({
                    "model": self.model,
                    "messages": [{"role": "user", "content": request.prompt}],
                    "max_tokens": request.max_output_tokens,
                    "stream": false,
                });
                if let Some(schema) = request.output_schema.as_ref() {
                    payload["response_format"] = json!({
                        "type": "json_schema",
                        "json_schema": {
                            "name": "deadreckon_structured_text",
                            "strict": true,
                            "schema": schema,
                        }
                    });
                }
                payload
            }
            ProviderKind::CliClaudeCode
            | ProviderKind::CliCodex
            | ProviderKind::ScriptedSmoke
            | ProviderKind::Generic(_) => {
                unreachable!("CLI providers do not use HTTP payloads")
            }
        }
    }

    async fn send(&self, request: &ProviderRequest) -> Result<ProviderResponse> {
        if matches!(
            self.kind,
            ProviderKind::OpenAi | ProviderKind::OpenAiCompatible
        ) && let Some(schema) = request.output_schema.as_ref()
        {
            validate_openai_strict_output_schema(&self.name, schema)?;
        }
        let headers = self.headers()?;
        let request_future = self
            .client
            .post(self.endpoint())
            .headers(headers)
            .json(&self.payload(request))
            .send();
        let response = if let Some(token) = request.cancellation_token.as_ref() {
            tokio::select! {
                _ = token.cancelled() => {
                    return Err(ProviderError::Http {
                        provider: self.name.clone(),
                        detail: "request cancelled".to_string(),
                        retryable: false,
                    });
                }
                response = request_future => response
            }
        } else {
            request_future.await
        }
        .map_err(|err| ProviderError::Http {
            provider: self.name.clone(),
            detail: err.to_string(),
            // Transport failures (connect, timeout, reset) are the canonical
            // retry case.
            retryable: true,
        })?;
        let status = response.status();
        let body = response.text().await.map_err(|err| ProviderError::Http {
            provider: self.name.clone(),
            detail: err.to_string(),
            retryable: true,
        })?;
        if !status.is_success() {
            return Err(ProviderError::Http {
                provider: self.name.clone(),
                detail: format!("HTTP {status}: {}", trim_for_error(&body)),
                retryable: matches!(status.as_u16(), 408 | 429 | 500 | 502 | 503 | 504),
            });
        }
        let mut parsed = self.parse_response(&body)?;
        parsed.trace["workspace_access"] = Value::String(request.workspace_access.as_str().into());
        Ok(parsed)
    }

    fn parse_response(&self, body: &str) -> Result<ProviderResponse> {
        let value: Value = serde_json::from_str(body).map_err(|err| ProviderError::Http {
            provider: self.name.clone(),
            detail: err.to_string(),
            // A 2xx with a malformed body won't improve on retry.
            retryable: false,
        })?;
        let (content, usage) = match &self.kind {
            ProviderKind::Anthropic => parse_anthropic_response(&value),
            ProviderKind::OpenAi | ProviderKind::OpenAiCompatible => parse_openai_response(&value),
            ProviderKind::CliClaudeCode
            | ProviderKind::CliCodex
            | ProviderKind::ScriptedSmoke
            | ProviderKind::Generic(_) => {
                unreachable!("CLI providers do not parse HTTP responses")
            }
        };
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
        self.kind.clone()
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

fn env_value(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn default_base_url(kind: &ProviderKind) -> String {
    match kind {
        ProviderKind::Anthropic => "https://api.anthropic.com".to_string(),
        ProviderKind::OpenAi => "https://api.openai.com/v1".to_string(),
        ProviderKind::OpenAiCompatible => "http://127.0.0.1:11434/v1".to_string(),
        ProviderKind::CliClaudeCode
        | ProviderKind::CliCodex
        | ProviderKind::ScriptedSmoke
        | ProviderKind::Generic(_) => String::new(),
    }
}

fn default_model(kind: &ProviderKind) -> String {
    match kind {
        ProviderKind::Anthropic => "claude-sonnet-4-5".to_string(),
        ProviderKind::OpenAi => "gpt-5.1-codex".to_string(),
        ProviderKind::OpenAiCompatible => "local-model".to_string(),
        ProviderKind::CliClaudeCode => "cli:claude-code".to_string(),
        ProviderKind::CliCodex => "cli:codex".to_string(),
        ProviderKind::ScriptedSmoke => "local-scripted-smoke".to_string(),
        ProviderKind::Generic(id) => id.clone(),
    }
}

fn parse_openai_response(value: &Value) -> (String, ProviderUsage) {
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
    (content, usage)
}

fn parse_anthropic_response(value: &Value) -> (String, ProviderUsage) {
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
    (content, usage)
}

fn trim_for_error(body: &str) -> String {
    const MAX: usize = 240;
    if body.len() <= MAX {
        return body.to_string();
    }
    // Cut on a char boundary: error bodies can carry multibyte text, and a
    // byte slice would panic exactly while reporting a provider failure.
    let cut = (0..=MAX)
        .rev()
        .find(|i| body.is_char_boundary(*i))
        .unwrap_or(0);
    format!("{}...", &body[..cut])
}

fn round_usd(value: f64) -> f64 {
    (value * 1_000_000.0).round() / 1_000_000.0
}
