use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;

use deadreckon_sandbox::{SandboxBackend, SandboxSpec, run as run_sandbox};
use tokio::process::Command;

use crate::{ProviderError, Result};

#[derive(Debug)]
pub(crate) struct CliOutput {
    pub stdout: String,
    pub stderr: String,
    pub status_code: Option<i32>,
    pub pid: Option<u32>,
    pub sandbox_backend: Option<SandboxBackend>,
    pub sandbox_warning: Option<String>,
}

pub(crate) async fn run_cli(
    provider: &str,
    binary: &str,
    args: &[String],
    cwd: Option<PathBuf>,
    sandbox_backend: Option<SandboxBackend>,
    pid_file: Option<PathBuf>,
) -> Result<CliOutput> {
    if let Some(backend) = sandbox_backend {
        let cwd = cwd.unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/Users/gdc/deadreckon"))
        });
        let output = run_sandbox(SandboxSpec {
            backend,
            cwd,
            program: OsString::from(binary),
            args: args.iter().map(OsString::from).collect(),
            env: BTreeMap::new(),
            allow_network: true,
            pid_file,
        })
        .await
        .map_err(|source| ProviderError::Cli {
            provider: provider.to_string(),
            detail: source.to_string(),
        })?;
        return Ok(CliOutput {
            stdout: output.stdout,
            stderr: output.stderr,
            status_code: output.status_code,
            pid: output.pid,
            sandbox_backend: Some(output.backend),
            sandbox_warning: output.warning,
        });
    }

    let mut command = Command::new(binary);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| ProviderError::Cli {
            provider: provider.to_string(),
            detail: source.to_string(),
        })?;
    let pid = child.id();
    if let (Some(pid), Some(pid_file)) = (pid, pid_file.as_ref()) {
        if let Some(parent) = pid_file.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|source| ProviderError::Io {
                    path: parent.display().to_string(),
                    source,
                })?;
        }
        tokio::fs::write(pid_file, format!("{pid}\n"))
            .await
            .map_err(|source| ProviderError::Io {
                path: pid_file.display().to_string(),
                source,
            })?;
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|source| ProviderError::Cli {
            provider: provider.to_string(),
            detail: source.to_string(),
        })?;
    if let Some(pid_file) = pid_file.as_ref() {
        let _ = tokio::fs::remove_file(pid_file).await;
    }
    Ok(CliOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        status_code: output.status.code(),
        pid,
        sandbox_backend: None,
        sandbox_warning: None,
    })
}

pub(crate) async fn write_output(path: Option<&PathBuf>, output: &CliOutput) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| ProviderError::Io {
                path: parent.display().to_string(),
                source,
            })?;
    }
    let body = format!("{}\n{}", output.stdout, output.stderr);
    tokio::fs::write(path, body)
        .await
        .map_err(|source| ProviderError::Io {
            path: path.display().to_string(),
            source,
        })
}

pub(crate) fn ensure_success(provider: &str, output: &CliOutput) -> Result<()> {
    if output.status_code == Some(0) {
        return Ok(());
    }
    Err(ProviderError::Cli {
        provider: provider.to_string(),
        detail: format!(
            "subprocess exited with {:?}: {}{}",
            output.status_code, output.stdout, output.stderr
        ),
    })
}
