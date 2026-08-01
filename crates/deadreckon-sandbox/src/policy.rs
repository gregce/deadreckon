use std::path::{Path, PathBuf};

use deadreckon_core::{DeadreckonPaths, read_trusted_codebase_record};

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
/// writable by provider or tool processes. The HMAC key store and operator
/// capture store are neither readable nor writable by those processes.
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
            read_denylist: vec![key_store.clone(), paths.operator_captures_dir()],
            write_denylist: vec![key_store, paths.jobs_dir(), paths.operator_captures_dir()],
        };

        for run_root in discover_run_roots(paths) {
            policy.write_denylist.extend([
                run_root.join("acceptance.yaml"),
                run_root.join(deadreckon_core::TRUSTED_CODEBASE_RECORD),
                run_root.join("sandbox.toml"),
                run_root.join("gate"),
                run_root.join("provider-evidence"),
                run_root.join("proofs"),
                run_root.join("snapshots"),
                run_root.join("provenance.jsonl"),
            ]);
            policy
                .write_denylist
                .extend(discover_trusted_git_control_paths(&run_root));
        }

        add_canonical_variants(&mut policy.read_denylist);
        add_canonical_variants(&mut policy.write_denylist);
        dedup_policy_paths(&mut policy.read_denylist);
        dedup_policy_paths(&mut policy.write_denylist);
        policy
    }

    /// Protect the workspace entry Git consults before it knows which
    /// repository controls the checkout.
    ///
    /// A linked worktree stores this routing information in a regular `.git`
    /// file. Protecting only the resolved Git directory is insufficient: an
    /// agent could otherwise redirect later DeadReckon Git commands to an
    /// unrelated repository.
    pub fn protect_workspace_git_control(&mut self, workspace: &Path) {
        let control = workspace.join(".git");
        self.write_denylist.push(control.clone());
        extend_resolved_git_metadata(&control, &mut self.write_denylist);
        if let Ok(canonical_workspace) = workspace.canonicalize() {
            self.write_denylist.push(canonical_workspace.join(".git"));
        }
        add_canonical_variants(&mut self.write_denylist);
        dedup_policy_paths(&mut self.write_denylist);
    }

    pub fn merge_into(&self, read_denylist: &mut Vec<PathBuf>, write_denylist: &mut Vec<PathBuf>) {
        read_denylist.extend(self.read_denylist.iter().cloned());
        write_denylist.extend(self.write_denylist.iter().cloned());
        dedup_policy_paths(read_denylist);
        dedup_policy_paths(write_denylist);
    }
}

fn discover_trusted_git_control_paths(run_root: &Path) -> Vec<PathBuf> {
    let Ok(record) = read_trusted_codebase_record(run_root) else {
        return Vec::new();
    };
    let mut protected = Vec::new();

    if let Some(worktree) = record.worktree_path.as_deref() {
        let control = worktree.join(".git");
        protected.push(control.clone());
        extend_resolved_git_metadata(&control, &mut protected);
    }
    if let Some(source_root) = record.source_git_root.as_deref() {
        let source_control = source_root.join(".git");
        protected.push(source_control.clone());
        extend_resolved_git_metadata(&source_control, &mut protected);
    }

    add_canonical_variants(&mut protected);
    dedup_policy_paths(&mut protected);
    protected
}

fn extend_resolved_git_metadata(control: &Path, protected: &mut Vec<PathBuf>) {
    let Ok(metadata) = std::fs::symlink_metadata(control) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if metadata.is_dir() {
        protected.push(control.to_path_buf());
        return;
    }
    if !metadata.is_file() {
        return;
    }
    let Some(git_dir) = read_single_path_record(control, b"gitdir: ") else {
        return;
    };
    protected.push(git_dir.clone());
    let common_dir_record = git_dir.join("commondir");
    if let Some(common_dir) = read_single_path_record(&common_dir_record, b"") {
        protected.push(common_dir);
    } else {
        protected.push(git_dir);
    }
}

fn read_single_path_record(path: &Path, prefix: &[u8]) -> Option<PathBuf> {
    let bytes = std::fs::read(path).ok()?;
    let line = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    if line.is_empty() || line.contains(&b'\n') || line.contains(&b'\r') || line.contains(&0) {
        return None;
    }
    let raw = line.strip_prefix(prefix)?;
    if raw.is_empty() {
        return None;
    }
    let raw = std::str::from_utf8(raw).ok()?;
    let candidate = PathBuf::from(raw);
    let resolved = if candidate.is_absolute() {
        candidate
    } else {
        path.parent()?.join(candidate)
    };
    Some(resolved)
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
    use deadreckon_core::{
        CodebaseMode, CodebaseRecord, DeadreckonPaths, write_trusted_codebase_record,
    };
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
        assert!(
            policy
                .read_denylist
                .contains(&paths.operator_captures_dir())
        );
        for protected in [
            paths.home().join("gate-keys"),
            paths.jobs_dir(),
            paths.operator_captures_dir(),
            run_root.join("acceptance.yaml"),
            run_root.join(deadreckon_core::TRUSTED_CODEBASE_RECORD),
            run_root.join("sandbox.toml"),
            run_root.join("gate"),
            run_root.join("provider-evidence"),
            run_root.join("proofs"),
            run_root.join("snapshots"),
            run_root.join("provenance.jsonl"),
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
    fn trusted_records_protect_worktree_and_repository_git_metadata() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("dr-home"));
        let run_root = paths.run_root("project", "run-1");
        let source = temp.path().join("source");
        let common_dir = source.join(".git");
        let git_dir = common_dir.join("worktrees/run-1");
        let worktree = temp.path().join("worktree");
        std::fs::create_dir_all(&git_dir).expect("linked-worktree git dir");
        std::fs::create_dir_all(&worktree).expect("worktree");
        std::fs::write(git_dir.join("commondir"), "../..\n").expect("commondir");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", git_dir.display()),
        )
        .expect("worktree git control");
        std::fs::create_dir_all(&run_root).expect("run root");
        let mut record = CodebaseRecord::fresh();
        record.mode = CodebaseMode::Worktree;
        record.source_git_root = Some(source);
        record.worktree_path = Some(worktree.clone());
        write_trusted_codebase_record(&run_root, &record).expect("trusted record");

        let policy = ProtectedPathPolicy::for_paths(&paths);

        for protected in [worktree.join(".git"), git_dir, common_dir] {
            assert!(
                policy.write_denylist.contains(&protected),
                "{} was writable",
                protected.display()
            );
        }
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
