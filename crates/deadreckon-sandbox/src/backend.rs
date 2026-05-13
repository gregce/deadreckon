use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use which::which;

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("invalid sandbox backend: {0}")]
    InvalidBackend(String),
    #[error("sandbox backend {0} is unavailable")]
    Unavailable(String),
    #[error("I/O error while running sandbox command: {0}")]
    Io(#[from] std::io::Error),
    #[error("sandboxed command cancelled")]
    Cancelled,
}

impl SandboxError {
    /// Transient — the operation may succeed on a retry.
    pub fn is_retryable(&self) -> bool {
        match self {
            SandboxError::InvalidBackend(_) => false,
            SandboxError::Unavailable(_) => false,
            SandboxError::Io(source) => is_retryable_io_kind(source.kind()),
            SandboxError::Cancelled => false,
        }
    }

    /// Unrecoverable — the watchdog should escalate, not retry.
    pub fn is_fatal(&self) -> bool {
        match self {
            SandboxError::InvalidBackend(_) => true,
            SandboxError::Unavailable(_) => true,
            SandboxError::Io(source) => !is_retryable_io_kind(source.kind()),
            SandboxError::Cancelled => true,
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

pub type Result<T> = std::result::Result<T, SandboxError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxBackend {
    Auto,
    SandboxExec,
    Bwrap,
    Docker,
    None,
}

impl fmt::Display for SandboxBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            SandboxBackend::Auto => "auto",
            SandboxBackend::SandboxExec => "sandbox-exec",
            SandboxBackend::Bwrap => "bwrap",
            SandboxBackend::Docker => "docker",
            SandboxBackend::None => "none",
        };
        f.write_str(value)
    }
}

impl FromStr for SandboxBackend {
    type Err = SandboxError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "sandbox-exec" => Ok(Self::SandboxExec),
            "bwrap" | "bubblewrap" => Ok(Self::Bwrap),
            "docker" => Ok(Self::Docker),
            "none" => Ok(Self::None),
            other => Err(SandboxError::InvalidBackend(other.to_string())),
        }
    }
}

pub fn resolve_backend(backend: SandboxBackend) -> Result<(SandboxBackend, Option<String>)> {
    match backend {
        SandboxBackend::Auto => {
            #[cfg(target_os = "macos")]
            {
                if which("sandbox-exec").is_ok() {
                    return Ok((SandboxBackend::SandboxExec, None));
                }
                Ok((
                    SandboxBackend::None,
                    Some("sandbox-exec not found; auto fell back to none".to_string()),
                ))
            }
            #[cfg(target_os = "linux")]
            {
                if which("bwrap").is_ok() {
                    return Ok((SandboxBackend::Bwrap, None));
                }
                Ok((
                    SandboxBackend::None,
                    Some("bwrap not found; auto fell back to none".to_string()),
                ))
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                Ok((
                    SandboxBackend::None,
                    Some("no native sandbox backend for this platform; using none".to_string()),
                ))
            }
        }
        SandboxBackend::SandboxExec => require_binary("sandbox-exec", SandboxBackend::SandboxExec),
        SandboxBackend::Bwrap => require_binary("bwrap", SandboxBackend::Bwrap),
        SandboxBackend::Docker => require_binary("docker", SandboxBackend::Docker),
        SandboxBackend::None => Ok((SandboxBackend::None, None)),
    }
}

fn require_binary(name: &str, backend: SandboxBackend) -> Result<(SandboxBackend, Option<String>)> {
    if which(name).is_ok() {
        Ok((backend, None))
    } else {
        Err(SandboxError::Unavailable(backend.to_string()))
    }
}
