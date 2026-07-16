use std::path::{Path, PathBuf};
use std::time::Instant;

use deadreckon_sandbox::SandboxBackend;
use serde_json::json;
use which::which;

use crate::cli_common::{CliOutput, ensure_success, run_cli, write_output};
use crate::cli_contract::{
    PROVIDER_ID_CODEX, ParsedStream, ProviderContract, ProviderSession, add_caveat,
    flight_rows_from, session_not_found, write_schema_file,
};
use crate::codex_events::{parse_codex_line, probe_codex_capabilities};
use crate::{
    Provider, ProviderEntry, ProviderFuture, ProviderKind, ProviderRequest, ProviderResponse,
    ProviderUsage, Result, SpendEstimate,
};

#[derive(Clone)]
pub struct CliCodexProvider {
    name: String,
    binary: String,
    extra_args: Vec<String>,
    model: String,
    model_arg: Option<String>,
}

struct CodexAttempt {
    output: CliOutput,
    args: Vec<String>,
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

    /// Build the argv for one attempt. A resume attempt runs `exec resume <id>`
    /// and omits `--sandbox` (resume inherits the session's original policy);
    /// a fresh attempt runs `exec` with the sandbox mode as today.
    fn build_args(
        &self,
        request: &ProviderRequest,
        caps: &crate::codex_events::CodexCapabilities,
        resume_id: Option<&str>,
        last_message: Option<&Path>,
        schema: Option<&Path>,
    ) -> Vec<String> {
        let mut args = vec![
            "--ask-for-approval".to_string(),
            "never".to_string(),
            "exec".to_string(),
        ];
        if let Some(id) = resume_id {
            args.push("resume".to_string());
            args.push(id.to_string());
        }
        if let Some(model) = self.model_arg.as_deref() {
            args.extend(["--model".to_string(), model.to_string()]);
        }
        args.push("--skip-git-repo-check".to_string());
        if resume_id.is_none() {
            args.extend([
                "--sandbox".to_string(),
                codex_sandbox_mode(request.sandbox_backend).to_string(),
            ]);
        }
        args.extend(self.extra_args.clone());
        if caps.json {
            args.push("--json".to_string());
        }
        if caps.output_last_message
            && let Some(path) = last_message
        {
            args.extend(["-o".to_string(), path.display().to_string()]);
        }
        if caps.output_schema
            && let Some(path) = schema
        {
            args.extend(["--output-schema".to_string(), path.display().to_string()]);
        }
        // YAML-frontmatter prompts begin with `---`; delimit so clap treats the
        // payload as the prompt value, not an option.
        args.push("--".to_string());
        args.push(request.prompt.clone());
        args
    }

    async fn run_attempt(
        &self,
        request: &ProviderRequest,
        caps: &crate::codex_events::CodexCapabilities,
        resume_id: Option<&str>,
        last_message: Option<&Path>,
        schema: Option<&Path>,
    ) -> Result<CodexAttempt> {
        let args = self.build_args(request, caps, resume_id, last_message, schema);
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
        Ok(CodexAttempt { output, args })
    }

    async fn run(&self, request: &ProviderRequest) -> Result<ProviderResponse> {
        let caps = probe_codex_capabilities(&self.binary);
        let session_dir = request.session_dir.clone();
        let session = session_dir
            .as_deref()
            .and_then(|dir| ProviderSession::read(dir, PROVIDER_ID_CODEX));
        let resume_id = session
            .as_ref()
            .filter(|_| caps.resume)
            .filter(|s| s.can_resume())
            .map(|s| s.conversation_id.clone());

        let started = Instant::now();
        let last_message_path = request
            .output_path
            .as_ref()
            .map(|p| p.with_extension("last.txt"));
        // codex writes the last-message file itself; ensure its parent exists
        // before the binary runs so the write lands.
        if let Some(parent) = last_message_path.as_ref().and_then(|p| p.parent()) {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        let schema_file = match (
            &request.output_schema,
            caps.output_schema,
            session_dir.as_deref(),
        ) {
            (Some(schema), true, Some(dir)) => Some(write_schema_file(dir, schema).await?),
            _ => None,
        };
        let schema_caveat = request.output_schema.is_some() && !caps.output_schema;

        let attempt = self
            .run_attempt(
                request,
                &caps,
                resume_id.as_deref(),
                last_message_path.as_deref(),
                schema_file.as_deref(),
            )
            .await?;

        // Vanished conversation: a resume that failed with a not-found signature
        // marks the session, retries once fresh, and records a reset.
        let (output, args, resumed, reset) = if attempt.output.status_code != Some(0)
            && resume_id.is_some()
            && session_not_found(&attempt.output)
        {
            if let (Some(dir), Some(mut s)) = (session_dir.as_deref(), session.clone()) {
                s.mark_resume_failure(chrono::Utc::now());
                let _ = s.write(dir);
            }
            let fresh = self
                .run_attempt(
                    request,
                    &caps,
                    None,
                    last_message_path.as_deref(),
                    schema_file.as_deref(),
                )
                .await?;
            (fresh.output, fresh.args, false, true)
        } else {
            (attempt.output, attempt.args, resume_id.is_some(), false)
        };

        write_output(request.output_path.as_ref(), &output).await?;
        ensure_success(&self.name, &output)?;

        let parsed = if caps.json {
            ProviderContract::from_event_mirror(parse_codex_line)
                .parse(&output.stdout)
                .parsed
        } else {
            ParsedStream::default()
        };
        let degraded = !caps.json || parsed.degraded();

        if let (Some(dir), Some(id)) = (session_dir.as_deref(), parsed.conversation_id.as_deref()) {
            persist_session(dir, PROVIDER_ID_CODEX, id, reset);
        }

        let last_message = last_message_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let content = match (&last_message, degraded) {
            (Some(msg), false) => msg.clone(),
            _ => output.stdout.clone(),
        };

        let usage = usage_from(&parsed);
        let wall_time_seconds = started.elapsed().as_secs_f64();
        let spend = self
            .estimate_spend(usage.clone())
            .with_wall_time(wall_time_seconds);

        let mut trace = json!({
            "kind": "cli_subagent",
            "binary": self.binary,
            "args": args,
            "stdout_path": request.output_path,
            "duration_ms": (wall_time_seconds * 1000.0).round() as u64,
            "exit_code": output.status_code,
            "pid": output.pid,
            "sandbox_backend": output.sandbox_backend,
            "sandbox_warning": output.sandbox_warning,
            "contract": {
                "json": caps.json,
                "resume": caps.resume,
                "resumed": resumed,
                "reset": reset,
                "unknown_lines": parsed.unknown_lines,
                "garbage_lines": parsed.garbage_lines,
            },
            "flight_rows": flight_rows_from(&parsed),
        });
        if degraded {
            add_caveat(
                &mut trace,
                "provider.contract.degraded",
                "codex output was not the structured JSONL contract; fell back to raw stdout",
            );
        }
        if reset {
            add_caveat(
                &mut trace,
                "provider.session.reset",
                "resume target vanished; retried once with a fresh conversation",
            );
        }
        if schema_caveat {
            add_caveat(
                &mut trace,
                "provider.output_schema.unsupported",
                "binary predates --output-schema; proceeded unconstrained (try: codex --version)",
            );
        }

        Ok(ProviderResponse {
            provider: self.name.clone(),
            model: self.model.clone(),
            content,
            usage,
            spend,
            trace,
        })
    }
}

fn usage_from(parsed: &ParsedStream) -> ProviderUsage {
    parsed
        .usage
        .map(|u| ProviderUsage {
            input_tokens: u.input_tokens,
            output_tokens: u.output_tokens,
        })
        .unwrap_or(ProviderUsage {
            input_tokens: 0,
            output_tokens: 0,
        })
}

/// Persist the conversation id for next turn's resume, resetting the failure
/// counter on a fresh conversation.
fn persist_session(dir: &Path, provider: &str, id: &str, reset: bool) {
    let now = chrono::Utc::now();
    let record = match ProviderSession::read(dir, provider) {
        Some(mut existing) if existing.conversation_id == id && !reset => {
            existing.touch(now);
            existing
        }
        _ => ProviderSession::new(provider, id, now),
    };
    let _ = record.write(dir);
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
