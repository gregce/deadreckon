use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use tokio_util::sync::CancellationToken;

use crate::backend::SandboxBackend;

#[derive(Debug, Clone)]
pub struct SandboxSpec {
    pub backend: SandboxBackend,
    pub cwd: PathBuf,
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env: BTreeMap<String, String>,
    pub allow_network: bool,
    pub pid_file: Option<PathBuf>,
    pub cancellation_token: Option<CancellationToken>,
    pub profile_dir: Option<PathBuf>,
    pub read_allowlist: Vec<PathBuf>,
    pub write_allowlist: Vec<PathBuf>,
    pub network_allowlist: Vec<String>,
}
