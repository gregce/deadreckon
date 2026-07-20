use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use deadreckon_protocol::{FlightEvent, LedgerItem};
#[cfg(test)]
use deadreckon_protocol::{FlightEventKind, FlightUsage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use walkdir::WalkDir;

use crate::artifacts::copy_tree;
use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::ledger_io::append_ledger_item;
use crate::state::{PipelineState, append_json_line, atomic_write_json};

pub const FLIGHT_MANIFEST_JSON: &str = "flight-manifest.json";
pub const FLIGHT_EVENTS_JSONL: &str = "flight-events.jsonl";
pub const REWIND_EVENTS_JSONL: &str = "rewind-events.jsonl";
pub const CHECKPOINTS_DIR: &str = "checkpoints";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlightManifest {
    pub version: u32,
    pub run_id: String,
    #[serde(default)]
    pub sessions: Vec<FlightSession>,
    pub checkpoint_policy: CheckpointPolicy,
}

impl FlightManifest {
    pub fn new(run_id: impl Into<String>) -> Self {
        Self {
            version: 1,
            run_id: run_id.into(),
            sessions: Vec::new(),
            checkpoint_policy: CheckpointPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlightSession {
    pub flight_session_id: String,
    pub provider: String,
    pub schema: String,
    pub deadreckon_turn: u32,
    pub attempt: u32,
    pub status: FlightSessionStatus,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub source_paths: Vec<FlightSourcePath>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlightSessionStatus {
    Running,
    Completed,
    Failed,
    Killed,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlightSourcePath {
    pub path: PathBuf,
    pub first_line: u64,
    pub last_line: u64,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointPolicy {
    pub mode: String,
    pub quiet_ms: u64,
    pub poll_ms: u64,
    pub anchor_every: u32,
}

impl Default for CheckpointPolicy {
    fn default() -> Self {
        Self {
            mode: "delta-with-anchors".to_string(),
            quiet_ms: 750,
            poll_ms: 500,
            anchor_every: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointManifest {
    pub version: u32,
    pub checkpoint_id: String,
    pub run_id: String,
    pub flight_session_id: String,
    pub deadreckon_turn: u32,
    pub attempt: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_event_seq: Option<u64>,
    pub created_at: DateTime<Utc>,
    pub trigger: CheckpointTrigger,
    pub base: CheckpointBase,
    pub full_anchor: bool,
    pub files: Vec<CheckpointFileChange>,
    pub working_tree_hash: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointTrigger {
    ProviderTool,
    FileQuiet,
    ProviderExit,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointBase {
    pub kind: CheckpointBaseKind,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointBaseKind {
    TurnSnapshot,
    CheckpointAnchor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointFileChange {
    pub path: PathBuf,
    pub change: CheckpointChangeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_bytes: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointChangeKind {
    Created,
    Modified,
    Deleted,
    SkippedOversize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewindEvent {
    pub version: u32,
    pub timestamp: DateTime<Utc>,
    pub run_id: String,
    pub target: RewindTarget,
    pub mode: RewindMode,
    pub status: RewindStatus,
    #[serde(default)]
    pub files: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewindTarget {
    pub kind: RewindTargetKind,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindTargetKind {
    Turn,
    ProviderEvent,
    Checkpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindMode {
    Preview,
    Apply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindStatus {
    Ok,
    Refused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFingerprint {
    pub hash: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkingFileIndex {
    pub files: BTreeMap<PathBuf, FileFingerprint>,
}

#[derive(Debug, Clone)]
pub struct CheckpointCaptureRequest {
    pub checkpoint_id: String,
    pub flight_session_id: String,
    pub deadreckon_turn: u32,
    pub attempt: u32,
    pub provider_event_seq: Option<u64>,
    pub trigger: CheckpointTrigger,
    pub base: CheckpointBase,
    pub full_anchor: bool,
}

pub fn flight_manifest_path(state: &PipelineState) -> PathBuf {
    state.run_root.join(FLIGHT_MANIFEST_JSON)
}

pub fn flight_events_path(state: &PipelineState) -> PathBuf {
    state.run_root.join(FLIGHT_EVENTS_JSONL)
}

pub fn rewind_events_path(state: &PipelineState) -> PathBuf {
    state.run_root.join(REWIND_EVENTS_JSONL)
}

pub fn checkpoints_dir(state: &PipelineState) -> PathBuf {
    state.run_root.join(CHECKPOINTS_DIR)
}

pub fn checkpoint_dir(state: &PipelineState, checkpoint_id: &str) -> PathBuf {
    checkpoints_dir(state).join(checkpoint_id)
}

pub fn checkpoint_manifest_path(state: &PipelineState, checkpoint_id: &str) -> PathBuf {
    checkpoint_dir(state, checkpoint_id).join("manifest.json")
}

pub fn write_flight_manifest(state: &PipelineState, manifest: &FlightManifest) -> Result<()> {
    atomic_write_json(&flight_manifest_path(state), manifest)
}

pub fn read_flight_manifest(state: &PipelineState) -> Result<Option<FlightManifest>> {
    let path = flight_manifest_path(state);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).with_path(&path)?;
    serde_json::from_str(&raw)
        .map(Some)
        .map_err(|source| DeadreckonError::Json { path, source })
}

pub fn append_flight_event(state: &PipelineState, event: &FlightEvent) -> Result<()> {
    append_ledger_item(&state.run_root, LedgerItem::Flight(event.clone()))
}

pub fn read_flight_events(state: &PipelineState) -> Result<Vec<FlightEvent>> {
    read_jsonl(&flight_events_path(state))
}

pub fn append_rewind_event(state: &PipelineState, event: &RewindEvent) -> Result<()> {
    append_json_line(&rewind_events_path(state), event)
}

pub fn read_rewind_events(state: &PipelineState) -> Result<Vec<RewindEvent>> {
    read_jsonl(&rewind_events_path(state))
}

pub fn read_checkpoint_manifest(
    state: &PipelineState,
    checkpoint_id: &str,
) -> Result<CheckpointManifest> {
    let path = checkpoint_manifest_path(state, checkpoint_id);
    let raw = fs::read_to_string(&path).with_path(&path)?;
    serde_json::from_str(&raw).map_err(|source| DeadreckonError::Json { path, source })
}

pub fn list_checkpoint_manifests(state: &PipelineState) -> Result<Vec<CheckpointManifest>> {
    let root = checkpoints_dir(state);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut manifests = Vec::new();
    for entry in fs::read_dir(&root).with_path(&root)? {
        let entry = entry.with_path(&root)?;
        let path = entry.path().join("manifest.json");
        if !path.exists() {
            continue;
        }
        let raw = fs::read_to_string(&path).with_path(&path)?;
        let manifest: CheckpointManifest =
            serde_json::from_str(&raw).map_err(|source| DeadreckonError::Json {
                path: path.clone(),
                source,
            })?;
        manifests.push(manifest);
    }
    manifests.sort_by(|left, right| left.checkpoint_id.cmp(&right.checkpoint_id));
    Ok(manifests)
}

pub fn build_working_file_index(root: &Path) -> Result<WorkingFileIndex> {
    let mut files = BTreeMap::new();
    if !root.exists() {
        return Ok(WorkingFileIndex { files });
    }
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| !ignored_working_entry(entry.path(), root))
    {
        let entry = entry.map_err(|source| DeadreckonError::Io {
            path: root.to_path_buf(),
            source: source.into(),
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|err| {
            DeadreckonError::InvalidInput(format!("working path prefix error: {err}"))
        })?;
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(DeadreckonError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let hash = match sha256_file(path) {
            Ok(hash) => hash,
            Err(DeadreckonError::Io { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        files.insert(
            relative.to_path_buf(),
            FileFingerprint {
                hash,
                size: metadata.len(),
            },
        );
    }
    Ok(WorkingFileIndex { files })
}

pub fn capture_delta_checkpoint(
    state: &PipelineState,
    before: &WorkingFileIndex,
    after: &WorkingFileIndex,
    request: CheckpointCaptureRequest,
) -> Result<CheckpointManifest> {
    let checkpoints = checkpoints_dir(state);
    fs::create_dir_all(&checkpoints).with_path(&checkpoints)?;
    let final_dir = checkpoints.join(&request.checkpoint_id);
    let tmp_dir = checkpoints.join(format!(".tmp-{}", request.checkpoint_id));
    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir).with_path(&tmp_dir)?;
    }
    if final_dir.exists() {
        fs::remove_dir_all(&final_dir).with_path(&final_dir)?;
    }
    fs::create_dir_all(&tmp_dir).with_path(&tmp_dir)?;

    let mut files = Vec::new();
    for (relative, after_fingerprint) in &after.files {
        match before.files.get(relative) {
            None => {
                copy_checkpoint_file(&state.working_dir, &tmp_dir, relative)?;
                files.push(CheckpointFileChange {
                    path: relative.clone(),
                    change: CheckpointChangeKind::Created,
                    before_hash: None,
                    after_hash: Some(after_fingerprint.hash.clone()),
                    after_bytes: Some(PathBuf::from("files").join(relative)),
                });
            }
            Some(before_fingerprint) if before_fingerprint.hash != after_fingerprint.hash => {
                copy_checkpoint_file(&state.working_dir, &tmp_dir, relative)?;
                files.push(CheckpointFileChange {
                    path: relative.clone(),
                    change: CheckpointChangeKind::Modified,
                    before_hash: Some(before_fingerprint.hash.clone()),
                    after_hash: Some(after_fingerprint.hash.clone()),
                    after_bytes: Some(PathBuf::from("files").join(relative)),
                });
            }
            Some(_) => {}
        }
    }
    for (relative, before_fingerprint) in &before.files {
        if after.files.contains_key(relative) {
            continue;
        }
        files.push(CheckpointFileChange {
            path: relative.clone(),
            change: CheckpointChangeKind::Deleted,
            before_hash: Some(before_fingerprint.hash.clone()),
            after_hash: None,
            after_bytes: None,
        });
    }

    if request.full_anchor {
        copy_tree(&state.working_dir, &tmp_dir.join("anchor"))?;
    }

    let manifest = CheckpointManifest {
        version: 1,
        checkpoint_id: request.checkpoint_id.clone(),
        run_id: state.run_id.clone(),
        flight_session_id: request.flight_session_id,
        deadreckon_turn: request.deadreckon_turn,
        attempt: request.attempt,
        provider_event_seq: request.provider_event_seq,
        created_at: Utc::now(),
        trigger: request.trigger,
        base: request.base,
        full_anchor: request.full_anchor,
        files,
        working_tree_hash: after.tree_hash(),
    };
    write_json_pretty_atomic(&tmp_dir.join("manifest.json"), &manifest)?;
    fs::rename(&tmp_dir, &final_dir).with_path(&final_dir)?;
    Ok(manifest)
}

pub fn materialize_checkpoint(
    state: &PipelineState,
    checkpoint_id: &str,
    output_dir: &Path,
) -> Result<CheckpointManifest> {
    let target = read_checkpoint_manifest(state, checkpoint_id)?;
    let manifests = list_checkpoint_manifests(state)?;
    let session_manifests = manifests
        .into_iter()
        .filter(|manifest| {
            manifest.flight_session_id == target.flight_session_id
                && manifest.attempt == target.attempt
                && manifest.checkpoint_id <= target.checkpoint_id
        })
        .collect::<Vec<_>>();

    if output_dir.exists() {
        fs::remove_dir_all(output_dir).with_path(output_dir)?;
    }
    if let Some(anchor) = session_manifests
        .iter()
        .rev()
        .find(|manifest| manifest.full_anchor && manifest.checkpoint_id <= target.checkpoint_id)
    {
        let anchor_dir = checkpoint_dir(state, &anchor.checkpoint_id).join("anchor");
        copy_tree(&anchor_dir, output_dir)?;
        for manifest in session_manifests
            .iter()
            .filter(|manifest| manifest.checkpoint_id > anchor.checkpoint_id)
        {
            apply_checkpoint_delta(state, output_dir, manifest)?;
        }
        return Ok(target);
    }

    let base_dir = base_dir_for_checkpoint(state, &target.base);
    copy_tree(&base_dir, output_dir)?;
    for manifest in &session_manifests {
        apply_checkpoint_delta(state, output_dir, manifest)?;
    }
    Ok(target)
}

impl WorkingFileIndex {
    pub fn tree_hash(&self) -> String {
        let mut hasher = Sha256::new();
        for (path, fingerprint) in &self.files {
            hasher.update(path.to_string_lossy().as_bytes());
            hasher.update(b"\0");
            hasher.update(fingerprint.hash.as_bytes());
            hasher.update(b"\0");
        }
        let digest = hasher.finalize();
        format_sha256(&digest)
    }
}

pub fn sha256_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    format_sha256(&digest)
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).with_path(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file.read(&mut buffer).with_path(path)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok(format_sha256(&digest))
}

fn read_jsonl<T>(path: &Path) -> Result<Vec<T>>
where
    T: for<'de> Deserialize<'de>,
{
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path).with_path(path)?;
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|source| DeadreckonError::Json {
                path: path.to_path_buf(),
                source,
            })
        })
        .collect()
}

fn ignored_working_entry(path: &Path, root: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.components().any(|component| {
        let value = component.as_os_str().to_string_lossy();
        matches!(
            value.as_ref(),
            ".git" | ".deadreckon" | "target" | "node_modules"
        )
    })
}

fn copy_checkpoint_file(working_dir: &Path, checkpoint_tmp: &Path, relative: &Path) -> Result<()> {
    let source = working_dir.join(relative);
    let dest = checkpoint_tmp.join("files").join(relative);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_path(parent)?;
    }
    fs::copy(&source, &dest).with_path(&dest)?;
    Ok(())
}

fn apply_checkpoint_delta(
    state: &PipelineState,
    output_dir: &Path,
    manifest: &CheckpointManifest,
) -> Result<()> {
    let checkpoint = checkpoint_dir(state, &manifest.checkpoint_id);
    for change in &manifest.files {
        let target = output_dir.join(&change.path);
        match change.change {
            CheckpointChangeKind::Created | CheckpointChangeKind::Modified => {
                let after = change.after_bytes.as_ref().ok_or_else(|| {
                    DeadreckonError::InvalidInput(format!(
                        "checkpoint {} missing after_bytes for {}",
                        manifest.checkpoint_id,
                        change.path.display()
                    ))
                })?;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).with_path(parent)?;
                }
                fs::copy(checkpoint.join(after), &target).with_path(&target)?;
            }
            CheckpointChangeKind::Deleted => {
                if target.exists() {
                    fs::remove_file(&target).with_path(&target)?;
                }
            }
            CheckpointChangeKind::SkippedOversize => {}
        }
    }
    Ok(())
}

fn base_dir_for_checkpoint(state: &PipelineState, base: &CheckpointBase) -> PathBuf {
    match base.kind {
        CheckpointBaseKind::TurnSnapshot => {
            let id = base.id.strip_prefix("turn-").unwrap_or(&base.id);
            state.run_root.join("snapshots").join(format!("turn-{id}"))
        }
        CheckpointBaseKind::CheckpointAnchor => checkpoint_dir(state, &base.id).join("anchor"),
    }
}

fn write_json_pretty_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!("path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent).with_path(parent)?;
    let mut temp = NamedTempFile::new_in(parent).with_path(parent)?;
    serde_json::to_writer_pretty(&mut temp, value).with_json_path(path)?;
    temp.write_all(b"\n").with_path(path)?;
    temp.as_file_mut().sync_all().with_path(path)?;
    match temp.persist(path) {
        Ok(_) => Ok(()),
        Err(err) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source: err.error,
        }),
    }
}

fn format_sha256(bytes: &[u8]) -> String {
    let mut out = String::with_capacity("sha256:".len() + bytes.len() * 2);
    out.push_str("sha256:");
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::TempDir;

    use super::*;
    use crate::DeadreckonPaths;
    use crate::RunOptions;
    use crate::create_run;
    use crate::snapshot_working;

    fn fixture() -> (TempDir, PipelineState) {
        let temp = TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = temp.path().join("project");
        fs::create_dir_all(&cwd).expect("cwd");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "flight".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: Some("cli:codex".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("dr-flight".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        (temp, state)
    }

    #[test]
    fn flight_event_round_trips_with_provider_source_and_checkpoint() {
        let (_temp, state) = fixture();
        let event = FlightEvent {
            version: 1,
            seq: 7,
            run_id: state.run_id.clone(),
            flight_session_id: "flight-turn-1-attempt-1".to_string(),
            deadreckon_turn: 1,
            attempt: 1,
            provider: "cli:codex".to_string(),
            schema: "codex-cli".to_string(),
            timestamp: Some(Utc::now()),
            source_path: Some(PathBuf::from("/tmp/codex.jsonl")),
            source_line: Some(3),
            source_event: "response_item:function_call".to_string(),
            raw_hash: sha256_text("raw"),
            kind: FlightEventKind::Tool,
            role: Some("assistant".to_string()),
            summary: "tool write src/lib.rs".to_string(),
            tool_name: Some("apply_patch".to_string()),
            tool_category: Some("write".to_string()),
            files: vec![PathBuf::from("src/lib.rs")],
            usage: Some(FlightUsage {
                input_tokens: 10,
                output_tokens: 3,
                context_window: Some(200_000),
            }),
            checkpoint_id: Some("cp-000007".to_string()),
        };
        append_flight_event(&state, &event).expect("append");
        assert_eq!(read_flight_events(&state).expect("read"), vec![event]);
    }

    #[test]
    fn flight_manifest_tracks_multiple_sessions_and_superseded_attempts() {
        let (_temp, state) = fixture();
        let mut manifest = FlightManifest::new(&state.run_id);
        manifest.sessions.push(FlightSession {
            flight_session_id: "flight-turn-1-attempt-1".to_string(),
            provider: "cli:codex".to_string(),
            schema: "codex-cli".to_string(),
            deadreckon_turn: 1,
            attempt: 1,
            status: FlightSessionStatus::Superseded,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            source_paths: vec![FlightSourcePath {
                path: PathBuf::from("/tmp/session.jsonl"),
                first_line: 1,
                last_line: 12,
                content_hash: sha256_text("session"),
            }],
        });
        manifest.sessions.push(FlightSession {
            flight_session_id: "flight-turn-1-attempt-2".to_string(),
            provider: "cli:codex".to_string(),
            schema: "codex-cli".to_string(),
            deadreckon_turn: 1,
            attempt: 2,
            status: FlightSessionStatus::Running,
            started_at: Utc::now(),
            completed_at: None,
            source_paths: Vec::new(),
        });
        write_flight_manifest(&state, &manifest).expect("write");
        assert_eq!(read_flight_manifest(&state).expect("read"), Some(manifest));
    }

    #[test]
    fn checkpoint_delta_captures_created_modified_and_deleted_files() {
        let (_temp, state) = fixture();
        fs::write(state.working_dir.join("modified.txt"), "before").expect("write");
        fs::write(state.working_dir.join("deleted.txt"), "delete me").expect("write");
        snapshot_working(&state, 0).expect("snapshot");
        let before = build_working_file_index(&state.working_dir).expect("before");
        fs::write(state.working_dir.join("modified.txt"), "after").expect("write");
        fs::remove_file(state.working_dir.join("deleted.txt")).expect("delete");
        fs::write(state.working_dir.join("created.txt"), "new").expect("write");
        let after = build_working_file_index(&state.working_dir).expect("after");

        let manifest = capture_delta_checkpoint(
            &state,
            &before,
            &after,
            CheckpointCaptureRequest {
                checkpoint_id: "cp-000001".to_string(),
                flight_session_id: "flight-turn-1-attempt-1".to_string(),
                deadreckon_turn: 1,
                attempt: 1,
                provider_event_seq: Some(1),
                trigger: CheckpointTrigger::FileQuiet,
                base: CheckpointBase {
                    kind: CheckpointBaseKind::TurnSnapshot,
                    id: "turn-0".to_string(),
                },
                full_anchor: false,
            },
        )
        .expect("checkpoint");

        assert!(
            manifest
                .files
                .iter()
                .any(|file| file.path == Path::new("created.txt")
                    && file.change == CheckpointChangeKind::Created)
        );
        assert!(
            manifest
                .files
                .iter()
                .any(|file| file.path == Path::new("modified.txt")
                    && file.change == CheckpointChangeKind::Modified)
        );
        assert!(
            manifest
                .files
                .iter()
                .any(|file| file.path == Path::new("deleted.txt")
                    && file.change == CheckpointChangeKind::Deleted)
        );
        assert!(
            checkpoint_dir(&state, "cp-000001")
                .join("files/created.txt")
                .exists()
        );
    }

    #[test]
    fn checkpoint_materializes_from_turn_snapshot_plus_deltas() {
        let (_temp, state) = fixture();
        fs::write(state.working_dir.join("a.txt"), "one").expect("write");
        snapshot_working(&state, 0).expect("snapshot");
        let mut before = build_working_file_index(&state.working_dir).expect("before");
        fs::write(state.working_dir.join("a.txt"), "two").expect("write");
        fs::write(state.working_dir.join("b.txt"), "bee").expect("write");
        let after_one = build_working_file_index(&state.working_dir).expect("after one");
        capture_delta_checkpoint(&state, &before, &after_one, request("cp-000001", false))
            .expect("cp1");
        before = after_one;
        fs::remove_file(state.working_dir.join("b.txt")).expect("delete");
        fs::write(state.working_dir.join("c.txt"), "see").expect("write");
        let after_two = build_working_file_index(&state.working_dir).expect("after two");
        capture_delta_checkpoint(&state, &before, &after_two, request("cp-000002", false))
            .expect("cp2");

        let out = state.run_root.join("preview");
        materialize_checkpoint(&state, "cp-000002", &out).expect("materialize");
        assert_eq!(fs::read_to_string(out.join("a.txt")).expect("a"), "two");
        assert!(!out.join("b.txt").exists());
        assert_eq!(fs::read_to_string(out.join("c.txt")).expect("c"), "see");
    }

    #[test]
    fn checkpoint_anchor_materialization_skips_prior_deltas() {
        let (_temp, state) = fixture();
        fs::write(state.working_dir.join("a.txt"), "one").expect("write");
        snapshot_working(&state, 0).expect("snapshot");
        let before = build_working_file_index(&state.working_dir).expect("before");
        fs::write(state.working_dir.join("a.txt"), "anchor").expect("write");
        let after_anchor = build_working_file_index(&state.working_dir).expect("anchor");
        capture_delta_checkpoint(&state, &before, &after_anchor, request("cp-000020", true))
            .expect("cp anchor");
        fs::write(state.working_dir.join("a.txt"), "after").expect("write");
        let after = build_working_file_index(&state.working_dir).expect("after");
        capture_delta_checkpoint(&state, &after_anchor, &after, request("cp-000021", false))
            .expect("cp after");

        let out = state.run_root.join("preview-anchor");
        materialize_checkpoint(&state, "cp-000021", &out).expect("materialize");
        assert_eq!(fs::read_to_string(out.join("a.txt")).expect("a"), "after");
    }

    #[test]
    fn rewind_event_append_is_jsonl_and_append_only() {
        let (_temp, state) = fixture();
        let event = RewindEvent {
            version: 1,
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            target: RewindTarget {
                kind: RewindTargetKind::Checkpoint,
                id: "cp-000001".to_string(),
            },
            mode: RewindMode::Preview,
            status: RewindStatus::Ok,
            files: vec![PathBuf::from("a.txt")],
            reason: None,
        };
        append_rewind_event(&state, &event).expect("append one");
        append_rewind_event(&state, &event).expect("append two");
        let events = read_rewind_events(&state).expect("read");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0], event);
    }

    fn request(checkpoint_id: &str, full_anchor: bool) -> CheckpointCaptureRequest {
        CheckpointCaptureRequest {
            checkpoint_id: checkpoint_id.to_string(),
            flight_session_id: "flight-turn-1-attempt-1".to_string(),
            deadreckon_turn: 1,
            attempt: 1,
            provider_event_seq: Some(1),
            trigger: CheckpointTrigger::ProviderTool,
            base: CheckpointBase {
                kind: CheckpointBaseKind::TurnSnapshot,
                id: "turn-0".to_string(),
            },
            full_anchor,
        }
    }
}
