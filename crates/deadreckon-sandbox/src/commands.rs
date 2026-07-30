use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::backend::{Result, SandboxBackend, resolve_backend};
use crate::policy::ProtectedPathPolicy;
use crate::spec::{SandboxSpec, WorkspaceAccess};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxCommand {
    pub backend: SandboxBackend,
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub warning: Option<String>,
}

pub fn build_command(spec: &SandboxSpec) -> Result<SandboxCommand> {
    // Boundary enforcement belongs here, at the final common route into every
    // outer sandbox. Callers may add narrower denials, but cannot accidentally
    // omit DeadReckon's signing and control-plane paths.
    let effective = with_protected_boundary(spec);
    let spec = &effective;
    // REPORT.md: Disposable Sandboxes are selected per run and degrade to an
    // explicit unsafe warning only when requested or unsupported.
    let (backend, warning) = resolve_backend(spec.backend)?;
    if spec.workspace_access == WorkspaceAccess::ReadOnly && backend == SandboxBackend::None {
        return Err(crate::SandboxError::ReadOnlyUnavailable(
            spec.backend.to_string(),
        ));
    }
    match backend {
        SandboxBackend::SandboxExec => sandbox_exec_command(spec, warning),
        SandboxBackend::Bwrap => bwrap_command(spec, warning),
        SandboxBackend::Docker => Ok(docker_command(spec, warning)),
        SandboxBackend::None => Ok(SandboxCommand {
            backend,
            program: spec.program.clone(),
            args: spec.args.clone(),
            env: spec.env.clone(),
            cwd: spec.cwd.clone(),
            warning: warning.or_else(|| {
                Some(
                    "sandbox backend none is unsafe; use only for explicit local verification"
                        .to_string(),
                )
            }),
        }),
        SandboxBackend::Auto => unreachable!("resolve_backend never returns auto"),
    }
}

fn with_protected_boundary(spec: &SandboxSpec) -> SandboxSpec {
    let mut effective = spec.clone();
    let mut boundary = ProtectedPathPolicy::discover();
    boundary.protect_workspace_git_control(&spec.cwd);
    boundary.merge_into(&mut effective.read_denylist, &mut effective.write_denylist);
    effective
}

fn sandbox_exec_command(spec: &SandboxSpec, warning: Option<String>) -> Result<SandboxCommand> {
    let profile = sandbox_exec_profile(spec)?;
    let mut args = vec![
        OsString::from("-p"),
        OsString::from(profile),
        OsString::from("--"),
    ];
    args.push(spec.program.clone());
    args.extend(spec.args.clone());
    Ok(SandboxCommand {
        backend: SandboxBackend::SandboxExec,
        program: OsString::from("sandbox-exec"),
        args,
        env: spec.env.clone(),
        cwd: spec.cwd.clone(),
        warning,
    })
}

fn bwrap_command(spec: &SandboxSpec, warning: Option<String>) -> Result<SandboxCommand> {
    let cwd = spec.cwd.to_string_lossy().to_string();
    let read_mounts = system_read_allowlist(&spec.cwd, &spec.read_allowlist);
    let sandbox_home = match spec.workspace_access {
        WorkspaceAccess::ReadWrite => {
            let path = spec.cwd.join(".deadreckon-home");
            std::fs::create_dir_all(&path)?;
            path
        }
        WorkspaceAccess::ReadOnly => PathBuf::from("/tmp/.deadreckon-home"),
    };
    let mut args = vec![
        "--die-with-parent".into(),
        "--unshare-pid".into(),
        "--unshare-ipc".into(),
        "--unshare-uts".into(),
        "--setenv".into(),
        "HOME".into(),
        sandbox_home.to_string_lossy().to_string().into(),
    ];
    if spec.workspace_access == WorkspaceAccess::ReadWrite {
        args.push("--tmpfs".into());
        args.push(sandbox_home.to_string_lossy().to_string().into());
    }
    for path in &read_mounts {
        let path = path.to_string_lossy().to_string();
        args.push("--ro-bind".into());
        args.push(path.clone().into());
        args.push(path.into());
    }
    for path in &spec.write_allowlist {
        let path = path.to_string_lossy().to_string();
        args.push("--bind".into());
        args.push(path.clone().into());
        args.push(path.into());
    }
    args.extend([
        if spec.workspace_access == WorkspaceAccess::ReadOnly {
            "--ro-bind".into()
        } else {
            "--bind".into()
        },
        cwd.clone().into(),
        cwd.clone().into(),
        "--tmpfs".into(),
        "/tmp".into(),
    ]);
    if spec.workspace_access == WorkspaceAccess::ReadOnly {
        args.push("--dir".into());
        args.push(sandbox_home.to_string_lossy().to_string().into());
    }
    append_bwrap_protected_mounts(&mut args, spec, &read_mounts, &spec.write_allowlist);
    args.extend([
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
        "--chdir".into(),
        cwd.into(),
    ]);
    if !spec.allow_network {
        args.push("--unshare-net".into());
    }
    args.push("--".into());
    args.push(spec.program.clone());
    args.extend(spec.args.clone());
    Ok(SandboxCommand {
        backend: SandboxBackend::Bwrap,
        program: OsString::from("bwrap"),
        args,
        env: spec.env.clone(),
        cwd: spec.cwd.clone(),
        warning,
    })
}

fn docker_command(spec: &SandboxSpec, warning: Option<String>) -> SandboxCommand {
    let cwd = spec.cwd.to_string_lossy().to_string();
    let mut args = vec![
        "run".into(),
        "--rm".into(),
        "-v".into(),
        if spec.workspace_access == WorkspaceAccess::ReadOnly {
            format!("{cwd}:{cwd}:ro").into()
        } else {
            format!("{cwd}:{cwd}").into()
        },
        "-w".into(),
        cwd.into(),
    ];
    if !spec.allow_network {
        args.push("--network".into());
        args.push("none".into());
    }
    for (key, value) in &spec.env {
        args.push("-e".into());
        args.push(format!("{key}={value}").into());
    }
    append_docker_protected_mounts(&mut args, spec);
    args.push("rust:1".into());
    args.push(spec.program.clone());
    args.extend(spec.args.clone());
    SandboxCommand {
        backend: SandboxBackend::Docker,
        program: OsString::from("docker"),
        args,
        env: BTreeMap::new(),
        cwd: spec.cwd.clone(),
        warning,
    }
}

fn append_bwrap_protected_mounts(
    args: &mut Vec<OsString>,
    spec: &SandboxSpec,
    read_mounts: &[PathBuf],
    write_mounts: &[PathBuf],
) {
    let mut exposed = read_mounts.to_vec();
    exposed.extend(write_mounts.iter().cloned());
    exposed.push(spec.cwd.clone());

    for path in &spec.read_denylist {
        if path_is_exposed(path, &exposed) {
            args.push("--tmpfs".into());
            args.push(path.to_string_lossy().to_string().into());
        }
    }
    for path in &spec.write_denylist {
        if spec.read_denylist.contains(path) || !path.exists() || !path_is_exposed(path, &exposed) {
            continue;
        }
        let path = path.to_string_lossy().to_string();
        args.push("--ro-bind".into());
        args.push(path.clone().into());
        args.push(path.into());
    }
}

fn append_docker_protected_mounts(args: &mut Vec<OsString>, spec: &SandboxSpec) {
    for path in &spec.read_denylist {
        if path.starts_with(&spec.cwd) {
            args.push("--mount".into());
            args.push(format!("type=tmpfs,destination={}", path.to_string_lossy()).into());
        }
    }
    for path in &spec.write_denylist {
        if spec.read_denylist.contains(path) || !path.exists() || !path.starts_with(&spec.cwd) {
            continue;
        }
        let path = path.to_string_lossy();
        args.push("--mount".into());
        args.push(format!("type=bind,source={path},destination={path},readonly").into());
    }
}

fn path_is_exposed(path: &Path, mounts: &[PathBuf]) -> bool {
    mounts
        .iter()
        .any(|mount| path == mount || path.starts_with(mount))
}

pub(crate) fn sandbox_exec_profile(spec: &SandboxSpec) -> Result<String> {
    let network = if spec.allow_network {
        if spec.network_allowlist.iter().any(|host| host == "*") {
            "(allow network*)".to_string()
        } else {
            "(deny network*)".to_string()
        }
    } else {
        "(deny network*)".to_string()
    };
    let mut read_rules = String::new();
    for path in system_read_allowlist(&spec.cwd, &spec.read_allowlist) {
        read_rules.push_str(&format!(
            "    (subpath \"{}\")\n",
            escape_seatbelt_path(&path)
        ));
    }
    let mut write_rules = String::new();
    for path in &spec.write_allowlist {
        write_rules.push_str(&format!(
            "    (subpath \"{}\")\n",
            escape_seatbelt_path(path)
        ));
    }
    let ssh_deny = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| {
            let ssh = escape_seatbelt_path(&home.join(".ssh"));
            format!("(deny file-read* (subpath \"{ssh}\"))\n(deny file-write* (subpath \"{ssh}\"))")
        })
        .unwrap_or_default();
    let mut read_deny_rules = String::new();
    for path in &spec.read_denylist {
        read_deny_rules.push_str(&seatbelt_deny_rule("file-read*", path));
    }
    let mut write_deny_rules = String::new();
    for path in &spec.write_denylist {
        write_deny_rules.push_str(&seatbelt_deny_rule("file-write*", path));
    }
    if spec.workspace_access == WorkspaceAccess::ReadOnly {
        let mut workspaces = vec![spec.cwd.clone()];
        if let Ok(canonical) = spec.cwd.canonicalize() {
            workspaces.push(canonical);
        }
        workspaces.sort();
        workspaces.dedup();
        for workspace in workspaces {
            write_deny_rules.push_str(&format!(
                "(deny file-write* (subpath \"{}\"))\n",
                escape_seatbelt_path(&workspace)
            ));
        }
    }
    let profile = format!(
        "(version 1)
(allow default)
{network}
(allow file-read*
{read_rules})
(allow file-write*
    (subpath \"{}\")
    (subpath \"/private/tmp\")
    (subpath \"/tmp\")
{write_rules})
{ssh_deny}
{read_deny_rules}{write_deny_rules}
",
        escape_seatbelt_path(&spec.cwd)
    );
    if let Some(profile_dir) = spec.profile_dir.as_ref() {
        std::fs::create_dir_all(profile_dir)?;
        std::fs::write(profile_dir.join("profile.sb"), &profile)?;
    }
    Ok(profile)
}

fn system_read_allowlist(cwd: &Path, extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = vec![
        PathBuf::from("/bin"),
        PathBuf::from("/sbin"),
        PathBuf::from("/usr"),
        PathBuf::from("/System"),
        PathBuf::from("/Library"),
        PathBuf::from("/Applications"),
        PathBuf::from("/opt/homebrew"),
        PathBuf::from("/opt/local"),
        PathBuf::from("/dev"),
        PathBuf::from("/private/tmp"),
        PathBuf::from("/tmp"),
        cwd.to_path_buf(),
    ];
    if let Ok(canonical) = cwd.canonicalize() {
        paths.push(canonical);
    }
    paths.extend(extra.iter().cloned());
    paths.extend(extra.iter().filter_map(|path| path.canonicalize().ok()));
    paths
}

fn escape_seatbelt_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn seatbelt_deny_rule(operation: &str, path: &Path) -> String {
    let path = escape_seatbelt_path(path);
    // `literal` protects a file and the directory entry itself; `subpath`
    // protects descendants if it is a directory. Keeping both closes the
    // file-vs-directory ambiguity without relying on the path already existing.
    format!(
        "(deny {operation} (literal \"{path}\"))\n\
         (deny {operation} (subpath \"{path}\"))\n"
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::PathBuf;

    use deadreckon_core::DeadreckonPaths;

    use super::{bwrap_command, docker_command, sandbox_exec_profile, with_protected_boundary};
    use crate::{ProtectedPathPolicy, SandboxBackend, SandboxSpec, WorkspaceAccess};
    use tempfile::TempDir;

    fn read_only_spec(backend: SandboxBackend) -> SandboxSpec {
        SandboxSpec {
            backend,
            cwd: PathBuf::from("/work/project"),
            program: OsString::from("judge"),
            args: Vec::new(),
            stdin: None,
            env: BTreeMap::new(),
            allow_network: false,
            pid_file: None,
            cancellation_token: None,
            profile_dir: None,
            read_allowlist: Vec::new(),
            write_allowlist: Vec::new(),
            read_denylist: Vec::new(),
            write_denylist: Vec::new(),
            network_allowlist: Vec::new(),
            workspace_access: WorkspaceAccess::ReadOnly,
            cleanup_process_group: false,
            guarded_launch: None,
        }
    }

    #[test]
    fn read_only_seatbelt_denies_workspace_writes() {
        let profile =
            sandbox_exec_profile(&read_only_spec(SandboxBackend::SandboxExec)).expect("profile");
        assert!(profile.contains("(deny file-write* (subpath \"/work/project\"))"));
    }

    #[test]
    fn read_only_bwrap_mounts_workspace_ro() {
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("project");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut spec = read_only_spec(SandboxBackend::Bwrap);
        spec.cwd = workspace.clone();
        let command = bwrap_command(&spec, None).expect("command");
        assert!(!workspace.join(".deadreckon-home").exists());
        let args = command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let workspace = workspace.to_string_lossy().into_owned();
        assert!(
            args.windows(3)
                .any(|parts| parts == ["--ro-bind", &workspace, &workspace])
        );
        assert!(
            !args
                .windows(3)
                .any(|parts| parts == ["--bind", &workspace, &workspace])
        );
    }

    #[test]
    fn read_only_docker_mounts_workspace_ro() {
        let spec = read_only_spec(SandboxBackend::Docker);
        let command = docker_command(&spec, None);
        let args = command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            args.windows(2)
                .any(|parts| parts == ["-v", "/work/project:/work/project:ro"])
        );
    }

    #[test]
    fn read_only_none_backend_fails_closed() {
        let error = super::build_command(&read_only_spec(SandboxBackend::None))
            .expect_err("none cannot enforce read-only");
        assert!(error.to_string().contains("read-only"));
    }

    #[test]
    fn every_outer_sandbox_receives_the_control_boundary() {
        let spec = with_protected_boundary(&read_only_spec(SandboxBackend::SandboxExec));
        let paths = DeadreckonPaths::discover();
        assert!(spec.read_denylist.contains(&paths.home().join("gate-keys")));
        assert!(spec.write_denylist.contains(&paths.jobs_dir()));
        assert!(
            spec.write_denylist
                .contains(&PathBuf::from("/work/project/.git"))
        );
    }

    #[test]
    fn every_writable_outer_sandbox_denies_the_workspace_git_router() {
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("worktree");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(
            workspace.join(".git"),
            "gitdir: /operator/repo.git/worktrees/run\n",
        )
        .expect("git control");
        let mut spec = read_only_spec(SandboxBackend::SandboxExec);
        spec.cwd = workspace.clone();
        spec.workspace_access = WorkspaceAccess::ReadWrite;

        let effective = with_protected_boundary(&spec);
        assert!(effective.write_denylist.contains(&workspace.join(".git")));
        assert!(
            effective
                .write_denylist
                .contains(&PathBuf::from("/operator/repo.git/worktrees/run"))
        );

        let profile = sandbox_exec_profile(&effective).expect("seatbelt profile");
        let control = workspace.join(".git").display().to_string();
        assert!(profile.contains(&format!("(deny file-write* (literal \"{control}\"))")));
    }

    #[test]
    fn seatbelt_denies_key_reads_but_keeps_evidence_readable() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("dr-home"));
        let run_root = paths.run_root("project", "run-1");
        std::fs::create_dir_all(run_root.join("proofs")).expect("proofs");
        let policy = ProtectedPathPolicy::for_paths(&paths);
        let mut spec = read_only_spec(SandboxBackend::SandboxExec);
        spec.read_denylist = policy.read_denylist;
        spec.write_denylist = policy.write_denylist;

        let profile = sandbox_exec_profile(&spec).expect("profile");
        let key_store = paths.home().join("gate-keys").display().to_string();
        let proofs = run_root.join("proofs").display().to_string();
        assert!(profile.contains(&format!("(deny file-read* (literal \"{key_store}\"))")));
        assert!(profile.contains(&format!("(deny file-write* (literal \"{key_store}\"))")));
        assert!(profile.contains(&format!("(deny file-write* (literal \"{proofs}\"))")));
        assert!(!profile.contains(&format!("(deny file-read* (literal \"{proofs}\"))")));
    }

    #[test]
    fn hostile_container_backends_mask_visible_control_paths() {
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let paths = DeadreckonPaths::from_home(workspace.join(".deadreckon"));
        let run_root = paths.run_root("project", "run-1");
        std::fs::create_dir_all(paths.home().join("gate-keys")).expect("keys");
        std::fs::create_dir_all(paths.jobs_dir()).expect("jobs");
        std::fs::create_dir_all(run_root.join("proofs")).expect("proofs");
        let policy = ProtectedPathPolicy::for_paths(&paths);
        let mut spec = read_only_spec(SandboxBackend::Bwrap);
        spec.cwd = workspace;
        spec.workspace_access = WorkspaceAccess::ReadWrite;
        spec.read_denylist = policy.read_denylist;
        spec.write_denylist = policy.write_denylist;

        let bwrap = bwrap_command(&spec, None).expect("bwrap");
        let bwrap_args = bwrap
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let key_store = paths.home().join("gate-keys").display().to_string();
        let jobs = paths.jobs_dir().display().to_string();
        assert!(
            bwrap_args
                .windows(2)
                .any(|parts| parts == ["--tmpfs", &key_store])
        );
        assert!(
            bwrap_args
                .windows(3)
                .any(|parts| parts == ["--ro-bind", &jobs, &jobs])
        );

        spec.backend = SandboxBackend::Docker;
        let docker = docker_command(&spec, None);
        let docker_args = docker
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(
            docker_args
                .iter()
                .any(|arg| { arg == &format!("type=tmpfs,destination={key_store}") })
        );
        assert!(
            docker_args.iter().any(|arg| {
                arg == &format!("type=bind,source={jobs},destination={jobs},readonly")
            })
        );
    }
}
