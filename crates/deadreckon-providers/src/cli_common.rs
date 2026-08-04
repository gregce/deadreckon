use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use deadreckon_sandbox::{
    SandboxBackend, SandboxSpec, ToolSandboxPolicy, WorkspaceAccess, run as run_sandbox,
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::registry::ProviderRegistry;
use crate::{ProviderError, Result};

pub(crate) const CLI_CAPABILITY_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CLI_CAPABILITY_PROBE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(10);

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
        return Err(ProviderError::Cli {
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
                return Err(ProviderError::Cli {
                    provider: provider.to_string(),
                    detail: format!(
                        "capability probe cleanup exceeded the bounded {}s safety window{}",
                        CLI_CAPABILITY_PROBE_CLEANUP_TIMEOUT.as_secs(),
                        pid_file.as_ref().map_or_else(String::new, |path| format!(
                            "; process authority remains at {}",
                            path.display()
                        ))
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
                return Err(ProviderError::Cli {
                    provider: provider.to_string(),
                    detail: format!(
                        "cancelled capability probe cleanup exceeded the bounded {}s safety window{}",
                        CLI_CAPABILITY_PROBE_CLEANUP_TIMEOUT.as_secs(),
                        pid_file.as_ref().map_or_else(String::new, |path| format!(
                            "; process authority remains at {}",
                            path.display()
                        ))
                    ),
                });
            }
            prove_capability_probe_cleanup(provider, pid_file.as_deref())?;
            Err(ProviderError::Cli {
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
    match std::fs::symlink_metadata(pid_file) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(ProviderError::Cli {
            provider: provider.to_string(),
            detail: format!(
                "capability probe returned without proving process cleanup; process authority remains at {}",
                pid_file.display()
            ),
        }),
        Err(source) => Err(ProviderError::Io {
            path: pid_file.display().to_string(),
            source,
        }),
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
                signal_process_group(pid, true);
                let _ = child.start_kill();
                let _ = child.wait().await;
                reconcile_process_group(pid).await?;
                return Err(error);
            }
        },
        _ => None,
    };
    let wait = child.wait_with_output();
    tokio::pin!(wait);
    let output = if let Some(token) = cancellation_token {
        tokio::select! {
            _ = token.cancelled() => {
                if let Some(pid) = pid {
                    signal_process_group(pid, false);
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    signal_process_group(pid, true);
                }
                // wait_with_output drains both pipes and reaps the direct
                // child after the whole process group has been signalled.
                let _ = wait.await;
                if let Some(pid) = pid {
                    reconcile_process_group(pid).await?;
                }
                if let (Some(pid_file), Some(record)) =
                    (pid_file.as_ref(), process_record.as_ref())
                {
                    remove_current_process_record(pid_file, record)?;
                }
                return Err(ProviderError::Cli {
                    provider: provider.to_string(),
                    detail: "request cancelled".to_string(),
                });
            }
            output = &mut wait => output
        }
    } else {
        wait.await
    }
    .map_err(|source| ProviderError::Cli {
        provider: provider.to_string(),
        detail: source.to_string(),
    })?;
    if let Some(pid) = pid.filter(|_| supervise_process_group) {
        reconcile_process_group(pid).await?;
    }
    if let (Some(pid_file), Some(record)) = (pid_file.as_ref(), process_record.as_ref()) {
        remove_current_process_record(pid_file, record)?;
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
    use super::{CliRunOptions, cli_provider_write_allowlist, run_cli_with_options};
    use deadreckon_sandbox::WorkspaceAccess;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn migrated_cli_codex_uses_descriptor_sandbox_writes() {
        let paths = cli_provider_write_allowlist("cli:codex");
        assert!(paths.iter().any(|path| path.ends_with(".codex")));
        assert!(!paths.iter().any(|path| path.ends_with(".claude")));
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
                "sleep 30 & child=$!; echo $child > '{}'; wait",
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
}
