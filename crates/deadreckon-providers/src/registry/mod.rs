use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct InstallHint {
    pub url: String,
    pub try_lines: Vec<String>,
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
}

#[derive(Debug, Clone, Default)]
pub struct ProviderRegistry {
    descriptors: BTreeMap<String, ProviderDescriptor>,
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
    toml::from_str(raw).map_err(|source| ProviderError::Toml {
        path: label.to_string(),
        source,
    })
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
