#[cfg(unix)]
use std::io;
use std::process::Stdio;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio::time::{Duration, Instant, sleep};
use uuid::Uuid;

use crate::backend::{Result, SandboxBackend, SandboxError};
use crate::commands::build_command;
use crate::docker::{DockerExecution, reconcile_docker_execution, reconcile_docker_execution_by};
use crate::spec::{GuardedLaunchSpec, SandboxSpec, WorkspaceAccess};

const OUTPUT_CAPTURE_LIMIT_BYTES: usize = 1024 * 1024;
const OUTPUT_CAPTURE_HEAD_BYTES: usize = OUTPUT_CAPTURE_LIMIT_BYTES / 2;
const OUTPUT_CAPTURE_TAIL_BYTES: usize = OUTPUT_CAPTURE_LIMIT_BYTES - OUTPUT_CAPTURE_HEAD_BYTES;
const OUTPUT_READ_BUFFER_BYTES: usize = 64 * 1024;
const PROCESS_CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);
const PROCESS_TERM_GRACE: Duration = Duration::from_secs(2);
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(unix)]
const PROCESS_GROUP_TERM_GRACE: Duration = Duration::from_secs(2);
#[cfg(unix)]
const PROCESS_GROUP_KILL_CONFIRM_TIMEOUT: Duration = Duration::from_secs(2);
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
    scrub_inherited_deadreckon_environment(&mut process);
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
    if guarded.is_some()
        && let Some(boot_id) = std::env::var_os("DEADRECKON_BOOT_ID")
        && !boot_id.is_empty()
    {
        // The guarded helper must compare the child against the same boot
        // identity used to prepare its durable record. This is principally a
        // reboot-test seam; dr-gate removes it before executing repository
        // code, so it does not weaken the sandbox environment boundary.
        // Apply it last so neither ambient state nor caller-provided env can
        // replace the controller's boot identity.
        process.env("DEADRECKON_BOOT_ID", boot_id);
    }
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
                cleanup_spawned_execution(
                    &mut child,
                    pid,
                    guarded.is_some(),
                    supervise_process_group,
                    docker_execution.as_ref(),
                )
                .await?;
                return Err(error.into());
            }
        };
        if let Err(error) = deadreckon_core::write_supervised_process_record(pid_file, &record) {
            cleanup_spawned_execution(
                &mut child,
                pid,
                guarded.is_some(),
                supervise_process_group,
                docker_execution.as_ref(),
            )
            .await?;
            return Err(error.into());
        }
        process_record_identity = Some((guard.launch_id.clone(), pid));
        let Some(mut release) = child.stdin.take() else {
            cleanup_spawned_execution(
                &mut child,
                pid,
                guarded.is_some(),
                supervise_process_group,
                docker_execution.as_ref(),
            )
            .await?;
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
            cleanup_spawned_execution(
                &mut child,
                pid,
                guarded.is_some(),
                supervise_process_group,
                docker_execution.as_ref(),
            )
            .await?;
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
                cleanup_spawned_execution(
                    &mut child,
                    pid,
                    false,
                    supervise_process_group,
                    docker_execution.as_ref(),
                )
                .await?;
                return Err(error.into());
            }
        };
        if let Err(error) = deadreckon_core::write_supervised_process_record(pid_file, &record) {
            cleanup_spawned_execution(
                &mut child,
                pid,
                false,
                supervise_process_group,
                docker_execution.as_ref(),
            )
            .await?;
            return Err(error.into());
        }
        process_record_identity = Some((record.launch_id, pid));
    }
    let mut stdin_task = if guarded.is_none() {
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
                if let Some(stdin_task) = stdin_task.take() {
                    stdin_task.abort();
                }
                let cleanup = if let Some(pid) = pid {
                    cleanup_spawned_execution(
                        &mut child,
                        pid,
                        guarded.is_some(),
                        supervise_process_group,
                        docker_execution.as_ref(),
                    ).await
                } else {
                    reconcile_optional_docker(docker_execution.as_ref())
                };
                stdout_task.abort();
                stderr_task.abort();
                cleanup?;
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
    let cleanup_deadline = Instant::now() + PROCESS_CLEANUP_TIMEOUT;
    let process_cleanup = if let Some(pid) = pid.filter(|_| supervise_process_group) {
        cleanup_residual_process_tree_until(pid, cleanup_deadline).await
    } else {
        Ok(())
    };
    let docker_cleanup = reconcile_optional_docker_by(docker_execution.as_ref(), cleanup_deadline);
    process_cleanup?;
    docker_cleanup?;
    let status = status?;
    let stdout = join_pipe_until(stdout_task, cleanup_deadline, "stdout").await?;
    let stderr = join_pipe_until(stderr_task, cleanup_deadline, "stderr").await?;
    if let Some(stdin_task) = stdin_task {
        join_stdin_until(stdin_task, cleanup_deadline).await?;
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

async fn cleanup_spawned_execution(
    child: &mut Child,
    pid: u32,
    guarded: bool,
    supervise_process_group: bool,
    docker: Option<&DockerExecution>,
) -> Result<()> {
    let cleanup_deadline = Instant::now() + PROCESS_CLEANUP_TIMEOUT;
    let process_cleanup = terminate_spawned_child(
        child,
        pid,
        guarded,
        supervise_process_group,
        cleanup_deadline,
    )
    .await;
    let docker_cleanup = reconcile_optional_docker_by(docker, cleanup_deadline);
    process_cleanup?;
    docker_cleanup
}

async fn terminate_spawned_child(
    child: &mut Child,
    pid: u32,
    guarded: bool,
    supervise_process_group: bool,
    cleanup_deadline: Instant,
) -> Result<()> {
    signal_child_identity(pid, false, guarded, supervise_process_group);
    let term_deadline = cleanup_deadline.min(Instant::now() + PROCESS_TERM_GRACE);
    if wait_for_child_exit_until(child, term_deadline)
        .await?
        .is_none()
    {
        signal_child_identity(pid, true, guarded, supervise_process_group);
        if wait_for_child_exit_until(child, cleanup_deadline)
            .await?
            .is_none()
        {
            return Err(cleanup_timeout(format!(
                "could not prove sandbox process {pid} exited after SIGKILL; retaining process authority"
            )));
        }
    }
    if supervise_process_group {
        cleanup_residual_process_tree_until(pid, cleanup_deadline).await?;
    }
    Ok(())
}

fn signal_child_identity(pid: u32, force: bool, guarded: bool, process_group: bool) {
    if guarded {
        // Before release the guard is a raw child in the outer group; after
        // release it is the leader of its own group. Target both identities to
        // close that race.
        signal_process(pid, force, true);
        signal_process(pid, force, false);
    } else {
        signal_process(pid, force, process_group);
    }
}

async fn wait_for_child_exit_until(
    child: &mut Child,
    deadline: Instant,
) -> Result<Option<std::process::ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        sleep(PROCESS_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now()))).await;
    }
}

async fn join_pipe_until(
    mut task: JoinHandle<std::io::Result<String>>,
    deadline: Instant,
    stream: &str,
) -> Result<String> {
    if task.is_finished() {
        return task
            .await
            .unwrap_or_else(|error| Ok(format!("{stream} join error: {error}")))
            .map_err(SandboxError::Io);
    }
    tokio::select! {
        joined = &mut task => joined
            .unwrap_or_else(|error| Ok(format!("{stream} join error: {error}")))
            .map_err(SandboxError::Io),
        _ = tokio::time::sleep_until(deadline) => {
            task.abort();
            Err(cleanup_timeout(format!(
                "sandbox {stream} pipe remained open after process cleanup"
            )))
        }
    }
}

async fn join_stdin_until(
    mut task: JoinHandle<std::io::Result<()>>,
    deadline: Instant,
) -> Result<()> {
    if task.is_finished() {
        return task
            .await
            .unwrap_or_else(|error| {
                Err(std::io::Error::other(format!("stdin join error: {error}")))
            })
            .map_err(SandboxError::Io);
    }
    tokio::select! {
        joined = &mut task => joined
            .unwrap_or_else(|error| Err(std::io::Error::other(format!("stdin join error: {error}"))))
            .map_err(SandboxError::Io),
        _ = tokio::time::sleep_until(deadline) => {
            task.abort();
            Err(cleanup_timeout(
                "sandbox stdin pipe remained blocked after process cleanup".to_string(),
            ))
        }
    }
}

fn cleanup_timeout(message: String) -> SandboxError {
    SandboxError::CleanupIncomplete(message)
}

fn scrub_inherited_deadreckon_environment(process: &mut Command) {
    scrub_deadreckon_environment_names(process, std::env::vars_os().map(|(name, _)| name));
}

fn scrub_deadreckon_environment_names(
    process: &mut Command,
    names: impl IntoIterator<Item = std::ffi::OsString>,
) {
    for name in names {
        let rendered = name.to_string_lossy();
        if rendered
            .get(.."DEADRECKON_".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("DEADRECKON_"))
        {
            process.env_remove(name);
        }
    }
}

fn reconcile_optional_docker(execution: Option<&DockerExecution>) -> Result<()> {
    execution.map_or(Ok(()), reconcile_docker_execution)
}

fn reconcile_optional_docker_by(
    execution: Option<&DockerExecution>,
    deadline: Instant,
) -> Result<()> {
    execution.map_or(Ok(()), |execution| {
        reconcile_docker_execution_by(execution, deadline.into_std())
    })
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
async fn cleanup_residual_process_tree_until(pid: u32, cleanup_deadline: Instant) -> Result<()> {
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
    let term_deadline = cleanup_deadline.min(Instant::now() + PROCESS_GROUP_TERM_GRACE);
    if wait_for_process_group_exit_until(group, pid, term_deadline).await? {
        return Ok(());
    }
    if !signal_process_group(group, pid, Signal::SIGKILL, "kill")? {
        return Ok(());
    }
    let kill_deadline = cleanup_deadline.min(Instant::now() + PROCESS_GROUP_KILL_CONFIRM_TIMEOUT);
    if wait_for_process_group_exit_until(group, pid, kill_deadline).await? {
        return Ok(());
    }
    Err(SandboxError::CleanupIncomplete(format!(
        "could not prove sandbox process group {pid} exited after SIGKILL; retaining process authority"
    )))
}

#[cfg(not(unix))]
async fn cleanup_residual_process_tree_until(_pid: u32, _cleanup_deadline: Instant) -> Result<()> {
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
async fn wait_for_process_group_exit_until(
    group: nix::unistd::Pid,
    pid: i32,
    deadline: Instant,
) -> Result<bool> {
    loop {
        if process_group_is_absent(pid, nix::sys::signal::kill(group, None))? {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        sleep(PROCESS_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now()))).await;
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
    use std::ffi::OsString;

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

    #[test]
    fn inherited_deadreckon_environment_is_removed_before_authorized_env() {
        let mut command = tokio::process::Command::new("ignored");
        super::scrub_deadreckon_environment_names(
            &mut command,
            [
                OsString::from("PATH"),
                OsString::from("DEADRECKON_BUNDLE_BUILD_ID"),
                OsString::from("DEADRECKON_GATE_KEY"),
                OsString::from("deadreckon_case_insensitive"),
            ],
        );
        command.env("DEADRECKON_SAFE_INPUT", "ordinary");

        let configured = command
            .as_std()
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(configured.get("DEADRECKON_BUNDLE_BUILD_ID"), Some(&None));
        assert_eq!(configured.get("DEADRECKON_GATE_KEY"), Some(&None));
        assert_eq!(configured.get("deadreckon_case_insensitive"), Some(&None));
        assert_eq!(
            configured.get("DEADRECKON_SAFE_INPUT"),
            Some(&Some("ordinary".to_string()))
        );
        assert!(!configured.contains_key("PATH"));
    }
}
