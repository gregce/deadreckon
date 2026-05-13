use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use deadreckon_sandbox::{SandboxBackend, SandboxSpec, ToolSandboxPolicy, run as run_sandbox};
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

pub(crate) async fn run_cli(
    provider: &str,
    binary: &str,
    args: &[String],
    cwd: Option<PathBuf>,
    sandbox_backend: Option<SandboxBackend>,
    pid_file: Option<PathBuf>,
    cancellation_token: Option<CancellationToken>,
) -> Result<CliOutput> {
    if let Some(backend) = sandbox_backend {
        let cwd = cwd.unwrap_or_else(|| {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/Users/gdc/deadreckon"))
        });
        let resolved = resolve_cli_binary(binary);
        let mut env = BTreeMap::new();
        if let Some(path) = std::env::var_os("PATH") {
            env.insert("PATH".to_string(), path.to_string_lossy().to_string());
        }
        if let Some(home) = std::env::var_os("HOME") {
            env.insert("HOME".to_string(), home.to_string_lossy().to_string());
        }
        let program = resolved.program.clone().into_os_string();
        let policy = ToolSandboxPolicy::cli_provider(
            cwd.clone(),
            resolved.read_allowlist,
            cli_provider_write_allowlist(provider),
        );
        let output = run_sandbox(SandboxSpec {
            backend,
            cwd,
            program,
            args: args.iter().map(OsString::from).collect(),
            env,
            allow_network: policy.allow_network,
            pid_file,
            cancellation_token,
            profile_dir: None,
            read_allowlist: policy.read_allowlist,
            write_allowlist: policy.write_allowlist,
            network_allowlist: policy.network_allowlist,
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
    use super::cli_provider_write_allowlist;

    #[test]
    fn migrated_cli_codex_uses_descriptor_sandbox_writes() {
        let paths = cli_provider_write_allowlist("cli:codex");
        assert!(paths.iter().any(|path| path.ends_with(".codex")));
        assert!(!paths.iter().any(|path| path.ends_with(".claude")));
    }
}
