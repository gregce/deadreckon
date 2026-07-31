use std::path::PathBuf;

use crate::backend::{SandboxBackend, backend_executable};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendAvailability {
    pub backend: SandboxBackend,
    pub available: bool,
    pub path: Option<PathBuf>,
    pub note: String,
}

pub fn doctor() -> Vec<BackendAvailability> {
    [
        SandboxBackend::SandboxExec,
        SandboxBackend::Bwrap,
        SandboxBackend::Docker,
    ]
    .into_iter()
    .map(|backend| match backend_executable(backend) {
        Ok(path) => BackendAvailability {
            backend,
            available: true,
            path: Some(path),
            note: "available".to_string(),
        },
        Err(_) => BackendAvailability {
            backend,
            available: false,
            path: None,
            note: missing_hint(backend),
        },
    })
    .chain(std::iter::once(BackendAvailability {
        backend: SandboxBackend::None,
        available: true,
        path: None,
        note: "available but unsafe; use only when explicitly requested".to_string(),
    }))
    .collect()
}

fn missing_hint(backend: SandboxBackend) -> String {
    match backend {
        SandboxBackend::SandboxExec => {
            "macOS sandbox-exec is unavailable at its trusted system path".to_string()
        }
        SandboxBackend::Bwrap => {
            "install bubblewrap (bwrap) for Linux native sandboxing".to_string()
        }
        SandboxBackend::Docker => "install Docker to use --sandbox docker".to_string(),
        SandboxBackend::Auto | SandboxBackend::None => "not applicable".to_string(),
    }
}
