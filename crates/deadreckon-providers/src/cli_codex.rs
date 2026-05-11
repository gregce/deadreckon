use std::path::PathBuf;
use std::time::Instant;

use serde_json::json;
use tokio::process::Command;
use which::which;

use crate::{
    Provider, ProviderEntry, ProviderError, ProviderFuture, ProviderKind, ProviderRequest,
    ProviderResponse, ProviderUsage, Result, SpendEstimate,
};

#[derive(Clone)]
pub struct CliCodexProvider {
    name: String,
    binary: String,
    extra_args: Vec<String>,
    model: String,
}

impl CliCodexProvider {
    pub fn new(name: impl Into<String>, entry: ProviderEntry) -> Self {
        Self {
            name: name.into(),
            binary: entry.binary.unwrap_or_else(|| "codex".to_string()),
            extra_args: entry.extra_args,
            model: entry.model.unwrap_or_else(|| "cli:codex".to_string()),
        }
    }

    async fn run(&self, request: &ProviderRequest) -> Result<ProviderResponse> {
        let started = Instant::now();
        let mut args = self.extra_args.clone();
        // `codex --help` on this machine lists `exec` as "Run Codex
        // non-interactively"; V0 uses that verb for subscription-BYOK turns.
        args.extend(["exec".to_string(), request.prompt.clone()]);
        let output = run_cli(&self.name, &self.binary, &args, request.cwd.clone()).await?;
        write_output(request.output_path.as_ref(), &output).await?;
        let wall_time_seconds = started.elapsed().as_secs_f64();
        let usage = ProviderUsage {
            input_tokens: 0,
            output_tokens: 0,
        };
        let spend = self
            .estimate_spend(usage.clone())
            .with_wall_time(wall_time_seconds);
        Ok(ProviderResponse {
            provider: self.name.clone(),
            model: self.model.clone(),
            content: output.stdout.clone(),
            usage,
            spend,
            trace: json!({
                "kind": "cli_subagent",
                "binary": self.binary,
                "args": args,
                "stdout_path": request.output_path,
                "duration_ms": (wall_time_seconds * 1000.0).round() as u64,
                "exit_code": output.status_code,
            }),
        })
    }
}

impl Provider for CliCodexProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::CliCodex
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn has_credential(&self) -> bool {
        which(&self.binary).is_ok() || PathBuf::from(&self.binary).exists()
    }

    fn estimate_spend(&self, usage: ProviderUsage) -> SpendEstimate {
        SpendEstimate {
            provider: self.name.clone(),
            model: self.model.clone(),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            cost_usd: 0.0,
            subscription: true,
            wall_time_seconds: None,
        }
    }

    fn complete<'a>(&'a self, request: &'a ProviderRequest) -> ProviderFuture<'a> {
        Box::pin(async move { self.run(request).await })
    }
}

#[derive(Debug)]
struct CliOutput {
    stdout: String,
    stderr: String,
    status_code: Option<i32>,
}

async fn run_cli(
    provider: &str,
    binary: &str,
    args: &[String],
    cwd: Option<PathBuf>,
) -> Result<CliOutput> {
    let mut command = Command::new(binary);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command
        .output()
        .await
        .map_err(|source| ProviderError::Cli {
            provider: provider.to_string(),
            detail: source.to_string(),
        })?;
    Ok(CliOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        status_code: output.status.code(),
    })
}

async fn write_output(path: Option<&PathBuf>, output: &CliOutput) -> Result<()> {
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

trait WithWallTime {
    fn with_wall_time(self, seconds: f64) -> Self;
}

impl WithWallTime for SpendEstimate {
    fn with_wall_time(mut self, seconds: f64) -> Self {
        self.wall_time_seconds = Some(seconds);
        self
    }
}
