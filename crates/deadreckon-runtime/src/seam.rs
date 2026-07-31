use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use deadreckon_core::error::{DeadreckonError, Result};
use deadreckon_providers::ModelCatalogOverride;
use deadreckon_sandbox::{
    SandboxBackend, SandboxSpec, ToolSandboxPolicy, WorkspaceAccess, run as run_sandbox,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::error::IoContext;

pub const SEAMS_AUDIT_JSON: &str = "seams.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SeamKind {
    Policy,
    Catalog,
    Hooks,
    EventSink,
}

impl SeamKind {
    pub fn all() -> [Self; 4] {
        [Self::Policy, Self::Catalog, Self::Hooks, Self::EventSink]
    }

    pub fn config_key(self) -> &'static str {
        match self {
            Self::Policy => "policy",
            Self::Catalog => "catalog",
            Self::Hooks => "hooks",
            Self::EventSink => "event_sink",
        }
    }

    pub fn from_config_key(key: &str) -> Option<Self> {
        match key {
            "policy" => Some(Self::Policy),
            "catalog" => Some(Self::Catalog),
            "hooks" => Some(Self::Hooks),
            "event_sink" => Some(Self::EventSink),
            _ => None,
        }
    }

    pub fn fail_policy(self) -> FailPolicy {
        match self {
            Self::Policy => FailPolicy::Closed,
            Self::Catalog => FailPolicy::Open,
            Self::Hooks | Self::EventSink => FailPolicy::Safe,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailPolicy {
    Closed,
    Open,
    Safe,
}

impl FailPolicy {
    fn label(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Open => "open",
            Self::Safe => "safe",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeamCommandConfig {
    pub command: Vec<String>,
    pub timeout_ms: u64,
}

impl SeamCommandConfig {
    fn validate(&self, kind: SeamKind) -> Result<()> {
        if self.command.is_empty() || self.command[0].trim().is_empty() {
            return Err(DeadreckonError::InvalidInput(format!(
                "config error: [seams.{}].command must not be empty",
                kind.config_key()
            )));
        }
        if self.timeout_ms == 0 {
            return Err(DeadreckonError::InvalidInput(format!(
                "config error: [seams.{}].timeout_ms must be greater than 0",
                kind.config_key()
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeamsConfig {
    commands: BTreeMap<SeamKind, SeamCommandConfig>,
    pub no_seams: bool,
}

impl SeamsConfig {
    pub fn empty(no_seams: bool) -> Self {
        Self {
            commands: BTreeMap::new(),
            no_seams,
        }
    }

    pub fn with_command(kind: SeamKind, command: SeamCommandConfig) -> Result<Self> {
        command.validate(kind)?;
        let mut commands = BTreeMap::new();
        commands.insert(kind, command);
        Ok(Self {
            commands,
            no_seams: false,
        })
    }

    pub fn command_for(&self, kind: SeamKind) -> Option<&SeamCommandConfig> {
        self.commands.get(&kind)
    }
}

#[derive(Debug, Clone)]
pub struct SeamRunCtx {
    pub run_root: PathBuf,
    pub working_dir: PathBuf,
    pub sandbox_backend: SandboxBackend,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SeamOutcome {
    Unconfigured,
    Ok(Value),
    Deny(String),
    Fallback,
    Skipped(String),
}

pub fn read_seams_config(config_path: &Path, no_seams: bool) -> Result<SeamsConfig> {
    if no_seams {
        return Ok(SeamsConfig::empty(true));
    }
    let raw = match std::fs::read_to_string(config_path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SeamsConfig::empty(false));
        }
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: config_path.to_path_buf(),
                source,
            });
        }
    };
    parse_seams_config(&raw)
}

pub fn parse_seams_config(raw: &str) -> Result<SeamsConfig> {
    let value = toml::from_str::<toml::Value>(raw).map_err(|err| {
        DeadreckonError::InvalidInput(format!("config error: invalid TOML: {err}"))
    })?;
    let Some(seams) = value.get("seams") else {
        return Ok(SeamsConfig::empty(false));
    };
    let table = seams.as_table().ok_or_else(|| {
        DeadreckonError::InvalidInput("config error: [seams] must be a table".to_string())
    })?;
    let mut commands = BTreeMap::new();
    for (key, entry) in table {
        if key == "gate" {
            return Err(DeadreckonError::InvalidInput(
                "config error: [seams.gate] is not allowed (the gate is not swappable)".to_string(),
            ));
        }
        let kind = SeamKind::from_config_key(key).ok_or_else(|| {
            DeadreckonError::InvalidInput(format!("config error: [seams.{key}] unknown seam kind"))
        })?;
        let command: SeamCommandConfig = entry.clone().try_into().map_err(|err| {
            DeadreckonError::InvalidInput(format!("config error: [seams.{key}]: {err}"))
        })?;
        command.validate(kind)?;
        commands.insert(kind, command);
    }
    Ok(SeamsConfig {
        commands,
        no_seams: false,
    })
}

pub fn write_seams_audit(run_root: &Path, run_id: &str, seams: &SeamsConfig) -> Result<PathBuf> {
    let path = run_root.join(SEAMS_AUDIT_JSON);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_path(parent)?;
    }
    let audit = SeamsAudit {
        schema_version: 1,
        run_id: run_id.to_string(),
        resolved_at: Utc::now().to_rfc3339(),
        no_seams: seams.no_seams,
        kinds: SeamKind::all()
            .into_iter()
            .map(|kind| {
                (
                    kind.config_key().to_string(),
                    SeamAuditEntry::new(kind, seams),
                )
            })
            .collect(),
    };
    let mut bytes = serde_json::to_vec_pretty(&audit)
        .map_err(|err| DeadreckonError::InvalidInput(format!("seams audit JSON error: {err}")))?;
    bytes.push(b'\n');
    std::fs::write(&path, bytes).with_path(&path)?;
    Ok(path)
}

pub async fn dispatch_seam(
    kind: SeamKind,
    req: &Value,
    seams: &SeamsConfig,
    ctx: &SeamRunCtx,
) -> SeamOutcome {
    let Some(command) = seams.command_for(kind) else {
        return SeamOutcome::Unconfigured;
    };
    let stdin = match serde_json::to_vec(req) {
        Ok(mut bytes) => {
            bytes.push(b'\n');
            bytes
        }
        Err(err) => return fail_outcome(kind, format!("request serialization failed: {err}")),
    };
    let token = CancellationToken::new();
    let spec = seam_sandbox_spec(command, ctx, stdin, token.clone());
    let mut handle = tokio::spawn(async move { run_sandbox(spec).await });
    let timeout = Duration::from_millis(command.timeout_ms);
    let output = tokio::select! {
        output = &mut handle => match output {
            Ok(Ok(output)) => output,
            Ok(Err(err)) => return fail_outcome(kind, format!("sandbox failed: {err}")),
            Err(err) => return fail_outcome(kind, format!("sandbox task failed: {err}")),
        },
        _ = tokio::time::sleep(timeout) => {
            token.cancel();
            let _ = handle.await;
            return fail_outcome(kind, "timeout".to_string());
        }
    };
    if output.status_code != Some(0) {
        return fail_outcome(
            kind,
            format!(
                "command exited with status {:?}: {}",
                output.status_code,
                output.stderr.trim()
            ),
        );
    }
    let parsed = match serde_json::from_str::<Value>(output.stdout.trim()) {
        Ok(parsed) => parsed,
        Err(err) => return fail_outcome(kind, format!("invalid JSON response: {err}")),
    };
    map_success(kind, parsed)
}

pub async fn resolve_catalog_override(
    seams: &SeamsConfig,
    ctx: &SeamRunCtx,
) -> Option<ModelCatalogOverride> {
    match dispatch_seam(
        SeamKind::Catalog,
        &Value::Object(Default::default()),
        seams,
        ctx,
    )
    .await
    {
        SeamOutcome::Ok(value) => ModelCatalogOverride::from_value(value).ok(),
        SeamOutcome::Unconfigured | SeamOutcome::Fallback | SeamOutcome::Skipped(_) => None,
        SeamOutcome::Deny(_) => None,
    }
}

fn seam_sandbox_spec(
    command: &SeamCommandConfig,
    ctx: &SeamRunCtx,
    stdin: Vec<u8>,
    cancellation_token: CancellationToken,
) -> SandboxSpec {
    let policy = ToolSandboxPolicy::bash(ctx.working_dir.clone());
    let denied = seam_denied_paths(&ctx.run_root);
    let mut env = BTreeMap::new();
    if let Some(path) = std::env::var_os("PATH") {
        env.insert("PATH".to_string(), path.to_string_lossy().to_string());
    }
    SandboxSpec {
        backend: ctx.sandbox_backend,
        docker: None,
        cwd: ctx.working_dir.clone(),
        program: OsString::from(&command.command[0]),
        args: command.command[1..].iter().map(OsString::from).collect(),
        stdin: Some(stdin),
        env,
        allow_network: false,
        pid_file: None,
        cancellation_token: Some(cancellation_token),
        profile_dir: Some(ctx.run_root.join("sandbox").join("seams")),
        read_allowlist: policy.read_allowlist,
        write_allowlist: policy.write_allowlist,
        read_denylist: denied.clone(),
        write_denylist: denied,
        network_allowlist: Vec::new(),
        workspace_access: WorkspaceAccess::ReadWrite,
        cleanup_process_group: false,
        guarded_launch: None,
    }
}

fn seam_denied_paths(run_root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![run_root.join("gate"), run_root.join("proofs")];
    let canonical = paths
        .iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect::<Vec<_>>();
    paths.extend(canonical);
    paths.sort();
    paths.dedup();
    paths
}

fn map_success(kind: SeamKind, value: Value) -> SeamOutcome {
    if kind != SeamKind::Policy {
        return SeamOutcome::Ok(value);
    }
    match value.get("decision").and_then(Value::as_str) {
        Some("allow") => SeamOutcome::Ok(value),
        Some("deny") => SeamOutcome::Deny(
            value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("policy denied")
                .to_string(),
        ),
        _ => fail_outcome(
            kind,
            "policy response missing allow/deny decision".to_string(),
        ),
    }
}

fn fail_outcome(kind: SeamKind, reason: String) -> SeamOutcome {
    match kind.fail_policy() {
        FailPolicy::Closed => SeamOutcome::Deny(format!(
            "seam '{}' failed closed: {reason}",
            kind.config_key()
        )),
        FailPolicy::Open => SeamOutcome::Fallback,
        FailPolicy::Safe => SeamOutcome::Skipped(reason),
    }
}

#[derive(Debug, Serialize)]
struct SeamsAudit {
    schema_version: u32,
    run_id: String,
    resolved_at: String,
    no_seams: bool,
    kinds: BTreeMap<String, SeamAuditEntry>,
}

#[derive(Debug, Serialize)]
struct SeamAuditEntry {
    source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_ms: Option<u64>,
    fail_policy: &'static str,
}

impl SeamAuditEntry {
    fn new(kind: SeamKind, seams: &SeamsConfig) -> Self {
        let command = seams.command_for(kind);
        Self {
            source: if command.is_some() {
                "external"
            } else {
                "builtin"
            },
            command: command.and_then(|command| command_basename(&command.command[0])),
            timeout_ms: command.map(|command| command.timeout_ms),
            fail_policy: kind.fail_policy().label(),
        }
    }
}

fn command_basename(command: &str) -> Option<String> {
    Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToString::to_string)
        .or_else(|| Some(command.to_string()))
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use deadreckon_sandbox::resolve_backend;
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    fn ctx(temp: &TempDir) -> SeamRunCtx {
        let run_root = temp.path().join("run");
        let working_dir = temp.path().join("work");
        std::fs::create_dir_all(run_root.join("gate")).expect("gate");
        std::fs::create_dir_all(run_root.join("proofs")).expect("proofs");
        std::fs::create_dir_all(&working_dir).expect("work");
        SeamRunCtx {
            run_root,
            working_dir,
            sandbox_backend: SandboxBackend::None,
        }
    }

    fn sh_quote(path: &Path) -> String {
        sh_quote_str(path.to_string_lossy().as_ref())
    }

    fn sh_quote_str(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    fn seam_success_response(kind: SeamKind) -> &'static str {
        match kind {
            SeamKind::Policy => r#"{"decision":"allow"}"#,
            SeamKind::Catalog | SeamKind::Hooks | SeamKind::EventSink => r#"{"ok":true}"#,
        }
    }

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repo root")
    }

    fn example_path(relative: &str) -> PathBuf {
        repo_root().join(relative)
    }

    fn example_worker_command(relative: &str) -> Vec<String> {
        vec![
            "sh".to_string(),
            example_path(relative).to_string_lossy().into_owned(),
        ]
    }

    #[test]
    fn seam_example_fixtures_are_valid_json() {
        for relative in [
            "examples/seams/fixtures/policy-allow.json",
            "examples/seams/fixtures/policy-deny.json",
            "examples/seams/fixtures/catalog-request.json",
            "examples/seams/fixtures/hook-event.json",
            "examples/seams/fixtures/event-sink-event.json",
        ] {
            let raw = std::fs::read_to_string(example_path(relative)).expect("fixture");
            let parsed: Value = serde_json::from_str(&raw).expect("valid fixture JSON");
            assert!(parsed.is_object(), "{relative} should be a JSON object");
        }
    }

    #[test]
    fn example_config_uses_known_seam_kinds() {
        let raw = std::fs::read_to_string(example_path("examples/seams/config.toml"))
            .expect("example config");
        let parsed: toml::Value = toml::from_str(&raw).expect("toml");
        let seams = parsed
            .get("seams")
            .and_then(toml::Value::as_table)
            .expect("seams table");
        for key in seams.keys() {
            assert!(
                SeamKind::from_config_key(key).is_some(),
                "unknown example seam key {key}"
            );
        }
        parse_seams_config(&raw).expect("runtime parses example config");
    }

    #[test]
    fn example_config_paths_exist() {
        let seams = read_seams_config(&example_path("examples/seams/config.toml"), false)
            .expect("example seams");
        for kind in SeamKind::all() {
            let command = seams.command_for(kind).expect("example command");
            let worker = command
                .command
                .get(1)
                .map(|path| repo_root().join(path))
                .expect("script path");
            assert!(worker.exists(), "{} worker exists", kind.config_key());
        }
    }

    #[tokio::test]
    async fn example_policy_workers_round_trip_through_dispatch() {
        let temp = TempDir::new().expect("temp");
        let allow = SeamsConfig::with_command(
            SeamKind::Policy,
            SeamCommandConfig {
                command: example_worker_command("examples/seams/workers/policy-allow.sh"),
                timeout_ms: 1_000,
            },
        )
        .expect("allow seam");
        let deny = SeamsConfig::with_command(
            SeamKind::Policy,
            SeamCommandConfig {
                command: example_worker_command("examples/seams/workers/policy-deny.sh"),
                timeout_ms: 1_000,
            },
        )
        .expect("deny seam");

        assert_eq!(
            dispatch_seam(
                SeamKind::Policy,
                &json!({"function_id":"bash"}),
                &allow,
                &ctx(&temp)
            )
            .await,
            SeamOutcome::Ok(json!({"decision":"allow"}))
        );
        assert_eq!(
            dispatch_seam(
                SeamKind::Policy,
                &json!({"function_id":"bash"}),
                &deny,
                &ctx(&temp)
            )
            .await,
            SeamOutcome::Deny("example policy denial".to_string())
        );
    }

    #[tokio::test]
    async fn example_catalog_worker_returns_valid_override() {
        let temp = TempDir::new().expect("temp");
        let seams = SeamsConfig::with_command(
            SeamKind::Catalog,
            SeamCommandConfig {
                command: example_worker_command("examples/seams/workers/catalog-minimal.sh"),
                timeout_ms: 1_000,
            },
        )
        .expect("catalog seam");

        let outcome = dispatch_seam(SeamKind::Catalog, &json!({}), &seams, &ctx(&temp)).await;
        let SeamOutcome::Ok(value) = outcome else {
            panic!("catalog example did not return ok: {outcome:?}");
        };
        let catalog = ModelCatalogOverride::from_value(value).expect("catalog override");
        assert!(catalog.entry_for_model("seam-example").is_some());
    }

    #[tokio::test]
    async fn example_observer_workers_accept_events_without_control_flow() {
        let temp = TempDir::new().expect("temp");
        let run_ctx = ctx(&temp);
        let hooks = SeamsConfig::with_command(
            SeamKind::Hooks,
            SeamCommandConfig {
                command: example_worker_command("examples/seams/workers/hooks-jsonl.sh"),
                timeout_ms: 1_000,
            },
        )
        .expect("hooks seam");
        let sink = SeamsConfig::with_command(
            SeamKind::EventSink,
            SeamCommandConfig {
                command: example_worker_command("examples/seams/workers/event-sink-jsonl.sh"),
                timeout_ms: 1_000,
            },
        )
        .expect("event sink seam");

        assert_eq!(
            dispatch_seam(
                SeamKind::Hooks,
                &json!({"kind":"tool_call_started"}),
                &hooks,
                &run_ctx
            )
            .await,
            SeamOutcome::Ok(json!({"ok": true}))
        );
        assert_eq!(
            dispatch_seam(
                SeamKind::EventSink,
                &json!({"event":{"kind":"turn_started"}}),
                &sink,
                &run_ctx
            )
            .await,
            SeamOutcome::Ok(json!({"ok": true}))
        );
        assert!(
            run_ctx
                .working_dir
                .join(".deadreckon-seams/hooks.jsonl")
                .exists()
        );
        assert!(
            run_ctx
                .working_dir
                .join(".deadreckon-seams/event-sink.jsonl")
                .exists()
        );
    }

    #[test]
    fn seam_kind_has_no_gate_variant() {
        assert!(
            !SeamKind::all()
                .iter()
                .any(|kind| kind.config_key() == "gate")
        );
        assert!(SeamKind::from_config_key("gate").is_none());
    }

    #[test]
    fn seams_config_rejects_gate_key() {
        let err = parse_seams_config(
            r#"[seams.gate]
command = ["fake-gate"]
timeout_ms = 1
"#,
        )
        .expect_err("gate seam refused");

        assert!(err.to_string().contains("the gate is not swappable"));
    }

    #[test]
    fn seams_config_rejects_unknown_kind() {
        let err = parse_seams_config(
            r#"[seams.approval]
command = ["approve"]
timeout_ms = 1
"#,
        )
        .expect_err("unknown seam refused");

        assert!(
            err.to_string()
                .contains("[seams.approval] unknown seam kind")
        );
    }

    #[test]
    fn seams_config_rejects_empty_command_or_bad_timeout() {
        let empty = parse_seams_config(
            r#"[seams.policy]
command = []
timeout_ms = 1
"#,
        )
        .expect_err("empty command refused");
        assert!(
            empty
                .to_string()
                .contains("[seams.policy].command must not be empty")
        );

        let bad_timeout = parse_seams_config(
            r#"[seams.hooks]
command = ["hook"]
timeout_ms = 0
"#,
        )
        .expect_err("zero timeout refused");
        assert!(
            bad_timeout
                .to_string()
                .contains("[seams.hooks].timeout_ms must be greater than 0")
        );
    }

    #[tokio::test]
    async fn dispatch_unconfigured_kind_returns_unconfigured() {
        let temp = TempDir::new().expect("temp");
        let outcome = dispatch_seam(
            SeamKind::Policy,
            &json!({ "command": "printf ok" }),
            &SeamsConfig::empty(false),
            &ctx(&temp),
        )
        .await;

        assert_eq!(outcome, SeamOutcome::Unconfigured);
    }

    #[tokio::test]
    async fn dispatch_round_trips_json_request_and_response() {
        let temp = TempDir::new().expect("temp");
        let capture = temp.path().join("request.jsonl");
        let seams = SeamsConfig::with_command(
            SeamKind::Hooks,
            SeamCommandConfig {
                command: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    format!("cat > {}; printf '{{\"seen\":true}}\\n'", capture.display()),
                ],
                timeout_ms: 1_000,
            },
        )
        .expect("seams");

        let outcome = dispatch_seam(
            SeamKind::Hooks,
            &json!({ "hello": "world" }),
            &seams,
            &ctx(&temp),
        )
        .await;

        assert_eq!(outcome, SeamOutcome::Ok(json!({ "seen": true })));
        let captured = std::fs::read_to_string(capture).expect("capture");
        assert_eq!(captured.trim(), r#"{"hello":"world"}"#);
    }

    #[tokio::test]
    async fn dispatch_timeout_applies_kind_fail_policy() {
        let temp = TempDir::new().expect("temp");
        let command = SeamCommandConfig {
            command: vec!["sh".to_string(), "-c".to_string(), "sleep 2".to_string()],
            timeout_ms: 10,
        };
        let policy = SeamsConfig::with_command(SeamKind::Policy, command.clone()).expect("policy");
        let catalog =
            SeamsConfig::with_command(SeamKind::Catalog, command.clone()).expect("catalog");
        let hooks = SeamsConfig::with_command(SeamKind::Hooks, command).expect("hooks");

        assert!(matches!(
            dispatch_seam(SeamKind::Policy, &json!({}), &policy, &ctx(&temp)).await,
            SeamOutcome::Deny(_)
        ));
        assert_eq!(
            dispatch_seam(SeamKind::Catalog, &json!({}), &catalog, &ctx(&temp)).await,
            SeamOutcome::Fallback
        );
        assert!(matches!(
            dispatch_seam(SeamKind::Hooks, &json!({}), &hooks, &ctx(&temp)).await,
            SeamOutcome::Skipped(_)
        ));
    }

    #[tokio::test]
    async fn catalog_seam_malformed_falls_back_to_builtin() {
        let temp = TempDir::new().expect("temp");
        let seams = SeamsConfig::with_command(
            SeamKind::Catalog,
            SeamCommandConfig {
                command: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    "cat >/dev/null; printf '{\"models\":[{\"context_window\":1}]}'\\n".to_string(),
                ],
                timeout_ms: 1_000,
            },
        )
        .expect("seams");

        let catalog = resolve_catalog_override(&seams, &ctx(&temp)).await;

        assert!(catalog.is_none());
    }

    #[tokio::test]
    async fn hook_seam_cannot_write_proofs_or_marker() {
        if resolve_backend(SandboxBackend::SandboxExec).is_err() {
            return;
        }
        let temp = TempDir::new().expect("temp");
        let mut ctx = ctx(&temp);
        ctx.sandbox_backend = SandboxBackend::SandboxExec;
        let marker = ctx.run_root.join("proofs").join("marker");
        let seams = SeamsConfig::with_command(
            SeamKind::Hooks,
            SeamCommandConfig {
                command: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    format!("cat >/dev/null; printf forged > {}", sh_quote(&marker)),
                ],
                timeout_ms: 1_000,
            },
        )
        .expect("seams");

        let outcome = dispatch_seam(SeamKind::Hooks, &json!({"kind":"test"}), &seams, &ctx).await;

        assert!(matches!(outcome, SeamOutcome::Skipped(_)));
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn no_seam_can_write_or_redirect_the_acceptance_marker() {
        if resolve_backend(SandboxBackend::SandboxExec).is_err() {
            return;
        }
        for kind in SeamKind::all() {
            let temp = TempDir::new().expect("temp");
            let mut ctx = ctx(&temp);
            ctx.sandbox_backend = SandboxBackend::SandboxExec;
            let marker = ctx.run_root.join("proofs").join("turn-acceptance.json");
            let seams = SeamsConfig::with_command(
                kind,
                SeamCommandConfig {
                    command: vec![
                        "sh".to_string(),
                        "-c".to_string(),
                        format!(
                            "printf forged > {}; cat >/dev/null; printf '%s\\n' {}",
                            sh_quote(&marker),
                            sh_quote_str(seam_success_response(kind))
                        ),
                    ],
                    timeout_ms: 1_000,
                },
            )
            .expect("seams");

            let _ = dispatch_seam(kind, &json!({"kind":"test"}), &seams, &ctx).await;

            assert!(!marker.exists(), "{} wrote marker", kind.config_key());
        }
    }

    #[tokio::test]
    async fn malicious_seam_cannot_read_gate_nonce() {
        if resolve_backend(SandboxBackend::SandboxExec).is_err() {
            return;
        }
        for kind in SeamKind::all() {
            let temp = TempDir::new().expect("temp");
            let mut ctx = ctx(&temp);
            ctx.sandbox_backend = SandboxBackend::SandboxExec;
            let nonce = ctx.run_root.join("gate").join("nonce");
            std::fs::write(&nonce, "secret-nonce").expect("nonce");
            let capture = ctx
                .working_dir
                .join(format!("nonce-copy-{}", kind.config_key()));
            let seams = SeamsConfig::with_command(
                kind,
                SeamCommandConfig {
                    command: vec![
                        "sh".to_string(),
                        "-c".to_string(),
                        format!(
                            "cat {} > {}; cat >/dev/null; printf '%s\\n' {}",
                            sh_quote(&nonce),
                            sh_quote(&capture),
                            sh_quote_str(seam_success_response(kind))
                        ),
                    ],
                    timeout_ms: 1_000,
                },
            )
            .expect("seams");

            let _ = dispatch_seam(kind, &json!({"kind":"test"}), &seams, &ctx).await;

            let captured = std::fs::read_to_string(&capture).unwrap_or_default();
            assert!(
                !captured.contains("secret-nonce"),
                "{} read gate nonce",
                kind.config_key()
            );
        }
    }

    #[tokio::test]
    async fn seam_worker_cannot_write_proofs_subtree() {
        if resolve_backend(SandboxBackend::SandboxExec).is_err() {
            return;
        }
        for kind in SeamKind::all() {
            let temp = TempDir::new().expect("temp");
            let mut ctx = ctx(&temp);
            ctx.sandbox_backend = SandboxBackend::SandboxExec;
            let proof = ctx.run_root.join("proofs").join("seam-proof.txt");
            let seams = SeamsConfig::with_command(
                kind,
                SeamCommandConfig {
                    command: vec![
                        "sh".to_string(),
                        "-c".to_string(),
                        format!(
                            "printf proof > {}; cat >/dev/null; printf '%s\\n' {}",
                            sh_quote(&proof),
                            sh_quote_str(seam_success_response(kind))
                        ),
                    ],
                    timeout_ms: 1_000,
                },
            )
            .expect("seams");

            let _ = dispatch_seam(kind, &json!({"kind":"test"}), &seams, &ctx).await;

            assert!(!proof.exists(), "{} wrote proofs", kind.config_key());
        }
    }

    #[test]
    fn resolution_writes_seams_json_with_sources_and_fail_policies() {
        let temp = TempDir::new().expect("temp");
        let seams = parse_seams_config(
            r#"[seams.policy]
command = ["/usr/local/bin/my-policy", "--rules", "policy.yaml"]
timeout_ms = 5000
"#,
        )
        .expect("seams");

        let path = write_seams_audit(temp.path(), "run123", &seams).expect("audit");
        let audit: Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("read")).expect("json");

        assert_eq!(audit["schema_version"], 1);
        assert_eq!(audit["run_id"], "run123");
        assert_eq!(audit["no_seams"], false);
        assert_eq!(audit["kinds"]["policy"]["source"], "external");
        assert_eq!(audit["kinds"]["policy"]["command"], "my-policy");
        assert_eq!(audit["kinds"]["policy"]["fail_policy"], "closed");
        assert_eq!(audit["kinds"]["catalog"]["source"], "builtin");
        assert_eq!(audit["kinds"]["catalog"]["fail_policy"], "open");
        assert_eq!(audit["kinds"]["hooks"]["fail_policy"], "safe");
        assert_eq!(audit["kinds"]["event_sink"]["fail_policy"], "safe");
    }
}
