use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use tokio::process::Command;
use which::which;

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("invalid sandbox backend: {0}")]
    InvalidBackend(String),
    #[error("sandbox backend {0} is unavailable")]
    Unavailable(String),
    #[error("I/O error while running sandbox command: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, SandboxError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxBackend {
    Auto,
    SandboxExec,
    Bwrap,
    Docker,
    None,
}

impl fmt::Display for SandboxBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            SandboxBackend::Auto => "auto",
            SandboxBackend::SandboxExec => "sandbox-exec",
            SandboxBackend::Bwrap => "bwrap",
            SandboxBackend::Docker => "docker",
            SandboxBackend::None => "none",
        };
        f.write_str(value)
    }
}

impl FromStr for SandboxBackend {
    type Err = SandboxError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "sandbox-exec" => Ok(Self::SandboxExec),
            "bwrap" | "bubblewrap" => Ok(Self::Bwrap),
            "docker" => Ok(Self::Docker),
            "none" => Ok(Self::None),
            other => Err(SandboxError::InvalidBackend(other.to_string())),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SandboxSpec {
    pub backend: SandboxBackend,
    pub cwd: PathBuf,
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env: BTreeMap<String, String>,
    pub allow_network: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxCommand {
    pub backend: SandboxBackend,
    pub program: OsString,
    pub args: Vec<OsString>,
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxRunOutput {
    pub backend: SandboxBackend,
    pub status_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendAvailability {
    pub backend: SandboxBackend,
    pub available: bool,
    pub path: Option<PathBuf>,
    pub note: String,
}

pub async fn run(spec: SandboxSpec) -> Result<SandboxRunOutput> {
    let command = build_command(&spec)?;
    let output = Command::new(&command.program)
        .args(&command.args)
        .current_dir(&command.cwd)
        .envs(&command.env)
        .output()
        .await?;
    Ok(SandboxRunOutput {
        backend: command.backend,
        status_code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        warning: command.warning,
    })
}

pub fn build_command(spec: &SandboxSpec) -> Result<SandboxCommand> {
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
                    "sandbox backend none is unsafe; V0 permits it only for explicit smoke runs"
                        .to_string(),
                )
            }),
        }),
        SandboxBackend::Auto => unreachable!("resolve_backend never returns auto"),
    }
}

pub fn resolve_backend(backend: SandboxBackend) -> Result<(SandboxBackend, Option<String>)> {
    match backend {
        SandboxBackend::Auto => {
            #[cfg(target_os = "macos")]
            {
                if which("sandbox-exec").is_ok() {
                    return Ok((SandboxBackend::SandboxExec, None));
                }
                return Ok((
                    SandboxBackend::None,
                    Some("sandbox-exec not found; auto fell back to none".to_string()),
                ));
            }
            #[cfg(target_os = "linux")]
            {
                if which("bwrap").is_ok() {
                    return Ok((SandboxBackend::Bwrap, None));
                }
                return Ok((
                    SandboxBackend::None,
                    Some("bwrap not found; auto fell back to none".to_string()),
                ));
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                Ok((
                    SandboxBackend::None,
                    Some("no native sandbox backend for this platform; using none".to_string()),
                ))
            }
        }
        SandboxBackend::SandboxExec => require_binary("sandbox-exec", SandboxBackend::SandboxExec),
        SandboxBackend::Bwrap => require_binary("bwrap", SandboxBackend::Bwrap),
        SandboxBackend::Docker => require_binary("docker", SandboxBackend::Docker),
        SandboxBackend::None => Ok((SandboxBackend::None, None)),
    }
}

pub fn doctor() -> Vec<BackendAvailability> {
    [
        (SandboxBackend::SandboxExec, "sandbox-exec"),
        (SandboxBackend::Bwrap, "bwrap"),
        (SandboxBackend::Docker, "docker"),
    ]
    .into_iter()
    .map(|(backend, binary)| match which(binary) {
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

fn require_binary(name: &str, backend: SandboxBackend) -> Result<(SandboxBackend, Option<String>)> {
    if which(name).is_ok() {
        Ok((backend, None))
    } else {
        Err(SandboxError::Unavailable(backend.to_string()))
    }
}

fn sandbox_exec_command(spec: &SandboxSpec, warning: Option<String>) -> Result<SandboxCommand> {
    let profile = sandbox_exec_profile(spec.allow_network);
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
    let mut args = vec![
        "--die-with-parent".into(),
        "--unshare-pid".into(),
        "--unshare-ipc".into(),
        "--unshare-uts".into(),
        "--ro-bind".into(),
        "/".into(),
        "/".into(),
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
    ];
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

fn sandbox_exec_profile(allow_network: bool) -> String {
    let network = if allow_network {
        "(allow network*)"
    } else {
        "(deny network*)"
    };
    format!(
        "(version 1)
(allow default)
{network}
(allow file-read*)
(allow file-write*)
"
    )
}

fn missing_hint(backend: SandboxBackend) -> String {
    match backend {
        SandboxBackend::SandboxExec => "macOS sandbox-exec is not on PATH".to_string(),
        SandboxBackend::Bwrap => {
            "install bubblewrap (bwrap) for Linux native sandboxing".to_string()
        }
        SandboxBackend::Docker => "install Docker to use --sandbox docker".to_string(),
        SandboxBackend::Auto | SandboxBackend::None => "not applicable".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    use super::{SandboxBackend, SandboxSpec, build_command, run};

    fn shell_spec() -> SandboxSpec {
        SandboxSpec {
            backend: SandboxBackend::None,
            cwd: std::env::current_dir().expect("cwd"),
            program: OsString::from("sh"),
            args: vec![OsString::from("-c"), OsString::from("printf ok")],
            env: BTreeMap::new(),
            allow_network: false,
        }
    }

    #[tokio::test]
    async fn none_backend_runs_command_with_warning() {
        let output = run(shell_spec()).await.expect("run");
        assert_eq!(output.status_code, Some(0));
        assert_eq!(output.stdout, "ok");
        assert!(output.warning.expect("warning").contains("unsafe"));
    }

    #[test]
    fn docker_backend_builds_network_none_when_requested() {
        let mut spec = shell_spec();
        spec.backend = SandboxBackend::Docker;
        spec.allow_network = false;
        let command = build_command(&spec).unwrap_or_else(|_| {
            let mut fallback = spec.clone();
            fallback.backend = SandboxBackend::None;
            build_command(&fallback).expect("fallback")
        });
        if command.backend == SandboxBackend::Docker {
            assert!(command.args.iter().any(|arg| arg == "--network"));
            assert!(command.args.iter().any(|arg| arg == "none"));
        }
    }
}
