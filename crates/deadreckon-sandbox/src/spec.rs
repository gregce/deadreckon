use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use tokio_util::sync::CancellationToken;

use crate::backend::SandboxBackend;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkspaceAccess {
    /// Ordinary coding-agent posture: the workspace is a writable work area.
    #[default]
    ReadWrite,
    /// Independent verification posture: the workspace may be inspected but
    /// must not be mutated by the provider or its tools.
    ReadOnly,
}

impl WorkspaceAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadWrite => "read-write",
            Self::ReadOnly => "read-only",
        }
    }
}

#[derive(Debug, Clone)]
pub struct SandboxSpec {
    pub backend: SandboxBackend,
    pub cwd: PathBuf,
    pub program: OsString,
    pub args: Vec<OsString>,
    pub stdin: Option<Vec<u8>>,
    pub env: BTreeMap<String, String>,
    pub allow_network: bool,
    pub pid_file: Option<PathBuf>,
    pub cancellation_token: Option<CancellationToken>,
    pub profile_dir: Option<PathBuf>,
    pub read_allowlist: Vec<PathBuf>,
    pub write_allowlist: Vec<PathBuf>,
    pub read_denylist: Vec<PathBuf>,
    pub write_denylist: Vec<PathBuf>,
    pub network_allowlist: Vec<String>,
    pub workspace_access: WorkspaceAccess,
}
