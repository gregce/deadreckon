use std::time::Instant;

use deadreckon_sandbox::SandboxBackend;
use serde_json::json;
use which::which;

use crate::cli_common::{ensure_success, run_cli, write_output};
use crate::{
    Provider, ProviderEntry, ProviderFuture, ProviderKind, ProviderRequest, ProviderResponse,
    ProviderUsage, Result, SpendEstimate,
};
use std::path::PathBuf;

#[derive(Clone)]
pub struct CliCodexProvider {
    name: String,
    binary: String,
    extra_args: Vec<String>,
    model: String,
    model_arg: Option<String>,
}

impl CliCodexProvider {
    pub fn new(name: impl Into<String>, entry: ProviderEntry) -> Self {
        let (model, model_arg) = cli_model(entry.model, "cli:codex");
        Self {
            name: name.into(),
            binary: entry.binary.unwrap_or_else(|| "codex".to_string()),
            extra_args: entry.extra_args,
            model,
            model_arg,
        }
    }

    async fn run(&self, request: &ProviderRequest) -> Result<ProviderResponse> {
        let started = Instant::now();
        let mut args = vec![
            "--ask-for-approval".to_string(),
            "never".to_string(),
            "exec".to_string(),
        ];
        if let Some(model) = self.model_arg.as_deref() {
            args.extend(["--model".to_string(), model.to_string()]);
        }
        args.extend([
            "--skip-git-repo-check".to_string(),
            "--sandbox".to_string(),
            codex_sandbox_mode(request.sandbox_backend).to_string(),
        ]);
        // `codex --help` on this machine lists `exec` as "Run Codex
        // non-interactively"; deadreckon uses that verb for subscription-BYOK turns.
        args.extend(self.extra_args.clone());
        args.push(request.prompt.clone());
        let output = run_cli(
            &self.name,
            &self.binary,
            &args,
            request.cwd.clone(),
            request.sandbox_backend,
            request.pid_file.clone(),
            request.cancellation_token.clone(),
        )
        .await?;
        write_output(request.output_path.as_ref(), &output).await?;
        ensure_success(&self.name, &output)?;
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

fn cli_model(model: Option<String>, legacy_label: &str) -> (String, Option<String>) {
    match model {
        Some(model) if model.trim().is_empty() || model == legacy_label => {
            ("provider default".to_string(), None)
        }
        Some(model) => (model.clone(), Some(model)),
        None => ("provider default".to_string(), None),
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

fn codex_sandbox_mode(outer_backend: Option<SandboxBackend>) -> &'static str {
    match outer_backend {
        Some(SandboxBackend::None) | None => "workspace-write",
        Some(
            SandboxBackend::Auto
            | SandboxBackend::SandboxExec
            | SandboxBackend::Bwrap
            | SandboxBackend::Docker,
        ) => "danger-full-access",
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
