use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time::{Duration, sleep};

use crate::backend::{Result, SandboxBackend, SandboxError};
use crate::commands::build_command;
use crate::spec::SandboxSpec;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxRunOutput {
    pub backend: SandboxBackend,
    pub pid: Option<u32>,
    pub status_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub warning: Option<String>,
}

pub async fn run(spec: SandboxSpec) -> Result<SandboxRunOutput> {
    let command = build_command(&spec)?;
    let mut child = Command::new(&command.program)
        .args(&command.args)
        .current_dir(&command.cwd)
        .envs(&command.env)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pid = child.id();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_task = tokio::spawn(read_pipe(stdout));
    let stderr_task = tokio::spawn(read_pipe(stderr));
    if let (Some(pid), Some(pid_file)) = (pid, spec.pid_file.as_ref()) {
        if let Some(parent) = pid_file.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(pid_file, format!("{pid}\n")).await?;
    }
    let status = if let Some(token) = spec.cancellation_token.as_ref() {
        tokio::select! {
            _ = token.cancelled() => {
                if let Some(pid) = pid {
                    signal_pid(pid, false);
                    sleep(Duration::from_secs(2)).await;
                    if child.try_wait()?.is_none() {
                        signal_pid(pid, true);
                    }
                }
                let _ = child.wait().await;
                if let Some(pid_file) = spec.pid_file.as_ref() {
                    let _ = tokio::fs::remove_file(pid_file).await;
                }
                return Err(SandboxError::Cancelled);
            }
            status = child.wait() => status
        }
    } else {
        child.wait().await
    }?;
    let stdout = stdout_task
        .await
        .unwrap_or_else(|err| Ok(format!("stdout join error: {err}")))?;
    let stderr = stderr_task
        .await
        .unwrap_or_else(|err| Ok(format!("stderr join error: {err}")))?;
    if let Some(pid_file) = spec.pid_file.as_ref() {
        let _ = tokio::fs::remove_file(pid_file).await;
    }
    Ok(SandboxRunOutput {
        backend: command.backend,
        pid,
        status_code: status.code(),
        stdout,
        stderr,
        warning: command.warning,
    })
}

async fn read_pipe<R>(pipe: Option<R>) -> std::io::Result<String>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let Some(mut pipe) = pipe else {
        return Ok(String::new());
    };
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes).await?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

#[cfg(unix)]
fn signal_pid(pid: u32, force: bool) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let signal = if force {
        Signal::SIGKILL
    } else {
        Signal::SIGTERM
    };
    let _ = kill(Pid::from_raw(pid as i32), Some(signal));
}

#[cfg(not(unix))]
fn signal_pid(_pid: u32, _force: bool) {}
