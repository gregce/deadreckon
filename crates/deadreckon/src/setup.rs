//! Shared setup contracts for provider selection and done criteria.
//!
//! This module is deliberately runtime-only: it standardizes how command
//! surfaces describe provider roles and done-criteria sources without adding
//! durable run, plan, or config fields.

use std::path::{Path, PathBuf};

use deadreckon_providers::registry::{
    AuthKind, DescriptorKind, ProviderDescriptor, ProviderRegistry,
};
use deadreckon_providers::{
    ProviderError, ProviderKind, ProviderRouteInfo, ProviderRouter, read_config,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SetupProviderRole {
    PrimaryRun,
    ConfigDefault,
    DocPolish,
    Planner,
    DefaultChild,
    ChildOverride { index: usize },
    Coder,
    Reviewer,
    Repair,
}

impl SetupProviderRole {
    pub(crate) fn label(&self) -> String {
        match self {
            Self::PrimaryRun => "primary".to_string(),
            Self::ConfigDefault => "default".to_string(),
            Self::DocPolish => "docs".to_string(),
            Self::Planner => "planner".to_string(),
            Self::DefaultChild => "default-child".to_string(),
            Self::ChildOverride { index } => format!("child-{index}"),
            Self::Coder => "coder".to_string(),
            Self::Reviewer => "reviewer".to_string(),
            Self::Repair => "repair".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupProviderSource {
    Flag,
    Config,
    AutoSubscription,
    RunProvider,
    BuiltInDefault,
    None,
}

impl SetupProviderSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Flag => "flag",
            Self::Config => "config",
            Self::AutoSubscription => "auto_subscription",
            Self::RunProvider => "run_provider",
            Self::BuiltInDefault => "built_in_default",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProviderSetupRequest<'a> {
    pub role: SetupProviderRoleRef,
    pub explicit_provider: Option<&'a str>,
    pub explicit_model: Option<&'a str>,
    pub config_default_provider: Option<&'a str>,
    pub config_doc_provider: Option<&'a str>,
    pub run_provider: Option<&'a str>,
    pub auto_subscription_provider: Option<&'a str>,
    pub built_in_default_provider: Option<&'a str>,
    pub use_router_default: bool,
    pub allow_auto_subscription: bool,
    pub require_usable_route: bool,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupProviderRoleRef {
    PrimaryRun,
    ConfigDefault,
    DocPolish,
    Planner,
    DefaultChild,
    ChildOverride(usize),
    Coder,
    Reviewer,
    Repair,
}

impl SetupProviderRoleRef {
    fn owned(self) -> SetupProviderRole {
        match self {
            Self::PrimaryRun => SetupProviderRole::PrimaryRun,
            Self::ConfigDefault => SetupProviderRole::ConfigDefault,
            Self::DocPolish => SetupProviderRole::DocPolish,
            Self::Planner => SetupProviderRole::Planner,
            Self::DefaultChild => SetupProviderRole::DefaultChild,
            Self::ChildOverride(index) => SetupProviderRole::ChildOverride { index },
            Self::Coder => SetupProviderRole::Coder,
            Self::Reviewer => SetupProviderRole::Reviewer,
            Self::Repair => SetupProviderRole::Repair,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProviderSetupSelection {
    pub role: SetupProviderRole,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub source: SetupProviderSource,
    pub kind: Option<String>,
    pub credential: Option<String>,
    pub install_hint: Option<String>,
    pub warnings: Vec<String>,
    pub try_lines: Vec<String>,
}

impl ProviderSetupSelection {
    pub(crate) fn none(role: SetupProviderRole) -> Self {
        Self {
            role,
            provider: None,
            model: None,
            source: SetupProviderSource::None,
            kind: None,
            credential: None,
            install_hint: None,
            warnings: Vec::new(),
            try_lines: Vec::new(),
        }
    }

    pub(crate) fn row_value(&self) -> String {
        let provider = self.provider.as_deref().unwrap_or("-");
        let model = self.model.as_deref().unwrap_or("provider default");
        let credential = self.credential.as_deref().unwrap_or("-");
        format!(
            "{provider} model={model} source={} credential={credential}",
            self.source.as_str()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SetupRefusal {
    pub message: String,
    pub try_line: String,
}

impl SetupRefusal {
    fn unknown_provider(provider: &str) -> Self {
        Self {
            message: format!("unknown provider route {provider}"),
            try_line: "deadreckon providers list --all".to_string(),
        }
    }
}

pub(crate) fn auto_subscription_cli_provider(registry: &ProviderRegistry) -> Option<String> {
    let providers = registry
        .iter()
        .filter(|descriptor| {
            descriptor.kind == DescriptorKind::Cli
                && descriptor.subscription
                && descriptor
                    .default_binary
                    .as_deref()
                    .is_some_and(crate::command_exists)
        })
        .map(|descriptor| descriptor.id.clone())
        .collect::<Vec<_>>();
    if providers.len() == 1 {
        providers.into_iter().next()
    } else {
        None
    }
}

pub(crate) fn select_provider_setup(
    config_path: &Path,
    registry: &ProviderRegistry,
    request: ProviderSetupRequest<'_>,
) -> Result<ProviderSetupSelection, SetupRefusal> {
    let role = request.role.owned();
    let (provider, source) = provider_candidate(&request);
    let Some(provider) = provider else {
        if request.use_router_default {
            return selection_for_route(
                config_path,
                registry,
                role,
                None,
                request.explicit_model,
                SetupProviderSource::BuiltInDefault,
                request.require_usable_route,
            );
        }
        let mut selection = ProviderSetupSelection::none(role);
        selection
            .try_lines
            .push("deadreckon config provider cli:codex".to_string());
        return Ok(selection);
    };
    let provider = provider.trim();
    if provider.is_empty() {
        let mut selection = ProviderSetupSelection::none(role);
        selection
            .try_lines
            .push("deadreckon config provider cli:codex".to_string());
        return Ok(selection);
    }
    if !provider_route_exists(config_path, registry, provider) {
        return Err(SetupRefusal::unknown_provider(provider));
    }

    selection_for_route(
        config_path,
        registry,
        role,
        Some(provider),
        request.explicit_model,
        source,
        request.require_usable_route,
    )
}

fn selection_for_route(
    config_path: &Path,
    registry: &ProviderRegistry,
    role: SetupProviderRole,
    provider: Option<&str>,
    model: Option<&str>,
    source: SetupProviderSource,
    require_usable_route: bool,
) -> Result<ProviderSetupSelection, SetupRefusal> {
    let route = route_info_for_provider(config_path, provider, model)
        .map_err(|err| setup_refusal_for_provider_error(provider, err))?;
    let descriptor = provider
        .and_then(|provider| registry.get(provider))
        .or_else(|| {
            route
                .as_ref()
                .and_then(|route| registry.get(route.name.as_str()))
        });
    let mut selection = ProviderSetupSelection {
        role,
        provider: Some(
            route
                .as_ref()
                .map(|route| route.name.clone())
                .or_else(|| provider.map(ToString::to_string))
                .unwrap_or_default(),
        ),
        model: route
            .as_ref()
            .map(|route| route.model.clone())
            .or_else(|| model.map(ToString::to_string)),
        source,
        kind: descriptor
            .map(|descriptor| descriptor_kind_label(&descriptor.kind).to_string())
            .or_else(|| route.as_ref().map(|route| provider_kind_label(&route.kind))),
        credential: Some(provider_credential_label(descriptor, route.as_ref())),
        install_hint: descriptor.and_then(first_install_hint),
        warnings: Vec::new(),
        try_lines: Vec::new(),
    };
    if route.as_ref().is_some_and(|route| !route.has_credential) {
        if let Some(provider) = selection.provider.as_deref() {
            selection.warnings.push(format!(
                "provider {provider} needs credentials or an installed binary"
            ));
        }
        if let Some(hint) = selection.install_hint.clone() {
            selection.try_lines.push(hint);
        } else if let Some(provider) = selection.provider.as_deref() {
            selection.try_lines.push(format!(
                "deadreckon config set providers.{provider}.api_key <KEY>"
            ));
        }
        if require_usable_route {
            let provider_label = selection
                .provider
                .as_deref()
                .or(provider)
                .unwrap_or("provider");
            return Err(SetupRefusal {
                message: selection
                    .warnings
                    .first()
                    .cloned()
                    .unwrap_or_else(|| format!("provider {provider_label} is not usable")),
                try_line: selection
                    .try_lines
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "deadreckon providers list --all".to_string()),
            });
        }
    }
    Ok(selection)
}

fn provider_candidate<'a>(
    request: &ProviderSetupRequest<'a>,
) -> (Option<&'a str>, SetupProviderSource) {
    if let Some(provider) = non_empty(request.explicit_provider) {
        return (Some(provider), SetupProviderSource::Flag);
    }
    if matches!(request.role, SetupProviderRoleRef::DocPolish)
        && let Some(provider) = non_empty(request.config_doc_provider)
    {
        return (Some(provider), SetupProviderSource::Config);
    }
    if !matches!(request.role, SetupProviderRoleRef::DocPolish)
        && let Some(provider) = non_empty(request.config_default_provider)
    {
        return (Some(provider), SetupProviderSource::Config);
    }
    if request.allow_auto_subscription
        && let Some(provider) = non_empty(request.auto_subscription_provider)
    {
        return (Some(provider), SetupProviderSource::AutoSubscription);
    }
    if let Some(provider) = non_empty(request.run_provider) {
        return (Some(provider), SetupProviderSource::RunProvider);
    }
    if let Some(provider) = non_empty(request.built_in_default_provider) {
        return (Some(provider), SetupProviderSource::BuiltInDefault);
    }
    (None, SetupProviderSource::None)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn provider_route_exists(config_path: &Path, registry: &ProviderRegistry, provider: &str) -> bool {
    registry.get(provider).is_some()
        || read_config(config_path)
            .ok()
            .is_some_and(|config| config.providers.contains_key(provider))
}

fn route_info_for_provider(
    config_path: &Path,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<Option<ProviderRouteInfo>, ProviderError> {
    let router = ProviderRouter::from_config_path_with_model(config_path, provider, model)?;
    Ok(router.selected_route_info())
}

fn setup_refusal_for_provider_error(provider: Option<&str>, error: ProviderError) -> SetupRefusal {
    match error {
        ProviderError::InvalidConfig(message) if message.contains("unknown provider route") => {
            match provider {
                Some(provider) => SetupRefusal::unknown_provider(provider),
                None => SetupRefusal {
                    message,
                    try_line: "deadreckon providers list --all".to_string(),
                },
            }
        }
        other => SetupRefusal {
            message: other.to_string(),
            try_line: "deadreckon providers list --all".to_string(),
        },
    }
}

fn provider_credential_label(
    descriptor: Option<&ProviderDescriptor>,
    route: Option<&ProviderRouteInfo>,
) -> String {
    let has_credential = route.is_some_and(|route| route.has_credential);
    match descriptor {
        Some(descriptor) if descriptor.subscription && has_credential => "subscription",
        Some(descriptor) if descriptor.subscription => "missing",
        Some(descriptor) if descriptor.auth.kind == AuthKind::None && has_credential => "none",
        Some(_) if has_credential => "ready",
        Some(_) => "missing",
        None if has_credential => "ready",
        None => "missing",
    }
    .to_string()
}

fn first_install_hint(descriptor: &ProviderDescriptor) -> Option<String> {
    descriptor.install_hint.try_lines.first().cloned()
}

fn descriptor_kind_label(kind: &DescriptorKind) -> &'static str {
    match kind {
        DescriptorKind::Http => "http",
        DescriptorKind::Cli => "cli",
        DescriptorKind::LocalHttp => "local-http",
        DescriptorKind::Scripted => "scripted",
    }
}

fn provider_kind_label(kind: &ProviderKind) -> String {
    match kind {
        ProviderKind::Anthropic => "anthropic".to_string(),
        ProviderKind::OpenAi => "openai".to_string(),
        ProviderKind::OpenAiCompatible => "openai-compatible".to_string(),
        ProviderKind::CliClaudeCode => "cli".to_string(),
        ProviderKind::CliCodex => "cli".to_string(),
        ProviderKind::ScriptedSmoke => "scripted".to_string(),
        ProviderKind::Generic(id) => id.clone(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DoneCriteriaSource {
    ExplicitPath,
    ProjectFile,
    Generated,
    DefaultGate,
}

impl DoneCriteriaSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitPath => "explicit",
            Self::ProjectFile => "project",
            Self::Generated => "generated",
            Self::DefaultGate => "default",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DoneCriteriaSelection {
    pub source: DoneCriteriaSource,
    pub path: Option<PathBuf>,
    pub companion_doc: Option<PathBuf>,
    pub checks: Option<usize>,
    pub label: String,
    pub technical_label: String,
    pub try_lines: Vec<String>,
}

impl DoneCriteriaSelection {
    pub(crate) fn explicit(
        path: PathBuf,
        companion_doc: Option<PathBuf>,
        checks: Option<usize>,
    ) -> Self {
        Self::with_path(
            DoneCriteriaSource::ExplicitPath,
            path,
            companion_doc,
            checks,
        )
    }

    pub(crate) fn project(
        path: PathBuf,
        companion_doc: Option<PathBuf>,
        checks: Option<usize>,
    ) -> Self {
        Self::with_path(DoneCriteriaSource::ProjectFile, path, companion_doc, checks)
    }

    pub(crate) fn generated(
        path: PathBuf,
        companion_doc: Option<PathBuf>,
        checks: Option<usize>,
    ) -> Self {
        Self::with_path(DoneCriteriaSource::Generated, path, companion_doc, checks)
    }

    pub(crate) fn default_gate() -> Self {
        Self {
            source: DoneCriteriaSource::DefaultGate,
            path: None,
            companion_doc: None,
            checks: None,
            label: "default dr-gate behavior".to_string(),
            technical_label: "default dr-gate behavior".to_string(),
            try_lines: vec!["deadreckon def-done \"what should count as done\"".to_string()],
        }
    }

    pub(crate) fn full_label(&self) -> String {
        let mut label = self.label.clone();
        if let Some(path) = self.path.as_ref() {
            label.push_str(&format!(" from {}", path.display()));
        }
        label
    }

    pub(crate) fn brief_label(&self) -> String {
        match self.checks {
            Some(checks) => format!("{}:{checks}", self.source.as_str()),
            None => self.source.as_str().to_string(),
        }
    }

    fn with_path(
        source: DoneCriteriaSource,
        path: PathBuf,
        companion_doc: Option<PathBuf>,
        checks: Option<usize>,
    ) -> Self {
        let label = match checks {
            Some(checks) => format!("{} ({checks} checks)", source.as_str()),
            None => source.as_str().to_string(),
        };
        Self {
            source,
            path: Some(path),
            companion_doc,
            checks,
            label,
            technical_label: ".deadreckon/acceptance.yaml".to_string(),
            try_lines: vec!["deadreckon def-done \"what should count as done\"".to_string()],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin_registry() -> ProviderRegistry {
        ProviderRegistry::builtin().expect("registry")
    }

    #[test]
    fn provider_setup_prefers_flag_over_config() {
        let temp = tempfile::TempDir::new().expect("temp");
        let registry = builtin_registry();
        let selection = select_provider_setup(
            &temp.path().join("config.toml"),
            &registry,
            ProviderSetupRequest {
                role: SetupProviderRoleRef::PrimaryRun,
                explicit_provider: Some("openai"),
                explicit_model: Some("gpt-test"),
                config_default_provider: Some("anthropic"),
                config_doc_provider: None,
                run_provider: None,
                auto_subscription_provider: None,
                built_in_default_provider: None,
                use_router_default: false,
                allow_auto_subscription: false,
                require_usable_route: false,
            },
        )
        .expect("selection");

        assert_eq!(selection.provider.as_deref(), Some("openai"));
        assert_eq!(selection.model.as_deref(), Some("gpt-test"));
        assert_eq!(selection.source, SetupProviderSource::Flag);
    }

    #[test]
    fn provider_setup_uses_config_default_for_primary_run() {
        let temp = tempfile::TempDir::new().expect("temp");
        let registry = builtin_registry();
        let selection = select_provider_setup(
            &temp.path().join("config.toml"),
            &registry,
            ProviderSetupRequest {
                role: SetupProviderRoleRef::PrimaryRun,
                explicit_provider: None,
                explicit_model: None,
                config_default_provider: Some("anthropic"),
                config_doc_provider: None,
                run_provider: None,
                auto_subscription_provider: None,
                built_in_default_provider: None,
                use_router_default: false,
                allow_auto_subscription: false,
                require_usable_route: false,
            },
        )
        .expect("selection");

        assert_eq!(selection.provider.as_deref(), Some("anthropic"));
        assert_eq!(selection.source, SetupProviderSource::Config);
    }

    #[test]
    fn provider_setup_doc_polish_preserves_source_order() {
        let temp = tempfile::TempDir::new().expect("temp");
        let registry = builtin_registry();
        let selection = select_provider_setup(
            &temp.path().join("config.toml"),
            &registry,
            ProviderSetupRequest {
                role: SetupProviderRoleRef::DocPolish,
                explicit_provider: None,
                explicit_model: None,
                config_default_provider: Some("anthropic"),
                config_doc_provider: Some("cli:codex"),
                run_provider: Some("openai"),
                auto_subscription_provider: Some("cli:claude-code"),
                built_in_default_provider: None,
                use_router_default: false,
                allow_auto_subscription: true,
                require_usable_route: false,
            },
        )
        .expect("selection");

        assert_eq!(selection.provider.as_deref(), Some("cli:codex"));
        assert_eq!(selection.source, SetupProviderSource::Config);
    }

    #[test]
    fn provider_setup_doc_polish_skips_primary_default_before_run_provider() {
        let temp = tempfile::TempDir::new().expect("temp");
        let registry = builtin_registry();
        let selection = select_provider_setup(
            &temp.path().join("config.toml"),
            &registry,
            ProviderSetupRequest {
                role: SetupProviderRoleRef::DocPolish,
                explicit_provider: None,
                explicit_model: None,
                config_default_provider: Some("anthropic"),
                config_doc_provider: None,
                run_provider: Some("openai"),
                auto_subscription_provider: None,
                built_in_default_provider: None,
                use_router_default: false,
                allow_auto_subscription: true,
                require_usable_route: false,
            },
        )
        .expect("selection");

        assert_eq!(selection.provider.as_deref(), Some("openai"));
        assert_eq!(selection.source, SetupProviderSource::RunProvider);
    }

    #[test]
    fn provider_setup_auto_subscription_uses_detected_cli_descriptor() {
        let temp = tempfile::TempDir::new().expect("temp");
        let registry = builtin_registry();
        let selection = select_provider_setup(
            &temp.path().join("config.toml"),
            &registry,
            ProviderSetupRequest {
                role: SetupProviderRoleRef::PrimaryRun,
                explicit_provider: None,
                explicit_model: None,
                config_default_provider: None,
                config_doc_provider: None,
                run_provider: None,
                auto_subscription_provider: Some("cli:codex"),
                built_in_default_provider: Some("anthropic"),
                use_router_default: false,
                allow_auto_subscription: true,
                require_usable_route: false,
            },
        )
        .expect("selection");

        assert_eq!(selection.provider.as_deref(), Some("cli:codex"));
        assert_eq!(selection.source, SetupProviderSource::AutoSubscription);
    }

    #[test]
    fn provider_setup_router_default_reports_selected_fallback_without_override_candidate() {
        let temp = tempfile::TempDir::new().expect("temp");
        let config_path = temp.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
fallback = ["first", "second"]

[providers.first]
kind = "open-ai-compatible"
base_url = "http://127.0.0.1:65531/v1"
model = "first-model"

[providers.second]
kind = "open-ai-compatible"
base_url = "http://127.0.0.1:65532/v1"
api_key = "test"
model = "second-model"
"#,
        )
        .expect("write config");
        let registry = builtin_registry();
        let selection = select_provider_setup(
            &config_path,
            &registry,
            ProviderSetupRequest {
                role: SetupProviderRoleRef::PrimaryRun,
                explicit_provider: None,
                explicit_model: None,
                config_default_provider: None,
                config_doc_provider: None,
                run_provider: None,
                auto_subscription_provider: None,
                built_in_default_provider: None,
                use_router_default: true,
                allow_auto_subscription: false,
                require_usable_route: false,
            },
        )
        .expect("selection");

        assert_eq!(selection.provider.as_deref(), Some("second"));
        assert_eq!(selection.model.as_deref(), Some("second-model"));
        assert_eq!(selection.source, SetupProviderSource::BuiltInDefault);
        assert_eq!(selection.credential.as_deref(), Some("ready"));
    }

    #[test]
    fn provider_setup_unknown_route_refuses_with_providers_list_try_line() {
        let temp = tempfile::TempDir::new().expect("temp");
        let registry = builtin_registry();
        let refusal = select_provider_setup(
            &temp.path().join("config.toml"),
            &registry,
            ProviderSetupRequest {
                role: SetupProviderRoleRef::PrimaryRun,
                explicit_provider: Some("cli:missing-provider"),
                explicit_model: None,
                config_default_provider: None,
                config_doc_provider: None,
                run_provider: None,
                auto_subscription_provider: None,
                built_in_default_provider: None,
                use_router_default: false,
                allow_auto_subscription: false,
                require_usable_route: false,
            },
        )
        .expect_err("unknown route");

        assert!(refusal.message.contains("unknown provider route"));
        assert_eq!(refusal.try_line, "deadreckon providers list --all");
    }

    #[test]
    fn done_criteria_labels_reserve_acceptance_for_file_paths() {
        let selection = DoneCriteriaSelection::project(
            PathBuf::from(".deadreckon/acceptance.yaml"),
            Some(PathBuf::from(".deadreckon/acceptance.md")),
            Some(3),
        );

        assert!(selection.label.contains("project"));
        assert!(!selection.label.contains("acceptance"));
        assert_eq!(selection.technical_label, ".deadreckon/acceptance.yaml");
        assert_eq!(selection.brief_label(), "project:3");
    }

    #[test]
    fn done_criteria_default_gate_label_is_shared() {
        let selection = DoneCriteriaSelection::default_gate();

        assert_eq!(selection.full_label(), "default dr-gate behavior");
        assert_eq!(selection.brief_label(), "default");
        assert!(
            selection
                .try_lines
                .contains(&"deadreckon def-done \"what should count as done\"".to_string())
        );
    }
}
