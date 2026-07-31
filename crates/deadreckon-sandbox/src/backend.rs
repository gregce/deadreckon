use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use deadreckon_core::is_retryable_io_kind;
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("invalid sandbox backend: {0}")]
    InvalidBackend(String),
    #[error("sandbox backend {0} is unavailable")]
    Unavailable(String),
    #[error("sandbox backend {0} cannot enforce read-only workspace access")]
    ReadOnlyUnavailable(String),
    #[error("invalid Docker execution: {0}")]
    InvalidDockerExecution(String),
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
            SandboxError::ReadOnlyUnavailable(_) => false,
            SandboxError::InvalidDockerExecution(_) => false,
            SandboxError::Io(source) => is_retryable_io_kind(source.kind()),
            SandboxError::Cancelled => false,
        }
    }

    /// Unrecoverable — the watchdog should escalate, not retry.
    pub fn is_fatal(&self) -> bool {
        match self {
            SandboxError::InvalidBackend(_) => true,
            SandboxError::Unavailable(_) => true,
            SandboxError::ReadOnlyUnavailable(_) => true,
            SandboxError::InvalidDockerExecution(_) => true,
            SandboxError::Io(source) => !is_retryable_io_kind(source.kind()),
            SandboxError::Cancelled => true,
        }
    }
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
                if backend_executable(SandboxBackend::SandboxExec).is_ok() {
                    return Ok((SandboxBackend::SandboxExec, None));
                }
                Ok((
                    SandboxBackend::None,
                    Some("sandbox-exec not found; auto fell back to none".to_string()),
                ))
            }
            #[cfg(target_os = "linux")]
            {
                if backend_executable(SandboxBackend::Bwrap).is_ok() {
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
        SandboxBackend::SandboxExec | SandboxBackend::Bwrap | SandboxBackend::Docker => {
            backend_executable(backend)?;
            Ok((backend, None))
        }
        SandboxBackend::None => Ok((SandboxBackend::None, None)),
    }
}

/// Resolve a sandbox wrapper independently of ambient `PATH`.
///
/// The wrapper is part of the containment claim, so a repository- or
/// direnv-provided shim must never be accepted merely because `which` found it.
pub(crate) fn backend_executable(backend: SandboxBackend) -> Result<PathBuf> {
    let candidates = backend_candidates(backend);
    candidates
        .iter()
        .map(Path::new)
        .find_map(trusted_executable)
        .ok_or_else(|| SandboxError::Unavailable(backend.to_string()))
}

fn trusted_executable(candidate: &Path) -> Option<PathBuf> {
    if !candidate.is_absolute() {
        return None;
    }
    let canonical = candidate.canonicalize().ok()?;
    let metadata = canonical.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        if metadata.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    Some(canonical)
}

#[cfg(target_os = "macos")]
fn backend_candidates(backend: SandboxBackend) -> &'static [&'static str] {
    match backend {
        SandboxBackend::SandboxExec => &["/usr/bin/sandbox-exec"],
        SandboxBackend::Docker => &[
            "/usr/local/bin/docker",
            "/opt/homebrew/bin/docker",
            "/Applications/Docker.app/Contents/Resources/bin/docker",
        ],
        SandboxBackend::Bwrap | SandboxBackend::Auto | SandboxBackend::None => &[],
    }
}

#[cfg(target_os = "linux")]
fn backend_candidates(backend: SandboxBackend) -> &'static [&'static str] {
    match backend {
        SandboxBackend::Bwrap => &["/usr/bin/bwrap", "/bin/bwrap"],
        SandboxBackend::Docker => &["/usr/bin/docker", "/bin/docker", "/usr/local/bin/docker"],
        SandboxBackend::SandboxExec | SandboxBackend::Auto | SandboxBackend::None => &[],
    }
}

#[cfg(target_os = "windows")]
fn backend_candidates(backend: SandboxBackend) -> &'static [&'static str] {
    match backend {
        SandboxBackend::Docker => &[r"C:\Program Files\Docker\Docker\resources\bin\docker.exe"],
        SandboxBackend::SandboxExec
        | SandboxBackend::Bwrap
        | SandboxBackend::Auto
        | SandboxBackend::None => &[],
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn backend_candidates(_backend: SandboxBackend) -> &'static [&'static str] {
    &[]
}
