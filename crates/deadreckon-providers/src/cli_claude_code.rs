use std::time::Instant;

use serde_json::json;
use which::which;

use crate::cli_common::{run_cli, write_output};
use crate::{
    Provider, ProviderEntry, ProviderFuture, ProviderKind, ProviderRequest, ProviderResponse,
    ProviderUsage, Result, SpendEstimate,
};
use std::path::PathBuf;

#[derive(Clone)]
pub struct CliClaudeCodeProvider {
    name: String,
    binary: String,
    extra_args: Vec<String>,
    model: String,
}

impl CliClaudeCodeProvider {
    pub fn new(name: impl Into<String>, entry: ProviderEntry) -> Self {
        Self {
            name: name.into(),
            binary: entry.binary.unwrap_or_else(|| "claude".to_string()),
            extra_args: entry.extra_args,
            model: entry.model.unwrap_or_else(|| "cli:claude-code".to_string()),
        }
    }

    async fn run(&self, request: &ProviderRequest) -> Result<ProviderResponse> {
        let started = Instant::now();
        let mut args = self.extra_args.clone();
        // `claude --help` on this machine documents `-p, --print` for
        // non-interactive output and `--dangerously-skip-permissions` for
        // bypassing Claude Code prompts inside an outer sandbox.
        args.extend([
            "--dangerously-skip-permissions".to_string(),
            "-p".to_string(),
            request.prompt.clone(),
        ]);
        let output = run_cli(
            &self.name,
            &self.binary,
            &args,
            request.cwd.clone(),
            request.sandbox_backend,
            request.pid_file.clone(),
        )
        .await?;
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
                "pid": output.pid,
                "sandbox_backend": output.sandbox_backend,
                "sandbox_warning": output.sandbox_warning,
            }),
        })
    }
}

impl Provider for CliClaudeCodeProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::CliClaudeCode
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

trait WithWallTime {
    fn with_wall_time(self, seconds: f64) -> Self;
}

impl WithWallTime for SpendEstimate {
    fn with_wall_time(mut self, seconds: f64) -> Self {
        self.wall_time_seconds = Some(seconds);
        self
    }
}
