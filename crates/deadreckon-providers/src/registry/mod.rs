use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value as JsonValue;
use toml::Value;

use crate::ProviderError;
use crate::Result;

const BUILTIN_DESCRIPTOR_SOURCES: &[(&str, &str)] = &[
    (
        "anthropic",
        include_str!("../../descriptors/anthropic.toml"),
    ),
    ("openai", include_str!("../../descriptors/openai.toml")),
    (
        "openai-compatible",
        include_str!("../../descriptors/openai-compatible.toml"),
    ),
    ("smoke", include_str!("../../descriptors/smoke.toml")),
    (
        "cli:claude-code",
        include_str!("../../descriptors/cli-claude-code.toml"),
    ),
    (
        "cli:codex",
        include_str!("../../descriptors/cli-codex.toml"),
    ),
    (
        "cli:gemini",
        include_str!("../../descriptors/cli-gemini.toml"),
    ),
    (
        "cli:opencode",
        include_str!("../../descriptors/cli-opencode.toml"),
    ),
    (
        "cli:copilot",
        include_str!("../../descriptors/cli-copilot.toml"),
    ),
    ("cli:pi", include_str!("../../descriptors/cli-pi.toml")),
];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DescriptorKind {
    Http,
    Cli,
    LocalHttp,
    #[default]
    Scripted,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthKind {
    ApiKey,
    #[default]
    None,
    Subscription,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthScheme {
    Bearer,
    XApiKey,
    Basic,
    #[default]
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AuthDescriptor {
    pub kind: AuthKind,
    pub env_var: Option<String>,
    pub header: Option<String>,
    pub scheme: AuthScheme,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct VersionProbe {
    pub args: Vec<String>,
    pub expect_substring: Option<String>,
    pub min_known_good: Option<String>,
}

/// A local, no-cost login-state probe for subscription CLI providers
/// (`<binary> <args>`, e.g. `claude auth status`). Matching is fail-open: when
/// neither marker appears the state is Unknown and callers behave exactly as
/// they did before the probe existed (binary presence only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AuthProbe {
    pub args: Vec<String>,
    pub logged_in_substring: Option<String>,
    pub logged_out_substring: Option<String>,
    pub login_try_lines: Vec<String>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RequestShape {
    OpenAiChat,
    AnthropicMessages,
    GeminiGenerateContent,
    OllamaChat,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ExecTemplate {
    pub args_template: Vec<String>,
    pub model_arg: Option<String>,
    pub timeout_seconds: Option<u64>,
    pub request_shape: Option<RequestShape>,
    pub path_template: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ModelEntry {
    pub id: String,
    pub context_window: Option<u32>,
    pub input_per_million: Option<f64>,
    pub output_per_million: Option<f64>,
    pub aliases: Vec<String>,
    /// Exactly one entry per descriptor catalog carries this; pickers
    /// preselect it when no default is configured.
    pub recommended: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelCatalogOverride {
    entries: BTreeMap<String, ModelEntry>,
}

impl ModelCatalogOverride {
    pub fn from_value(value: JsonValue) -> Result<Self> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct CatalogResponse {
            models: Vec<ModelEntry>,
        }

        let response: CatalogResponse = serde_json::from_value(value).map_err(|source| {
            ProviderError::InvalidConfig(format!(
                "catalog seam response must be {{\"models\": [...]}}: {source}"
            ))
        })?;
        Self::from_models(response.models)
    }

    pub fn from_models(models: Vec<ModelEntry>) -> Result<Self> {
        if models.is_empty() {
            return Err(ProviderError::InvalidConfig(
                "catalog seam response must include at least one model".to_string(),
            ));
        }
        let mut entries = BTreeMap::new();
        for model in models {
            if model.id.trim().is_empty() {
                return Err(ProviderError::InvalidConfig(
                    "catalog seam model id must not be empty".to_string(),
                ));
            }
            entries.insert(model.id.clone(), model.clone());
            for alias in &model.aliases {
                if !alias.trim().is_empty() {
                    entries.insert(alias.clone(), model.clone());
                }
            }
        }
        Ok(Self { entries })
    }

    pub fn entry_for_model(&self, model: &str) -> Option<&ModelEntry> {
        self.entries.get(model)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct InstallHint {
    pub url: String,
    pub try_lines: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IngestCwdMatch {
    #[default]
    None,
    SessionMeta,
    TopLevel,
    JsonPointer,
    ClaudeProjectDir,
    DirectoryField,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IngestStorage {
    Jsonl,
    Json,
    JsonOrJsonl,
    #[serde(rename = "opencode-storage")]
    OpenCodeStorage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct IngestDescriptor {
    pub id_prefix: Option<String>,
    pub env_var: Option<String>,
    pub default_dirs: Vec<PathBuf>,
    pub watch_subdirs: Vec<PathBuf>,
    pub shallow_watch: bool,
    pub schema: String,
    pub cwd_match: IngestCwdMatch,
    pub cwd_match_path: Option<String>,
    pub session_id_from: Option<String>,
    pub file_glob: Option<String>,
    pub freshness_minutes: Option<i64>,
    pub storage: Option<IngestStorage>,
    /// The driver ingests tool rows live from the provider's structured stream
    /// (Semaphore); the post-hoc file scraper yields to it so the two never
    /// double-count. Old descriptors default false and keep file scraping.
    pub live_contract: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderDescriptor {
    pub id: String,
    pub display_name: String,
    pub kind: DescriptorKind,
    pub default_binary: Option<String>,
    pub default_endpoint: Option<String>,
    pub auth: AuthDescriptor,
    pub version_probe: Option<VersionProbe>,
    pub auth_probe: Option<AuthProbe>,
    pub exec_template: ExecTemplate,
    pub sandbox_writes: Vec<PathBuf>,
    pub sandbox_reads: Vec<PathBuf>,
    pub allow_network_default: bool,
    pub model_catalog: Vec<ModelEntry>,
    pub default_model: Option<String>,
    pub fs_detection_paths: Vec<PathBuf>,
    pub install_hint: InstallHint,
    pub docs_url: Option<String>,
    pub subscription: bool,
    pub ingest: Option<IngestDescriptor>,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderRegistry {
    descriptors: BTreeMap<String, ProviderDescriptor>,
}

pub type ProviderProbeFuture<'a> = Pin<Box<dyn Future<Output = ProviderProbeResult> + Send + 'a>>;

pub trait ProviderProbe {
    fn probe<'a>(&'a self, options: ProviderProbeOptions) -> ProviderProbeFuture<'a>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ProviderProbeOptions {
    pub ping: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Ok,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeErrorKind {
    NotFound,
    PermissionDenied,
    VersionMismatch,
    AuthMissing,
    EndpointUnreachable,
    ProbeFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderProbeResult {
    pub id: String,
    pub display_name: String,
    pub kind: DescriptorKind,
    pub status: ProbeStatus,
    pub location: Option<String>,
    pub version: Option<String>,
    pub credential: String,
    pub subscription: bool,
    pub metering: String,
    pub fs_artifacts: Vec<String>,
    pub error_kind: Option<ProbeErrorKind>,
    pub message: Option<String>,
    pub try_lines: Vec<String>,
}

impl ProviderRegistry {
    pub fn builtin() -> Result<Self> {
        let mut registry = Self::default();
        for (id, raw) in BUILTIN_DESCRIPTOR_SOURCES {
            let descriptor = parse_descriptor(raw, &format!("builtin:{id}"))?;
            registry
                .descriptors
                .insert(descriptor.id.clone(), descriptor);
        }
        Ok(registry)
    }

    pub fn with_overrides(home: &Path) -> Result<Self> {
        let mut registry = Self::builtin()?;
        let overrides_dir = home.join("providers.d");
        let mut paths = match fs::read_dir(&overrides_dir) {
            Ok(entries) => entries
                .filter_map(std::result::Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
                .collect::<Vec<_>>(),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(registry),
            Err(source) => {
                return Err(ProviderError::Io {
                    path: overrides_dir.display().to_string(),
                    source,
                });
            }
        };
        paths.sort();
        for path in paths {
            registry.load_override(&path)?;
        }
        Ok(registry)
    }

    pub fn get(&self, id: &str) -> Option<&ProviderDescriptor> {
        self.descriptors.get(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ProviderDescriptor> {
        self.descriptors.values()
    }

    pub fn ids(&self) -> Vec<String> {
        self.descriptors.keys().cloned().collect()
    }

    pub async fn probe_all(&self, options: ProviderProbeOptions) -> Vec<ProviderProbeResult> {
        let mut results = Vec::new();
        for descriptor in self.iter() {
            results.push(descriptor.probe(options).await);
        }
        results
    }

    fn load_override(&mut self, path: &Path) -> Result<()> {
        let raw = fs::read_to_string(path).map_err(|source| ProviderError::Io {
            path: path.display().to_string(),
            source,
        })?;
        let mut override_value = parse_toml_value(&raw, path)?;
        let id = descriptor_id_from_value(&override_value)
            .or_else(|| id_from_path(path))
            .ok_or_else(|| {
                ProviderError::InvalidConfig(format!(
                    "provider override {} must include id",
                    path.display()
                ))
            })?;
        ensure_id(&mut override_value, &id)?;
        let merged = if let Some(existing) = self.descriptors.get(&id) {
            let mut base = descriptor_to_value(existing)?;
            merge_values(&mut base, override_value);
            base
        } else {
            override_value
        };
        let descriptor = value_to_descriptor(merged, path)?;
        self.descriptors.insert(descriptor.id.clone(), descriptor);
        Ok(())
    }
}

pub fn parse_descriptor(raw: &str, label: &str) -> Result<ProviderDescriptor> {
    let descriptor: ProviderDescriptor =
        toml::from_str(raw).map_err(|source| ProviderError::Toml {
            path: label.to_string(),
            source,
        })?;
    // A catalog with zero or several recommended entries gives pickers no
    // honest default; fail closed at parse time like other descriptor
    // validation. Catalogs are optional, but once present exactly one entry
    // is the recommendation.
    if !descriptor.model_catalog.is_empty() {
        let recommended = descriptor
            .model_catalog
            .iter()
            .filter(|entry| entry.recommended)
            .count();
        if recommended > 1 {
            return Err(ProviderError::InvalidConfig(format!(
                "{label}: model catalog marks {recommended} entries recommended; exactly one is allowed"
            )));
        }
    }
    Ok(descriptor)
}

pub fn parse_custom_command(command: &str) -> Result<(String, Vec<String>)> {
    let args = split_command_line(command);
    let Some((binary, rest)) = args.split_first() else {
        return Err(ProviderError::InvalidConfig(
            "custom provider command cannot be empty".to_string(),
        ));
    };
    Ok((binary.clone(), rest.to_vec()))
}

impl ProviderProbe for ProviderDescriptor {
    fn probe<'a>(&'a self, options: ProviderProbeOptions) -> ProviderProbeFuture<'a> {
        Box::pin(async move { probe_descriptor(self, options).await })
    }
}

async fn probe_descriptor(
    descriptor: &ProviderDescriptor,
    options: ProviderProbeOptions,
) -> ProviderProbeResult {
    match descriptor.kind {
        DescriptorKind::Cli => probe_cli_descriptor(descriptor),
        DescriptorKind::Http => probe_http_descriptor(descriptor, options).await,
        DescriptorKind::LocalHttp => probe_local_http_descriptor(descriptor, options).await,
        DescriptorKind::Scripted => base_probe_result(
            descriptor,
            ProbeStatus::Ok,
            Some("built-in".to_string()),
            None,
            "ready",
            None,
            None,
        ),
    }
}

fn probe_cli_descriptor(descriptor: &ProviderDescriptor) -> ProviderProbeResult {
    let Some(binary) = descriptor.default_binary.as_deref() else {
        return base_probe_result(
            descriptor,
            ProbeStatus::Failed,
            None,
            None,
            "missing",
            Some(ProbeErrorKind::NotFound),
            Some("no default binary configured".to_string()),
        );
    };
    let Some(location) = resolve_binary(binary) else {
        return base_probe_result(
            descriptor,
            ProbeStatus::Failed,
            Some(binary.to_string()),
            None,
            "missing",
            Some(ProbeErrorKind::NotFound),
            Some(format!("{binary} not on PATH")),
        );
    };
    if let Some(probe) = descriptor.version_probe.as_ref()
        && !probe.args.is_empty()
    {
        match Command::new(&location).args(&probe.args).output() {
            Ok(output) => {
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return base_probe_result(
                        descriptor,
                        ProbeStatus::Failed,
                        Some(location.display().to_string()),
                        None,
                        "missing",
                        Some(ProbeErrorKind::ProbeFailed),
                        Some(trim_probe_message(&stderr)),
                    );
                }
                let version = trim_probe_message(&format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ));
                if let Some(expected) = probe.expect_substring.as_deref()
                    && !version.contains(expected)
                {
                    return base_probe_result(
                        descriptor,
                        ProbeStatus::Failed,
                        Some(location.display().to_string()),
                        Some(version),
                        "missing",
                        Some(ProbeErrorKind::VersionMismatch),
                        Some(format!("version output did not contain {expected}")),
                    );
                }
                if let Some(min) = probe.min_known_good.as_deref()
                    && !version_at_least(&version, min)
                {
                    return base_probe_result(
                        descriptor,
                        ProbeStatus::Failed,
                        Some(location.display().to_string()),
                        Some(version),
                        "missing",
                        Some(ProbeErrorKind::VersionMismatch),
                        Some(format!("version below known-good {min}")),
                    );
                }
                return base_probe_result(
                    descriptor,
                    ProbeStatus::Ok,
                    Some(location.display().to_string()),
                    Some(version),
                    "ready",
                    None,
                    None,
                );
            }
            Err(source) if source.kind() == std::io::ErrorKind::PermissionDenied => {
                return base_probe_result(
                    descriptor,
                    ProbeStatus::Failed,
                    Some(location.display().to_string()),
                    None,
                    "missing",
                    Some(ProbeErrorKind::PermissionDenied),
                    Some(format!("permission denied running {binary}")),
                );
            }
            Err(source) => {
                return base_probe_result(
                    descriptor,
                    ProbeStatus::Failed,
                    Some(location.display().to_string()),
                    None,
                    "missing",
                    Some(ProbeErrorKind::ProbeFailed),
                    Some(source.to_string()),
                );
            }
        }
    }
    base_probe_result(
        descriptor,
        ProbeStatus::Ok,
        Some(location.display().to_string()),
        None,
        "ready",
        None,
        None,
    )
}

async fn probe_http_descriptor(
    descriptor: &ProviderDescriptor,
    options: ProviderProbeOptions,
) -> ProviderProbeResult {
    if let AuthKind::ApiKey = descriptor.auth.kind {
        let Some(env_var) = descriptor.auth.env_var.as_deref() else {
            return base_probe_result(
                descriptor,
                ProbeStatus::Failed,
                descriptor.default_endpoint.clone(),
                None,
                "missing",
                Some(ProbeErrorKind::AuthMissing),
                Some("api-key provider has no env var configured".to_string()),
            );
        };
        if std::env::var(env_var)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .is_none()
        {
            return base_probe_result(
                descriptor,
                ProbeStatus::Failed,
                descriptor.default_endpoint.clone(),
                None,
                "missing",
                Some(ProbeErrorKind::AuthMissing),
                Some(format!("{env_var} missing")),
            );
        }
    }
    if options.ping {
        return ping_endpoint(descriptor).await;
    }
    base_probe_result(
        descriptor,
        ProbeStatus::Ok,
        descriptor.default_endpoint.clone(),
        Some("ping skipped".to_string()),
        "ready",
        None,
        None,
    )
}

async fn probe_local_http_descriptor(
    descriptor: &ProviderDescriptor,
    options: ProviderProbeOptions,
) -> ProviderProbeResult {
    if options.ping {
        return ping_endpoint(descriptor).await;
    }
    base_probe_result(
        descriptor,
        ProbeStatus::Skipped,
        descriptor.default_endpoint.clone(),
        Some("ping required".to_string()),
        "unknown",
        None,
        Some("local endpoint probe skipped; pass --ping".to_string()),
    )
}

async fn ping_endpoint(descriptor: &ProviderDescriptor) -> ProviderProbeResult {
    let endpoint = descriptor.default_endpoint.clone().unwrap_or_default();
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(client) => client,
        Err(source) => {
            return base_probe_result(
                descriptor,
                ProbeStatus::Failed,
                Some(endpoint),
                None,
                "missing",
                Some(ProbeErrorKind::ProbeFailed),
                Some(source.to_string()),
            );
        }
    };
    match client.get(&endpoint).send().await {
        Ok(response) => base_probe_result(
            descriptor,
            ProbeStatus::Ok,
            Some(endpoint),
            Some(format!("HTTP {}", response.status())),
            "ready",
            None,
            None,
        ),
        Err(source) => base_probe_result(
            descriptor,
            ProbeStatus::Failed,
            Some(endpoint),
            None,
            "missing",
            Some(ProbeErrorKind::EndpointUnreachable),
            Some(source.to_string()),
        ),
    }
}

fn base_probe_result(
    descriptor: &ProviderDescriptor,
    status: ProbeStatus,
    location: Option<String>,
    version: Option<String>,
    credential: &str,
    error_kind: Option<ProbeErrorKind>,
    message: Option<String>,
) -> ProviderProbeResult {
    ProviderProbeResult {
        id: descriptor.id.clone(),
        display_name: descriptor.display_name.clone(),
        kind: descriptor.kind.clone(),
        status,
        location,
        version,
        credential: credential.to_string(),
        subscription: descriptor.subscription,
        metering: if descriptor.subscription {
            "subscription".to_string()
        } else {
            "metered".to_string()
        },
        fs_artifacts: descriptor
            .fs_detection_paths
            .iter()
            .map(|path| expand_home_path(path).display().to_string())
            .collect(),
        error_kind,
        message,
        try_lines: descriptor.install_hint.try_lines.clone(),
    }
}

fn resolve_binary(binary: &str) -> Option<PathBuf> {
    let configured = PathBuf::from(binary);
    if configured.components().count() > 1 || configured.is_absolute() {
        return configured.exists().then_some(configured);
    }
    which::which(binary).ok()
}

fn expand_home_path(path: &Path) -> PathBuf {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return path.to_path_buf();
    };
    if path == Path::new("~") {
        return home;
    }
    if let Ok(rest) = path.strip_prefix("~") {
        return home.join(rest);
    }
    path.to_path_buf()
}

fn trim_probe_message(message: &str) -> String {
    let first_line = message
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let trimmed = first_line.trim();
    if trimmed.len() > 120 {
        format!("{}...", &trimmed[..120])
    } else {
        trimmed.to_string()
    }
}

fn version_at_least(output: &str, minimum: &str) -> bool {
    let observed = first_version_tuple(output);
    let required = first_version_tuple(minimum);
    if observed.is_empty() || required.is_empty() {
        return output >= minimum;
    }
    compare_version_tuples(&observed, &required) != std::cmp::Ordering::Less
}

fn first_version_tuple(text: &str) -> Vec<u64> {
    text.split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .find(|part| part.chars().any(|ch| ch.is_ascii_digit()))
        .unwrap_or("")
        .split('.')
        .filter_map(|part| part.parse::<u64>().ok())
        .collect()
}

fn compare_version_tuples(left: &[u64], right: &[u64]) -> std::cmp::Ordering {
    let len = left.len().max(right.len());
    for index in 0..len {
        let l = left.get(index).copied().unwrap_or(0);
        let r = right.get(index).copied().unwrap_or(0);
        match l.cmp(&r) {
            std::cmp::Ordering::Equal => {}
            ordering => return ordering,
        }
    }
    std::cmp::Ordering::Equal
}

fn split_command_line(command: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None::<char>;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            ' ' | '\t' | '\n' => {
                if !current.is_empty() {
                    args.push(current.clone());
                    current.clear();
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

fn parse_toml_value(raw: &str, path: &Path) -> Result<Value> {
    toml::from_str(raw).map_err(|source| ProviderError::Toml {
        path: path.display().to_string(),
        source,
    })
}

fn descriptor_id_from_value(value: &Value) -> Option<String> {
    value
        .get("id")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn id_from_path(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(ToString::to_string)
}

fn ensure_id(value: &mut Value, id: &str) -> Result<()> {
    let Some(table) = value.as_table_mut() else {
        return Err(ProviderError::InvalidConfig(
            "provider override must be a TOML table".to_string(),
        ));
    };
    table
        .entry("id".to_string())
        .or_insert_with(|| Value::String(id.to_string()));
    Ok(())
}

fn descriptor_to_value(descriptor: &ProviderDescriptor) -> Result<Value> {
    Value::try_from(descriptor)
        .map_err(|err| ProviderError::InvalidConfig(format!("serialize descriptor: {err}")))
}

fn value_to_descriptor(value: Value, path: &Path) -> Result<ProviderDescriptor> {
    value.try_into().map_err(|source| ProviderError::Toml {
        path: path.display().to_string(),
        source,
    })
}

fn merge_values(base: &mut Value, override_value: Value) {
    match (base, override_value) {
        (Value::Table(base_table), Value::Table(override_table)) => {
            for (key, value) in override_table {
                match base_table.get_mut(&key) {
                    Some(base_value) => merge_values(base_value, value),
                    None => {
                        base_table.insert(key, value);
                    }
                }
            }
        }
        (base_value, override_value) => *base_value = override_value,
    }
}

#[cfg(test)]
mod model_catalog_tests {
    use super::ProviderRegistry;

    #[test]
    fn every_builtin_descriptor_has_a_populated_model_catalog() {
        let registry = ProviderRegistry::builtin().expect("builtin registry");
        for descriptor in registry.iter() {
            if descriptor.id == "smoke" {
                continue; // the scripted test provider has no model space
            }
            assert!(
                descriptor.model_catalog.len() >= 2,
                "{} must catalog real models beyond 'provider default' (has {})",
                descriptor.id,
                descriptor.model_catalog.len()
            );
        }
    }

    #[test]
    fn every_builtin_descriptor_has_exactly_one_recommended_model() {
        let registry = ProviderRegistry::builtin().expect("builtin registry");
        for descriptor in registry.iter() {
            if descriptor.id == "smoke" {
                continue;
            }
            let recommended = descriptor
                .model_catalog
                .iter()
                .filter(|entry| entry.recommended)
                .count();
            assert_eq!(
                recommended, 1,
                "{} must mark exactly one recommended model",
                descriptor.id
            );
        }
    }

    #[test]
    fn model_entry_without_recommended_field_still_parses() {
        // Pre-rider descriptor TOML omits `recommended`; it must default off.
        let raw = r#"
id = "fixture"
display_name = "Fixture"
kind = "cli"
default_binary = "fixture"

[[model_catalog]]
id = "provider default"
context_window = 1000
"#;
        let descriptor = super::parse_descriptor(raw, "fixture-test").expect("parse");
        assert_eq!(descriptor.model_catalog.len(), 1);
        assert!(!descriptor.model_catalog[0].recommended);
    }
}
