use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::backend::{Result, SandboxBackend, resolve_backend};
use crate::spec::SandboxSpec;

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
    // REPORT.md: Disposable Sandboxes are selected per run and degrade to an
    // explicit unsafe warning only when requested or unsupported.
    let (backend, warning) = resolve_backend(spec.backend)?;
    match backend {
        SandboxBackend::SandboxExec => sandbox_exec_command(spec, warning),
        SandboxBackend::Bwrap => bwrap_command(spec, warning),
        SandboxBackend::Docker => docker_command(spec, warning),
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
    let sandbox_home = spec.cwd.join(".deadreckon-home");
    std::fs::create_dir_all(&sandbox_home)?;
    let mut args = vec![
        "--die-with-parent".into(),
        "--unshare-pid".into(),
        "--unshare-ipc".into(),
        "--unshare-uts".into(),
        "--tmpfs".into(),
        sandbox_home.to_string_lossy().to_string().into(),
        "--setenv".into(),
        "HOME".into(),
        sandbox_home.to_string_lossy().to_string().into(),
    ];
    for path in system_read_allowlist(&spec.cwd, &spec.read_allowlist) {
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
        "--bind".into(),
        cwd.clone().into(),
        cwd.clone().into(),
        "--tmpfs".into(),
        "/tmp".into(),
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

fn docker_command(spec: &SandboxSpec, warning: Option<String>) -> Result<SandboxCommand> {
    let cwd = spec.cwd.to_string_lossy().to_string();
    let mut args = vec![
        "run".into(),
        "--rm".into(),
        "-v".into(),
        format!("{cwd}:{cwd}").into(),
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
    args.push("rust:1".into());
    args.push(spec.program.clone());
    args.extend(spec.args.clone());
    Ok(SandboxCommand {
        backend: SandboxBackend::Docker,
        program: OsString::from("docker"),
        args,
        env: BTreeMap::new(),
        cwd: spec.cwd.clone(),
        warning,
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
    let profile = format!(
        "(version 1)
(allow default)
{ssh_deny}
{network}
(allow file-read*
{read_rules})
(allow file-write*
    (subpath \"{}\")
    (subpath \"/private/tmp\")
    (subpath \"/tmp\")
{write_rules})
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
