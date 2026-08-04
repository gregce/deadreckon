#[cfg(unix)]
use std::io;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::time::{Duration, sleep};
use uuid::Uuid;

use crate::backend::{Result, SandboxBackend, SandboxError};
use crate::commands::build_command;
use crate::docker::{DockerExecution, reconcile_docker_execution};
use crate::spec::{GuardedLaunchSpec, SandboxSpec, WorkspaceAccess};

const OUTPUT_CAPTURE_LIMIT_BYTES: usize = 1024 * 1024;
const OUTPUT_CAPTURE_HEAD_BYTES: usize = OUTPUT_CAPTURE_LIMIT_BYTES / 2;
const OUTPUT_CAPTURE_TAIL_BYTES: usize = OUTPUT_CAPTURE_LIMIT_BYTES - OUTPUT_CAPTURE_HEAD_BYTES;
const OUTPUT_READ_BUFFER_BYTES: usize = 64 * 1024;

#[cfg(unix)]
const PROCESS_GROUP_TERM_GRACE: Duration = Duration::from_millis(250);
#[cfg(unix)]
const PROCESS_GROUP_KILL_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(unix)]
const PROCESS_GROUP_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxRunOutput {
    pub backend: SandboxBackend,
    pub pid: Option<u32>,
    pub status_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub warning: Option<String>,
}

pub async fn run(mut spec: SandboxSpec) -> Result<SandboxRunOutput> {
    let command = build_command(&spec)?;
    let guarded = validate_guarded_launch(&spec)?;
    let docker_execution = if command.backend == SandboxBackend::Docker {
        spec.docker.clone()
    } else {
        None
    };
    if let Some(docker) = docker_execution.as_ref() {
        validate_docker_guard_identity(&spec, docker, guarded.as_ref())?;
        // Reusing an approved launch identity after a worker crash must first
        // reconcile the daemon-owned container, not merely its old host client.
        reconcile_docker_execution(docker)?;
    }
    let guarded_release = guarded.as_ref().map(|guard| {
        let token = format!("{}:{}", guard.launch_id, Uuid::new_v4());
        let digest = deadreckon_core::flight::sha256_text(&token);
        (token, digest)
    });
    // Every persisted process identity must own a fresh group on Unix. This
    // lets a restarted controller reconcile the provider and all descendants
    // from the identity-bound sidecar instead of trusting a raw PID.
    let supervise_process_group = spec.cleanup_process_group || spec.pid_file.is_some();
    let mut process =
        if let (Some(guard), Some((_, digest))) = (guarded.as_ref(), guarded_release.as_ref()) {
            let pid_file = spec.pid_file.as_ref().ok_or_else(|| {
                SandboxError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "guarded sandbox launch requires a pid file",
                ))
            })?;
            let mut process = Command::new(&guard.program);
            process
                .arg("guarded-exec")
                .arg("--metadata")
                .arg(pid_file)
                .arg("--launch-id")
                .arg(&guard.launch_id)
                .arg("--attempt")
                .arg(guard.attempt.to_string())
                .arg("--release-token-sha256")
                .arg(digest)
                .arg("--")
                .arg(&command.program)
                .args(&command.args);
            process
        } else {
            let mut process = Command::new(&command.program);
            process.args(&command.args);
            process
        };
    process.current_dir(&command.cwd);
    if spec.workspace_access == WorkspaceAccess::Disposable {
        // Strict independent evaluation receives an explicit environment.
        // Repository-controlled checks must not inherit provider credentials,
        // signing material, or host-specific command routing.
        process.env_clear();
    }
    if guarded.is_some()
        && let Some(boot_id) = std::env::var_os("DEADRECKON_BOOT_ID")
        && !boot_id.is_empty()
    {
        // The guarded helper must compare the child against the same boot
        // identity used to prepare its durable record. This is principally a
        // reboot-test seam; dr-gate removes it before executing repository
        // code, so it does not weaken the sandbox environment boundary.
        process.env("DEADRECKON_BOOT_ID", boot_id);
    }
    process
        .envs(&command.env)
        // Signing inputs belong only to the trusted gate-signing phase. The
        // common sandbox boundary must scrub inherited copies even when the
        // caller did not put them in `SandboxSpec::env`.
        .env_remove(deadreckon_core::GATE_KEY_ENV)
        .env_remove(deadreckon_core::GATE_CONTAINED_ENV)
        .env_remove(deadreckon_core::GATE_SANDBOX_BACKEND_ENV)
        .stdin(if spec.stdin.is_some() || guarded.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A guarded helper must remain in the worker's existing group until its
    // identity is durable. It creates the fresh group itself only after the
    // private release capability is validated.
    configure_process_tree(&mut process, supervise_process_group && guarded.is_none());
    let mut child = process.spawn()?;
    let pid = child.id();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_task = tokio::spawn(read_pipe(stdout));
    let stderr_task = tokio::spawn(read_pipe(stderr));
    let mut process_record_identity = None;
    if let (Some(pid), Some(pid_file), Some(guard), Some((token, digest))) = (
        pid,
        spec.pid_file.as_ref(),
        guarded.as_ref(),
        guarded_release.as_ref(),
    ) {
        let record = match deadreckon_core::SupervisedProcessRecord::prepared(
            deadreckon_core::SupervisedProcess { pid, pgid: None },
            guard.launch_id.clone(),
            guard.attempt,
            guard.owner_launch_id.clone(),
            digest.clone(),
        ) {
            Ok(record) => record,
            Err(error) => {
                signal_process(pid, true, false);
                let _ = child.wait().await;
                reconcile_optional_docker(docker_execution.as_ref())?;
                return Err(error.into());
            }
        };
        if let Err(error) = deadreckon_core::write_supervised_process_record(pid_file, &record) {
            signal_process(pid, true, false);
            let _ = child.wait().await;
            reconcile_optional_docker(docker_execution.as_ref())?;
            return Err(error.into());
        }
        process_record_identity = Some((guard.launch_id.clone(), pid));
        let Some(mut release) = child.stdin.take() else {
            signal_process(pid, true, false);
            let _ = child.wait().await;
            cleanup_residual_process_tree(pid).await?;
            reconcile_optional_docker(docker_execution.as_ref())?;
            remove_pid_file(&spec, process_record_identity.as_ref()).await?;
            return Err(SandboxError::Io(std::io::Error::other(
                "guarded sandbox helper did not expose its release pipe",
            )));
        };
        if let Err(error) = async {
            release.write_all(token.as_bytes()).await?;
            release.write_all(b"\n").await?;
            release.shutdown().await
        }
        .await
        {
            signal_process(pid, true, false);
            let _ = child.wait().await;
            let process_cleanup = cleanup_residual_process_tree(pid).await;
            let docker_cleanup = reconcile_optional_docker(docker_execution.as_ref());
            process_cleanup?;
            docker_cleanup?;
            remove_pid_file(&spec, process_record_identity.as_ref()).await?;
            return Err(error.into());
        }
    } else if let (Some(pid), Some(pid_file)) = (pid, spec.pid_file.as_ref()) {
        let record = match deadreckon_core::SupervisedProcessRecord::running(
            deadreckon_core::SupervisedProcess {
                pid,
                pgid: if cfg!(unix) { Some(pid) } else { None },
            },
        ) {
            Ok(record) => record,
            Err(error) => {
                signal_process(pid, true, supervise_process_group);
                let _ = child.wait().await;
                cleanup_residual_process_tree(pid).await?;
                reconcile_optional_docker(docker_execution.as_ref())?;
                return Err(error.into());
            }
        };
        if let Err(error) = deadreckon_core::write_supervised_process_record(pid_file, &record) {
            signal_process(pid, true, supervise_process_group);
            let _ = child.wait().await;
            cleanup_residual_process_tree(pid).await?;
            reconcile_optional_docker(docker_execution.as_ref())?;
            return Err(error.into());
        }
        process_record_identity = Some((record.launch_id, pid));
    }
    let stdin_task = if guarded.is_none() {
        spec.stdin.take().and_then(|bytes| {
            child.stdin.take().map(|mut stdin| {
                tokio::spawn(async move {
                    stdin.write_all(&bytes).await?;
                    stdin.shutdown().await
                })
            })
        })
    } else {
        None
    };
    let status = if let Some(token) = spec.cancellation_token.as_ref() {
        tokio::select! {
            _ = token.cancelled() => {
                if let Some(pid) = pid {
                    if guarded.is_some() {
                        // Before release the guard is a raw child in the outer
                        // group; after release it is the leader of its own
                        // group. Target both identities to close that race.
                        signal_process(pid, false, true);
                        signal_process(pid, false, false);
                    } else {
                        signal_process(pid, false, supervise_process_group);
                    }
                    sleep(Duration::from_secs(2)).await;
                    if child.try_wait()?.is_none() {
                        if guarded.is_some() {
                            signal_process(pid, true, true);
                            signal_process(pid, true, false);
                        } else {
                            signal_process(pid, true, supervise_process_group);
                        }
                    }
                }
                let _ = child.wait().await;
                let process_cleanup = if let Some(pid) =
                    pid.filter(|_| supervise_process_group)
                {
                    cleanup_residual_process_tree(pid).await
                } else {
                    Ok(())
                };
                let docker_cleanup = reconcile_optional_docker(docker_execution.as_ref());
                process_cleanup?;
                docker_cleanup?;
                remove_pid_file(&spec, process_record_identity.as_ref()).await?;
                return Err(SandboxError::Cancelled);
            }
            status = child.wait() => status
        }
    } else {
        child.wait().await
    };
    // A check can exit its direct shell while leaving background descendants
    // alive. Clean the process group before consuming output or returning
    // control to a caller that may subsequently read a signing key.
    let process_cleanup = if let Some(pid) = pid.filter(|_| supervise_process_group) {
        cleanup_residual_process_tree(pid).await
    } else {
        Ok(())
    };
    let docker_cleanup = reconcile_optional_docker(docker_execution.as_ref());
    process_cleanup?;
    docker_cleanup?;
    let status = status?;
    let stdout = stdout_task
        .await
        .unwrap_or_else(|err| Ok(format!("stdout join error: {err}")))?;
    let stderr = stderr_task
        .await
        .unwrap_or_else(|err| Ok(format!("stderr join error: {err}")))?;
    if let Some(stdin_task) = stdin_task {
        stdin_task
            .await
            .unwrap_or_else(|err| Err(std::io::Error::other(format!("stdin join error: {err}"))))?;
    }
    remove_pid_file(&spec, process_record_identity.as_ref()).await?;
    Ok(SandboxRunOutput {
        backend: command.backend,
        pid,
        status_code: status.code(),
        stdout,
        stderr,
        warning: command.warning,
    })
}

fn reconcile_optional_docker(execution: Option<&DockerExecution>) -> Result<()> {
    execution.map_or(Ok(()), reconcile_docker_execution)
}

fn validate_docker_guard_identity(
    spec: &SandboxSpec,
    docker: &DockerExecution,
    guarded: Option<&GuardedLaunchSpec>,
) -> Result<()> {
    if spec.workspace_access != WorkspaceAccess::Disposable {
        return Err(SandboxError::InvalidDockerExecution(
            "trusted Docker execution requires disposable workspace access".to_string(),
        ));
    }
    let guard = guarded.ok_or_else(|| {
        SandboxError::InvalidDockerExecution(
            "trusted Docker execution requires a guarded launch".to_string(),
        )
    })?;
    if docker.launch_id() != guard.launch_id
        || docker.attempt() != guard.attempt
        || docker.owner_launch_id() != guard.owner_launch_id.as_deref()
    {
        return Err(SandboxError::InvalidDockerExecution(
            "Docker container labels do not match the guarded launch identity".to_string(),
        ));
    }
    let pid_file = spec.pid_file.as_deref().ok_or_else(|| {
        SandboxError::InvalidDockerExecution(
            "trusted Docker execution requires a guarded process record".to_string(),
        )
    })?;
    if docker.cid_file().parent() != pid_file.parent() {
        return Err(SandboxError::InvalidDockerExecution(
            "Docker cidfile must share the guarded process-record directory".to_string(),
        ));
    }
    let workspace = spec.cwd.canonicalize().unwrap_or_else(|_| spec.cwd.clone());
    if docker.sidecar_host_path().starts_with(&workspace) {
        return Err(SandboxError::InvalidDockerExecution(
            "Docker evaluator sidecar must live outside the writable workspace".to_string(),
        ));
    }
    Ok(())
}

fn validate_guarded_launch(spec: &SandboxSpec) -> Result<Option<GuardedLaunchSpec>> {
    let Some(guard) = spec.guarded_launch.clone() else {
        return Ok(None);
    };
    if !spec.cleanup_process_group
        || spec.pid_file.is_none()
        || spec.stdin.is_some()
        || guard.program.is_empty()
        || guard.launch_id.trim().is_empty()
        || guard.attempt == 0
    {
        return Err(SandboxError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "guarded sandbox launch requires cleanup, a pid file, no command stdin, and complete launch identity",
        )));
    }
    Ok(Some(guard))
}

async fn remove_pid_file(
    spec: &SandboxSpec,
    process_record_identity: Option<&(String, u32)>,
) -> Result<()> {
    let Some(path) = spec.pid_file.as_ref() else {
        return Ok(());
    };
    if let Some((launch_id, pid)) = process_record_identity {
        let removed =
            deadreckon_core::remove_supervised_process_record_if_matches(path, launch_id, *pid)?;
        if !removed {
            match std::fs::symlink_metadata(path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Ok(_) => {
                    return Err(SandboxError::Io(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "supervised process identity changed before cleanup",
                    )));
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn configure_process_tree(command: &mut Command, cleanup_process_group: bool) {
    use std::os::unix::process::CommandExt as _;

    if cleanup_process_group {
        command.as_std_mut().process_group(0);
    }
}

#[cfg(not(unix))]
fn configure_process_tree(_command: &mut Command, _cleanup_process_group: bool) {}

#[cfg(unix)]
async fn cleanup_residual_process_tree(pid: u32) -> Result<()> {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let pid = i32::try_from(pid)
        .map_err(|_| SandboxError::Io(io::Error::other("sandbox process id exceeds i32")))?;
    if pid <= 0 {
        return Err(SandboxError::Io(io::Error::other(
            "sandbox process id must be positive",
        )));
    }
    let group = Pid::from_raw(-pid);
    if process_group_is_absent(pid, kill(group, None))? {
        return Ok(());
    }
    if !signal_process_group(group, pid, Signal::SIGTERM, "terminate")? {
        return Ok(());
    }
    if wait_for_process_group_exit(group, pid, PROCESS_GROUP_TERM_GRACE).await? {
        return Ok(());
    }
    if !signal_process_group(group, pid, Signal::SIGKILL, "kill")? {
        return Ok(());
    }
    if wait_for_process_group_exit(group, pid, PROCESS_GROUP_KILL_CONFIRM_TIMEOUT).await? {
        return Ok(());
    }
    Err(SandboxError::Io(io::Error::new(
        io::ErrorKind::TimedOut,
        format!(
            "could not prove sandbox process group {pid} exited after SIGKILL; retaining process authority"
        ),
    )))
}

#[cfg(not(unix))]
async fn cleanup_residual_process_tree(_pid: u32) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn process_tree_error(operation: &str, pid: i32, error: nix::errno::Errno) -> SandboxError {
    SandboxError::Io(io::Error::other(format!(
        "failed to {operation} sandbox process group {pid}: {error}"
    )))
}

#[cfg(unix)]
fn process_group_is_absent(
    pid: i32,
    probe: std::result::Result<(), nix::errno::Errno>,
) -> Result<bool> {
    match probe {
        Err(nix::errno::Errno::ESRCH) => Ok(true),
        Ok(()) => Ok(false),
        Err(error) => Err(process_tree_error("inspect", pid, error)),
    }
}

#[cfg(unix)]
fn signal_process_group(
    group: nix::unistd::Pid,
    pid: i32,
    signal: nix::sys::signal::Signal,
    operation: &str,
) -> Result<bool> {
    match nix::sys::signal::kill(group, Some(signal)) {
        Err(nix::errno::Errno::ESRCH) => Ok(false),
        Ok(()) => Ok(true),
        Err(error) => Err(process_tree_error(operation, pid, error)),
    }
}

#[cfg(unix)]
async fn wait_for_process_group_exit(
    group: nix::unistd::Pid,
    pid: i32,
    timeout: Duration,
) -> Result<bool> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if process_group_is_absent(pid, nix::sys::signal::kill(group, None))? {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        sleep(PROCESS_GROUP_POLL_INTERVAL).await;
    }
}

async fn read_pipe<R>(pipe: Option<R>) -> std::io::Result<String>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let Some(mut pipe) = pipe else {
        return Ok(String::new());
    };
    let mut capture = BoundedOutputCapture::default();
    let mut buffer = [0_u8; OUTPUT_READ_BUFFER_BYTES];
    loop {
        let read = pipe.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        capture.push(&buffer[..read]);
    }
    Ok(capture.finish())
}

#[derive(Default)]
struct BoundedOutputCapture {
    head: Vec<u8>,
    tail: Vec<u8>,
    total_bytes: usize,
}

impl BoundedOutputCapture {
    fn push(&mut self, bytes: &[u8]) {
        self.total_bytes = self.total_bytes.saturating_add(bytes.len());
        let head_remaining = OUTPUT_CAPTURE_HEAD_BYTES.saturating_sub(self.head.len());
        let head_bytes = head_remaining.min(bytes.len());
        self.head.extend_from_slice(&bytes[..head_bytes]);
        self.push_tail(&bytes[head_bytes..]);
    }

    fn push_tail(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if bytes.len() >= OUTPUT_CAPTURE_TAIL_BYTES {
            self.tail.clear();
            self.tail
                .extend_from_slice(&bytes[bytes.len() - OUTPUT_CAPTURE_TAIL_BYTES..]);
            return;
        }
        let overflow = self
            .tail
            .len()
            .saturating_add(bytes.len())
            .saturating_sub(OUTPUT_CAPTURE_TAIL_BYTES);
        if overflow > 0 {
            self.tail.drain(..overflow);
        }
        self.tail.extend_from_slice(bytes);
    }

    fn finish(self) -> String {
        let omitted = self
            .total_bytes
            .saturating_sub(self.head.len().saturating_add(self.tail.len()));
        let mut retained = self.head;
        if omitted > 0 {
            retained.extend_from_slice(
                format!(
                    "\n[... {omitted} bytes omitted; sandbox output bounded to {OUTPUT_CAPTURE_LIMIT_BYTES} bytes ...]\n"
                )
                .as_bytes(),
            );
        }
        retained.extend_from_slice(&self.tail);
        String::from_utf8_lossy(&retained).into_owned()
    }
}

#[cfg(unix)]
fn signal_process(pid: u32, force: bool, process_group: bool) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let signal = if force {
        Signal::SIGKILL
    } else {
        Signal::SIGTERM
    };
    if let Ok(pid) = i32::try_from(pid) {
        let target = if process_group { -pid } else { pid };
        let _ = kill(Pid::from_raw(target), Some(signal));
    }
}

#[cfg(not(unix))]
fn signal_process(_pid: u32, _force: bool, _process_group: bool) {}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncWriteExt as _;

    use super::{OUTPUT_CAPTURE_LIMIT_BYTES, read_pipe};

    #[tokio::test]
    async fn output_flood_is_drained_while_retaining_bounded_head_and_tail() {
        let (mut writer, reader) = tokio::io::duplex(1024);
        let writer_task = tokio::spawn(async move {
            writer.write_all(b"stdout-head-sentinel\n").await?;
            let flood = vec![b'x'; 64 * 1024];
            for _ in 0..32 {
                writer.write_all(&flood).await?;
            }
            writer.write_all(b"\nstdout-tail-sentinel").await?;
            writer.shutdown().await
        });

        let captured = read_pipe(Some(reader)).await.expect("bounded capture");
        writer_task.await.expect("writer join").expect("writer");

        assert!(captured.starts_with("stdout-head-sentinel\n"));
        assert!(captured.ends_with("\nstdout-tail-sentinel"));
        assert!(captured.contains("bytes omitted; sandbox output bounded"));
        assert!(captured.len() <= OUTPUT_CAPTURE_LIMIT_BYTES + 128);
    }

    #[cfg(unix)]
    #[test]
    fn permission_denied_does_not_prove_process_group_absence() {
        let error = super::process_group_is_absent(123, Err(nix::errno::Errno::EPERM))
            .expect_err("EPERM must retain process authority");

        assert!(error.to_string().contains("failed to inspect"));
        assert!(error.to_string().contains("EPERM"));
    }
}
