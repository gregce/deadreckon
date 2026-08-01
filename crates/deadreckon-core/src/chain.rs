use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use deadreckon_protocol::JobId;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::flight::sha256_text;
use crate::job_lease::{LeaseToken, with_fenced_job_control};
use crate::paths::DeadreckonPaths;
use crate::state::{append_json_line, atomic_write_json};

pub const CHAIN_JSON: &str = "chain.json";
pub const CHAIN_EVENTS_JSONL: &str = "chain-events.jsonl";
pub const CONDUCTOR_JSON: &str = "conductor.json";
pub const CHAIN_STEP_JSON: &str = ".deadreckon/chain-step.json";
pub const CHAIN_LOCK_PREFIX: &str = "chain--";
pub const DURABLE_CHAIN_HOOK_EVENTS_JSONL: &str = "chain-hook-events.jsonl";
pub const DURABLE_CHAIN_HOOK_INPUTS_DIR: &str = "approved-chain-hooks";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChainHookName {
    PreStep,
    PostStep,
    OnPromote,
    OnChainEnd,
}

impl ChainHookName {
    pub const ALL: [Self; 4] = [
        Self::PreStep,
        Self::PostStep,
        Self::OnPromote,
        Self::OnChainEnd,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreStep => "pre-step",
            Self::PostStep => "post-step",
            Self::OnPromote => "on-promote",
            Self::OnChainEnd => "on-chain-end",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainHookSource {
    Workspace,
    User,
    Installation,
}

impl ChainHookSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::User => "user",
            Self::Installation => "installation",
        }
    }
}

/// Immutable identity for one hook discovered before a durable Chain Job is
/// approved. The launch-plan digest binds these fields. A future graph
/// consumer must verify both digests again before invoking the hook.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenChainHook {
    pub name: ChainHookName,
    pub source: ChainHookSource,
    pub source_path: PathBuf,
    /// Exact bytes approved in the launch plan. Execution must materialize
    /// these bytes inside protected Job state and must never reopen
    /// `source_path`, which is provenance only after approval.
    pub approved_bytes: Vec<u8>,
    pub content_sha256: String,
    pub identity_sha256: String,
}

impl FrozenChainHook {
    pub fn freeze(name: ChainHookName, source: ChainHookSource, path: &Path) -> Result<Self> {
        let source_path = fs::canonicalize(path).with_path(path)?;
        if !source_path.is_file() {
            return Err(DeadreckonError::InvalidInput(format!(
                "chain hook is not a regular file: {}",
                source_path.display()
            )));
        }
        let approved_bytes = fs::read(&source_path).with_path(&source_path)?;
        let content_sha256 = sha256_bytes(&approved_bytes);
        let identity_sha256 =
            chain_hook_identity_sha256(name, source, &source_path, &content_sha256);
        Ok(Self {
            name,
            source,
            source_path,
            approved_bytes,
            content_sha256,
            identity_sha256,
        })
    }

    /// Recheck the signed, embedded identity and bytes before execution.
    /// The original path is intentionally not opened: it may be changed or
    /// deleted after approval without changing the approved program.
    pub fn verify(&self) -> Result<()> {
        let expected_identity = chain_hook_identity_sha256(
            self.name,
            self.source,
            &self.source_path,
            &self.content_sha256,
        );
        if expected_identity != self.identity_sha256 {
            return Err(DeadreckonError::InvalidInput(format!(
                "chain hook {} identity changed after approval",
                self.name.as_str()
            )));
        }
        let actual_content = sha256_bytes(&self.approved_bytes);
        if actual_content != self.content_sha256 {
            return Err(DeadreckonError::InvalidInput(format!(
                "chain hook {} approved bytes do not match their content digest",
                self.name.as_str()
            )));
        }
        Ok(())
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("sha256:{:x}", hasher.finalize())
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonicalize_json).collect()),
        Value::Object(values) => {
            let sorted = values
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize_json(value)))
                .collect::<std::collections::BTreeMap<_, _>>();
            Value::Object(sorted.into_iter().collect())
        }
        scalar => scalar.clone(),
    }
}

pub fn canonical_chain_hook_payload(payload: &Value) -> Result<(Vec<u8>, String)> {
    let canonical = canonicalize_json(payload);
    let bytes = serde_json::to_vec(&canonical)
        .with_json_path(PathBuf::from("<durable-chain-hook-payload>"))?;
    let digest = sha256_bytes(&bytes);
    Ok((bytes, digest))
}

pub fn durable_chain_hook_invocation_id(
    job_id: &str,
    source_chain_id: &str,
    hook: &FrozenChainHook,
    step_index: Option<u32>,
    attempt: u32,
    payload_sha256: &str,
) -> String {
    sha256_text(&format!(
        "durable-chain-hook-v2\0{job_id}\0{source_chain_id}\0{}\0{}\0{attempt}\0{}\0{payload_sha256}",
        hook.name.as_str(),
        step_index.map_or_else(|| "end".to_string(), |index| index.to_string()),
        hook.identity_sha256,
    ))
}

fn chain_hook_identity_sha256(
    name: ChainHookName,
    source: ChainHookSource,
    source_path: &Path,
    content_sha256: &str,
) -> String {
    let identity = format!(
        "{}\0{}\0{}\0{}",
        name.as_str(),
        source.as_str(),
        source_path.display(),
        content_sha256
    );
    sha256_text(&identity)
}

fn approved_chain_hook_path(
    paths: &DeadreckonPaths,
    job_id: &JobId,
    hook: &FrozenChainHook,
) -> Result<PathBuf> {
    let digest = hook
        .identity_sha256
        .strip_prefix("sha256:")
        .filter(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or_else(|| {
            DeadreckonError::InvalidInput(format!(
                "chain hook {} has an invalid identity digest",
                hook.name.as_str()
            ))
        })?;
    Ok(paths
        .job_dir(job_id.as_ref())
        .join(DURABLE_CHAIN_HOOK_INPUTS_DIR)
        .join(digest)
        .join("hook"))
}

fn ensure_real_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(DeadreckonError::InvalidInput(format!(
                "protected chain hook input directory is not a real directory: {}",
                path.display()
            )));
        }
        Ok(_) => return Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            ensure_real_directory(path)
        }
        Err(source) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Materialize the exact launch-bound hook bytes inside protected Job state.
/// The path is derived from the immutable identity. Existing bytes must match
/// exactly, so recovery is idempotent and tampering fails closed.
pub fn materialize_fenced_approved_chain_hook(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    hook: &FrozenChainHook,
    now: DateTime<Utc>,
) -> Result<PathBuf> {
    hook.verify()?;
    let path = approved_chain_hook_path(paths, &token.job_id, hook)?;
    with_fenced_job_control(paths, token, now, || {
        let parent = path.parent().ok_or_else(|| {
            DeadreckonError::InvalidInput(format!(
                "protected chain hook input path has no parent: {}",
                path.display()
            ))
        })?;
        let inputs_dir = parent.parent().ok_or_else(|| {
            DeadreckonError::InvalidInput(format!(
                "protected chain hook input path has no inputs directory: {}",
                path.display()
            ))
        })?;
        let job_dir = inputs_dir.parent().ok_or_else(|| {
            DeadreckonError::InvalidInput(format!(
                "protected chain hook input path has no Job directory: {}",
                path.display()
            ))
        })?;
        // Validate only DeadReckon-owned path components. Walking to the
        // filesystem root would incorrectly reject a legitimate home reached
        // through a platform symlink such as macOS `/var`.
        ensure_real_directory(job_dir)?;
        ensure_real_directory(inputs_dir)?;
        ensure_real_directory(parent)?;
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err(DeadreckonError::InvalidInput(format!(
                        "protected chain hook input is not a regular file: {}",
                        path.display()
                    )));
                }
                if fs::read(&path).with_path(&path)? != hook.approved_bytes {
                    return Err(DeadreckonError::InvalidInput(format!(
                        "protected chain hook input changed at {}",
                        path.display()
                    )));
                }
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .with_path(&path)?;
                use std::io::Write as _;
                file.write_all(&hook.approved_bytes).with_path(&path)?;
                file.sync_all().with_path(&path)?;
                fs::File::open(parent)
                    .with_path(parent)?
                    .sync_all()
                    .with_path(parent)?;
            }
            Err(source) => {
                return Err(DeadreckonError::Io {
                    path: path.clone(),
                    source,
                });
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o500)).with_path(&path)?;
        }
        Ok(path.clone())
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableChainUndoKind {
    /// Undo the exact revision recorded by a successful Job `result_applied`
    /// delivery event. The ordinary `undo <job-id>` dispatcher is the future
    /// consumer; snapshots are not an adequate substitute for delivered code.
    RevertAppliedDelivery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableChainUndoPolicy {
    pub kind: DurableChainUndoKind,
    pub source_base_sha: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableChainAdapterManifest {
    pub schema_version: u32,
    pub source_chain_id: String,
    pub source_base_sha: String,
    pub branch_policy: BranchPolicy,
    pub apply_mode: ApplyMode,
    pub apply_strategy: ApplyStrategy,
    pub on_fail: OnFail,
    pub hooks: Vec<FrozenChainHook>,
    pub undo: DurableChainUndoPolicy,
}

impl DurableChainAdapterManifest {
    pub fn new(chain: &Chain, mut hooks: Vec<FrozenChainHook>) -> Self {
        hooks.sort_by_key(|hook| hook.name);
        Self {
            schema_version: 1,
            source_chain_id: chain.chain_id.clone(),
            source_base_sha: chain.base_sha.clone(),
            branch_policy: chain.branch_policy,
            apply_mode: chain.apply_mode,
            apply_strategy: chain.apply_strategy,
            on_fail: chain.on_fail,
            hooks,
            undo: DurableChainUndoPolicy {
                kind: DurableChainUndoKind::RevertAppliedDelivery,
                source_base_sha: chain.base_sha.clone(),
            },
        }
    }

    /// Revalidate the complete compatibility contract at the execution
    /// boundary. The launch-plan digest freezes these bytes; this check also
    /// rejects internally inconsistent or duplicate hook identities before a
    /// worker is allowed to invoke any external policy.
    pub fn verify(&self) -> Result<()> {
        if self.schema_version != 1
            || self.source_chain_id.trim().is_empty()
            || self.source_base_sha.trim().is_empty()
            || self.undo.source_base_sha != self.source_base_sha
            || self.undo.kind != DurableChainUndoKind::RevertAppliedDelivery
        {
            return Err(DeadreckonError::InvalidInput(
                "durable chain adapter manifest is malformed or internally inconsistent"
                    .to_string(),
            ));
        }
        let mut names = std::collections::BTreeSet::new();
        for hook in &self.hooks {
            if !names.insert(hook.name) {
                return Err(DeadreckonError::InvalidInput(format!(
                    "durable chain adapter repeats hook {}",
                    hook.name.as_str()
                )));
            }
            hook.verify()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurableChainHookEventKind {
    Started,
    Completed,
}

/// Fenced evidence for one immutable hook invocation. A started event without
/// a completed event is deliberately visible after recovery: hooks can have
/// external effects, so a new lease must block rather than replay blindly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableChainHookEvent {
    pub schema_version: u32,
    pub job_id: JobId,
    pub source_chain_id: String,
    pub invocation_id: String,
    pub hook: FrozenChainHook,
    pub step_index: Option<u32>,
    pub attempt: u32,
    /// Canonical JSON bytes written to hook stdin (before the terminating
    /// newline) and their digest. These fields are part of invocation identity
    /// so recovery cannot reuse an outcome for a different payload.
    pub payload_bytes: Vec<u8>,
    pub payload_sha256: String,
    pub kind: DurableChainHookEventKind,
    pub recorded_at: DateTime<Utc>,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl DurableChainHookEvent {
    pub fn started(
        job_id: JobId,
        source_chain_id: impl Into<String>,
        hook: FrozenChainHook,
        step_index: Option<u32>,
        attempt: u32,
        recorded_at: DateTime<Utc>,
        payload: &Value,
    ) -> Result<Self> {
        if attempt == 0 {
            return Err(DeadreckonError::InvalidInput(
                "durable chain hook attempt starts at one".to_string(),
            ));
        }
        let source_chain_id = source_chain_id.into();
        let (payload_bytes, payload_sha256) = canonical_chain_hook_payload(payload)?;
        let invocation_id = durable_chain_hook_invocation_id(
            job_id.as_ref(),
            &source_chain_id,
            &hook,
            step_index,
            attempt,
            &payload_sha256,
        );
        Ok(Self {
            schema_version: 1,
            job_id,
            source_chain_id,
            invocation_id,
            hook,
            step_index,
            attempt,
            payload_bytes,
            payload_sha256,
            kind: DurableChainHookEventKind::Started,
            recorded_at,
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    pub fn completed(
        started: &Self,
        recorded_at: DateTime<Utc>,
        exit_code: i32,
        stdout: impl Into<String>,
        stderr: impl Into<String>,
    ) -> Result<Self> {
        if started.kind != DurableChainHookEventKind::Started {
            return Err(DeadreckonError::InvalidInput(
                "completed chain hook event requires its started event".to_string(),
            ));
        }
        Ok(Self {
            kind: DurableChainHookEventKind::Completed,
            recorded_at,
            exit_code: Some(exit_code),
            stdout: stdout.into(),
            stderr: stderr.into(),
            ..started.clone()
        })
    }

    pub fn verify(&self) -> Result<()> {
        self.hook.verify()?;
        let payload: Value = serde_json::from_slice(&self.payload_bytes)
            .with_json_path(PathBuf::from("<durable-chain-hook-event-payload>"))?;
        let (canonical_bytes, payload_sha256) = canonical_chain_hook_payload(&payload)?;
        let expected_invocation = durable_chain_hook_invocation_id(
            self.job_id.as_ref(),
            &self.source_chain_id,
            &self.hook,
            self.step_index,
            self.attempt,
            &payload_sha256,
        );
        let shape_is_valid = match self.kind {
            DurableChainHookEventKind::Started => {
                self.exit_code.is_none() && self.stdout.is_empty() && self.stderr.is_empty()
            }
            DurableChainHookEventKind::Completed => self.exit_code.is_some(),
        };
        if self.schema_version != 1
            || self.attempt == 0
            || self.source_chain_id.trim().is_empty()
            || canonical_bytes != self.payload_bytes
            || payload_sha256 != self.payload_sha256
            || expected_invocation != self.invocation_id
            || !shape_is_valid
        {
            return Err(DeadreckonError::InvalidInput(format!(
                "durable chain hook invocation {} has invalid payload-bound evidence",
                self.invocation_id
            )));
        }
        Ok(())
    }
}

/// Return a completed outcome only when its canonical payload is byte-for-byte
/// identical to the requested invocation. A different payload at the same
/// hook/step/attempt coordinate is an authority mismatch, not a cache miss.
pub fn reusable_durable_chain_hook_completion(
    events: &[DurableChainHookEvent],
    requested: &DurableChainHookEvent,
) -> Result<Option<DurableChainHookEvent>> {
    if requested.kind != DurableChainHookEventKind::Started {
        return Err(DeadreckonError::InvalidInput(
            "durable chain hook completion lookup requires a started invocation".to_string(),
        ));
    }
    requested.verify()?;
    let mut completed = None;
    for event in events.iter().filter(|event| {
        event.job_id == requested.job_id
            && event.source_chain_id == requested.source_chain_id
            && event.hook.name == requested.hook.name
            && event.step_index == requested.step_index
            && event.attempt == requested.attempt
    }) {
        event.verify()?;
        if event.payload_sha256 != requested.payload_sha256
            || event.payload_bytes != requested.payload_bytes
            || event.invocation_id != requested.invocation_id
        {
            return Err(DeadreckonError::InvalidInput(format!(
                "chain hook {} attempt {} cannot reuse evidence from a different payload",
                requested.hook.name.as_str(),
                requested.attempt
            )));
        }
        if event.kind == DurableChainHookEventKind::Completed
            && completed.replace(event.clone()).is_some()
        {
            return Err(DeadreckonError::InvalidInput(format!(
                "chain hook invocation {} has duplicate completed evidence",
                requested.invocation_id
            )));
        }
    }
    Ok(completed)
}

pub fn durable_chain_hook_events_path(paths: &DeadreckonPaths, job_id: &JobId) -> PathBuf {
    paths
        .job_dir(job_id.as_ref())
        .join(DURABLE_CHAIN_HOOK_EVENTS_JSONL)
}

pub fn read_durable_chain_hook_events(
    paths: &DeadreckonPaths,
    job_id: &JobId,
) -> Result<Vec<DurableChainHookEvent>> {
    let path = durable_chain_hook_events_path(paths, job_id);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(DeadreckonError::Io { path, source }),
    };
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let event: DurableChainHookEvent = serde_json::from_str(line).with_json_path(&path)?;
            if event.job_id != *job_id {
                return Err(DeadreckonError::InvalidInput(format!(
                    "chain hook evidence in {} belongs to Job {}",
                    path.display(),
                    event.job_id
                )));
            }
            Ok(event)
        })
        .collect()
}

pub fn append_fenced_durable_chain_hook_event(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    event: &DurableChainHookEvent,
) -> Result<()> {
    if event.job_id != token.job_id {
        return Err(DeadreckonError::InvalidInput(format!(
            "chain hook event belongs to Job {}, not lease Job {}",
            event.job_id, token.job_id
        )));
    }
    event.verify()?;
    with_fenced_job_control(paths, token, event.recorded_at, || {
        let existing = read_durable_chain_hook_events(paths, &event.job_id)?;
        for prior in &existing {
            prior.verify()?;
        }
        if let Some(prior) = existing.iter().find(|prior| {
            prior.hook.name == event.hook.name
                && prior.step_index == event.step_index
                && prior.attempt == event.attempt
                && prior.kind == event.kind
        }) && (prior.payload_sha256 != event.payload_sha256
            || prior.payload_bytes != event.payload_bytes)
        {
            return Err(DeadreckonError::InvalidInput(format!(
                "chain hook {} attempt {} payload does not match existing {:?} evidence",
                event.hook.name.as_str(),
                event.attempt,
                event.kind
            )));
        }
        if event.kind == DurableChainHookEventKind::Completed
            && !existing.iter().any(|prior| {
                prior.kind == DurableChainHookEventKind::Started
                    && prior.invocation_id == event.invocation_id
                    && prior.payload_sha256 == event.payload_sha256
                    && prior.payload_bytes == event.payload_bytes
                    && prior.hook == event.hook
            })
        {
            return Err(DeadreckonError::InvalidInput(format!(
                "chain hook invocation {} cannot complete without its exact started payload",
                event.invocation_id
            )));
        }
        if let Some(prior) = existing
            .iter()
            .find(|prior| prior.invocation_id == event.invocation_id && prior.kind == event.kind)
        {
            if prior == event {
                return Ok(());
            }
            return Err(DeadreckonError::InvalidInput(format!(
                "chain hook invocation {} has conflicting {:?} evidence",
                event.invocation_id, event.kind
            )));
        }
        append_json_line(&durable_chain_hook_events_path(paths, &event.job_id), event)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
    Killed,
    Undone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainStepStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
    Applied,
    Undone,
}

// The apply/branch/failure policy vocabulary is owned by `plan`, because a
// plan is the execution unit these settings describe; a chain is one shape of
// plan. Re-exported here so `deadreckon_core::chain::BranchPolicy` and the
// crate-root re-export in lib.rs keep resolving unchanged.
pub use crate::plan::{ApplyMode, ApplyStrategy, BranchPolicy, OnFail};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainStep {
    pub index: u32,
    pub goal: String,
    pub status: ChainStepStatus,
    pub run_id: Option<String>,
    pub applied_at: Option<DateTime<Utc>>,
    pub applied_sha: Option<String>,
    pub fail_reason: Option<String>,
    pub max_spend_usd: Option<f64>,
    pub spend_usd: f64,
}

impl ChainStep {
    pub fn new(index: u32, goal: impl Into<String>) -> Self {
        Self {
            index,
            goal: goal.into(),
            status: ChainStepStatus::Pending,
            run_id: None,
            applied_at: None,
            applied_sha: None,
            fail_reason: None,
            max_spend_usd: None,
            spend_usd: 0.0,
        }
    }

    pub fn transition_to(&mut self, status: ChainStepStatus) -> Result<()> {
        if !step_transition_allowed(self.status, status) {
            return Err(DeadreckonError::InvalidInput(format!(
                "invalid chain step transition {:?} -> {:?}",
                self.status, status
            )));
        }
        self.status = status;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chain {
    pub schema_version: u32,
    pub chain_id: String,
    pub root_goal: String,
    pub steps: Vec<ChainStep>,
    pub branch_policy: BranchPolicy,
    pub apply_mode: ApplyMode,
    pub apply_strategy: ApplyStrategy,
    pub apply_allowlist: Vec<String>,
    pub on_fail: OnFail,
    pub circuit_breaker_threshold: u32,
    pub circuit_breaker_consecutive_failures: u32,
    pub max_spend_usd: Option<f64>,
    pub max_wall_seconds: Option<f64>,
    pub total_spend_usd: f64,
    pub total_wall_seconds: f64,
    pub scope: String,
    pub base_branch: String,
    pub base_sha: String,
    pub cwd: PathBuf,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub sandbox: String,
    pub status: ChainStatus,
    pub paused_reason: Option<String>,
    pub failure_reason: Option<String>,
    pub conductor_pid: Option<u32>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub deadreckon_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainNewOptions {
    pub root_goal: String,
    pub goals: Vec<String>,
    pub scope: String,
    pub base_branch: String,
    pub base_sha: String,
    pub cwd: PathBuf,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub sandbox: String,
    pub branch_policy: BranchPolicy,
    pub apply_mode: ApplyMode,
    pub apply_strategy: ApplyStrategy,
    pub apply_allowlist: Vec<String>,
    pub on_fail: OnFail,
    pub circuit_breaker_threshold: u32,
    pub max_spend_usd: Option<f64>,
    pub max_wall_seconds: Option<f64>,
    pub deadreckon_version: String,
}

impl Chain {
    pub fn new(options: ChainNewOptions) -> Result<Self> {
        validate_goal_count(options.goals.len())?;
        let now = Utc::now();
        Ok(Self {
            schema_version: 1,
            chain_id: Uuid::new_v4().simple().to_string(),
            root_goal: options.root_goal,
            steps: options
                .goals
                .into_iter()
                .enumerate()
                .map(|(index, goal)| ChainStep::new(index as u32, goal))
                .collect(),
            branch_policy: options.branch_policy,
            apply_mode: options.apply_mode,
            apply_strategy: options.apply_strategy,
            apply_allowlist: options.apply_allowlist,
            on_fail: options.on_fail,
            circuit_breaker_threshold: options.circuit_breaker_threshold,
            circuit_breaker_consecutive_failures: 0,
            max_spend_usd: options.max_spend_usd,
            max_wall_seconds: options.max_wall_seconds,
            total_spend_usd: 0.0,
            total_wall_seconds: 0.0,
            scope: options.scope,
            base_branch: options.base_branch,
            base_sha: options.base_sha,
            cwd: options.cwd,
            provider: options.provider,
            model: options.model,
            sandbox: options.sandbox,
            status: ChainStatus::Pending,
            paused_reason: None,
            failure_reason: None,
            conductor_pid: None,
            created_at: now,
            started_at: None,
            completed_at: None,
            deadreckon_version: options.deadreckon_version,
        })
    }

    pub fn task_key(&self) -> String {
        chain_task_key(&self.chain_id)
    }

    pub fn pending_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| step.status == ChainStepStatus::Pending)
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainStepMarker {
    pub schema_version: u32,
    pub kind: String,
    pub chain_id: String,
    pub step_index: u32,
    pub chain_root_goal: String,
    pub step_goal: String,
    pub prior_applied_sha: Option<String>,
    pub created_at: DateTime<Utc>,
    pub deadreckon_version: String,
}

impl ChainStepMarker {
    pub fn new(chain: &Chain, step: &ChainStep, prior_applied_sha: Option<String>) -> Self {
        Self {
            schema_version: 1,
            kind: "chain_step".to_string(),
            chain_id: chain.chain_id.clone(),
            step_index: step.index,
            chain_root_goal: chain.root_goal.clone(),
            step_goal: step.goal.clone(),
            prior_applied_sha,
            created_at: Utc::now(),
            deadreckon_version: chain.deadreckon_version.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConductorState {
    pub schema_version: u32,
    pub chain_id: String,
    pub conductor_pid: u32,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub live_step: Option<u32>,
    #[serde(default)]
    pub live_run_id: Option<String>,
    #[serde(default)]
    pub live_child_pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChainEvent {
    pub timestamp: DateTime<Utc>,
    pub chain_id: String,
    pub event: ChainEventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_index: Option<u32>,
    #[serde(default)]
    pub detail: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainEventKind {
    ChainCreated,
    ChainStepStarted,
    ChainRunCompleted,
    ChainApplyStarted,
    ChainApplied,
    ChainApplyRefused,
    ChainStepFailed,
    ChainPaused,
    ChainResumed,
    ChainKilled,
    ChainCompleted,
    ChainUndoStarted,
    ChainUndoneStep,
    ChainHookInvoked,
    ChainStepExtended,
    ChainStepRedone,
    LegacyExecutionSelected,
}

pub fn validate_goal_count(count: usize) -> Result<()> {
    match count {
        0 | 1 => Err(DeadreckonError::InvalidInput(
            "chain must have >= 2 steps\ntry: deadreckon run \"<the only step>\"".to_string(),
        )),
        2..=12 => Ok(()),
        _ => Err(DeadreckonError::InvalidInput(format!(
            "chain capped at 12 steps; got {count}\ntry: split the input into multiple chains"
        ))),
    }
}

pub fn chain_task_key(chain_id: &str) -> String {
    format!("{CHAIN_LOCK_PREFIX}{chain_id}")
}

pub fn chain_json_path(paths: &DeadreckonPaths, chain_id: &str) -> PathBuf {
    paths.chain_json(chain_id)
}

pub fn save_chain(paths: &DeadreckonPaths, chain: &Chain) -> Result<()> {
    atomic_write_json(&chain_json_path(paths, &chain.chain_id), chain)
}

pub fn load_chain(paths: &DeadreckonPaths, chain_id: &str) -> Result<Chain> {
    let path = chain_json_path(paths, chain_id);
    let raw = fs::read(&path).with_path(&path)?;
    serde_json::from_slice(&raw).with_json_path(&path)
}

pub fn append_chain_event(
    paths: &DeadreckonPaths,
    chain_id: &str,
    event: ChainEventKind,
    step_index: Option<u32>,
    detail: serde_json::Value,
) -> Result<()> {
    append_json_line(
        &paths.chain_events(chain_id),
        &ChainEvent {
            timestamp: Utc::now(),
            chain_id: chain_id.to_string(),
            event,
            step_index,
            detail,
        },
    )
}

pub fn write_chain_step_marker(working_dir: &Path, marker: &ChainStepMarker) -> Result<PathBuf> {
    let path = working_dir.join(CHAIN_STEP_JSON);
    atomic_write_json(&path, marker)?;
    Ok(path)
}

pub fn read_chain_step_marker(working_dir: &Path) -> Result<Option<ChainStepMarker>> {
    let path = working_dir.join(CHAIN_STEP_JSON);
    match fs::read(&path) {
        Ok(raw) => serde_json::from_slice(&raw).map(Some).with_json_path(&path),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(DeadreckonError::Io { path, source }),
    }
}

fn step_transition_allowed(from: ChainStepStatus, to: ChainStepStatus) -> bool {
    use ChainStepStatus::{Applied, Completed, Failed, Pending, Running, Skipped, Undone};
    matches!(
        (from, to),
        (Pending, Running)
            | (Pending, Skipped)
            | (Pending, Failed)
            | (Running, Completed)
            | (Running, Failed)
            | (Running, Skipped)
            | (Completed, Applied)
            | (Completed, Failed)
            | (Completed, Pending)
            | (Failed, Pending)
            | (Applied, Pending)
            | (Applied, Undone)
            | (Skipped, Pending)
            | (Undone, Pending)
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use chrono::{DateTime, TimeDelta, Utc};
    use deadreckon_protocol::JobId;
    use tempfile::TempDir;

    use super::{
        ApplyMode, ApplyStrategy, BranchPolicy, CHAIN_EVENTS_JSONL, CHAIN_JSON, CHAIN_LOCK_PREFIX,
        Chain, ChainEventKind, ChainHookName, ChainHookSource, ChainNewOptions, ChainStepStatus,
        DurableChainAdapterManifest, DurableChainHookEvent, DurableChainUndoKind, FrozenChainHook,
        OnFail, append_chain_event, append_fenced_durable_chain_hook_event, chain_task_key,
        load_chain, read_durable_chain_hook_events, reusable_durable_chain_hook_completion,
        save_chain,
    };
    use crate::job_lease::{LeaseOwner, claim_job_lease};
    use crate::lock::lock_path;
    use crate::paths::DeadreckonPaths;

    fn sample_chain(temp: &TempDir) -> Chain {
        Chain::new(ChainNewOptions {
            root_goal: "manual: 2 steps".to_string(),
            goals: vec!["one".to_string(), "two".to_string()],
            scope: "scope-a".to_string(),
            base_branch: "main".to_string(),
            base_sha: "abc123".to_string(),
            cwd: temp.path().join("repo"),
            provider: Some("mock".to_string()),
            model: Some("test-model".to_string()),
            sandbox: "none".to_string(),
            branch_policy: BranchPolicy::Stack,
            apply_mode: ApplyMode::Auto,
            apply_strategy: ApplyStrategy::Squash,
            apply_allowlist: Vec::new(),
            on_fail: OnFail::Stop,
            circuit_breaker_threshold: 2,
            max_spend_usd: Some(10.0),
            max_wall_seconds: Some(3600.0),
            deadreckon_version: "0.1.0".to_string(),
        })
        .expect("chain")
    }

    #[test]
    fn chain_json_serializes_roundtrip() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let chain = sample_chain(&temp);

        save_chain(&paths, &chain).expect("save");
        let loaded = load_chain(&paths, &chain.chain_id).expect("load");

        assert_eq!(loaded, chain);
        assert!(paths.chain_json(&chain.chain_id).ends_with(CHAIN_JSON));
    }

    #[test]
    fn chain_step_status_transitions_pending_running_completed() {
        let temp = TempDir::new().expect("tempdir");
        let mut chain = sample_chain(&temp);
        let step = &mut chain.steps[0];

        step.transition_to(ChainStepStatus::Running)
            .expect("running");
        step.transition_to(ChainStepStatus::Completed)
            .expect("completed");
        step.transition_to(ChainStepStatus::Applied)
            .expect("applied");
        let err = step
            .transition_to(ChainStepStatus::Skipped)
            .expect_err("cannot skip applied");
        assert!(err.to_string().contains("invalid chain step transition"));
    }

    #[test]
    fn chain_paths_match_locks_pattern() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let chain = sample_chain(&temp);

        assert_eq!(
            paths.chain_dir(&chain.chain_id),
            paths.chains_dir().join(&chain.chain_id)
        );
        assert_eq!(
            paths.chain_json(&chain.chain_id),
            paths.chain_dir(&chain.chain_id).join(CHAIN_JSON)
        );
        assert_eq!(
            paths.chain_events(&chain.chain_id),
            paths.chain_dir(&chain.chain_id).join(CHAIN_EVENTS_JSONL)
        );
        assert_eq!(
            lock_path(&paths, &chain.scope, &chain.task_key()),
            paths
                .locks_dir()
                .join(format!("{}--{}.lock", chain.scope, chain.task_key()))
        );
    }

    #[test]
    fn chain_lock_task_key_prefix_chain_double_dash() {
        let task_key = chain_task_key("abc123");

        assert_eq!(task_key, "chain--abc123");
        assert!(task_key.starts_with(CHAIN_LOCK_PREFIX));
    }

    #[test]
    fn append_chain_event_writes_jsonl() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let chain = sample_chain(&temp);
        save_chain(&paths, &chain).expect("save");

        append_chain_event(
            &paths,
            &chain.chain_id,
            ChainEventKind::ChainCreated,
            None,
            serde_json::json!({ "steps": chain.steps.len() }),
        )
        .expect("event");

        let raw = fs::read_to_string(paths.chain_events(&chain.chain_id)).expect("events");
        assert!(raw.contains(r#""event":"chain_created""#));
        assert!(raw.contains(r#""steps":2"#));
    }

    #[test]
    fn approved_hook_bytes_survive_original_mutation_and_deletion() {
        let temp = TempDir::new().expect("tempdir");
        let hook = temp.path().join("pre-step");
        fs::write(&hook, "#!/bin/sh\nexit 0\n").expect("hook");
        let frozen =
            FrozenChainHook::freeze(ChainHookName::PreStep, ChainHookSource::Workspace, &hook)
                .expect("freeze hook");

        frozen.verify().expect("unchanged hook");
        let approved = frozen.approved_bytes.clone();
        assert!(frozen.content_sha256.starts_with("sha256:"));
        assert!(frozen.identity_sha256.starts_with("sha256:"));

        fs::write(&hook, "#!/bin/sh\nexit 2\n").expect("mutate hook");
        frozen
            .verify()
            .expect("mutable original is not execution authority");
        fs::remove_file(&hook).expect("delete original hook");
        frozen
            .verify()
            .expect("deleted original is not execution authority");
        assert_eq!(frozen.approved_bytes, approved);
    }

    #[test]
    fn durable_chain_manifest_pins_hook_and_delivery_undo_policy() {
        let temp = TempDir::new().expect("tempdir");
        let chain = sample_chain(&temp);
        let hook_path = temp.path().join("post-step");
        fs::write(&hook_path, "#!/bin/sh\nexit 0\n").expect("hook");
        let hook = FrozenChainHook::freeze(
            ChainHookName::PostStep,
            ChainHookSource::Workspace,
            &hook_path,
        )
        .expect("freeze hook");

        let manifest = DurableChainAdapterManifest::new(&chain, vec![hook.clone()]);
        let roundtrip: DurableChainAdapterManifest =
            serde_json::from_value(serde_json::to_value(&manifest).expect("manifest JSON"))
                .expect("manifest roundtrip");

        assert_eq!(roundtrip, manifest);
        assert_eq!(manifest.hooks, vec![hook]);
        assert_eq!(manifest.source_base_sha, chain.base_sha);
        assert_eq!(
            manifest.undo.kind,
            DurableChainUndoKind::RevertAppliedDelivery
        );
        assert_eq!(manifest.undo.source_base_sha, chain.base_sha);
    }

    #[test]
    fn durable_chain_hook_outcomes_are_fenced_idempotent_and_recoverable() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let job_id = JobId("durable-chain-hook-job".to_string());
        let at = |raw: &str| -> DateTime<Utc> {
            DateTime::parse_from_rfc3339(raw)
                .expect("timestamp")
                .with_timezone(&Utc)
        };
        let now = at("2026-07-31T16:00:00Z");
        let owner = LeaseOwner {
            owner_id: "chain-hook-supervisor".to_string(),
            boot_id: "boot-a".to_string(),
            pid: 4100,
            process_group: 4100,
        };
        let token = claim_job_lease(&paths, &job_id, &owner, now, Duration::from_secs(60))
            .expect("claim")
            .token();
        let hook_path = temp.path().join("pre-step");
        fs::write(&hook_path, "#!/bin/sh\nexit 0\n").expect("hook");
        let hook = FrozenChainHook::freeze(
            ChainHookName::PreStep,
            ChainHookSource::Workspace,
            &hook_path,
        )
        .expect("freeze hook");
        let started = DurableChainHookEvent::started(
            job_id.clone(),
            "source-chain",
            hook,
            Some(0),
            1,
            now + TimeDelta::seconds(1),
            &serde_json::json!({"z": 1, "a": 2}),
        )
        .expect("started event");
        let completed = DurableChainHookEvent::completed(
            &started,
            now + TimeDelta::seconds(2),
            0,
            "allowed\n",
            "",
        )
        .expect("completed event");

        append_fenced_durable_chain_hook_event(&paths, &token, &started).expect("started");
        append_fenced_durable_chain_hook_event(&paths, &token, &started)
            .expect("same started event is idempotent");
        append_fenced_durable_chain_hook_event(&paths, &token, &completed).expect("completed");

        let events = read_durable_chain_hook_events(&paths, &job_id).expect("events");
        assert_eq!(events, vec![started.clone(), completed.clone()]);

        let reordered = DurableChainHookEvent::started(
            job_id.clone(),
            "source-chain",
            started.hook.clone(),
            Some(0),
            1,
            now + TimeDelta::seconds(3),
            &serde_json::json!({"a": 2, "z": 1}),
        )
        .expect("canonical reordered payload");
        assert_eq!(reordered.payload_bytes, started.payload_bytes);
        assert_eq!(reordered.payload_sha256, started.payload_sha256);
        assert_eq!(reordered.invocation_id, started.invocation_id);
        assert_eq!(
            reusable_durable_chain_hook_completion(&events, &reordered)
                .expect("exact payload reuse")
                .and_then(|event| event.exit_code),
            Some(0)
        );

        let mismatched = DurableChainHookEvent::started(
            job_id.clone(),
            "source-chain",
            started.hook.clone(),
            Some(0),
            1,
            now + TimeDelta::seconds(3),
            &serde_json::json!({"a": 3, "z": 1}),
        )
        .expect("different payload");
        let error = reusable_durable_chain_hook_completion(&events, &mismatched)
            .expect_err("completed outcome cannot cross payloads");
        assert!(error.to_string().contains("different payload"));
        let error = append_fenced_durable_chain_hook_event(&paths, &token, &mismatched)
            .expect_err("same logical attempt cannot start with another payload");
        assert!(error.to_string().contains("payload does not match"));

        let mut conflicting = completed;
        conflicting.exit_code = Some(2);
        let error = append_fenced_durable_chain_hook_event(&paths, &token, &conflicting)
            .expect_err("conflicting outcome");
        assert!(error.to_string().contains("conflicting Completed evidence"));

        let replacement = LeaseOwner {
            owner_id: "replacement-supervisor".to_string(),
            boot_id: "boot-a".to_string(),
            pid: 4200,
            process_group: 4200,
        };
        claim_job_lease(
            &paths,
            &job_id,
            &replacement,
            now + TimeDelta::seconds(61),
            Duration::from_secs(60),
        )
        .expect("reclaim");
        let stale = DurableChainHookEvent::started(
            job_id,
            "source-chain",
            conflicting.hook,
            Some(0),
            1,
            now + TimeDelta::seconds(62),
            &serde_json::json!({"status": "done"}),
        )
        .expect("stale event");
        let error = append_fenced_durable_chain_hook_event(&paths, &token, &stale)
            .expect_err("stale hook writer");
        assert!(error.to_string().contains("stale lease token"));
    }
}
