use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSandboxPolicy {
    pub allow_network: bool,
    pub read_allowlist: Vec<PathBuf>,
    pub write_allowlist: Vec<PathBuf>,
    pub network_allowlist: Vec<String>,
}

impl ToolSandboxPolicy {
    pub fn bash(working_dir: impl Into<PathBuf>) -> Self {
        let working_dir = working_dir.into();
        Self {
            allow_network: false,
            read_allowlist: vec![working_dir.clone()],
            write_allowlist: vec![working_dir],
            network_allowlist: Vec::new(),
        }
    }

    pub fn cli_provider(
        working_dir: impl Into<PathBuf>,
        mut read_allowlist: Vec<PathBuf>,
        mut write_allowlist: Vec<PathBuf>,
    ) -> Self {
        let working_dir = working_dir.into();
        read_allowlist.push(working_dir.clone());
        write_allowlist.push(working_dir);
        dedup_policy_paths(&mut read_allowlist);
        dedup_policy_paths(&mut write_allowlist);
        Self {
            allow_network: true,
            read_allowlist,
            write_allowlist,
            network_allowlist: vec!["*".to_string()],
        }
    }
}

fn dedup_policy_paths(paths: &mut Vec<PathBuf>) {
    paths.sort();
    paths.dedup();
}
