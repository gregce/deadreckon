use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use deadreckon_core::error::{DeadreckonError, Result};
use deadreckon_providers::{ModelCatalogOverride, ProviderCleanup, ProviderPhaseDeadline};
use deadreckon_sandbox::{
    SandboxBackend, SandboxSpec, ToolSandboxPolicy, WorkspaceAccess, run as run_sandbox,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::error::IoContext;

pub const SEAMS_AUDIT_JSON: &str = "seams.json";
const SEAM_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);

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
    /// The worker exceeded its execution bound and DeadReckon could not prove
    /// that every process retaining the seam's authority was reaped.
    LostContainment(String),
}

/// A seam result observed through the enclosing Job's work boundary.
///
/// `Completed` includes the seam's own configured timeout/fail-policy result.
/// The other variants are reserved for the enclosing Job deadline and
/// controller cancellation so a short worker timeout remains a safety ceiling
/// instead of becoming a fresh phase-local Job clock.
#[derive(Debug, PartialEq)]
pub enum SeamPhaseOutcome<T> {
    Completed(T),
    WorkExpired { cleanup: ProviderCleanup },
    Cancelled { cleanup: ProviderCleanup },
}

impl<T> SeamPhaseOutcome<T> {
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> SeamPhaseOutcome<U> {
        match self {
            Self::Completed(value) => SeamPhaseOutcome::Completed(map(value)),
            Self::WorkExpired { cleanup } => SeamPhaseOutcome::WorkExpired { cleanup },
            Self::Cancelled { cleanup } => SeamPhaseOutcome::Cancelled { cleanup },
        }
    }
}

impl<T, E> SeamPhaseOutcome<std::result::Result<T, E>> {
    pub fn transpose(self) -> std::result::Result<SeamPhaseOutcome<T>, E> {
        match self {
            Self::Completed(result) => result.map(SeamPhaseOutcome::Completed),
            Self::WorkExpired { cleanup } => Ok(SeamPhaseOutcome::WorkExpired { cleanup }),
            Self::Cancelled { cleanup } => Ok(SeamPhaseOutcome::Cancelled { cleanup }),
        }
    }
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
    match dispatch_seam_inner(kind, req, seams, ctx, None, None).await {
        SeamPhaseOutcome::Completed(outcome) => outcome,
        SeamPhaseOutcome::WorkExpired { .. } | SeamPhaseOutcome::Cancelled { .. } => {
            unreachable!("standalone seam dispatch has no enclosing Job boundary")
        }
    }
}

/// Dispatch a seam under the exact absolute work cutoff and cancellation
/// authority owned by the enclosing Job attempt.
pub async fn dispatch_seam_phase(
    kind: SeamKind,
    req: &Value,
    seams: &SeamsConfig,
    ctx: &SeamRunCtx,
    deadline: ProviderPhaseDeadline,
    cancellation: &CancellationToken,
) -> SeamPhaseOutcome<SeamOutcome> {
    dispatch_seam_inner(kind, req, seams, ctx, Some(deadline), Some(cancellation)).await
}

async fn dispatch_seam_inner(
    kind: SeamKind,
    req: &Value,
    seams: &SeamsConfig,
    ctx: &SeamRunCtx,
    deadline: Option<ProviderPhaseDeadline>,
    cancellation: Option<&CancellationToken>,
) -> SeamPhaseOutcome<SeamOutcome> {
    let Some(command) = seams.command_for(kind) else {
        return SeamPhaseOutcome::Completed(SeamOutcome::Unconfigured);
    };
    let stdin = match serde_json::to_vec(req) {
        Ok(mut bytes) => {
            bytes.push(b'\n');
            bytes
        }
        Err(err) => {
            return SeamPhaseOutcome::Completed(fail_outcome(
                kind,
                format!("request serialization failed: {err}"),
            ));
        }
    };
    if cancellation.is_some_and(CancellationToken::is_cancelled) {
        return SeamPhaseOutcome::Cancelled {
            cleanup: ProviderCleanup::NotApplicable,
        };
    }
    if deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline.work_expires_at) {
        return SeamPhaseOutcome::WorkExpired {
            cleanup: ProviderCleanup::NotApplicable,
        };
    }

    let token = CancellationToken::new();
    let process_record = seam_process_record_path(ctx, kind);
    let spec = seam_sandbox_spec(command, ctx, stdin, token.clone(), process_record.clone());
    let execution = run_sandbox(spec);
    tokio::pin!(execution);

    let now = tokio::time::Instant::now();
    let configured_expires_at = now
        .checked_add(Duration::from_millis(command.timeout_ms))
        .unwrap_or_else(|| {
            deadline
                .map(|deadline| deadline.work_expires_at)
                .unwrap_or(now + Duration::from_secs(100 * 365 * 24 * 60 * 60))
        });
    let outer_wins =
        deadline.is_some_and(|deadline| deadline.work_expires_at <= configured_expires_at);
    let work_expires_at = deadline
        .map(|deadline| deadline.work_expires_at.min(configured_expires_at))
        .unwrap_or(configured_expires_at);

    enum Boundary<T> {
        Completed(T),
        SafetyExpired,
        WorkExpired,
        Cancelled,
    }

    let boundary = if let Some(cancellation) = cancellation {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Boundary::Cancelled,
            result = &mut execution => Boundary::Completed(result),
            () = tokio::time::sleep_until(work_expires_at) => {
                if outer_wins { Boundary::WorkExpired } else { Boundary::SafetyExpired }
            }
        }
    } else {
        tokio::select! {
            result = &mut execution => Boundary::Completed(result),
            () = tokio::time::sleep_until(work_expires_at) => Boundary::SafetyExpired,
        }
    };

    let output = match boundary {
        Boundary::Completed(Ok(output)) => output,
        Boundary::Completed(Err(err)) => {
            return SeamPhaseOutcome::Completed(fail_outcome(
                kind,
                format!("sandbox failed: {err}"),
            ));
        }
        Boundary::SafetyExpired | Boundary::WorkExpired | Boundary::Cancelled => {
            token.cancel();
            let cleanup_budget = deadline
                .map(|deadline| deadline.cleanup_budget.min(SEAM_CLEANUP_TIMEOUT))
                .unwrap_or(SEAM_CLEANUP_TIMEOUT);
            let cleanup_resolved = tokio::time::timeout(cleanup_budget, &mut execution)
                .await
                .is_ok();
            let cleanup = classify_seam_cleanup(&process_record, cleanup_resolved);
            return match boundary {
                Boundary::SafetyExpired => SeamPhaseOutcome::Completed(seam_timeout_outcome(
                    kind,
                    &cleanup,
                    &process_record,
                    cleanup_budget,
                )),
                Boundary::WorkExpired => SeamPhaseOutcome::WorkExpired { cleanup },
                Boundary::Cancelled => SeamPhaseOutcome::Cancelled { cleanup },
                Boundary::Completed(_) => unreachable!(),
            };
        }
    };
    if output.status_code != Some(0) {
        return SeamPhaseOutcome::Completed(fail_outcome(
            kind,
            format!(
                "command exited with status {:?}: {}",
                output.status_code,
                output.stderr.trim()
            ),
        ));
    }
    let parsed = match serde_json::from_str::<Value>(output.stdout.trim()) {
        Ok(parsed) => parsed,
        Err(err) => {
            return SeamPhaseOutcome::Completed(fail_outcome(
                kind,
                format!("invalid JSON response: {err}"),
            ));
        }
    };
    SeamPhaseOutcome::Completed(map_success(kind, parsed))
}

pub async fn resolve_catalog_override(
    seams: &SeamsConfig,
    ctx: &SeamRunCtx,
) -> Result<Option<ModelCatalogOverride>> {
    let override_ = match dispatch_seam(
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
        SeamOutcome::LostContainment(reason) => {
            return Err(lost_containment_error(SeamKind::Catalog, &reason));
        }
    };
    Ok(override_)
}

/// Resolve a catalog seam under the caller's fixed Job boundary. The result
/// keeps expiry/cancellation typed so a pre-turn catalog hook cannot silently
/// reset or outlive the durable Run cutoff.
pub async fn resolve_catalog_override_phase(
    seams: &SeamsConfig,
    ctx: &SeamRunCtx,
    deadline: ProviderPhaseDeadline,
    cancellation: &CancellationToken,
) -> Result<SeamPhaseOutcome<Option<ModelCatalogOverride>>> {
    let outcome = dispatch_seam_phase(
        SeamKind::Catalog,
        &Value::Object(Default::default()),
        seams,
        ctx,
        deadline,
        cancellation,
    )
    .await;
    Ok(match outcome {
        SeamPhaseOutcome::Completed(outcome) => {
            let override_ = match outcome {
                SeamOutcome::Ok(value) => ModelCatalogOverride::from_value(value).ok(),
                SeamOutcome::Unconfigured
                | SeamOutcome::Fallback
                | SeamOutcome::Skipped(_)
                | SeamOutcome::Deny(_) => None,
                SeamOutcome::LostContainment(reason) => {
                    return Err(lost_containment_error(SeamKind::Catalog, &reason));
                }
            };
            SeamPhaseOutcome::Completed(override_)
        }
        SeamPhaseOutcome::WorkExpired { cleanup } => SeamPhaseOutcome::WorkExpired { cleanup },
        SeamPhaseOutcome::Cancelled { cleanup } => SeamPhaseOutcome::Cancelled { cleanup },
    })
}

fn seam_sandbox_spec(
    command: &SeamCommandConfig,
    ctx: &SeamRunCtx,
    stdin: Vec<u8>,
    cancellation_token: CancellationToken,
    process_record: PathBuf,
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
        // The record makes a timeout observable and restart-recoverable. The
        // sandbox runner removes it only after the exact process group and
        // residual descendants have been reconciled.
        pid_file: Some(process_record),
        cancellation_token: Some(cancellation_token),
        profile_dir: Some(ctx.run_root.join("sandbox").join("seams")),
        read_allowlist: policy.read_allowlist,
        write_allowlist: policy.write_allowlist,
        read_denylist: denied.clone(),
        write_denylist: denied,
        network_allowlist: Vec::new(),
        workspace_access: WorkspaceAccess::ReadWrite,
        cleanup_process_group: true,
        guarded_launch: None,
    }
}

fn seam_process_record_path(ctx: &SeamRunCtx, kind: SeamKind) -> PathBuf {
    ctx.run_root.join("child-pids").join(format!(
        "seam-{}-{}.json",
        kind.config_key(),
        Uuid::new_v4().simple()
    ))
}

fn classify_seam_cleanup(process_record: &Path, execution_resolved: bool) -> ProviderCleanup {
    match std::fs::symlink_metadata(process_record) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && execution_resolved => {
            ProviderCleanup::Proven
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ProviderCleanup::RetainedAuthority {
                path: process_record.to_path_buf(),
                detail: "seam cleanup future did not resolve before its separate cleanup deadline"
                    .to_string(),
            }
        }
        Ok(_) => ProviderCleanup::RetainedAuthority {
            path: process_record.to_path_buf(),
            detail: "seam process authority remains after the phase boundary".to_string(),
        },
        Err(error) => ProviderCleanup::RetainedAuthority {
            path: process_record.to_path_buf(),
            detail: format!("seam process authority could not be inspected: {error}"),
        },
    }
}

fn seam_timeout_reason(
    cleanup_proven: bool,
    process_record: &Path,
    cleanup_budget: Duration,
) -> String {
    if cleanup_proven {
        return "timeout; process-group cleanup proven".to_string();
    }
    if process_record.exists() {
        return format!(
            "timeout; process-group cleanup was not proven within {:.1}s; process authority retained at {}",
            cleanup_budget.as_secs_f64(),
            process_record.display()
        );
    }
    format!(
        "timeout; process-group cleanup was not proven within {:.1}s and no durable process record was available",
        cleanup_budget.as_secs_f64()
    )
}

fn seam_timeout_outcome(
    kind: SeamKind,
    cleanup: &ProviderCleanup,
    process_record: &Path,
    cleanup_budget: Duration,
) -> SeamOutcome {
    let cleanup_proven = matches!(
        cleanup,
        ProviderCleanup::Proven | ProviderCleanup::NotApplicable
    );
    let reason = seam_timeout_reason(cleanup_proven, process_record, cleanup_budget);
    if cleanup_proven {
        fail_outcome(kind, reason)
    } else {
        SeamOutcome::LostContainment(reason)
    }
}

pub fn lost_containment_error(kind: SeamKind, reason: &str) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!(
        "seam '{}' lost process containment: {reason}",
        kind.config_key()
    ))
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
    async fn job_cutoff_preempts_longer_configured_seam_timeout() {
        let temp = TempDir::new().expect("temp");
        let seams = SeamsConfig::with_command(
            SeamKind::Hooks,
            SeamCommandConfig {
                command: vec!["sh".to_string(), "-c".to_string(), "sleep 60".to_string()],
                timeout_ms: 30_000,
            },
        )
        .expect("hooks");
        let cancellation = CancellationToken::new();
        let deadline =
            ProviderPhaseDeadline::from_now(Duration::from_millis(100), Duration::from_secs(2));
        let started = std::time::Instant::now();

        let outcome = dispatch_seam_phase(
            SeamKind::Hooks,
            &json!({"kind":"deadline"}),
            &seams,
            &ctx(&temp),
            deadline,
            &cancellation,
        )
        .await;

        assert!(matches!(
            outcome,
            SeamPhaseOutcome::WorkExpired {
                cleanup: ProviderCleanup::Proven
            }
        ));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "configured timeout extended the Job cutoff: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn repeated_seams_share_one_absolute_job_cutoff() {
        let temp = TempDir::new().expect("temp");
        let seams = SeamsConfig::with_command(
            SeamKind::Hooks,
            SeamCommandConfig {
                command: vec!["sh".to_string(), "-c".to_string(), "sleep 60".to_string()],
                timeout_ms: 30_000,
            },
        )
        .expect("hooks");
        let cancellation = CancellationToken::new();
        let deadline =
            ProviderPhaseDeadline::from_now(Duration::from_millis(100), Duration::from_secs(2));

        let first = dispatch_seam_phase(
            SeamKind::Hooks,
            &json!({"kind":"first"}),
            &seams,
            &ctx(&temp),
            deadline,
            &cancellation,
        )
        .await;
        assert!(matches!(first, SeamPhaseOutcome::WorkExpired { .. }));

        let second_started = std::time::Instant::now();
        let second = dispatch_seam_phase(
            SeamKind::Hooks,
            &json!({"kind":"second"}),
            &seams,
            &ctx(&temp),
            deadline,
            &cancellation,
        )
        .await;
        assert_eq!(
            second,
            SeamPhaseOutcome::WorkExpired {
                cleanup: ProviderCleanup::NotApplicable
            }
        );
        assert!(
            second_started.elapsed() < Duration::from_millis(100),
            "the second seam received a fresh timeout: {:?}",
            second_started.elapsed()
        );
    }

    #[tokio::test]
    async fn controller_cancellation_preempts_seam_and_proves_cleanup() {
        let temp = TempDir::new().expect("temp");
        let seams = SeamsConfig::with_command(
            SeamKind::Hooks,
            SeamCommandConfig {
                command: vec!["sh".to_string(), "-c".to_string(), "sleep 60".to_string()],
                timeout_ms: 30_000,
            },
        )
        .expect("hooks");
        let cancellation = CancellationToken::new();
        let cancellation_for_task = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            cancellation_for_task.cancel();
        });

        let outcome = dispatch_seam_phase(
            SeamKind::Hooks,
            &json!({"kind":"cancel"}),
            &seams,
            &ctx(&temp),
            ProviderPhaseDeadline::from_now(Duration::from_secs(30), Duration::from_secs(2)),
            &cancellation,
        )
        .await;

        assert!(matches!(
            outcome,
            SeamPhaseOutcome::Cancelled {
                cleanup: ProviderCleanup::Proven
            }
        ));
    }

    #[test]
    fn seam_workers_use_recoverable_process_group_authority() {
        let temp = TempDir::new().expect("temp");
        let run_ctx = ctx(&temp);
        let process_record = run_ctx.run_root.join("child-pids/seam-test.json");
        let command = SeamCommandConfig {
            command: vec!["worker".to_string()],
            timeout_ms: 100,
        };

        let spec = seam_sandbox_spec(
            &command,
            &run_ctx,
            b"{}\n".to_vec(),
            CancellationToken::new(),
            process_record.clone(),
        );

        assert!(spec.cleanup_process_group);
        assert_eq!(spec.pid_file.as_deref(), Some(process_record.as_path()));
        assert!(spec.cancellation_token.is_some());
    }

    #[test]
    fn unproven_cleanup_never_uses_the_configured_failure_fallback() {
        let temp = TempDir::new().expect("temp");
        let process_record = temp.path().join("seam-policy-test.json");
        std::fs::write(&process_record, "retained authority").expect("process record");

        for kind in SeamKind::all() {
            let cleanup = ProviderCleanup::RetainedAuthority {
                path: process_record.clone(),
                detail: "retained by test".to_string(),
            };
            let outcome =
                seam_timeout_outcome(kind, &cleanup, &process_record, SEAM_CLEANUP_TIMEOUT);
            let SeamOutcome::LostContainment(reason) = outcome else {
                panic!(
                    "{} seam treated lost containment as an ordinary failure: {outcome:?}",
                    kind.config_key()
                );
            };
            assert!(reason.contains("cleanup was not proven"), "{reason}");
            assert!(
                reason.contains(process_record.to_string_lossy().as_ref()),
                "{reason}"
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn hook_timeout_reaps_background_descendant_that_retains_output_pipes() {
        let temp = TempDir::new().expect("temp");
        let run_ctx = ctx(&temp);
        let descendant_pid_path = temp.path().join("descendant.pid");
        let seams = SeamsConfig::with_command(
            SeamKind::Hooks,
            SeamCommandConfig {
                command: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    format!(
                        "(trap '' TERM; sleep 60) & descendant=$!; printf '%s\\n' \"$descendant\" > {}; trap '' TERM; sleep 60",
                        sh_quote(&descendant_pid_path)
                    ),
                ],
                timeout_ms: 250,
            },
        )
        .expect("hooks seam");

        let started = std::time::Instant::now();
        let outcome = dispatch_seam(
            SeamKind::Hooks,
            &json!({"kind":"adversarial_pipe_holder"}),
            &seams,
            &run_ctx,
        )
        .await;

        let SeamOutcome::Skipped(reason) = outcome else {
            panic!("timed-out hook did not fail safe: {outcome:?}");
        };
        assert!(reason.contains("timeout"), "{reason}");
        assert!(reason.contains("cleanup proven"), "{reason}");
        assert!(
            started.elapsed()
                < Duration::from_millis(250) + SEAM_CLEANUP_TIMEOUT + Duration::from_secs(1),
            "configured execution timeout plus bounded cleanup grace was exceeded: {:?}",
            started.elapsed()
        );

        let descendant_pid = std::fs::read_to_string(&descendant_pid_path)
            .expect("descendant pid")
            .trim()
            .parse::<u32>()
            .expect("numeric descendant pid");
        let exit_deadline = std::time::Instant::now() + Duration::from_secs(2);
        while deadreckon_core::pid_is_alive(descendant_pid)
            && std::time::Instant::now() < exit_deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !deadreckon_core::pid_is_alive(descendant_pid),
            "background hook descendant {descendant_pid} survived timeout cleanup"
        );

        let authority_dir = run_ctx.run_root.join("child-pids");
        let remaining = std::fs::read_dir(&authority_dir)
            .expect("process authority directory")
            .collect::<std::io::Result<Vec<_>>>()
            .expect("process authority entries");
        assert!(
            remaining.is_empty(),
            "proven cleanup retained stale process authority: {remaining:?}"
        );
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

        let catalog = resolve_catalog_override(&seams, &ctx(&temp))
            .await
            .expect("catalog seam dispatch");

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
