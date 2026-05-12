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
