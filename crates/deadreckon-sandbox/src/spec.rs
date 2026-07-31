use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use tokio_util::sync::CancellationToken;

use crate::backend::SandboxBackend;

/// Trusted release helper used when the sandboxed command must move into a
/// separately recoverable process group.
#[derive(Debug, Clone)]
pub struct GuardedLaunchSpec {
    /// Executable providing the hidden `guarded-exec` release protocol.
    pub program: OsString,
    /// Unique identity for this evaluator launch, not merely the Job attempt.
    pub launch_id: String,
    /// Durable supervisor attempt that owns this launch.
    pub attempt: u32,
    /// Unique outer Job launch, when available, for audit correlation.
    pub owner_launch_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkspaceAccess {
    /// Ordinary coding-agent posture: the workspace is a writable work area.
    #[default]
    ReadWrite,
    /// Independent verification posture: the workspace may be inspected but
    /// must not be mutated by the provider or its tools.
    ReadOnly,
    /// Independent evaluation posture: only this disposable workspace may be
    /// written. Unlike ordinary read-write execution, macOS uses a deny-by-
    /// default profile so absolute paths cannot escape the scratch tree.
    Disposable,
}

impl WorkspaceAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadWrite => "read-write",
            Self::ReadOnly => "read-only",
            Self::Disposable => "disposable",
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
    /// Put the wrapper and all ordinary descendants in a fresh process group,
    /// then terminate residual members before returning.
    ///
    /// This is opt-in because a long-lived worker may already belong to a
    /// supervisor-owned process group. Moving every nested tool into a new
    /// group would let it escape that outer cancellation boundary.
    pub cleanup_process_group: bool,
    /// Block the command behind a private release pipe until its per-launch,
    /// boot-aware identity is atomically persisted. Required for strict gate
    /// evaluation, which intentionally leaves the outer worker process group.
    pub guarded_launch: Option<GuardedLaunchSpec>,
}
