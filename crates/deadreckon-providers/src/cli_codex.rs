use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use deadreckon_sandbox::{SandboxBackend, WorkspaceAccess};
use serde_json::json;
use tokio::sync::{Mutex, OnceCell};
use which::which;

use crate::cli_common::{CliOutput, ensure_success, run_cli, write_output};
use crate::cli_contract::{
    PROVIDER_ID_CODEX, ParsedStream, ProviderContract, ProviderSession, add_caveat,
    flight_rows_from, session_not_found, write_schema_file,
};
use crate::codex_events::{
    CodexCapabilities, parse_codex_capabilities, parse_codex_capabilities_with_features,
    parse_codex_line, probe_codex_capabilities, structured_text_features_to_disable,
};
#[cfg(test)]
use crate::codex_events::STRUCTURED_TEXT_DISABLED_FEATURES;
use crate::{
    Provider, ProviderEntry, ProviderError, ProviderFuture, ProviderKind, ProviderRequest,
    ProviderResponse, ProviderUsage, Result, SpendEstimate, validate_openai_strict_output_schema,
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

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct CodexBinaryVersion {
    binary: String,
    version: String,
}

fn codex_probe_cache()
-> &'static Mutex<HashMap<CodexBinaryVersion, Arc<OnceCell<CodexCapabilities>>>> {
    static CACHE: OnceLock<Mutex<HashMap<CodexBinaryVersion, Arc<OnceCell<CodexCapabilities>>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

struct RemoveFileOnDrop(Option<PathBuf>);

impl Drop for RemoveFileOnDrop {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
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
        let structured_text_only = request.output_schema.is_some();
        let mut args = vec!["--ask-for-approval".to_string(), "never".to_string()];
        if structured_text_only {
            for feature in structured_text_features_to_disable(*caps) {
                args.extend(["--disable".to_string(), feature.to_string()]);
            }
        }
        args.push("exec".to_string());
        if structured_text_only {
            args.extend([
                "--ephemeral".to_string(),
                "--ignore-user-config".to_string(),
                "--ignore-rules".to_string(),
                "--strict-config".to_string(),
            ]);
        }
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
                codex_sandbox_mode(request.sandbox_backend, request.workspace_access).to_string(),
            ]);
        }
        if request.workspace_access == WorkspaceAccess::ReadOnly {
            args.extend(read_only_safe_extra_args(&self.extra_args));
        } else {
            args.extend(self.extra_args.clone());
        }
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
            request.workspace_access,
            true,
        )
        .await?;
        Ok(CodexAttempt { output, args })
    }

    async fn capabilities_for_request(
        &self,
        request: &ProviderRequest,
    ) -> Result<CodexCapabilities> {
        if request.workspace_access == WorkspaceAccess::ReadWrite {
            return Ok(probe_codex_capabilities(&self.binary));
        }
        if request.output_schema.is_none() {
            return Ok(run_cli(
                &self.name,
                &self.binary,
                &["exec".to_string(), "--help".to_string()],
                request.cwd.clone(),
                request.sandbox_backend,
                None,
                request.cancellation_token.clone(),
                WorkspaceAccess::ReadOnly,
                true,
            )
            .await
            .ok()
            .filter(|output| output.status_code == Some(0))
            .map(|output| parse_codex_capabilities(&output.stdout))
            .unwrap_or_else(CodexCapabilities::none));
        }

        let version_output = run_cli(
            &self.name,
            &self.binary,
            &["--version".to_string()],
            request.cwd.clone(),
            request.sandbox_backend,
            None,
            request.cancellation_token.clone(),
            WorkspaceAccess::ReadOnly,
            true,
        )
        .await?;
        if version_output.status_code != Some(0) {
            return Err(ProviderError::Cli {
                provider: self.name.clone(),
                detail: "schema-only request could not prove the Codex binary version".to_string(),
            });
        }
        let key = CodexBinaryVersion {
            binary: self.binary.clone(),
            version: format!("{}{}", version_output.stdout, version_output.stderr)
                .trim()
                .to_string(),
        };
        let cell = {
            let mut cache = codex_probe_cache().lock().await;
            cache
                .entry(key)
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        let capabilities = cell
            .get_or_try_init(|| async {
                let help = run_cli(
                    &self.name,
                    &self.binary,
                    &["exec".to_string(), "--help".to_string()],
                    request.cwd.clone(),
                    request.sandbox_backend,
                    None,
                    request.cancellation_token.clone(),
                    WorkspaceAccess::ReadOnly,
                    true,
                )
                .await;
                let features = run_cli(
                    &self.name,
                    &self.binary,
                    &["features".to_string(), "list".to_string()],
                    request.cwd.clone(),
                    request.sandbox_backend,
                    None,
                    request.cancellation_token.clone(),
                    WorkspaceAccess::ReadOnly,
                    true,
                )
                .await;
                match (help, features) {
                    (Ok(help), Ok(features))
                        if help.status_code == Some(0) && features.status_code == Some(0) =>
                    {
                        Ok(parse_codex_capabilities_with_features(
                            &help.stdout,
                            &features.stdout,
                        ))
                    }
                    (help, features) => Err(ProviderError::Cli {
                        provider: self.name.clone(),
                        detail: format!(
                            "schema-only Codex capability probe failed: exec help: {}; feature list: {}",
                            probe_failure(help),
                            probe_failure(features)
                        ),
                    }),
                }
            })
            .await?;
        Ok(*capabilities)
    }

    pub(crate) async fn run(&self, request: &ProviderRequest) -> Result<ProviderResponse> {
        if let Some(schema) = request.output_schema.as_ref() {
            validate_openai_strict_output_schema(&self.name, schema)?;
        }
        let caps = self.capabilities_for_request(request).await?;
        if request.output_schema.is_some() && !supports_schema_only_posture(caps) {
            return Err(ProviderError::Cli {
                provider: self.name.clone(),
                detail: "installed Codex cannot prove schema-only structured-text posture; update Codex or select a capable provider".to_string(),
            });
        }
        let session_dir = (request.workspace_access == WorkspaceAccess::ReadWrite)
            .then(|| request.session_dir.clone())
            .flatten();
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

        // A fresh read-only request deliberately has no resumable worker
        // session. Stage its controller-owned schema beside the temporary
        // output file, never inside the inspected operator source.
        let fallback_schema_dir = std::env::temp_dir();
        let schema_dir = session_dir
            .as_deref()
            .or_else(|| {
                request
                    .output_path
                    .as_deref()
                    .and_then(std::path::Path::parent)
            })
            .unwrap_or(fallback_schema_dir.as_path());
        let schema_file = match (&request.output_schema, caps.output_schema) {
            (Some(schema), true) => Some(write_schema_file(schema_dir, schema).await?),
            _ => None,
        };
        let _schema_cleanup = RemoveFileOnDrop(schema_file.clone());

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
        let content = match (&last_message, parsed.answer.as_ref(), degraded) {
            (Some(msg), _, false) => msg.clone(),
            (_, Some(answer), false) => answer.clone(),
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
            "workspace_access": request.workspace_access.as_str(),
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

fn supports_schema_only_posture(caps: CodexCapabilities) -> bool {
    caps.output_schema
        && caps.ephemeral
        && caps.ignore_user_config
        && caps.ignore_rules
        && caps.strict_config
        && caps.disable_features
        && caps.structured_text_features
}

fn probe_failure(result: Result<CliOutput>) -> String {
    match result {
        Ok(output) => format!(
            "exit {:?}: {}{}",
            output.status_code, output.stdout, output.stderr
        ),
        Err(error) => error.to_string(),
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

fn codex_sandbox_mode(
    outer_backend: Option<SandboxBackend>,
    workspace_access: WorkspaceAccess,
) -> &'static str {
    if workspace_access == WorkspaceAccess::ReadOnly {
        return "read-only";
    }
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

fn read_only_safe_extra_args(extra_args: &[String]) -> Vec<String> {
    let mut safe = Vec::with_capacity(extra_args.len());
    let mut skip_next = false;
    for arg in extra_args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if arg == "--sandbox" {
            skip_next = true;
            continue;
        }
        if arg.starts_with("--sandbox=")
            || matches!(
                arg.as_str(),
                "--dangerously-bypass-approvals-and-sandbox" | "--yolo"
            )
        {
            continue;
        }
        safe.push(arg.clone());
    }
    safe
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
    use super::*;

    #[test]
    fn read_only_codex_strips_configured_sandbox_bypasses() {
        let safe = read_only_safe_extra_args(&[
            "--sandbox".to_string(),
            "danger-full-access".to_string(),
            "--sandbox=danger-full-access".to_string(),
            "--yolo".to_string(),
            "--json".to_string(),
        ]);
        assert_eq!(safe, vec!["--json"]);
    }

    fn schema_only_capabilities() -> CodexCapabilities {
        CodexCapabilities {
            json: true,
            output_last_message: true,
            output_schema: true,
            resume: true,
            ephemeral: true,
            ignore_user_config: true,
            ignore_rules: true,
            strict_config: true,
            disable_features: true,
            structured_text_features: true,
            structured_text_disable_mask: (1_u32 << STRUCTURED_TEXT_DISABLED_FEATURES.len()) - 1,
        }
    }

    #[test]
    fn codex_done_authoring_is_ephemeral_and_disables_tool_surfaces() {
        let provider = CliCodexProvider::new(
            "cli:codex",
            ProviderEntry {
                kind: Some(ProviderKind::CliCodex),
                api_key: None,
                api_key_env: None,
                base_url: None,
                model: Some("gpt-test".to_string()),
                input_cost_per_million: None,
                output_cost_per_million: None,
                binary: Some("codex".to_string()),
                extra_args: Vec::new(),
            },
        );
        let mut request = ProviderRequest::enforceably_read_only("return json", 100, ".");
        request.output_schema = Some(json!({"type": "object"}));
        let args = provider.build_args(
            &request,
            &schema_only_capabilities(),
            None,
            None,
            Some(Path::new("schema.json")),
        );

        for required in [
            "--ephemeral",
            "--ignore-user-config",
            "--ignore-rules",
            "--strict-config",
            "--output-schema",
        ] {
            assert!(args.iter().any(|arg| arg == required), "{args:?}");
        }
        for feature in structured_text_features_to_disable(schema_only_capabilities()) {
            assert!(
                args.windows(2).any(|pair| pair == ["--disable", feature]),
                "{args:?}"
            );
        }
        assert!(!args.iter().any(|arg| arg == "resume"), "{args:?}");
    }

    #[test]
    fn structured_text_posture_never_silently_degrades_to_tools() {
        let mut capabilities = schema_only_capabilities();
        capabilities.structured_text_features = false;
        assert!(!supports_schema_only_posture(capabilities));
        capabilities.structured_text_features = true;
        capabilities.output_schema = false;
        assert!(!supports_schema_only_posture(capabilities));
    }

    #[tokio::test]
    async fn incompatible_schema_fails_before_codex_is_started() {
        let provider = CliCodexProvider::new(
            "cli:codex",
            ProviderEntry {
                kind: Some(ProviderKind::CliCodex),
                api_key: None,
                api_key_env: None,
                base_url: None,
                model: Some("gpt-test".to_string()),
                input_cost_per_million: None,
                output_cost_per_million: None,
                binary: Some("binary-that-must-not-run".to_string()),
                extra_args: Vec::new(),
            },
        );
        let mut request = ProviderRequest::enforceably_read_only("json", 100, ".");
        request.output_schema = Some(json!({
            "type": "object",
            "additionalProperties": {"type": "string"},
            "required": [],
            "properties": {}
        }));
        let error = provider
            .run(&request)
            .await
            .expect_err("controller schema must fail locally");
        assert!(matches!(error, ProviderError::InvalidOutputSchema { .. }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn codex_capabilities_are_probed_once_per_binary_version() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp = tempfile::tempdir().expect("tempdir");
        let binary = temp.path().join("fake-codex");
        let count = temp.path().join("probes.log");
        let features = STRUCTURED_TEXT_DISABLED_FEATURES
            .iter()
            .map(|feature| format!("{feature} stable true"))
            .collect::<Vec<_>>()
            .join("\\n");
        let script = format!(
            "#!/bin/sh\n\
if printf '%s\\n' \"$*\" | grep -q -- '--version'; then echo 'codex-cli test-1'; exit 0; fi\n\
if [ \"$*\" = 'exec --help' ]; then\n\
  echo help >> '{count}'\n\
  printf '%s\\n' 'resume --json --output-last-message --output-schema --ephemeral --ignore-user-config --ignore-rules --strict-config --disable'\n\
  exit 0\n\
fi\n\
if [ \"$*\" = 'features list' ]; then\n\
  echo features >> '{count}'\n\
  printf '%b\\n' '{features}'\n\
  exit 0\n\
fi\n\
printf '%s\\n' '{{\"type\":\"thread.started\",\"thread_id\":\"schema-only\"}}'\n\
printf '%s\\n' '{{\"type\":\"item.completed\",\"item\":{{\"id\":\"item-1\",\"type\":\"agent_message\",\"text\":\"{{\\\"ok\\\":true}}\"}}}}'\n\
printf '%s\\n' '{{\"type\":\"turn.completed\",\"usage\":{{\"input_tokens\":1,\"output_tokens\":1}}}}'\n",
            count = count.display(),
        );
        std::fs::write(&binary, script).expect("fake codex");
        let mut permissions = std::fs::metadata(&binary).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&binary, permissions).expect("chmod");
        let provider = CliCodexProvider::new(
            "cli:codex",
            ProviderEntry {
                kind: Some(ProviderKind::CliCodex),
                api_key: None,
                api_key_env: None,
                base_url: None,
                model: Some("gpt-test".to_string()),
                input_cost_per_million: None,
                output_cost_per_million: None,
                binary: Some(binary.display().to_string()),
                extra_args: Vec::new(),
            },
        );
        let mut request = ProviderRequest::enforceably_read_only("json", 100, temp.path());
        request.sandbox_backend = None;
        request.output_schema = Some(json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["ok"],
            "properties": {"ok": {"type": "boolean"}}
        }));

        provider.run(&request).await.expect("first request");
        provider.run(&request).await.expect("second request");
        assert_eq!(
            std::fs::read_to_string(count)
                .expect("probe count")
                .lines()
                .count(),
            2
        );
    }
}
