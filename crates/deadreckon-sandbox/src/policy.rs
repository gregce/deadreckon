use std::path::PathBuf;

use deadreckon_core::DeadreckonPaths;

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

/// Files which belong to DeadReckon's control plane rather than to the coding
/// agent's workspace.
///
/// The distinction between the two lists is intentional. The approved
/// contract, deterministic evidence, authority, and receipt remain readable so
/// an independent judge and an operator can inspect them. They are not
/// writable by provider or tool processes. The HMAC key store is neither
/// readable nor writable by those processes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProtectedPathPolicy {
    pub read_denylist: Vec<PathBuf>,
    pub write_denylist: Vec<PathBuf>,
}

impl ProtectedPathPolicy {
    pub fn discover() -> Self {
        Self::for_paths(&DeadreckonPaths::discover())
    }

    pub fn for_paths(paths: &DeadreckonPaths) -> Self {
        let key_store = paths.home().join("gate-keys");
        let mut policy = Self {
            read_denylist: vec![key_store.clone()],
            write_denylist: vec![key_store, paths.jobs_dir()],
        };

        for run_root in discover_run_roots(paths) {
            policy.write_denylist.extend([
                run_root.join("acceptance.yaml"),
                run_root.join("gate"),
                run_root.join("proofs"),
            ]);
        }

        add_canonical_variants(&mut policy.read_denylist);
        add_canonical_variants(&mut policy.write_denylist);
        dedup_policy_paths(&mut policy.read_denylist);
        dedup_policy_paths(&mut policy.write_denylist);
        policy
    }

    pub fn merge_into(&self, read_denylist: &mut Vec<PathBuf>, write_denylist: &mut Vec<PathBuf>) {
        read_denylist.extend(self.read_denylist.iter().cloned());
        write_denylist.extend(self.write_denylist.iter().cloned());
        dedup_policy_paths(read_denylist);
        dedup_policy_paths(write_denylist);
    }
}

fn discover_run_roots(paths: &DeadreckonPaths) -> Vec<PathBuf> {
    let Ok(scopes) = std::fs::read_dir(paths.runstate_dir()) else {
        return Vec::new();
    };
    let mut roots = Vec::new();
    for scope in scopes.flatten() {
        let runs = scope.path().join("runs");
        let Ok(entries) = std::fs::read_dir(runs) else {
            continue;
        };
        roots.extend(
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir()),
        );
    }
    roots.sort();
    roots.dedup();
    roots
}

fn add_canonical_variants(paths: &mut Vec<PathBuf>) {
    let variants = paths
        .iter()
        .filter_map(|path| canonicalize_with_existing_parent(path))
        .collect::<Vec<_>>();
    paths.extend(variants);
}

fn canonicalize_with_existing_parent(path: &std::path::Path) -> Option<PathBuf> {
    if let Ok(canonical) = path.canonicalize() {
        return Some(canonical);
    }
    let name = path.file_name()?;
    let parent = path.parent()?.canonicalize().ok()?;
    Some(parent.join(name))
}

fn dedup_policy_paths(paths: &mut Vec<PathBuf>) {
    paths.sort();
    paths.dedup();
}

#[cfg(test)]
mod tests {
    use deadreckon_core::DeadreckonPaths;
    use tempfile::TempDir;

    use super::ProtectedPathPolicy;

    #[test]
    fn control_boundary_keeps_evidence_readable_but_not_writable() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("dr-home"));
        let run_root = paths.run_root("project", "run-1");
        std::fs::create_dir_all(run_root.join("proofs")).expect("proofs");
        std::fs::create_dir_all(run_root.join("gate")).expect("gate");
        std::fs::write(run_root.join("acceptance.yaml"), "checks: []\n").expect("contract");

        let policy = ProtectedPathPolicy::for_paths(&paths);

        assert!(
            policy
                .read_denylist
                .contains(&paths.home().join("gate-keys"))
        );
        for protected in [
            paths.home().join("gate-keys"),
            paths.jobs_dir(),
            run_root.join("acceptance.yaml"),
            run_root.join("gate"),
            run_root.join("proofs"),
        ] {
            assert!(
                policy.write_denylist.contains(&protected),
                "{} was writable",
                protected.display()
            );
        }
        assert!(!policy.read_denylist.contains(&run_root.join("proofs")));
        assert!(!policy.read_denylist.contains(&paths.jobs_dir()));
    }

    #[test]
    fn canonical_home_cannot_bypass_the_key_store_boundary() {
        let temp = TempDir::new().expect("tempdir");
        let real_home = temp.path().join("real");
        std::fs::create_dir_all(real_home.join("gate-keys")).expect("keys");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&real_home, temp.path().join("alias")).expect("symlink");
            let paths = DeadreckonPaths::from_home(temp.path().join("alias"));
            let policy = ProtectedPathPolicy::for_paths(&paths);
            let canonical_key_store = real_home
                .join("gate-keys")
                .canonicalize()
                .expect("canonical key store");
            assert!(policy.read_denylist.contains(&canonical_key_store));
        }
    }
}
