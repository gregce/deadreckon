use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use deadreckon_core::HeadTailBuffer;
use deadreckon_sandbox::{
    SandboxBackend, SandboxSpec, ToolSandboxPolicy, WorkspaceAccess, run as run_sandbox,
};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::registry::ProviderRegistry;
use crate::{ProviderError, Result};

pub(crate) const CLI_CAPABILITY_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CLI_CAPABILITY_PROBE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);
// Leave two seconds inside the oldest 10-second phase cleanup budget for the
// caller to classify and persist retained authority after this inner proof.
const CLI_PROCESS_CLEANUP_TIMEOUT: Duration = Duration::from_secs(8);
const CLI_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub(crate) struct CliOutput {
    pub stdout: String,
    pub stderr: String,
    pub status_code: Option<i32>,
    pub pid: Option<u32>,
    pub sandbox_backend: Option<SandboxBackend>,
    pub sandbox_warning: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_cli(
    provider: &str,
    binary: &str,
    args: &[String],
    cwd: Option<PathBuf>,
    sandbox_backend: Option<SandboxBackend>,
    pid_file: Option<PathBuf>,
    cancellation_token: Option<CancellationToken>,
    workspace_access: WorkspaceAccess,
    inner_read_only_enforced: bool,
) -> Result<CliOutput> {
    run_cli_with_options(
        provider,
        binary,
        args,
        CliRunOptions {
            cwd,
            sandbox_backend,
            pid_file,
            cancellation_token,
            extra_read_allowlist: Vec::new(),
            extra_write_allowlist: Vec::new(),
            extra_write_denylist: Vec::new(),
            workspace_access,
            inner_read_only_enforced,
        },
    )
    .await
}

/// Run a CLI capability probe under the same process authority as the provider
/// request, but with its own bounded cancellation token.
///
/// Capability discovery is advisory: launch failures, non-zero exits, and a
/// clean timeout return `Ok(None)` so the adapter can degrade its optional
/// features. Controller cancellation and any failure to prove PID-authority
/// cleanup remain hard errors.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_cli_capability_probe(
    provider: &str,
    binary: &str,
    args: &[String],
    cwd: Option<PathBuf>,
    sandbox_backend: Option<SandboxBackend>,
    pid_file: Option<PathBuf>,
    external_cancellation: Option<CancellationToken>,
    workspace_access: WorkspaceAccess,
    inner_read_only_enforced: bool,
    timeout: Duration,
) -> Result<Option<CliOutput>> {
    if external_cancellation
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled)
    {
        return Err(ProviderError::Cancelled {
            provider: provider.to_string(),
            detail: "request cancelled before capability probe".to_string(),
        });
    }

    let probe_cancellation = CancellationToken::new();
    let completion = run_cli(
        provider,
        binary,
        args,
        cwd,
        sandbox_backend,
        pid_file.clone(),
        Some(probe_cancellation.clone()),
        workspace_access,
        inner_read_only_enforced,
    );
    tokio::pin!(completion);

    enum ProbeBoundary<T> {
        Finished(T),
        TimedOut,
        Cancelled,
    }

    let boundary = if let Some(external) = external_cancellation.as_ref() {
        tokio::select! {
            biased;
            () = external.cancelled() => ProbeBoundary::Cancelled,
            result = &mut completion => ProbeBoundary::Finished(result),
            () = tokio::time::sleep(timeout) => ProbeBoundary::TimedOut,
        }
    } else {
        tokio::select! {
            result = &mut completion => ProbeBoundary::Finished(result),
            () = tokio::time::sleep(timeout) => ProbeBoundary::TimedOut,
        }
    };

    match boundary {
        ProbeBoundary::Finished(result) => {
            prove_capability_probe_cleanup(provider, pid_file.as_deref())?;
            Ok(result.ok().filter(|output| output.status_code == Some(0)))
        }
        ProbeBoundary::TimedOut => {
            probe_cancellation.cancel();
            // `run_cli` returns only after it has drained the child pipes,
            // reaped the direct child, reconciled descendants, and removed the
            // identity-bound PID record. Do not drop this future at the phase
            // boundary: awaiting it is the cleanup proof.
            if tokio::time::timeout(CLI_CAPABILITY_PROBE_CLEANUP_TIMEOUT, &mut completion)
                .await
                .is_err()
            {
                return Err(ProviderError::CleanupIncomplete {
                    provider: provider.to_string(),
                    authority: pid_file.clone(),
                    detail: format!(
                        "capability probe cleanup exceeded the bounded {}s safety window",
                        CLI_CAPABILITY_PROBE_CLEANUP_TIMEOUT.as_secs()
                    ),
                });
            }
            prove_capability_probe_cleanup(provider, pid_file.as_deref())?;
            Ok(None)
        }
        ProbeBoundary::Cancelled => {
            probe_cancellation.cancel();
            if tokio::time::timeout(CLI_CAPABILITY_PROBE_CLEANUP_TIMEOUT, &mut completion)
                .await
                .is_err()
            {
                return Err(ProviderError::CleanupIncomplete {
                    provider: provider.to_string(),
                    authority: pid_file.clone(),
                    detail: format!(
                        "cancelled capability probe cleanup exceeded the bounded {}s safety window",
                        CLI_CAPABILITY_PROBE_CLEANUP_TIMEOUT.as_secs()
                    ),
                });
            }
            prove_capability_probe_cleanup(provider, pid_file.as_deref())?;
            Err(ProviderError::Cancelled {
                provider: provider.to_string(),
                detail: "request cancelled during capability probe".to_string(),
            })
        }
    }
}

fn prove_capability_probe_cleanup(provider: &str, pid_file: Option<&Path>) -> Result<()> {
    let Some(pid_file) = pid_file else {
        return Ok(());
    };
    match pid_authority_absent(pid_file) {
        Ok(true) => Ok(()),
        Ok(false) => Err(ProviderError::CleanupIncomplete {
            provider: provider.to_string(),
            authority: Some(pid_file.to_path_buf()),
            detail: format!(
                "capability probe returned without proving process cleanup; process authority remains at {}",
                pid_file.display()
            ),
        }),
        Err(source) => Err(ProviderError::CleanupIncomplete {
            provider: provider.to_string(),
            authority: Some(pid_file.to_path_buf()),
            detail: format!("could not inspect capability-probe PID authority: {source}"),
        }),
    }
}

/// Only a confirmed `NotFound` means process authority is absent. Following a
/// symlink with `Path::exists` would incorrectly treat dangling authority as
/// clean, while swallowing other metadata errors would turn an inspection
/// failure into false cleanup proof.
fn pid_authority_absent(path: &Path) -> std::io::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Ok(_) => Ok(false),
        Err(error) => Err(error),
    }
}

pub(crate) struct CliRunOptions {
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) sandbox_backend: Option<SandboxBackend>,
    pub(crate) pid_file: Option<PathBuf>,
    pub(crate) cancellation_token: Option<CancellationToken>,
    pub(crate) extra_read_allowlist: Vec<PathBuf>,
    pub(crate) extra_write_allowlist: Vec<PathBuf>,
    pub(crate) extra_write_denylist: Vec<PathBuf>,
    pub(crate) workspace_access: WorkspaceAccess,
    /// True only when the provider CLI has its own enforceable read-only
    /// sandbox (currently Codex). This permits a read-only request without an
    /// outer backend while preserving fail-closed behavior for other CLIs.
    pub(crate) inner_read_only_enforced: bool,
}

pub(crate) async fn run_cli_with_options(
    provider: &str,
    binary: &str,
    args: &[String],
    options: CliRunOptions,
) -> Result<CliOutput> {
    let cleanup_process_group = options.cancellation_token.is_some();
    if options.workspace_access == WorkspaceAccess::ReadOnly
        && options.sandbox_backend.is_none()
        && !options.inner_read_only_enforced
    {
        return Err(ProviderError::Cli {
            provider: provider.to_string(),
            detail: "read-only workspace access requires an enforceable sandbox backend"
                .to_string(),
        });
    }
    if let Some(backend) = options.sandbox_backend {
        let sandbox_pid_file = options.pid_file.clone();
        let sandbox_cancellation = options.cancellation_token.clone();
        let cwd = options
            .cwd
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()));
        let resolved = resolve_cli_binary(binary);
        let mut env = BTreeMap::new();
        if let Some(path) = std::env::var_os("PATH") {
            env.insert("PATH".to_string(), path.to_string_lossy().to_string());
        }
        if let Some(home) = std::env::var_os("HOME") {
            env.insert("HOME".to_string(), home.to_string_lossy().to_string());
        }
        let program = resolved.program.clone().into_os_string();
        let mut write_allowlist = cli_provider_write_allowlist(provider);
        write_allowlist.extend(options.extra_write_allowlist);
        dedup_paths(&mut write_allowlist);
        let mut write_denylist = options.extra_write_denylist;
        write_denylist.extend(
            write_denylist
                .clone()
                .into_iter()
                .filter_map(|path| path.canonicalize().ok()),
        );
        dedup_paths(&mut write_denylist);
        let mut policy =
            ToolSandboxPolicy::cli_provider(cwd.clone(), resolved.read_allowlist, write_allowlist);
        policy.read_allowlist.extend(options.extra_read_allowlist);
        dedup_paths(&mut policy.read_allowlist);
        let output = run_sandbox(SandboxSpec {
            backend,
            docker: None,
            cwd,
            program,
            args: args.iter().map(OsString::from).collect(),
            stdin: None,
            env,
            allow_network: policy.allow_network,
            pid_file: options.pid_file,
            cancellation_token: options.cancellation_token,
            profile_dir: None,
            read_allowlist: policy.read_allowlist,
            write_allowlist: policy.write_allowlist,
            read_denylist: Vec::new(),
            write_denylist,
            network_allowlist: policy.network_allowlist,
            workspace_access: options.workspace_access,
            // A deadline/cancellation is not complete until every descendant
            // has been reaped. Provider CLIs routinely launch helper trees.
            cleanup_process_group,
            guarded_launch: None,
        })
        .await
        .map_err(|source| {
            if let Some(pid_file) = sandbox_pid_file.as_deref() {
                match pid_authority_absent(pid_file) {
                    Ok(true) => {}
                    Ok(false) => {
                        return ProviderError::CleanupIncomplete {
                            provider: provider.to_string(),
                            authority: sandbox_pid_file.clone(),
                            detail: format!(
                                "sandboxed provider returned with retained authority: {source}"
                            ),
                        };
                    }
                    Err(error) => {
                        return ProviderError::CleanupIncomplete {
                            provider: provider.to_string(),
                            authority: sandbox_pid_file.clone(),
                            detail: format!(
                                "sandboxed provider returned and PID authority could not be inspected: {error}; original error: {source}"
                            ),
                        };
                    }
                }
            }
            if sandbox_cancellation
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
            {
                return ProviderError::Cancelled {
                    provider: provider.to_string(),
                    detail: source.to_string(),
                };
            }
            ProviderError::Cli {
                provider: provider.to_string(),
                detail: source.to_string(),
            }
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

    let cancellation_token = options.cancellation_token;
    let pid_file = options.pid_file.clone();
    let mut command = Command::new(binary);
    command.args(args);
    if let Some(cwd) = options.cwd {
        command.current_dir(cwd);
    }
    let supervise_process_group = cancellation_token.is_some() || pid_file.is_some();
    if supervise_process_group {
        command.kill_on_drop(true);
        configure_process_group(&mut command);
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| ProviderError::Cli {
            provider: provider.to_string(),
            detail: source.to_string(),
        })?;
    let pid = child.id();
    let process_record = match (pid, pid_file.as_ref()) {
        (Some(pid), Some(pid_file)) => match write_current_process_record(pid_file, pid) {
            Ok(record) => Some(record),
            Err(error) => {
                let cleanup_deadline = Instant::now() + CLI_PROCESS_CLEANUP_TIMEOUT;
                stop_and_reap_cli_child(&mut child, Some(pid), cleanup_deadline)
                    .await
                    .map_err(|detail| {
                        cleanup_incomplete(provider, Some(pid_file.as_ref()), detail)
                    })?;
                remove_partial_process_authority(pid_file).map_err(|detail| {
                    cleanup_incomplete(provider, Some(pid_file.as_ref()), detail)
                })?;
                return Err(error);
            }
        },
        _ => None,
    };
    let Some(stdout) = child.stdout.take() else {
        return finish_missing_cli_pipe(
            provider,
            &mut child,
            pid,
            pid_file.as_deref(),
            process_record.as_ref(),
            "stdout",
        )
        .await;
    };
    let Some(stderr) = child.stderr.take() else {
        return finish_missing_cli_pipe(
            provider,
            &mut child,
            pid,
            pid_file.as_deref(),
            process_record.as_ref(),
            "stderr",
        )
        .await;
    };
    let output_drains = CliOutputDrains::new(stdout, stderr);

    let boundary = if let Some(token) = cancellation_token.as_ref() {
        tokio::select! {
            biased;
            () = token.cancelled() => CliWaitBoundary::Cancelled,
            output = child.wait() => CliWaitBoundary::Completed(output),
        }
    } else {
        CliWaitBoundary::Completed(child.wait().await)
    };

    match boundary {
        CliWaitBoundary::Cancelled => {
            let cleanup_deadline = Instant::now() + CLI_PROCESS_CLEANUP_TIMEOUT;
            let process_cleanup = stop_and_reap_cli_child(&mut child, pid, cleanup_deadline).await;
            let output_cleanup = output_drains.finish_before(cleanup_deadline).await;
            if let Err(detail) = process_cleanup.and(output_cleanup.map(|_| ())) {
                return Err(cleanup_incomplete(provider, pid_file.as_deref(), detail));
            }
            remove_cli_process_authority(provider, pid_file.as_deref(), process_record.as_ref())?;
            Err(ProviderError::Cancelled {
                provider: provider.to_string(),
                detail: "request cancelled".to_string(),
            })
        }
        CliWaitBoundary::Completed(Err(source)) => {
            let cleanup_deadline = Instant::now() + CLI_PROCESS_CLEANUP_TIMEOUT;
            let process_cleanup = stop_and_reap_cli_child(&mut child, pid, cleanup_deadline).await;
            let output_cleanup = output_drains.finish_before(cleanup_deadline).await;
            if let Err(detail) = process_cleanup.and(output_cleanup.map(|_| ())) {
                return Err(cleanup_incomplete(provider, pid_file.as_deref(), detail));
            }
            remove_cli_process_authority(provider, pid_file.as_deref(), process_record.as_ref())?;
            Err(ProviderError::Cli {
                provider: provider.to_string(),
                detail: source.to_string(),
            })
        }
        CliWaitBoundary::Completed(Ok(status)) => {
            let cleanup_deadline = Instant::now() + CLI_PROCESS_CLEANUP_TIMEOUT;
            if let Some(pid) = pid.filter(|_| supervise_process_group) {
                reconcile_cli_process_group_before(pid, cleanup_deadline)
                    .await
                    .map_err(|detail| cleanup_incomplete(provider, pid_file.as_deref(), detail))?;
            }
            let (stdout, stderr) = output_drains
                .finish_before(cleanup_deadline)
                .await
                .map_err(|detail| cleanup_incomplete(provider, pid_file.as_deref(), detail))?;
            remove_cli_process_authority(provider, pid_file.as_deref(), process_record.as_ref())?;
            Ok(CliOutput {
                stdout,
                stderr,
                status_code: status.code(),
                pid,
                sandbox_backend: None,
                sandbox_warning: None,
            })
        }
    }
}

enum CliWaitBoundary {
    Completed(std::io::Result<std::process::ExitStatus>),
    Cancelled,
}

struct CliOutputDrains {
    stdout: JoinHandle<String>,
    stderr: JoinHandle<String>,
}

impl CliOutputDrains {
    fn new(stdout: tokio::process::ChildStdout, stderr: tokio::process::ChildStderr) -> Self {
        Self {
            stdout: tokio::spawn(drain_bounded_cli_output(stdout)),
            stderr: tokio::spawn(drain_bounded_cli_output(stderr)),
        }
    }

    async fn finish_before(
        mut self,
        deadline: Instant,
    ) -> std::result::Result<(String, String), String> {
        let (stdout, stderr) = tokio::join!(
            tokio::time::timeout_at(deadline, &mut self.stdout),
            tokio::time::timeout_at(deadline, &mut self.stderr),
        );
        match (stdout, stderr) {
            (Ok(Ok(stdout)), Ok(Ok(stderr))) => Ok((stdout, stderr)),
            (stdout, stderr) => {
                self.stdout.abort();
                self.stderr.abort();
                Err(format!(
                    "provider output drains did not both resolve before the cleanup deadline (stdout: {}; stderr: {})",
                    output_drain_status(&stdout),
                    output_drain_status(&stderr)
                ))
            }
        }
    }
}

fn output_drain_status(
    status: &std::result::Result<
        std::result::Result<String, tokio::task::JoinError>,
        tokio::time::error::Elapsed,
    >,
) -> String {
    match status {
        Ok(Ok(_)) => "completed".to_string(),
        Ok(Err(error)) => format!("task failed: {error}"),
        Err(_) => "timed out".to_string(),
    }
}

async fn drain_bounded_cli_output<R>(mut reader: R) -> String
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut output = HeadTailBuffer::new(CLI_OUTPUT_LIMIT_BYTES);
    let mut chunk = [0_u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(read) => output.push(&chunk[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    output.render(None)
}

async fn stop_and_reap_cli_child(
    child: &mut Child,
    pid: Option<u32>,
    deadline: Instant,
) -> std::result::Result<(), String> {
    if let Some(pid) = pid {
        signal_process_group(pid, false);
        tokio::time::sleep_until(deadline.min(Instant::now() + Duration::from_millis(250))).await;
        signal_process_group(pid, true);
    }
    let _ = child.start_kill();
    let direct_reaped = match tokio::time::timeout_at(deadline, child.wait()).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(format!("provider direct-child reap failed: {error}")),
        Err(_) => Err(format!(
            "provider direct child was not reaped within {:.0}s",
            CLI_PROCESS_CLEANUP_TIMEOUT.as_secs_f64()
        )),
    };
    let group_reconciled = if let Some(pid) = pid {
        reconcile_cli_process_group_before(pid, deadline).await
    } else {
        Ok(())
    };
    match (direct_reaped, group_reconciled) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(direct), Ok(())) => Err(direct),
        (Ok(()), Err(group)) => Err(group),
        (Err(direct), Err(group)) => Err(format!("{direct}; {group}")),
    }
}

async fn reconcile_cli_process_group_before(
    pid: u32,
    deadline: Instant,
) -> std::result::Result<(), String> {
    match tokio::time::timeout_at(deadline, reconcile_process_group(pid)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(format!("provider process-group cleanup failed: {error}")),
        Err(_) => Err("provider process-group cleanup exceeded its deadline".to_string()),
    }
}

fn remove_cli_process_authority(
    provider: &str,
    pid_file: Option<&Path>,
    process_record: Option<&deadreckon_core::SupervisedProcessRecord>,
) -> Result<()> {
    if let (Some(pid_file), Some(record)) = (pid_file, process_record) {
        remove_current_process_record(pid_file, record)
            .map_err(|error| cleanup_incomplete(provider, Some(pid_file), error.to_string()))?;
    }
    Ok(())
}

fn remove_partial_process_authority(path: &Path) -> std::result::Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "partially written process authority could not be removed: {error}"
        )),
    }
}

async fn finish_missing_cli_pipe(
    provider: &str,
    child: &mut Child,
    pid: Option<u32>,
    pid_file: Option<&Path>,
    process_record: Option<&deadreckon_core::SupervisedProcessRecord>,
    pipe: &str,
) -> Result<CliOutput> {
    let cleanup_deadline = Instant::now() + CLI_PROCESS_CLEANUP_TIMEOUT;
    stop_and_reap_cli_child(child, pid, cleanup_deadline)
        .await
        .map_err(|detail| cleanup_incomplete(provider, pid_file, detail))?;
    remove_cli_process_authority(provider, pid_file, process_record)?;
    Err(ProviderError::Cli {
        provider: provider.to_string(),
        detail: format!("provider {pipe} pipe was unavailable"),
    })
}

fn cleanup_incomplete(provider: &str, pid_file: Option<&Path>, detail: String) -> ProviderError {
    ProviderError::CleanupIncomplete {
        provider: provider.to_string(),
        authority: pid_file.map(Path::to_path_buf),
        detail,
    }
}

#[cfg(unix)]
pub(crate) fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;

    command.as_std_mut().process_group(0);
}

#[cfg(not(unix))]
pub(crate) fn configure_process_group(_command: &mut Command) {}

#[cfg(unix)]
pub(crate) fn signal_process_group(pid: u32, force: bool) {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    if let Ok(pid) = i32::try_from(pid) {
        let signal = if force {
            Signal::SIGKILL
        } else {
            Signal::SIGTERM
        };
        let _ = kill(Pid::from_raw(-pid), Some(signal));
    }
}

#[cfg(not(unix))]
pub(crate) fn signal_process_group(_pid: u32, _force: bool) {}

pub(crate) fn write_current_process_record(
    path: &Path,
    pid: u32,
) -> Result<deadreckon_core::SupervisedProcessRecord> {
    let record =
        deadreckon_core::SupervisedProcessRecord::running(deadreckon_core::SupervisedProcess {
            pid,
            pgid: if cfg!(unix) { Some(pid) } else { None },
        })
        .map_err(|source| ProviderError::Io {
            path: path.display().to_string(),
            source,
        })?;
    deadreckon_core::write_supervised_process_record(path, &record).map_err(|source| {
        ProviderError::Io {
            path: path.display().to_string(),
            source,
        }
    })?;
    Ok(record)
}

pub(crate) fn remove_current_process_record(
    path: &Path,
    record: &deadreckon_core::SupervisedProcessRecord,
) -> Result<()> {
    let removed = deadreckon_core::remove_supervised_process_record_if_same(path, record).map_err(
        |source| ProviderError::Io {
            path: path.display().to_string(),
            source,
        },
    )?;
    if !removed {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(ProviderError::Io {
                    path: path.display().to_string(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "supervised process identity changed before cleanup",
                    ),
                });
            }
            Err(source) => {
                return Err(ProviderError::Io {
                    path: path.display().to_string(),
                    source,
                });
            }
        }
    }
    Ok(())
}

pub(crate) async fn reconcile_process_group(pid: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use deadreckon_core::ChildTerminator as _;

        let pgid = i32::try_from(pid).map_err(|_| ProviderError::Cli {
            provider: "provider-process".to_string(),
            detail: format!("process group id {pid} exceeds i32"),
        })?;
        let outcome = tokio::task::spawn_blocking(move || {
            deadreckon_core::ProcessGroupTerminator::new(pgid)
                .terminate(std::time::Duration::from_millis(250))
        })
        .await
        .map_err(|error| ProviderError::Cli {
            provider: "provider-process".to_string(),
            detail: format!("process-group reconciliation task failed: {error}"),
        })?;
        if let deadreckon_core::TerminationOutcome::Failed(detail) = outcome {
            return Err(ProviderError::Cli {
                provider: "provider-process".to_string(),
                detail: format!("could not reconcile provider process group {pid}: {detail}"),
            });
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ResolvedCliBinary {
    program: PathBuf,
    read_allowlist: Vec<PathBuf>,
}

fn resolve_cli_binary(binary: &str) -> ResolvedCliBinary {
    let configured = PathBuf::from(binary);
    let located = if configured.components().count() > 1 || configured.is_absolute() {
        configured
    } else {
        which::which(binary).unwrap_or_else(|_| PathBuf::from(binary))
    };
    let canonical = located.canonicalize().unwrap_or_else(|_| located.clone());
    let mut read_allowlist = Vec::new();
    push_existing_parent_roots(&mut read_allowlist, &located);
    push_existing_parent_roots(&mut read_allowlist, &canonical);
    if let Ok(canonical_parent) = canonical.parent().unwrap_or(&canonical).canonicalize() {
        push_if_exists(&mut read_allowlist, canonical_parent);
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for root in [".bun", ".local", ".npm-global", ".opencode"] {
            push_if_exists(&mut read_allowlist, home.join(root));
        }
    }
    dedup_paths(&mut read_allowlist);
    ResolvedCliBinary {
        program: canonical,
        read_allowlist,
    }
}

fn cli_provider_write_allowlist(provider: &str) -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    if let Ok(registry) = ProviderRegistry::builtin()
        && let Some(descriptor) = registry.get(provider)
    {
        let mut paths = descriptor
            .sandbox_writes
            .iter()
            .map(|path| expand_home_path(path, &home))
            .collect::<Vec<_>>();
        dedup_paths(&mut paths);
        return paths;
    }
    let mut paths = Vec::new();
    if provider.contains("codex") {
        paths.push(home.join(".codex"));
    }
    if provider.contains("claude") {
        paths.push(home.join(".claude"));
    }
    paths
}

fn expand_home_path(path: &Path, home: &Path) -> PathBuf {
    if path == Path::new("~") {
        return home.to_path_buf();
    }
    if let Ok(rest) = path.strip_prefix("~") {
        return home.join(rest);
    }
    path.to_path_buf()
}

fn push_existing_parent_roots(paths: &mut Vec<PathBuf>, path: &Path) {
    push_if_exists(paths, path);
    if let Some(parent) = path.parent() {
        push_if_exists(paths, parent);
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        for root in [".bun", ".local", ".npm-global", ".opencode"] {
            let candidate = home.join(root);
            if path.starts_with(&candidate) {
                push_if_exists(paths, candidate);
            }
        }
    }
}

fn push_if_exists(paths: &mut Vec<PathBuf>, path: impl Into<PathBuf>) {
    let path = path.into();
    if path.exists() {
        paths.push(path);
    }
}

fn dedup_paths(paths: &mut Vec<PathBuf>) {
    paths.sort();
    paths.dedup();
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{
        CliRunOptions, cli_provider_write_allowlist, pid_authority_absent, run_cli_with_options,
    };
    use deadreckon_sandbox::WorkspaceAccess;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn migrated_cli_codex_uses_descriptor_sandbox_writes() {
        let paths = cli_provider_write_allowlist("cli:codex");
        assert!(paths.iter().any(|path| path.ends_with(".codex")));
        assert!(!paths.iter().any(|path| path.ends_with(".claude")));
    }

    #[cfg(unix)]
    #[test]
    fn dangling_pid_authority_is_retained_not_mistaken_for_absence() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let authority = temp.path().join("provider.pid");
        symlink(temp.path().join("missing-record"), &authority).expect("dangling authority");

        assert!(matches!(pid_authority_absent(&authority), Ok(false)));
        assert!(matches!(
            pid_authority_absent(&temp.path().join("actually-absent")),
            Ok(true)
        ));
    }

    #[tokio::test]
    async fn read_only_cli_without_inner_or_outer_sandbox_fails_closed() {
        let error = run_cli_with_options(
            "cli:generic",
            "true",
            &[],
            CliRunOptions {
                cwd: None,
                sandbox_backend: None,
                pid_file: None,
                cancellation_token: None,
                extra_read_allowlist: Vec::new(),
                extra_write_allowlist: Vec::new(),
                extra_write_denylist: Vec::new(),
                workspace_access: WorkspaceAccess::ReadOnly,
                inner_read_only_enforced: false,
            },
        )
        .await
        .expect_err("read-only must be enforced");

        assert!(
            error
                .to_string()
                .contains("requires an enforceable sandbox backend")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn done_timeout_reaps_provider_and_grandchild_processes() {
        use nix::errno::Errno;
        use nix::sys::signal::kill;
        use nix::unistd::Pid;

        let temp = tempfile::tempdir().expect("tempdir");
        let grandchild_file = temp.path().join("grandchild.pid");
        let token = CancellationToken::new();
        let args = vec![
            "-c".to_string(),
            format!(
                "(trap '' TERM; while :; do printf 'child-stdout-flood-0123456789abcdef\\n'; printf 'child-stderr-flood-0123456789abcdef\\n' >&2; done) & child=$!; echo $child > '{}'; wait",
                grandchild_file.display()
            ),
        ];
        let completion = run_cli_with_options(
            "cli:test",
            "sh",
            &args,
            CliRunOptions {
                cwd: Some(temp.path().to_path_buf()),
                sandbox_backend: None,
                pid_file: Some(temp.path().join("provider.pid")),
                cancellation_token: Some(token.clone()),
                extra_read_allowlist: Vec::new(),
                extra_write_allowlist: Vec::new(),
                extra_write_denylist: Vec::new(),
                workspace_access: WorkspaceAccess::ReadWrite,
                inner_read_only_enforced: false,
            },
        );
        tokio::pin!(completion);
        let grandchild_started = async {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
            while !grandchild_file.exists() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        };
        tokio::select! {
            result = &mut completion => panic!("provider exited before cancellation: {result:?}"),
            () = grandchild_started => {}
        }
        let record =
            deadreckon_core::read_supervised_process_record(&temp.path().join("provider.pid"))
                .expect("identity-bound provider record");
        assert_eq!(
            record.identity(),
            deadreckon_core::SupervisedProcessIdentity::Current
        );
        assert_eq!(record.process.pgid, Some(record.process.pid));
        let grandchild = std::fs::read_to_string(&grandchild_file)
            .expect("grandchild pid")
            .trim()
            .parse::<i32>()
            .expect("numeric pid");
        token.cancel();
        completion.await.expect_err("cancelled completion");

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            match kill(Pid::from_raw(grandchild), None) {
                Err(Errno::ESRCH) => break,
                _ if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                result => panic!("grandchild survived cancellation: {result:?}"),
            }
        }
        assert!(!temp.path().join("provider.pid").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelled_cli_bounds_escaped_pipe_holder_and_retains_authority() {
        struct EscapedProcessGuard {
            pid_path: std::path::PathBuf,
            authority: std::path::PathBuf,
        }

        impl Drop for EscapedProcessGuard {
            fn drop(&mut self) {
                if let Ok(raw) = std::fs::read_to_string(&self.pid_path)
                    && let Ok(pid) = raw.trim().parse::<u32>()
                    && deadreckon_core::pid_is_alive(pid)
                {
                    let _ = deadreckon_core::terminate_pid(pid, true);
                }
                let _ = std::fs::remove_file(&self.authority);
            }
        }

        let Ok(python) = which::which("python3") else {
            return;
        };
        let temp = tempfile::tempdir().expect("tempdir");
        let escaped_pid = temp.path().join("escaped.pid");
        let authority = temp.path().join("provider.pid");
        let _guard = EscapedProcessGuard {
            pid_path: escaped_pid.clone(),
            authority: authority.clone(),
        };
        let python_program = concat!(
            "import os,signal,sys; ",
            "os.setsid(); ",
            "signal.signal(signal.SIGTERM, signal.SIG_IGN); ",
            "f=open(sys.argv[1], \"w\"); f.write(str(os.getpid())); f.close(); ",
            "payload=b\"escaped-output-flood-0123456789abcdef\\n\"; ",
            "exec(\"while True:\\n os.write(1,payload)\\n os.write(2,payload)\")"
        );
        let args = vec![
            "-c".to_string(),
            format!(
                "\"{}\" -c '{}' \"{}\" & trap '' TERM; while :; do printf 'parent-output-flood-0123456789abcdef\\n'; done",
                python.display(),
                python_program,
                escaped_pid.display()
            ),
        ];
        let token = CancellationToken::new();
        let completion = run_cli_with_options(
            "cli:test",
            "sh",
            &args,
            CliRunOptions {
                cwd: Some(temp.path().to_path_buf()),
                sandbox_backend: None,
                pid_file: Some(authority.clone()),
                cancellation_token: Some(token.clone()),
                extra_read_allowlist: Vec::new(),
                extra_write_allowlist: Vec::new(),
                extra_write_denylist: Vec::new(),
                workspace_access: WorkspaceAccess::ReadWrite,
                inner_read_only_enforced: false,
            },
        );
        tokio::pin!(completion);
        let escaped_started = async {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            while !escaped_pid.exists() && tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::select! {
            result = &mut completion => panic!("provider exited before cancellation: {result:?}"),
            () = escaped_started => {}
        }
        assert!(escaped_pid.exists(), "escaped pipe holder did not start");

        let cancelled_at = tokio::time::Instant::now();
        token.cancel();
        let error = tokio::time::timeout(Duration::from_secs(10), &mut completion)
            .await
            .expect("cancellation remained bounded")
            .expect_err("escaped pipe holder must prevent cleanup proof");

        assert!(matches!(
            error,
            crate::ProviderError::CleanupIncomplete {
                authority: Some(ref retained),
                ..
            } if retained == &authority
        ));
        assert!(
            cancelled_at.elapsed() < Duration::from_secs(10),
            "escaped pipe holder extended cleanup beyond its bound"
        );
        assert!(
            std::fs::symlink_metadata(&authority).is_ok(),
            "unproved cleanup removed process authority"
        );
    }
}
