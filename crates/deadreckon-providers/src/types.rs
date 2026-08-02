use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use deadreckon_core::NetworkCapability;
use deadreckon_sandbox::{SandboxBackend, WorkspaceAccess};
use serde::Deserializer;
use serde::Serializer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::error::Result;

pub type ProviderFuture<'a> = Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    OpenAiCompatible,
    CliClaudeCode,
    CliCodex,
    ScriptedSmoke,
    Generic(String),
}

impl ProviderKind {
    pub fn as_config_str(&self) -> &str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAi => "open-ai",
            ProviderKind::OpenAiCompatible => "open-ai-compatible",
            ProviderKind::CliClaudeCode => "cli-claude-code",
            ProviderKind::CliCodex => "cli-codex",
            ProviderKind::ScriptedSmoke => "scripted-smoke",
            ProviderKind::Generic(id) => id,
        }
    }
}

impl Serialize for ProviderKind {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_config_str())
    }
}

impl<'de> Deserialize<'de> for ProviderKind {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "anthropic" => ProviderKind::Anthropic,
            "open-ai" | "openai" => ProviderKind::OpenAi,
            "open-ai-compatible" | "openai-compatible" => ProviderKind::OpenAiCompatible,
            "cli-claude-code" | "cli:claude-code" => ProviderKind::CliClaudeCode,
            "cli-codex" | "cli:codex" => ProviderKind::CliCodex,
            "scripted-smoke" | "smoke" => ProviderKind::ScriptedSmoke,
            _ => ProviderKind::Generic(value),
        })
    }
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

#[derive(Debug, Clone, Default)]
pub struct ProviderRequest {
    pub prompt: String,
    pub max_output_tokens: u32,
    pub cwd: Option<PathBuf>,
    pub output_path: Option<PathBuf>,
    pub sandbox_backend: Option<SandboxBackend>,
    /// Filesystem posture for the provider process. Semantic verification uses
    /// `ReadOnly`; ordinary worker turns retain the backward-compatible
    /// `ReadWrite` default.
    pub workspace_access: WorkspaceAccess,
    pub pid_file: Option<PathBuf>,
    pub cancellation_token: Option<CancellationToken>,
    /// Run root holding the per-run `provider-session.json` (Semaphore).
    /// Threaded from the turn loop; providers that don't resume ignore it.
    pub session_dir: Option<PathBuf>,
    /// JSON Schema for schema-constrained final output. Setting this enters a
    /// request-scoped structured-text-only posture: adapters must both enforce
    /// the exact schema and remove their tool surfaces, or fail closed before
    /// starting model work. It must never silently degrade to unconstrained
    /// text or an agentic/tool-enabled invocation.
    pub output_schema: Option<Value>,
    /// Existing run capability facts used to answer app-server approvals.
    /// This is request-scoped transport, not a second persisted policy schema.
    pub capability_posture: Option<CapabilityPosture>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityPosture {
    pub network: NetworkCapability,
    pub network_allowlist: Vec<String>,
    pub deploy: bool,
    pub install: bool,
    pub working_dir: PathBuf,
    pub additional_write_roots: Vec<PathBuf>,
    pub workspace_access: WorkspaceAccess,
}

impl ProviderRequest {
    /// Build a provider request that may inspect a workspace but cannot write
    /// it. Local CLI routes run under the host's automatically selected
    /// sandbox; when no backend can enforce read-only access, the common CLI
    /// runner refuses before spawning the provider.
    pub fn enforceably_read_only(
        prompt: impl Into<String>,
        max_output_tokens: u32,
        cwd: impl Into<PathBuf>,
    ) -> Self {
        Self::enforceably_read_only_with_backend(
            prompt,
            max_output_tokens,
            cwd,
            SandboxBackend::Auto,
        )
    }

    /// Build an enforceably read-only request using an already approved
    /// sandbox backend. This keeps preflight planners and trusted Job helpers
    /// inside the same operator-selected containment boundary.
    pub fn enforceably_read_only_with_backend(
        prompt: impl Into<String>,
        max_output_tokens: u32,
        cwd: impl Into<PathBuf>,
        sandbox_backend: SandboxBackend,
    ) -> Self {
        // An uncontained worker policy never weakens a planner or judge.
        // Elevate `none` to automatic host containment and let the common CLI
        // runner fail closed when no enforceable backend exists.
        let sandbox_backend = if sandbox_backend == SandboxBackend::None {
            SandboxBackend::Auto
        } else {
            sandbox_backend
        };
        Self {
            prompt: prompt.into(),
            max_output_tokens,
            cwd: Some(cwd.into()),
            output_path: None,
            sandbox_backend: Some(sandbox_backend),
            workspace_access: WorkspaceAccess::ReadOnly,
            pid_file: None,
            cancellation_token: None,
            session_dir: None,
            output_schema: None,
            capability_posture: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn set_capability_posture(
        &mut self,
        network: NetworkCapability,
        network_allowlist: Vec<String>,
        deploy: bool,
        install: bool,
        working_dir: PathBuf,
        additional_write_roots: Vec<PathBuf>,
    ) {
        self.capability_posture = Some(CapabilityPosture {
            network,
            network_allowlist,
            deploy,
            install,
            working_dir,
            additional_write_roots,
            workspace_access: self.workspace_access,
        });
    }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRouteInfo {
    pub name: String,
    pub kind: ProviderKind,
    pub model: String,
    pub has_credential: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforceably_read_only_request_carries_a_real_boundary() {
        let cwd = PathBuf::from("/tmp/deadreckon-planner");
        let request = ProviderRequest::enforceably_read_only("plan", 512, cwd.clone());
        assert_eq!(request.cwd.as_deref(), Some(cwd.as_path()));
        assert_eq!(request.workspace_access, WorkspaceAccess::ReadOnly);
        assert_eq!(request.sandbox_backend, Some(SandboxBackend::Auto));
        assert!(request.session_dir.is_none());
        assert!(request.capability_posture.is_none());
    }

    #[test]
    fn uncontained_worker_policy_cannot_weaken_a_read_only_request() {
        let request = ProviderRequest::enforceably_read_only_with_backend(
            "plan",
            512,
            "/tmp/deadreckon-planner",
            SandboxBackend::None,
        );
        assert_eq!(request.sandbox_backend, Some(SandboxBackend::Auto));
    }
}
