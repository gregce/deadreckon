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

impl ProviderError {
    /// Transient — the operation may succeed on a retry.
    pub fn is_retryable(&self) -> bool {
        match self {
            ProviderError::Io { source, .. } => is_retryable_io_kind(source.kind()),
            ProviderError::Toml { .. } => false,
            ProviderError::MissingCredential(_) => false,
            ProviderError::NoRoute(_) => false,
            ProviderError::Http { .. } => false,
            ProviderError::Cli { .. } => false,
            ProviderError::InvalidConfig(_) => false,
        }
    }

    /// Unrecoverable — the watchdog should escalate, not retry.
    pub fn is_fatal(&self) -> bool {
        match self {
            ProviderError::Io { source, .. } => !is_retryable_io_kind(source.kind()),
            ProviderError::Toml { .. } => true,
            ProviderError::MissingCredential(_) => true,
            ProviderError::NoRoute(_) => true,
            ProviderError::Http { .. } => true,
            ProviderError::Cli { .. } => true,
            ProviderError::InvalidConfig(_) => true,
        }
    }
}

fn is_retryable_io_kind(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
    )
}

pub type Result<T> = std::result::Result<T, ProviderError>;
