use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum DeadreckonError {
    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("JSON error at {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("lock held for {task_key} by run {run_id} in phase {phase}")]
    LockHeld {
        task_key: String,
        run_id: String,
        phase: String,
    },
    #[error("provider error: {0}")]
    Provider(#[from] deadreckon_providers::ProviderError),
    #[error("sandbox error: {0}")]
    Sandbox(#[from] deadreckon_sandbox::SandboxError),
}

pub type Result<T> = std::result::Result<T, DeadreckonError>;

pub(crate) trait IoContext<T> {
    fn with_path(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> IoContext<T> for std::io::Result<T> {
    fn with_path(self, path: impl Into<PathBuf>) -> Result<T> {
        let path = path.into();
        self.map_err(|source| DeadreckonError::Io { path, source })
    }
}

pub(crate) trait JsonContext<T> {
    fn with_json_path(self, path: impl Into<PathBuf>) -> Result<T>;
}

impl<T> JsonContext<T> for serde_json::Result<T> {
    fn with_json_path(self, path: impl Into<PathBuf>) -> Result<T> {
        let path = path.into();
        self.map_err(|source| DeadreckonError::Json { path, source })
    }
}
