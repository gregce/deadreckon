use deadreckon_core::is_retryable_io_kind;

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
    Http {
        provider: String,
        detail: String,
        /// Set at construction where the failure kind is known: transport
        /// blips and 408/429/5xx are retryable; cancellations, auth failures,
        /// and malformed responses are not.
        retryable: bool,
    },
    #[error("CLI provider error for {provider}: {detail}")]
    Cli { provider: String, detail: String },
    #[error("invalid provider configuration: {0}")]
    InvalidConfig(String),
    #[error("invalid strict output schema for {provider} at {path}: {detail}")]
    InvalidOutputSchema {
        provider: String,
        path: String,
        detail: String,
    },
}

impl ProviderError {
    /// Transient — the operation may succeed on a retry.
    pub fn is_retryable(&self) -> bool {
        match self {
            ProviderError::Io { source, .. } => is_retryable_io_kind(source.kind()),
            ProviderError::Toml { .. } => false,
            ProviderError::MissingCredential(_) => false,
            ProviderError::NoRoute(_) => false,
            ProviderError::Http { retryable, .. } => *retryable,
            // CLI providers only report an exit code and output; recognize the
            // well-known transient phrasings and treat everything else as
            // final (a wrong retry costs a full provider turn).
            ProviderError::Cli { detail, .. } => {
                let lower = detail.to_ascii_lowercase();
                [
                    "rate limit",
                    "rate_limit",
                    "too many requests",
                    "overloaded",
                    "temporarily unavailable",
                    "connection reset",
                    "timed out",
                ]
                .iter()
                .any(|marker| lower.contains(marker))
            }
            ProviderError::InvalidConfig(_) => false,
            ProviderError::InvalidOutputSchema { .. } => false,
        }
    }

    /// Unrecoverable — the watchdog should escalate, not retry.
    pub fn is_fatal(&self) -> bool {
        !self.is_retryable()
    }
}

pub type Result<T> = std::result::Result<T, ProviderError>;

#[cfg(test)]
mod tests {
    use super::*;

    fn http(detail: &str, retryable: bool) -> ProviderError {
        ProviderError::Http {
            provider: "test".to_string(),
            detail: detail.to_string(),
            retryable,
        }
    }

    fn cli(detail: &str) -> ProviderError {
        ProviderError::Cli {
            provider: "test".to_string(),
            detail: detail.to_string(),
        }
    }

    #[test]
    fn http_retryability_follows_construction_site() {
        assert!(http("HTTP 429 Too Many Requests: slow down", true).is_retryable());
        assert!(http("connection reset by peer", true).is_retryable());
        assert!(!http("HTTP 401 Unauthorized: bad key", false).is_retryable());
        assert!(!http("request cancelled", false).is_retryable());
    }

    #[test]
    fn cli_transient_phrasings_are_retryable_everything_else_final() {
        assert!(cli("subprocess exited with 1: API Error: rate limit reached").is_retryable());
        assert!(cli("Overloaded — please retry shortly").is_retryable());
        assert!(cli("request timed out after 60s").is_retryable());
        assert!(!cli("subprocess exited with 1: invalid api key").is_retryable());
        assert!(!cli("unknown flag --frobnicate").is_retryable());
    }

    #[test]
    fn config_and_credential_errors_are_fatal() {
        for err in [
            ProviderError::MissingCredential("anthropic".to_string()),
            ProviderError::NoRoute("all failed".to_string()),
            ProviderError::InvalidConfig("bad".to_string()),
            ProviderError::InvalidOutputSchema {
                provider: "cli:codex".to_string(),
                path: "$.files".to_string(),
                detail: "dynamic object keys are unsupported".to_string(),
            },
        ] {
            assert!(!err.is_retryable());
            assert!(err.is_fatal());
        }
    }
}
