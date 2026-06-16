//! Backend selection for the live run narrator.
//!
//! The narrator wants a cheap, fast model and must never fail a run when no
//! provider is available. Selection is subscription-first: prefer an installed,
//! logged-in CLI (whose narration is free under the user's subscription), then
//! a cheap direct-API model, then a deterministic template floor that needs no
//! provider at all.
//!
//! The core [`select_narrator_route`] is pure over an availability predicate so
//! it is exhaustively testable without real CLIs or keys;
//! [`narrator_route_available`] is the production predicate that wires binary
//! presence + [`probe_cli_auth`] for CLIs and an API-key-env check for HTTP.

use std::path::PathBuf;

use crate::auth_probe::{CliAuthStatus, probe_cli_auth};
use crate::registry::{AuthKind, DescriptorKind, ProviderRegistry};

/// One step in the subscription-first preference order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NarratorCandidate {
    /// Provider route id, matching the descriptor id and router route name.
    pub provider: &'static str,
    /// Cheap model id for that provider.
    pub model: &'static str,
}

/// Subscription-first preference order: free CLIs, then cheap API models.
pub const NARRATOR_PREFERENCE: [NarratorCandidate; 4] = [
    NarratorCandidate {
        provider: "cli:claude-code",
        model: "haiku",
    },
    NarratorCandidate {
        provider: "cli:codex",
        model: "gpt-5.1-codex-mini",
    },
    NarratorCandidate {
        provider: "anthropic",
        model: "claude-haiku-4-5",
    },
    NarratorCandidate {
        provider: "openai",
        model: "gpt-4o-mini",
    },
];

/// The chosen narrator backend, or the no-provider deterministic floor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NarratorBackend {
    /// Route a cheap model through the provider router.
    Model { provider: String, model: String },
    /// No provider available — narrate via the deterministic template only.
    DeterministicFloor,
}

/// Pick the first available candidate in preference order. A `model_override`
/// replaces the model but never the provider order: the chosen provider is
/// still the first available one, just narrated with the requested model.
pub fn select_narrator_route(
    model_override: Option<&str>,
    is_available: impl Fn(&str) -> bool,
) -> NarratorBackend {
    for candidate in NARRATOR_PREFERENCE {
        if is_available(candidate.provider) {
            let model = model_override.unwrap_or(candidate.model).to_string();
            return NarratorBackend::Model {
                provider: candidate.provider.to_string(),
                model,
            };
        }
    }
    NarratorBackend::DeterministicFloor
}

/// Production availability predicate. CLI routes are available when the binary
/// is present AND the auth probe does not explicitly report logged-out (an
/// `Unknown` probe is treated as available, matching the rest of the codebase).
/// HTTP routes are available when their API-key env var is set and non-empty.
pub fn narrator_route_available(registry: &ProviderRegistry, provider: &str) -> bool {
    let Some(descriptor) = registry.get(provider) else {
        return false;
    };
    match descriptor.kind {
        DescriptorKind::Cli => {
            let Some(binary) = descriptor.default_binary.as_deref() else {
                return false;
            };
            if !binary_present(binary) {
                return false;
            }
            match &descriptor.auth_probe {
                Some(probe) => !matches!(
                    probe_cli_auth(binary, probe),
                    CliAuthStatus::NotLoggedIn { .. }
                ),
                None => true,
            }
        }
        DescriptorKind::Http => match (&descriptor.auth.kind, descriptor.auth.env_var.as_deref()) {
            (AuthKind::ApiKey, Some(var)) => std::env::var(var)
                .map(|value| !value.trim().is_empty())
                .unwrap_or(false),
            _ => false,
        },
        _ => false,
    }
}

fn binary_present(binary: &str) -> bool {
    which::which(binary).is_ok() || PathBuf::from(binary).exists()
}

/// Convenience: select using the production predicate against a registry.
pub fn select_narrator_backend(
    registry: &ProviderRegistry,
    model_override: Option<&str>,
) -> NarratorBackend {
    select_narrator_route(model_override, |provider| {
        narrator_route_available(registry, provider)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn only<'a>(available: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |provider: &str| available.contains(&provider)
    }

    #[test]
    fn narrator_prefers_claude_code_haiku_when_logged_in() {
        let backend = select_narrator_route(None, only(&["cli:claude-code", "anthropic"]));
        assert_eq!(
            backend,
            NarratorBackend::Model {
                provider: "cli:claude-code".to_string(),
                model: "haiku".to_string(),
            }
        );
    }

    #[test]
    fn narrator_falls_through_logged_out_cli_to_next_candidate() {
        // claude-code logged out (unavailable) → codex is the next free CLI.
        let backend = select_narrator_route(None, only(&["cli:codex", "anthropic", "openai"]));
        assert_eq!(
            backend,
            NarratorBackend::Model {
                provider: "cli:codex".to_string(),
                model: "gpt-5.1-codex-mini".to_string(),
            }
        );
    }

    #[test]
    fn narrator_uses_anthropic_haiku_when_no_cli_but_api_key() {
        let backend = select_narrator_route(None, only(&["anthropic", "openai"]));
        assert_eq!(
            backend,
            NarratorBackend::Model {
                provider: "anthropic".to_string(),
                model: "claude-haiku-4-5".to_string(),
            }
        );
    }

    #[test]
    fn narrator_returns_deterministic_floor_when_nothing_available() {
        let backend = select_narrator_route(None, only(&[]));
        assert_eq!(backend, NarratorBackend::DeterministicFloor);
    }

    #[test]
    fn narrator_model_override_keeps_provider_order() {
        // Override changes the model, not which provider wins: claude-code is
        // still first-available, now narrated with the overridden model.
        let backend = select_narrator_route(Some("opus"), only(&["cli:claude-code", "cli:codex"]));
        assert_eq!(
            backend,
            NarratorBackend::Model {
                provider: "cli:claude-code".to_string(),
                model: "opus".to_string(),
            }
        );
    }
}
