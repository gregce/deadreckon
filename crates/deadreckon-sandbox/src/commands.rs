use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::backend::{Result, SandboxBackend, backend_executable, resolve_backend};
use crate::docker::DockerExecution;
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
    if spec.docker.is_some() && backend != SandboxBackend::Docker {
        return Err(crate::SandboxError::InvalidDockerExecution(
            "Docker execution configuration requires the Docker backend".to_string(),
        ));
    }
    if spec.workspace_access != WorkspaceAccess::ReadWrite && backend == SandboxBackend::None {
        return Err(crate::SandboxError::ReadOnlyUnavailable(
            spec.backend.to_string(),
        ));
    }
    match backend {
        SandboxBackend::SandboxExec => {
            sandbox_exec_command(spec, backend_executable(backend)?, warning)
        }
        SandboxBackend::Bwrap => bwrap_command(spec, backend_executable(backend)?, warning),
        SandboxBackend::Docker => docker_command(spec, backend_executable(backend)?, warning),
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
    for forbidden in [
        deadreckon_core::GATE_KEY_ENV,
        deadreckon_core::GATE_CONTAINED_ENV,
        deadreckon_core::GATE_SANDBOX_BACKEND_ENV,
    ] {
        effective.env.remove(forbidden);
    }
    let mut boundary = ProtectedPathPolicy::discover();
    boundary.protect_workspace_git_control(&spec.cwd);
    boundary.merge_into(&mut effective.read_denylist, &mut effective.write_denylist);
    effective
}

fn sandbox_exec_command(
    spec: &SandboxSpec,
    wrapper: PathBuf,
    warning: Option<String>,
) -> Result<SandboxCommand> {
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
        program: wrapper.into_os_string(),
        args,
        env: spec.env.clone(),
        cwd: spec.cwd.clone(),
        warning,
    })
}

fn bwrap_command(
    spec: &SandboxSpec,
    wrapper: PathBuf,
    warning: Option<String>,
) -> Result<SandboxCommand> {
    let cwd = spec.cwd.to_string_lossy().to_string();
    // Bubblewrap starts from an empty mount namespace. Give it a private /tmp
    // before reconstructing absolute host paths beneath that directory;
    // mounting host /tmp and replacing it later would hide the workspace and
    // test/provider binaries that were already mounted below it.
    let read_mounts = system_read_allowlist(&spec.cwd, &spec.read_allowlist)
        .into_iter()
        .filter(|path| path != Path::new("/tmp") && path != Path::new("/private/tmp"))
        .collect::<Vec<_>>();
    let sandbox_home = match spec.workspace_access {
        WorkspaceAccess::ReadWrite => {
            let path = spec.cwd.join(".deadreckon-home");
            std::fs::create_dir_all(&path)?;
            path
        }
        WorkspaceAccess::Disposable => spec
            .env
            .get("HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute() && path.is_dir())
            .ok_or_else(|| {
                crate::SandboxError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "disposable bubblewrap evaluation requires an existing absolute HOME",
                ))
            })?,
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
        "--tmpfs".into(),
        "/tmp".into(),
    ];
    let mut destinations = read_mounts.clone();
    destinations.extend(spec.write_allowlist.iter().cloned());
    destinations.push(spec.cwd.clone());
    destinations.push(sandbox_home.clone());
    append_bwrap_destination_parents(&mut args, &destinations);
    for path in &read_mounts {
        let path = path.to_string_lossy().to_string();
        args.push("--ro-bind".into());
        args.push(path.clone().into());
        args.push(path.into());
    }
    for path in &spec.write_allowlist {
        let path = path.to_string_lossy().to_string();
        // Provider state roots are optional until their CLI creates them. A
        // missing optional source must not make an otherwise valid sandbox
        // unusable; existing sources retain their explicit writable mount.
        args.push("--bind-try".into());
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
    ]);
    match spec.workspace_access {
        WorkspaceAccess::ReadWrite => {
            // Apply this after the workspace bind so it cannot be hidden by
            // that broader mount and leak CLI state into the worktree.
            args.push("--tmpfs".into());
            args.push(sandbox_home.to_string_lossy().to_string().into());
        }
        WorkspaceAccess::ReadOnly => {
            args.push("--dir".into());
            args.push(sandbox_home.to_string_lossy().to_string().into());
        }
        WorkspaceAccess::Disposable => {}
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
        program: wrapper.into_os_string(),
        args,
        env: spec.env.clone(),
        cwd: spec.cwd.clone(),
        warning,
    })
}

fn append_bwrap_destination_parents(args: &mut Vec<OsString>, paths: &[PathBuf]) {
    let mut parents = std::collections::BTreeSet::new();
    for path in paths {
        for parent in path.ancestors().skip(1) {
            if parent == Path::new("/") || parent == Path::new("/tmp") {
                continue;
            }
            parents.insert(parent.to_path_buf());
        }
    }
    let mut parents = parents.into_iter().collect::<Vec<_>>();
    parents.sort_by_key(|path| path.components().count());
    for parent in parents {
        args.push("--dir".into());
        args.push(parent.into_os_string());
    }
}

fn docker_command(
    spec: &SandboxSpec,
    wrapper: PathBuf,
    warning: Option<String>,
) -> Result<SandboxCommand> {
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
    if let Some(docker) = spec.docker.as_ref() {
        validate_docker_execution(docker)?;
        args.extend([
            "--pull=never".into(),
            format!("--platform={}", docker.image().platform().as_str()).into(),
            "--entrypoint".into(),
            docker.container_program().into(),
            "--name".into(),
            docker.container_name().into(),
            "--cidfile".into(),
            docker.cid_file().as_os_str().into(),
        ]);
        for (key, value) in docker.labels() {
            args.push("--label".into());
            args.push(format!("{key}={value}").into());
        }
    }
    let mut mounted = vec![spec.cwd.clone()];
    for path in spec
        .read_allowlist
        .iter()
        .filter(|path| path.is_absolute() && path.exists() && !path.starts_with(&spec.cwd))
    {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if exact_path_is_protected(&canonical, &spec.read_denylist) {
            continue;
        }
        if mounted
            .iter()
            .any(|root| canonical == *root || canonical.starts_with(root))
        {
            continue;
        }
        let rendered = canonical.to_string_lossy();
        args.push("--mount".into());
        args.push(format!("type=bind,source={rendered},destination={rendered},readonly").into());
        mounted.push(canonical);
    }
    for path in spec
        .write_allowlist
        .iter()
        .filter(|path| path.is_absolute() && path.exists() && !path.starts_with(&spec.cwd))
    {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if exact_path_is_protected(&canonical, &spec.read_denylist)
            || exact_path_is_protected(&canonical, &spec.write_denylist)
        {
            continue;
        }
        if mounted
            .iter()
            .any(|root| canonical == *root || canonical.starts_with(root))
        {
            continue;
        }
        let rendered = canonical.to_string_lossy();
        args.push("--mount".into());
        args.push(format!("type=bind,source={rendered},destination={rendered}").into());
        mounted.push(canonical);
    }
    if !spec.allow_network {
        args.push("--network".into());
        args.push("none".into());
    }
    for (key, value) in spec.env.iter().filter(|(key, _)| {
        !matches!(
            key.as_str(),
            deadreckon_core::GATE_KEY_ENV
                | deadreckon_core::GATE_CONTAINED_ENV
                | deadreckon_core::GATE_SANDBOX_BACKEND_ENV
        )
    }) {
        args.push("-e".into());
        args.push(format!("{key}={value}").into());
    }
    append_docker_protected_mounts(&mut args, spec, &mounted);
    if let Some(docker) = spec.docker.as_ref() {
        let source = docker.sidecar_host_path().to_string_lossy();
        args.push("--mount".into());
        args.push(
            format!(
                "type=bind,source={source},destination={},readonly",
                docker.container_program()
            )
            .into(),
        );
        args.push(docker.image().id().into());
    } else {
        args.push("rust:1".into());
        args.push(spec.program.clone());
    }
    args.extend(spec.args.clone());
    Ok(SandboxCommand {
        backend: SandboxBackend::Docker,
        program: wrapper.into_os_string(),
        args,
        env: BTreeMap::new(),
        cwd: spec.cwd.clone(),
        warning,
    })
}

fn validate_docker_execution(docker: &DockerExecution) -> Result<()> {
    let metadata = std::fs::symlink_metadata(docker.sidecar_host_path())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(crate::SandboxError::InvalidDockerExecution(format!(
            "Docker evaluator sidecar changed after configuration: {}",
            docker.sidecar_host_path().display()
        )));
    }
    if docker.cid_file().exists() {
        let cid_metadata = std::fs::symlink_metadata(docker.cid_file())?;
        if cid_metadata.file_type().is_symlink() || !cid_metadata.is_file() {
            return Err(crate::SandboxError::InvalidDockerExecution(format!(
                "Docker cidfile is not a regular non-symlink file: {}",
                docker.cid_file().display()
            )));
        }
    }
    Ok(())
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
        if path.exists() && path_is_exposed(path, &exposed) {
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

fn append_docker_protected_mounts(
    args: &mut Vec<OsString>,
    spec: &SandboxSpec,
    exposed: &[PathBuf],
) {
    for path in &spec.read_denylist {
        if path.exists() && path_is_exposed(path, exposed) {
            args.push("--mount".into());
            let rendered = path.to_string_lossy();
            if path.is_dir() {
                args.push(format!("type=tmpfs,destination={rendered}").into());
            } else {
                args.push(
                    format!("type=bind,source=/dev/null,destination={rendered},readonly").into(),
                );
            }
        }
    }
    for path in &spec.write_denylist {
        if spec.read_denylist.contains(path) || !path.exists() || !path_is_exposed(path, exposed) {
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

fn exact_path_is_protected(path: &Path, protected: &[PathBuf]) -> bool {
    protected.iter().any(|candidate| {
        candidate == path
            || candidate
                .canonicalize()
                .is_ok_and(|resolved| resolved == path)
    })
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
    let read_rules = seatbelt_read_rules(&system_read_allowlist(&spec.cwd, &spec.read_allowlist));
    let mut writable_paths = vec![spec.cwd.clone()];
    writable_paths.extend(spec.write_allowlist.iter().cloned());
    writable_paths.extend(
        writable_paths
            .clone()
            .into_iter()
            .filter_map(|path| path.canonicalize().ok()),
    );
    writable_paths.sort();
    writable_paths.dedup();
    let mut write_rules = String::new();
    for path in &writable_paths {
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
    let default_posture = if spec.workspace_access == WorkspaceAccess::Disposable {
        "(deny default)\n(allow process*)\n(allow mach-lookup)\n(allow sysctl-read)"
    } else {
        "(allow default)"
    };
    let file_read_policy = format!("(allow file-read*\n{read_rules})");
    let host_temp_writes = if spec.workspace_access == WorkspaceAccess::Disposable {
        String::new()
    } else {
        "    (subpath \"/private/tmp\")\n    (subpath \"/tmp\")\n".to_string()
    };
    let profile = format!(
        "(version 1)
{default_posture}
{network}
{file_read_policy}
(allow file-write*
{host_temp_writes}
{write_rules})
{ssh_deny}
{read_deny_rules}{write_deny_rules}
"
    );
    if let Some(profile_dir) = spec.profile_dir.as_ref() {
        std::fs::create_dir_all(profile_dir)?;
        std::fs::write(profile_dir.join("profile.sb"), &profile)?;
    }
    Ok(profile)
}

fn seatbelt_read_rules(paths: &[PathBuf]) -> String {
    let mut literal_paths = std::collections::BTreeSet::new();
    for path in paths {
        literal_paths.extend(path.ancestors().map(Path::to_path_buf));
    }
    let mut rules = String::new();
    for path in literal_paths {
        rules.push_str(&format!(
            "    (literal \"{}\")\n",
            escape_seatbelt_path(&path)
        ));
    }
    for path in paths {
        rules.push_str(&format!(
            "    (subpath \"{}\")\n",
            escape_seatbelt_path(path)
        ));
    }
    rules
}

fn system_read_allowlist(cwd: &Path, extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut paths = vec![
        PathBuf::from("/bin"),
        PathBuf::from("/sbin"),
        // Bubblewrap starts from an empty filesystem. Even when the program
        // and `/usr` are mounted, dynamically linked Linux executables still
        // fail with ENOENT unless their ELF interpreter is visible at its
        // absolute path (commonly `/lib64/ld-linux-*.so.*`). Keep the standard
        // merged-/usr compatibility roots read-only when they exist.
        PathBuf::from("/lib"),
        PathBuf::from("/lib32"),
        PathBuf::from("/lib64"),
        PathBuf::from("/libx32"),
        PathBuf::from("/usr"),
        PathBuf::from("/System"),
        PathBuf::from("/System/Volumes/Preboot/Cryptexes"),
        PathBuf::from("/Library"),
        PathBuf::from("/Applications"),
        PathBuf::from("/opt/homebrew"),
        PathBuf::from("/opt/local"),
        PathBuf::from("/dev"),
        PathBuf::from("/etc"),
        PathBuf::from("/private/etc"),
        PathBuf::from("/private/var/db/dyld"),
        PathBuf::from("/var/db/dyld"),
        PathBuf::from("/private/var/select"),
        PathBuf::from("/private/tmp"),
        PathBuf::from("/tmp"),
        cwd.to_path_buf(),
    ];
    if let Ok(canonical) = cwd.canonicalize() {
        paths.push(canonical);
    }
    paths.extend(extra.iter().cloned());
    paths.extend(extra.iter().filter_map(|path| path.canonicalize().ok()));
    paths.retain(|path| path.exists());
    paths.sort();
    paths.dedup();
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

    use super::{
        bwrap_command, docker_command, sandbox_exec_profile, system_read_allowlist,
        with_protected_boundary,
    };
    use crate::{
        DOCKER_SIDECAR_CONTAINER_PROGRAM, DockerExecution, DockerImage, DockerPlatform,
        ProtectedPathPolicy, SandboxBackend, SandboxSpec, WorkspaceAccess,
    };
    use tempfile::TempDir;

    fn read_only_spec(backend: SandboxBackend) -> SandboxSpec {
        SandboxSpec {
            backend,
            docker: None,
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
    fn disposable_seatbelt_is_deny_by_default_with_one_writable_workspace() {
        let mut spec = read_only_spec(SandboxBackend::SandboxExec);
        spec.workspace_access = WorkspaceAccess::Disposable;
        spec.write_allowlist.push(spec.cwd.clone());
        let profile = sandbox_exec_profile(&spec).expect("profile");

        assert!(profile.contains("(deny default)"), "{profile}");
        assert!(!profile.contains("(allow default)"), "{profile}");
        assert!(profile.contains("(subpath \"/work/project\")"), "{profile}");
        let write_policy = profile
            .split_once("(allow file-write*")
            .expect("write policy")
            .1
            .split_once("(deny file-read*")
            .map_or(profile.as_str(), |(policy, _)| policy);
        assert!(!write_policy.contains("(subpath \"/tmp\")"), "{profile}");
        assert!(
            !write_policy.contains("(subpath \"/private/tmp\")"),
            "{profile}"
        );
    }

    #[test]
    fn read_only_bwrap_mounts_workspace_ro() {
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("project");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut spec = read_only_spec(SandboxBackend::Bwrap);
        spec.cwd = workspace.clone();
        let command = bwrap_command(&spec, PathBuf::from("/usr/bin/bwrap"), None).expect("command");
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
        let private_tmp = args
            .windows(2)
            .position(|parts| parts == ["--tmpfs", "/tmp"])
            .expect("private tmp mount");
        let workspace_mount = args
            .windows(3)
            .position(|parts| parts == ["--ro-bind", &workspace, &workspace])
            .expect("workspace mount");
        assert!(
            private_tmp < workspace_mount,
            "private /tmp must exist before rebuilding the workspace path: {args:?}"
        );
        assert!(
            !args
                .windows(3)
                .any(|parts| parts == ["--ro-bind", "/tmp", "/tmp"]),
            "host /tmp would hide or expose nested mounts: {args:?}"
        );
    }

    #[test]
    fn bwrap_mounts_linux_dynamic_loader_roots_when_present() {
        let temp = TempDir::new().expect("tempdir");
        let mut expected = Vec::new();
        for root in ["/lib", "/lib32", "/lib64", "/libx32"] {
            let root = PathBuf::from(root);
            if root.exists() {
                expected.push(root);
            }
        }

        let mounts = system_read_allowlist(temp.path(), &[]);
        for root in expected {
            assert!(
                mounts.contains(&root),
                "missing Linux dynamic-loader root {} from {mounts:?}",
                root.display()
            );
        }
    }

    #[test]
    fn bwrap_tolerates_missing_optional_write_roots_and_masks_home_after_workspace() {
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("project");
        let missing = temp.path().join("optional-cli-home");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut spec = read_only_spec(SandboxBackend::Bwrap);
        spec.cwd = workspace.clone();
        spec.workspace_access = WorkspaceAccess::ReadWrite;
        spec.write_allowlist = vec![workspace.clone(), missing.clone()];

        let command = bwrap_command(&spec, PathBuf::from("/usr/bin/bwrap"), None).expect("command");
        let args = command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let workspace = workspace.to_string_lossy().into_owned();
        let missing = missing.to_string_lossy().into_owned();
        let sandbox_home = format!("{workspace}/.deadreckon-home");
        assert!(
            args.windows(3)
                .any(|parts| parts == ["--bind-try", &missing, &missing]),
            "missing optional provider state should be a try-mount: {args:?}"
        );
        assert!(!std::path::Path::new(&missing).exists());
        let workspace_mount = args
            .windows(3)
            .rposition(|parts| parts == ["--bind", &workspace, &workspace])
            .expect("final workspace mount");
        let home_mask = args
            .windows(2)
            .position(|parts| parts == ["--tmpfs", &sandbox_home])
            .expect("sandbox home mask");
        assert!(
            workspace_mount < home_mask,
            "the workspace mount must not hide the private CLI home: {args:?}"
        );
    }

    #[test]
    fn bwrap_omits_nonexistent_protected_mount_targets() {
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("project");
        let missing = workspace.join("operator-captures");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let mut spec = read_only_spec(SandboxBackend::Bwrap);
        spec.cwd = workspace;
        spec.read_denylist = vec![missing.clone()];

        let command = bwrap_command(&spec, PathBuf::from("/usr/bin/bwrap"), None).expect("command");
        let args = command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let missing = missing.to_string_lossy().into_owned();
        assert!(
            !args.windows(2).any(|parts| parts == ["--tmpfs", &missing]),
            "bubblewrap cannot mount over a missing child of a read-only parent: {args:?}"
        );
    }

    #[test]
    fn read_only_docker_mounts_workspace_ro() {
        let spec = read_only_spec(SandboxBackend::Docker);
        let command =
            docker_command(&spec, PathBuf::from("/usr/bin/docker"), None).expect("command");
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
    fn docker_mounts_gate_inputs_read_only_and_runtime_scratch_writable() {
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let run_root = temp.path().join("run-root");
        let gate_bin = temp.path().join("gate-bin");
        let runtime = temp.path().join("runtime");
        for directory in [&workspace, &run_root, &gate_bin, &runtime] {
            std::fs::create_dir_all(directory).expect("fixture directory");
        }
        let mut spec = read_only_spec(SandboxBackend::Docker);
        spec.cwd = workspace;
        spec.workspace_access = WorkspaceAccess::Disposable;
        spec.read_allowlist = vec![run_root.clone(), gate_bin.clone()];
        spec.write_allowlist = vec![runtime.clone()];

        let command =
            docker_command(&spec, PathBuf::from("/usr/bin/docker"), None).expect("command");
        let args = command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        for path in [&run_root, &gate_bin] {
            let path = path.canonicalize().expect("canonical read mount");
            let rendered = path.to_string_lossy();
            assert!(
                args.iter().any(|arg| {
                    arg == &format!("type=bind,source={rendered},destination={rendered},readonly")
                }),
                "missing read-only Docker mount for {rendered}: {args:?}"
            );
        }
        let runtime = runtime.canonicalize().expect("canonical runtime");
        let rendered = runtime.to_string_lossy();
        assert!(
            args.iter().any(|arg| {
                arg == &format!("type=bind,source={rendered},destination={rendered}")
            }),
            "missing writable Docker runtime mount for {rendered}: {args:?}"
        );
    }

    #[test]
    fn docker_does_not_duplicate_exact_allow_and_deny_mount_destinations() {
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let ordinary_read = temp.path().join("ordinary-read");
        let protected_read = temp.path().join("gate-keys");
        let protected_write = temp.path().join("controller-proof");
        for directory in [&workspace, &ordinary_read, &protected_read] {
            std::fs::create_dir_all(directory).expect("fixture directory");
        }
        std::fs::write(&protected_write, b"controller").expect("protected write fixture");
        let mut spec = read_only_spec(SandboxBackend::Docker);
        spec.cwd = workspace;
        spec.workspace_access = WorkspaceAccess::Disposable;
        spec.read_allowlist = vec![ordinary_read.clone(), protected_read.clone()];
        spec.write_allowlist = vec![protected_write.clone()];
        spec.read_denylist = vec![protected_read.clone()];
        spec.write_denylist = vec![protected_write.clone()];

        let command =
            docker_command(&spec, PathBuf::from("/usr/bin/docker"), None).expect("command");
        let args = command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let ordinary_read = ordinary_read.canonicalize().expect("ordinary read");
        assert!(args.iter().any(|arg| {
            arg == &format!(
                "type=bind,source={},destination={},readonly",
                ordinary_read.display(),
                ordinary_read.display()
            )
        }));
        for protected in [protected_read, protected_write] {
            let protected = protected.canonicalize().expect("protected path");
            let destination = format!("destination={}", protected.display());
            assert!(
                args.iter().all(|arg| !arg.contains(&destination)),
                "an exactly denied path must remain unmounted instead of producing duplicate Docker destinations: {args:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn trusted_docker_execution_uses_immutable_image_sidecar_identity_and_labels() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let sidecar = temp.path().join("dr-gate-linux");
        let cid_file = temp.path().join("gate.cid");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(&sidecar, b"sidecar").expect("sidecar");
        let mut permissions = std::fs::metadata(&sidecar).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&sidecar, permissions).expect("permissions");
        let image_id = format!("sha256:{}", "a".repeat(64));
        let image = DockerImage::new(&image_id, DockerPlatform::LinuxArm64).expect("image");
        let execution = DockerExecution::new(
            image,
            &sidecar,
            "deadreckon-gate-launch-1",
            &cid_file,
            "job-1",
            "launch-1",
            2,
            Some("owner-1".to_string()),
        )
        .expect("execution");
        let mut spec = read_only_spec(SandboxBackend::Docker);
        spec.cwd = workspace;
        spec.workspace_access = WorkspaceAccess::Disposable;
        spec.docker = Some(execution);

        let command =
            docker_command(&spec, PathBuf::from("/usr/bin/docker"), None).expect("command");
        let args = command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        for expected in [
            "--pull=never",
            "--platform=linux/arm64",
            "--name",
            "deadreckon-gate-launch-1",
            "--cidfile",
            cid_file.to_str().expect("cidfile"),
            "io.deadreckon.managed=gate-evaluator",
            "io.deadreckon.job-id=job-1",
            "io.deadreckon.launch-id=launch-1",
            "io.deadreckon.attempt=2",
            "io.deadreckon.owner-launch-id=owner-1",
            &image_id,
            DOCKER_SIDECAR_CONTAINER_PROGRAM,
        ] {
            assert!(
                args.iter().any(|arg| arg == expected),
                "{expected}: {args:?}"
            );
        }
        let sidecar = sidecar.canonicalize().expect("canonical sidecar");
        assert!(args.iter().any(|arg| {
            arg == &format!(
                "type=bind,source={},destination={DOCKER_SIDECAR_CONTAINER_PROGRAM},readonly",
                sidecar.display()
            )
        }));
        let image_index = args
            .iter()
            .position(|arg| arg == &image_id)
            .expect("image ID");
        assert!(
            args.windows(2)
                .any(|pair| { pair == ["--entrypoint", DOCKER_SIDECAR_CONTAINER_PROGRAM] })
        );
        assert!(
            !args[..image_index].iter().any(|arg| arg == "judge")
                && !args[image_index + 1..].iter().any(|arg| arg == "judge"),
            "host program crossed into the container: {args:?}"
        );
    }

    #[test]
    fn ordinary_docker_without_typed_execution_keeps_legacy_program_shape() {
        let spec = read_only_spec(SandboxBackend::Docker);
        let command =
            docker_command(&spec, PathBuf::from("/usr/bin/docker"), None).expect("command");
        let args = command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args.windows(2).any(|pair| pair == ["rust:1", "judge"]));
        assert!(!args.iter().any(|arg| arg == "--pull=never"));
        assert!(!args.iter().any(|arg| arg.starts_with("--platform=")));
    }

    #[test]
    fn native_mount_inventory_omits_nonexistent_host_paths() {
        let temp = TempDir::new().expect("tempdir");
        let missing = temp.path().join("not-present");
        let mounts = system_read_allowlist(temp.path(), std::slice::from_ref(&missing));
        assert!(!mounts.contains(&missing), "{mounts:?}");
        assert!(mounts.iter().all(|path| path.exists()), "{mounts:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn production_wrapper_is_an_absolute_trusted_system_executable() {
        let temp = TempDir::new().expect("tempdir");
        let mut spec = read_only_spec(SandboxBackend::SandboxExec);
        spec.cwd = temp.path().to_path_buf();
        let command = super::build_command(&spec).expect("sandbox command");
        assert!(PathBuf::from(&command.program).is_absolute(), "{command:?}");
        assert_eq!(
            PathBuf::from(&command.program),
            PathBuf::from("/usr/bin/sandbox-exec")
                .canonicalize()
                .expect("system sandbox-exec"),
            "{command:?}"
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
        assert!(spec.read_denylist.contains(&paths.operator_captures_dir()));
        assert!(spec.write_denylist.contains(&paths.jobs_dir()));
        assert!(spec.write_denylist.contains(&paths.operator_captures_dir()));
        assert!(
            spec.write_denylist
                .contains(&PathBuf::from("/work/project/.git"))
        );
    }

    #[test]
    fn docker_serialization_cannot_reintroduce_gate_signing_inputs() {
        let mut spec = read_only_spec(SandboxBackend::Docker);
        for (name, value) in [
            (deadreckon_core::GATE_KEY_ENV, "must-not-cross"),
            (deadreckon_core::GATE_CONTAINED_ENV, "true"),
            (deadreckon_core::GATE_SANDBOX_BACKEND_ENV, "sandbox-exec"),
        ] {
            spec.env.insert(name.to_string(), value.to_string());
        }
        spec.env
            .insert("DEADRECKON_SAFE_INPUT".to_string(), "ordinary".to_string());

        // Exercise the final serializer directly as a second boundary behind
        // `with_protected_boundary`; Docker guest env is encoded into argv and
        // cannot be removed later by `Command::env_remove`.
        let command =
            docker_command(&spec, PathBuf::from("/usr/bin/docker"), None).expect("command");
        let args = command
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        for forbidden in [
            deadreckon_core::GATE_KEY_ENV,
            deadreckon_core::GATE_CONTAINED_ENV,
            deadreckon_core::GATE_SANDBOX_BACKEND_ENV,
            "must-not-cross",
        ] {
            assert!(
                !command.env.keys().any(|name| name == forbidden)
                    && args.iter().all(|arg| !arg.contains(forbidden)),
                "{forbidden} crossed the Docker command boundary: {command:?}"
            );
        }
        assert!(
            args.windows(2)
                .any(|pair| pair == ["-e", "DEADRECKON_SAFE_INPUT=ordinary"]),
            "ordinary Docker guest env was removed: {args:?}"
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
    fn seatbelt_serializes_the_complete_run_control_boundary() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("dr-home"));
        let run_root = paths.run_root("project", "run-1");
        std::fs::create_dir_all(run_root.join("proofs")).expect("proofs");
        std::fs::create_dir_all(run_root.join("snapshots")).expect("snapshots");
        std::fs::write(run_root.join("sandbox.toml"), "backend = 'sandbox-exec'\n")
            .expect("sandbox policy");
        std::fs::write(run_root.join("provenance.jsonl"), "{}\n").expect("provenance");
        let policy = ProtectedPathPolicy::for_paths(&paths);
        let mut spec = read_only_spec(SandboxBackend::SandboxExec);
        spec.read_denylist = policy.read_denylist;
        spec.write_denylist = policy.write_denylist;

        let profile = sandbox_exec_profile(&spec).expect("profile");
        let key_store = paths.home().join("gate-keys").display().to_string();
        let operator_captures = paths.operator_captures_dir().display().to_string();
        let proofs = run_root.join("proofs").display().to_string();
        let sandbox_policy = run_root.join("sandbox.toml").display().to_string();
        let snapshots = run_root.join("snapshots").display().to_string();
        let provenance = run_root.join("provenance.jsonl").display().to_string();
        assert!(profile.contains(&format!("(deny file-read* (literal \"{key_store}\"))")));
        assert!(profile.contains(&format!("(deny file-write* (literal \"{key_store}\"))")));
        assert!(profile.contains(&format!(
            "(deny file-read* (literal \"{operator_captures}\"))"
        )));
        assert!(profile.contains(&format!(
            "(deny file-write* (literal \"{operator_captures}\"))"
        )));
        assert!(profile.contains(&format!("(deny file-write* (literal \"{proofs}\"))")));
        assert!(!profile.contains(&format!("(deny file-read* (literal \"{proofs}\"))")));
        for protected in [sandbox_policy, snapshots, provenance] {
            assert!(
                profile.contains(&format!("(deny file-write* (literal \"{protected}\"))")),
                "Seatbelt omitted run control path {protected}: {profile}"
            );
            assert!(
                !profile.contains(&format!("(deny file-read* (literal \"{protected}\"))")),
                "Seatbelt made inspectable run evidence unreadable: {protected}"
            );
        }
    }

    #[test]
    fn hostile_container_backends_mask_visible_control_paths() {
        let temp = TempDir::new().expect("tempdir");
        let workspace = temp.path().join("workspace");
        let paths = DeadreckonPaths::from_home(workspace.join(".deadreckon"));
        let run_root = paths.run_root("project", "run-1");
        std::fs::create_dir_all(paths.home().join("gate-keys")).expect("keys");
        std::fs::create_dir_all(paths.jobs_dir()).expect("jobs");
        std::fs::create_dir_all(paths.operator_captures_dir()).expect("operator captures");
        std::fs::create_dir_all(run_root.join("proofs")).expect("proofs");
        std::fs::create_dir_all(run_root.join("snapshots")).expect("snapshots");
        std::fs::write(run_root.join("sandbox.toml"), "backend = 'container'\n")
            .expect("sandbox policy");
        std::fs::write(run_root.join("provenance.jsonl"), "{}\n").expect("provenance");
        let policy = ProtectedPathPolicy::for_paths(&paths);
        let mut spec = read_only_spec(SandboxBackend::Bwrap);
        spec.cwd = workspace;
        spec.workspace_access = WorkspaceAccess::ReadWrite;
        spec.read_denylist = policy.read_denylist;
        spec.write_denylist = policy.write_denylist;

        let bwrap = bwrap_command(&spec, PathBuf::from("/usr/bin/bwrap"), None).expect("bwrap");
        let bwrap_args = bwrap
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let key_store = paths.home().join("gate-keys").display().to_string();
        let jobs = paths.jobs_dir().display().to_string();
        let operator_captures = paths.operator_captures_dir().display().to_string();
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
        assert!(
            bwrap_args
                .windows(2)
                .any(|parts| parts == ["--tmpfs", &operator_captures])
        );
        for protected in [
            run_root.join("sandbox.toml"),
            run_root.join("snapshots"),
            run_root.join("provenance.jsonl"),
        ] {
            let protected = protected.display().to_string();
            assert!(
                bwrap_args
                    .windows(3)
                    .any(|parts| parts == ["--ro-bind", &protected, &protected]),
                "bubblewrap omitted read-only run control mount {protected}: {bwrap_args:?}"
            );
        }

        spec.backend = SandboxBackend::Docker;
        let docker =
            docker_command(&spec, PathBuf::from("/usr/bin/docker"), None).expect("command");
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
        assert!(
            docker_args
                .iter()
                .any(|arg| { arg == &format!("type=tmpfs,destination={operator_captures}") })
        );
        for protected in [
            run_root.join("sandbox.toml"),
            run_root.join("snapshots"),
            run_root.join("provenance.jsonl"),
        ] {
            let protected = protected.display();
            assert!(
                docker_args.iter().any(|arg| {
                    arg == &format!("type=bind,source={protected},destination={protected},readonly")
                }),
                "Docker omitted read-only run control mount {protected}: {docker_args:?}"
            );
        }
    }
}
