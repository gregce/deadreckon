use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use deadreckon_sandbox::{
    SandboxBackend, SandboxSpec, ToolSandboxPolicy, WorkspaceAccess, run as run_sandbox,
};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::registry::ProviderRegistry;
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
            extra_write_allowlist: Vec::new(),
            workspace_access,
            inner_read_only_enforced,
        },
    )
    .await
}

pub(crate) struct CliRunOptions {
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) sandbox_backend: Option<SandboxBackend>,
    pub(crate) pid_file: Option<PathBuf>,
    pub(crate) cancellation_token: Option<CancellationToken>,
    pub(crate) extra_write_allowlist: Vec<PathBuf>,
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
        let policy =
            ToolSandboxPolicy::cli_provider(cwd.clone(), resolved.read_allowlist, write_allowlist);
        let output = run_sandbox(SandboxSpec {
            backend,
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
            write_denylist: Vec::new(),
            network_allowlist: policy.network_allowlist,
            workspace_access: options.workspace_access,
            cleanup_process_group: false,
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
    if cancellation_token.is_some() {
        command.kill_on_drop(true);
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
    let wait = child.wait_with_output();
    let output = if let Some(token) = cancellation_token {
        tokio::select! {
            _ = token.cancelled() => {
                if let Some(pid_file) = pid_file.as_ref() {
                    let _ = tokio::fs::remove_file(pid_file).await;
                }
                return Err(ProviderError::Cli {
                    provider: provider.to_string(),
                    detail: "request cancelled".to_string(),
                });
            }
            output = wait => output
        }
    } else {
        wait.await
    }
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
                extra_write_allowlist: Vec::new(),
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
}
