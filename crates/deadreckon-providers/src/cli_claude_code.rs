use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use deadreckon_sandbox::WorkspaceAccess;
use serde_json::json;
use tokio::sync::Mutex;
use which::which;

use crate::claude_events::{ClaudeCapabilities, parse_claude_capabilities, parse_claude_line};
use crate::cli_common::{
    CLI_CAPABILITY_PROBE_TIMEOUT, CliOutput, ensure_success, run_cli, run_cli_capability_probe,
    write_output,
};
use crate::cli_contract::{
    PROVIDER_ID_CLAUDE, ParsedStream, ProviderContract, ProviderSession, add_caveat,
    flight_rows_from, session_not_found,
};
use crate::{
    Provider, ProviderEntry, ProviderError, ProviderFuture, ProviderKind, ProviderRequest,
    ProviderResponse, ProviderUsage, Result, SpendEstimate,
};

const EMPTY_MCP_CONFIG: &str = r#"{"mcpServers":{}}"#;

fn claude_read_write_probe_cache() -> &'static Mutex<HashMap<String, ClaudeCapabilities>> {
    static CACHE: OnceLock<Mutex<HashMap<String, ClaudeCapabilities>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Clone)]
pub struct CliClaudeCodeProvider {
    name: String,
    binary: String,
    extra_args: Vec<String>,
    model: String,
    model_arg: Option<String>,
}

struct ClaudeAttempt {
    output: CliOutput,
    args: Vec<String>,
}

impl CliClaudeCodeProvider {
    pub fn new(name: impl Into<String>, entry: ProviderEntry) -> Self {
        let (model, model_arg) = cli_model(entry.model, "cli:claude-code");
        Self {
            name: name.into(),
            binary: entry.binary.unwrap_or_else(|| "claude".to_string()),
            extra_args: entry.extra_args,
            model,
            model_arg,
        }
    }

    fn build_args(
        &self,
        request: &ProviderRequest,
        caps: &crate::claude_events::ClaudeCapabilities,
        resume_id: Option<&str>,
    ) -> Vec<String> {
        let mut args = if request.workspace_access == WorkspaceAccess::ReadOnly {
            self.extra_args
                .iter()
                .filter(|arg| arg.as_str() != "--dangerously-skip-permissions")
                .cloned()
                .collect()
        } else {
            self.extra_args.clone()
        };
        if let Some(model) = self.model_arg.as_deref() {
            args.extend(["--model".to_string(), model.to_string()]);
        }
        if request.workspace_access == WorkspaceAccess::ReadWrite {
            args.push("--dangerously-skip-permissions".to_string());
        }
        if caps.stream_json {
            args.extend([
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--verbose".to_string(),
            ]);
        }
        if let Some(schema) = request.output_schema.as_ref() {
            args.extend([
                "--safe-mode".to_string(),
                "--tools".to_string(),
                String::new(),
                "--strict-mcp-config".to_string(),
                "--mcp-config".to_string(),
                EMPTY_MCP_CONFIG.to_string(),
                "--setting-sources".to_string(),
                String::new(),
                "--json-schema".to_string(),
                schema.to_string(),
            ]);
        }
        if let Some(id) = resume_id {
            args.extend(["--resume".to_string(), id.to_string()]);
        }
        args.push("-p".to_string());
        args.push(request.prompt.clone());
        args
    }

    async fn run_attempt(
        &self,
        request: &ProviderRequest,
        caps: &crate::claude_events::ClaudeCapabilities,
        resume_id: Option<&str>,
    ) -> Result<ClaudeAttempt> {
        let args = self.build_args(request, caps, resume_id);
        let output = run_cli(
            &self.name,
            &self.binary,
            &args,
            request.cwd.clone(),
            request.sandbox_backend,
            request.pid_file.clone(),
            request.cancellation_token.clone(),
            request.workspace_access,
            false,
        )
        .await?;
        Ok(ClaudeAttempt { output, args })
    }

    async fn capabilities_for_request(
        &self,
        request: &ProviderRequest,
    ) -> Result<ClaudeCapabilities> {
        self.capabilities_for_request_with_timeout(request, CLI_CAPABILITY_PROBE_TIMEOUT)
            .await
    }

    async fn capabilities_for_request_with_timeout(
        &self,
        request: &ProviderRequest,
        probe_timeout: Duration,
    ) -> Result<ClaudeCapabilities> {
        if request.workspace_access == WorkspaceAccess::ReadWrite {
            if let Some(cached) = claude_read_write_probe_cache()
                .lock()
                .await
                .get(&self.binary)
                .copied()
            {
                return Ok(cached);
            }
            let capabilities = run_cli_capability_probe(
                &self.name,
                &self.binary,
                &["--help".to_string()],
                request.cwd.clone(),
                request.sandbox_backend,
                request.pid_file.clone(),
                request.cancellation_token.clone(),
                WorkspaceAccess::ReadWrite,
                false,
                probe_timeout,
            )
            .await?
            .map(|output| parse_claude_capabilities(&output.stdout))
            .unwrap_or_else(ClaudeCapabilities::none);
            claude_read_write_probe_cache()
                .lock()
                .await
                .insert(self.binary.clone(), capabilities);
            return Ok(capabilities);
        }
        Ok(run_cli(
            &self.name,
            &self.binary,
            &["--help".to_string()],
            request.cwd.clone(),
            request.sandbox_backend,
            None,
            request.cancellation_token.clone(),
            WorkspaceAccess::ReadOnly,
            false,
        )
        .await
        .ok()
        .filter(|output| output.status_code == Some(0))
        .map(|output| parse_claude_capabilities(&output.stdout))
        .unwrap_or_else(ClaudeCapabilities::none))
    }

    async fn run(&self, request: &ProviderRequest) -> Result<ProviderResponse> {
        let caps = self.capabilities_for_request(request).await?;
        if request.output_schema.is_some() && !(caps.json_schema && caps.schema_only_posture) {
            return Err(ProviderError::Cli {
                provider: self.name.clone(),
                detail: "installed Claude Code cannot prove schema-only structured-text posture; update Claude Code or select a capable provider".to_string(),
            });
        }
        let session_dir = (request.workspace_access == WorkspaceAccess::ReadWrite)
            .then(|| request.session_dir.clone())
            .flatten();
        let session = session_dir
            .as_deref()
            .and_then(|dir| ProviderSession::read(dir, PROVIDER_ID_CLAUDE));
        let resume_id = session
            .as_ref()
            .filter(|_| caps.resume)
            .filter(|s| s.can_resume())
            .map(|s| s.conversation_id.clone());

        let started = Instant::now();
        let attempt = self
            .run_attempt(request, &caps, resume_id.as_deref())
            .await?;
        let (output, args, resumed, reset) = if attempt.output.status_code != Some(0)
            && resume_id.is_some()
            && session_not_found(&attempt.output)
        {
            if let (Some(dir), Some(mut s)) = (session_dir.as_deref(), session.clone()) {
                s.mark_resume_failure(chrono::Utc::now());
                let _ = s.write(dir);
            }
            let fresh = self.run_attempt(request, &caps, None).await?;
            (fresh.output, fresh.args, false, true)
        } else {
            (attempt.output, attempt.args, resume_id.is_some(), false)
        };

        write_output(request.output_path.as_ref(), &output).await?;
        ensure_success(&self.name, &output)?;

        let parsed = if caps.stream_json {
            ProviderContract::from_event_mirror(parse_claude_line)
                .parse(&output.stdout)
                .parsed
        } else {
            ParsedStream::default()
        };
        let degraded = !caps.stream_json || parsed.degraded();

        // is_error in the structured result maps to a provider error.
        if let Some(message) = parsed.failure.clone() {
            return Err(ProviderError::Cli {
                provider: self.name.clone(),
                detail: format!("claude reported an error result: {message}"),
            });
        }

        if let (Some(dir), Some(id)) = (session_dir.as_deref(), parsed.conversation_id.as_deref()) {
            persist_session(dir, PROVIDER_ID_CLAUDE, id, reset);
        }

        let content = match (&parsed.answer, degraded) {
            (Some(answer), false) => answer.clone(),
            _ => output.stdout.clone(),
        };
        let usage = usage_from(&parsed);
        let reported_cost = parsed.usage.and_then(|u| u.cost_usd);
        let wall = started.elapsed().as_secs_f64();
        let spend = self.estimate_spend(usage.clone()).with_wall_time(wall);

        let mut trace = json!({
            "kind": "cli_subagent",
            "binary": self.binary,
            "args": args,
            "stdout_path": request.output_path,
            "duration_ms": (wall * 1000.0).round() as u64,
            "exit_code": output.status_code,
            "pid": output.pid,
            "workspace_access": request.workspace_access.as_str(),
            "sandbox_backend": output.sandbox_backend,
            "sandbox_warning": output.sandbox_warning,
            "contract": {
                "stream_json": caps.stream_json,
                "resume": caps.resume,
                "resumed": resumed,
                "reset": reset,
                "reported_cost_usd": reported_cost,
                "unknown_lines": parsed.unknown_lines,
                "garbage_lines": parsed.garbage_lines,
            },
            "flight_rows": flight_rows_from(&parsed),
        });
        if degraded {
            add_caveat(
                &mut trace,
                "provider.contract.degraded",
                "claude output was not the structured stream-json contract; fell back to raw stdout",
            );
        }
        if reset {
            add_caveat(
                &mut trace,
                "provider.session.reset",
                "resume target vanished; retried once with a fresh conversation",
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

fn persist_session(dir: &std::path::Path, provider: &str, id: &str, reset: bool) {
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

#[cfg(test)]
mod tests {
    use super::{CliClaudeCodeProvider, EMPTY_MCP_CONFIG};
    #[cfg(unix)]
    use crate::ProviderKind;
    use crate::claude_events::ClaudeCapabilities;
    use crate::{ProviderEntry, ProviderRequest};
    use deadreckon_sandbox::WorkspaceAccess;

    #[cfg(unix)]
    struct DescendantGuard(std::path::PathBuf);

    #[cfg(unix)]
    impl Drop for DescendantGuard {
        fn drop(&mut self) {
            let Ok(raw) = std::fs::read_to_string(&self.0) else {
                return;
            };
            let Ok(pid) = raw.trim().parse::<u32>() else {
                return;
            };
            if deadreckon_core::pid_is_alive(pid) {
                let _ = deadreckon_core::terminate_pid(pid, true);
            }
        }
    }

    fn provider() -> CliClaudeCodeProvider {
        CliClaudeCodeProvider::new(
            "cli:claude-code",
            ProviderEntry {
                kind: None,
                api_key: None,
                api_key_env: None,
                base_url: None,
                model: None,
                input_cost_per_million: None,
                output_cost_per_million: None,
                binary: Some("claude".to_string()),
                extra_args: vec!["--dangerously-skip-permissions".to_string()],
            },
        )
    }

    #[test]
    fn read_only_claude_never_skips_permissions() {
        let args = provider().build_args(
            &ProviderRequest {
                workspace_access: WorkspaceAccess::ReadOnly,
                ..ProviderRequest::default()
            },
            &ClaudeCapabilities::none(),
            None,
        );
        assert!(
            !args
                .iter()
                .any(|arg| arg == "--dangerously-skip-permissions")
        );
    }

    #[test]
    fn worker_claude_keeps_legacy_permission_posture() {
        let args = provider().build_args(
            &ProviderRequest::default(),
            &ClaudeCapabilities::none(),
            None,
        );
        assert!(
            args.iter()
                .any(|arg| arg == "--dangerously-skip-permissions")
        );
    }

    #[test]
    fn schema_only_claude_uses_valid_empty_mcp_config() {
        let args = provider().build_args(
            &ProviderRequest {
                workspace_access: WorkspaceAccess::ReadOnly,
                output_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {"ok": {"type": "boolean"}},
                    "required": ["ok"],
                    "additionalProperties": false
                })),
                ..ProviderRequest::default()
            },
            &ClaudeCapabilities {
                stream_json: true,
                resume: true,
                json_schema: true,
                schema_only_posture: true,
            },
            None,
        );

        let config_index = args
            .iter()
            .position(|arg| arg == "--mcp-config")
            .expect("schema-only posture must supply an isolated MCP config");
        assert_eq!(
            args.get(config_index + 1).map(String::as_str),
            Some(EMPTY_MCP_CONFIG)
        );

        let config: serde_json::Value =
            serde_json::from_str(EMPTY_MCP_CONFIG).expect("empty MCP config must be valid JSON");
        assert!(config["mcpServers"].is_object());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_write_claude_probe_timeout_reaps_descendant_and_pid_authority() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::time::Duration;

        let temp = tempfile::tempdir().expect("tempdir");
        let binary = temp.path().join("hanging-claude-probe");
        let descendant_path = temp.path().join("descendant.pid");
        let _descendant_guard = DescendantGuard(descendant_path.clone());
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\nif [ \"${{1:-}}\" = '--help' ]; then\n  (trap '' TERM; sleep 30) &\n  descendant=$!\n  printf '%s\\n' \"$descendant\" > '{}'\n  trap '' TERM\n  wait\nfi\nexit 1\n",
                descendant_path.display()
            ),
        )
        .expect("fake claude");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
            .expect("fake claude permissions");
        let provider = CliClaudeCodeProvider::new(
            "cli:claude-code",
            ProviderEntry {
                kind: Some(ProviderKind::CliClaudeCode),
                api_key: None,
                api_key_env: None,
                base_url: None,
                model: None,
                input_cost_per_million: None,
                output_cost_per_million: None,
                binary: Some(binary.display().to_string()),
                extra_args: Vec::new(),
            },
        );
        let pid_file = temp.path().join("claude-probe.pid");
        let request = ProviderRequest {
            cwd: Some(temp.path().to_path_buf()),
            pid_file: Some(pid_file.clone()),
            ..ProviderRequest::default()
        };

        let started = std::time::Instant::now();
        let capabilities = provider
            .capabilities_for_request_with_timeout(&request, Duration::from_secs(1))
            .await
            .expect("clean probe timeout degrades capabilities");

        assert_eq!(capabilities, ClaudeCapabilities::none());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!pid_file.exists(), "probe retained PID authority");
        let descendant = std::fs::read_to_string(&descendant_path)
            .expect("descendant pid")
            .trim()
            .parse::<u32>()
            .expect("numeric descendant pid");
        assert!(
            !deadreckon_core::pid_is_alive(descendant),
            "Claude capability-probe descendant survived timeout"
        );
    }
}
