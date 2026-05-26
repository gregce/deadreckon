use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use chrono::Utc;
use deadreckon_core::error::{DeadreckonError, Result};
use deadreckon_core::flight::{
    CheckpointBase, CheckpointBaseKind, CheckpointCaptureRequest, CheckpointTrigger, FlightEvent,
    FlightEventKind, FlightManifest, FlightSession, FlightSessionStatus, FlightSourcePath,
    FlightUsage, WorkingFileIndex, append_flight_event, build_working_file_index,
    capture_delta_checkpoint, list_checkpoint_manifests, read_flight_events, read_flight_manifest,
    sha256_text, write_flight_manifest,
};
use deadreckon_providers::registry::{
    DescriptorKind, IngestDescriptor, IngestStorage, ProviderDescriptor, ProviderRegistry,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::error::IoContext;
use deadreckon_core::state::PipelineState;

const MAX_LOG_SCAN_DEPTH: usize = 8;
const MAX_SUMMARY_CHARS: usize = 240;

#[derive(Debug)]
pub struct ProviderFlightRecorder {
    provider: String,
    schema: String,
    flight_session_id: String,
    deadreckon_turn: u32,
    attempt: u32,
    ingest: Option<IngestDescriptor>,
    started_system_time: SystemTime,
    source_cursors: BTreeMap<PathBuf, LogCursor>,
    source_paths: BTreeMap<PathBuf, SourcePathAccumulator>,
    next_seq: u64,
    last_provider_event_seq: Option<u64>,
    last_checkpoint_index: WorkingFileIndex,
    last_observed_index: WorkingFileIndex,
    pending_checkpoint_since: Option<Instant>,
    anchor_every: u32,
    quiet_duration: Duration,
}

#[derive(Debug, Clone, Copy)]
struct LogCursor {
    line_count: u64,
}

#[derive(Debug, Clone)]
struct SourcePathAccumulator {
    first_line: u64,
    last_line: u64,
    content: String,
}

pub struct ProviderFlightRecorderHandle {
    shutdown: CancellationToken,
    handle: JoinHandle<Result<ProviderFlightRecorder>>,
}

impl ProviderFlightRecorderHandle {
    fn spawn(state: PipelineState, mut recorder: ProviderFlightRecorder) -> Self {
        let shutdown = CancellationToken::new();
        let task_shutdown = shutdown.clone();
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            loop {
                tokio::select! {
                    _ = task_shutdown.cancelled() => break,
                    _ = interval.tick() => recorder.poll(&state)?,
                }
            }
            Ok(recorder)
        });
        Self { shutdown, handle }
    }

    pub async fn finish(
        self,
        state: &PipelineState,
        status: FlightSessionStatus,
    ) -> Result<Option<String>> {
        self.shutdown.cancel();
        let mut recorder = self.handle.await.map_err(|err| {
            DeadreckonError::InvalidInput(format!("provider flight recorder task failed: {err}"))
        })??;
        recorder.finish(state, status)
    }
}

impl ProviderFlightRecorder {
    pub fn start(
        state: &PipelineState,
        provider_name: &str,
        deadreckon_home: &Path,
        deadreckon_turn: u32,
    ) -> Result<Option<Self>> {
        let descriptor = provider_descriptor(deadreckon_home, provider_name)?;
        if descriptor
            .as_ref()
            .is_some_and(|descriptor| descriptor.kind != DescriptorKind::Cli)
        {
            return Ok(None);
        }

        let ingest = descriptor
            .as_ref()
            .and_then(|descriptor| descriptor.ingest.clone());
        let schema = ingest
            .as_ref()
            .map(|ingest| ingest.schema.clone())
            .filter(|schema| !schema.trim().is_empty())
            .unwrap_or_else(|| "unknown".to_string());
        let mut manifest = read_flight_manifest(state)?
            .unwrap_or_else(|| FlightManifest::new(state.run_id.clone()));
        mark_superseded_sessions(&mut manifest, deadreckon_turn);
        let attempt = next_attempt(&manifest, deadreckon_turn);
        let flight_session_id = format!("flight-turn-{deadreckon_turn}-attempt-{attempt}");
        let started_at = Utc::now();
        manifest.sessions.push(FlightSession {
            flight_session_id: flight_session_id.clone(),
            provider: provider_name.to_string(),
            schema: schema.clone(),
            deadreckon_turn,
            attempt,
            status: FlightSessionStatus::Running,
            started_at,
            completed_at: None,
            source_paths: Vec::new(),
        });
        let anchor_every = manifest.checkpoint_policy.anchor_every.max(1);
        let quiet_duration = Duration::from_millis(manifest.checkpoint_policy.quiet_ms);
        write_flight_manifest(state, &manifest)?;

        let started_system_time = SystemTime::now();
        let source_cursors = ingest
            .as_ref()
            .map(initial_log_cursors)
            .transpose()?
            .unwrap_or_default();
        let existing_events = read_flight_events(state)?;
        let next_seq = existing_events
            .iter()
            .map(|event| event.seq)
            .max()
            .unwrap_or(0)
            + 1;
        let last_checkpoint_index = build_working_file_index(&state.working_dir)?;
        let last_observed_index = last_checkpoint_index.clone();
        let mut recorder = Self {
            provider: provider_name.to_string(),
            schema,
            flight_session_id,
            deadreckon_turn,
            attempt,
            ingest,
            started_system_time,
            source_cursors,
            source_paths: BTreeMap::new(),
            next_seq,
            last_provider_event_seq: None,
            last_checkpoint_index,
            last_observed_index,
            pending_checkpoint_since: None,
            anchor_every,
            quiet_duration,
        };
        recorder.append_session_event(state, "started provider flight session")?;
        Ok(Some(recorder))
    }

    pub fn spawn(self, state: PipelineState) -> ProviderFlightRecorderHandle {
        ProviderFlightRecorderHandle::spawn(state, self)
    }

    pub fn poll(&mut self, state: &PipelineState) -> Result<()> {
        let saw_tool_event = self.ingest_new_log_rows(state)?;
        if saw_tool_event {
            self.capture_checkpoint_if_changed(state, CheckpointTrigger::ProviderTool)?;
        }
        self.capture_quiet_checkpoint_if_ready(state)?;
        Ok(())
    }

    pub fn finish(
        &mut self,
        state: &PipelineState,
        status: FlightSessionStatus,
    ) -> Result<Option<String>> {
        let saw_tool_event = self.ingest_new_log_rows(state)?;
        let mut checkpoint_id = None;
        if saw_tool_event {
            checkpoint_id =
                self.capture_checkpoint_if_changed(state, CheckpointTrigger::ProviderTool)?;
        }
        if let Some(final_checkpoint) =
            self.capture_checkpoint_if_changed(state, CheckpointTrigger::ProviderExit)?
        {
            checkpoint_id = Some(final_checkpoint);
        }
        self.append_session_event(
            state,
            match status {
                FlightSessionStatus::Completed => "completed provider flight session",
                FlightSessionStatus::Failed => "failed provider flight session",
                FlightSessionStatus::Killed => "killed provider flight session",
                FlightSessionStatus::Running | FlightSessionStatus::Superseded => {
                    "closed provider flight session"
                }
            },
        )?;
        self.update_manifest_status(state, status)?;
        Ok(checkpoint_id)
    }

    fn ingest_new_log_rows(&mut self, state: &PipelineState) -> Result<bool> {
        let Some(ingest) = self.ingest.clone() else {
            return Ok(false);
        };
        let mut saw_tool_event = false;
        for path in discover_log_files(&ingest)? {
            if !self.source_cursors.contains_key(&path)
                && !modified_since_provider_start(&path, self.started_system_time)?
            {
                continue;
            }
            let lines = read_log_lines(&path)?;
            let previous_count = self
                .source_cursors
                .get(&path)
                .map(|cursor| cursor.line_count)
                .unwrap_or(0);
            let start_line = if previous_count <= lines.len() as u64 {
                previous_count
            } else {
                0
            };
            for (index, line) in lines.iter().enumerate() {
                let line_number = index as u64 + 1;
                if line_number <= start_line {
                    continue;
                }
                let parsed = parse_log_line(line);
                let event = self.provider_event(state, &path, line_number, line, &parsed);
                saw_tool_event |= event.kind == FlightEventKind::Tool;
                append_flight_event(state, &event)?;
                self.last_provider_event_seq = Some(event.seq);
                self.next_seq += 1;
                self.record_source_path(&path, line_number, line);
            }
            self.source_cursors.insert(
                path,
                LogCursor {
                    line_count: lines.len() as u64,
                },
            );
        }
        Ok(saw_tool_event)
    }

    fn capture_checkpoint_if_changed(
        &mut self,
        state: &PipelineState,
        trigger: CheckpointTrigger,
    ) -> Result<Option<String>> {
        let after = build_working_file_index(&state.working_dir)?;
        self.capture_checkpoint_with_after(state, after, trigger)
    }

    fn capture_quiet_checkpoint_if_ready(
        &mut self,
        state: &PipelineState,
    ) -> Result<Option<String>> {
        let after = build_working_file_index(&state.working_dir)?;
        if after.tree_hash() == self.last_checkpoint_index.tree_hash() {
            self.last_observed_index = after;
            self.pending_checkpoint_since = None;
            return Ok(None);
        }
        if after.tree_hash() != self.last_observed_index.tree_hash() {
            self.last_observed_index = after;
            self.pending_checkpoint_since = Some(Instant::now());
            return Ok(None);
        }
        let Some(since) = self.pending_checkpoint_since else {
            self.pending_checkpoint_since = Some(Instant::now());
            return Ok(None);
        };
        if since.elapsed() < self.quiet_duration {
            return Ok(None);
        }
        self.capture_checkpoint_with_after(state, after, CheckpointTrigger::FileQuiet)
    }

    fn capture_checkpoint_with_after(
        &mut self,
        state: &PipelineState,
        after: WorkingFileIndex,
        trigger: CheckpointTrigger,
    ) -> Result<Option<String>> {
        if after.tree_hash() == self.last_checkpoint_index.tree_hash() {
            self.last_observed_index = after;
            self.pending_checkpoint_since = None;
            return Ok(None);
        }
        let checkpoint_id = next_checkpoint_id(state)?;
        let checkpoint_number = checkpoint_number(&checkpoint_id).unwrap_or(1);
        let manifest = capture_delta_checkpoint(
            state,
            &self.last_checkpoint_index,
            &after,
            CheckpointCaptureRequest {
                checkpoint_id: checkpoint_id.clone(),
                flight_session_id: self.flight_session_id.clone(),
                deadreckon_turn: self.deadreckon_turn,
                attempt: self.attempt,
                provider_event_seq: self.last_provider_event_seq,
                trigger,
                base: CheckpointBase {
                    kind: CheckpointBaseKind::TurnSnapshot,
                    id: format!("turn-{}", self.deadreckon_turn.saturating_sub(1)),
                },
                full_anchor: checkpoint_number.is_multiple_of(self.anchor_every),
            },
        )?;
        self.last_checkpoint_index = after;
        self.last_observed_index = self.last_checkpoint_index.clone();
        self.pending_checkpoint_since = None;
        let files = manifest
            .files
            .iter()
            .map(|change| change.path.clone())
            .collect::<Vec<_>>();
        self.append_checkpoint_event(
            state,
            &checkpoint_id,
            files,
            format!(
                "checkpoint {checkpoint_id} captured {} file changes",
                manifest.files.len()
            ),
        )?;
        Ok(Some(checkpoint_id))
    }

    fn append_session_event(&mut self, state: &PipelineState, summary: &str) -> Result<()> {
        let raw = json!({
            "kind": "session",
            "summary": summary,
        })
        .to_string();
        append_flight_event(
            state,
            &FlightEvent {
                version: 1,
                seq: self.next_seq,
                run_id: state.run_id.clone(),
                flight_session_id: self.flight_session_id.clone(),
                deadreckon_turn: self.deadreckon_turn,
                attempt: self.attempt,
                provider: self.provider.clone(),
                schema: self.schema.clone(),
                timestamp: Some(Utc::now()),
                source_path: None,
                source_line: None,
                raw_hash: sha256_text(&raw),
                source_event: raw,
                kind: FlightEventKind::Session,
                role: None,
                summary: summary.to_string(),
                tool_name: None,
                tool_category: None,
                files: Vec::new(),
                usage: None,
                checkpoint_id: None,
            },
        )?;
        self.next_seq += 1;
        Ok(())
    }

    fn append_checkpoint_event(
        &mut self,
        state: &PipelineState,
        checkpoint_id: &str,
        files: Vec<PathBuf>,
        summary: String,
    ) -> Result<()> {
        let raw = json!({
            "kind": "checkpoint",
            "checkpoint_id": checkpoint_id,
            "files": files,
        })
        .to_string();
        append_flight_event(
            state,
            &FlightEvent {
                version: 1,
                seq: self.next_seq,
                run_id: state.run_id.clone(),
                flight_session_id: self.flight_session_id.clone(),
                deadreckon_turn: self.deadreckon_turn,
                attempt: self.attempt,
                provider: self.provider.clone(),
                schema: self.schema.clone(),
                timestamp: Some(Utc::now()),
                source_path: None,
                source_line: None,
                raw_hash: sha256_text(&raw),
                source_event: raw,
                kind: FlightEventKind::Checkpoint,
                role: None,
                summary,
                tool_name: None,
                tool_category: None,
                files,
                usage: None,
                checkpoint_id: Some(checkpoint_id.to_string()),
            },
        )?;
        self.next_seq += 1;
        Ok(())
    }

    fn provider_event(
        &self,
        state: &PipelineState,
        source_path: &Path,
        source_line: u64,
        line: &str,
        parsed: &Value,
    ) -> FlightEvent {
        let kind = classify_provider_event(parsed, line);
        FlightEvent {
            version: 1,
            seq: self.next_seq,
            run_id: state.run_id.clone(),
            flight_session_id: self.flight_session_id.clone(),
            deadreckon_turn: self.deadreckon_turn,
            attempt: self.attempt,
            provider: self.provider.clone(),
            schema: self.schema.clone(),
            timestamp: Some(Utc::now()),
            source_path: Some(source_path.to_path_buf()),
            source_line: Some(source_line),
            source_event: line.to_string(),
            raw_hash: sha256_text(line),
            kind,
            role: extract_string(parsed, &["role", "author", "speaker"]),
            summary: event_summary(parsed, line),
            tool_name: extract_string(parsed, &["tool_name", "tool", "name"]),
            tool_category: tool_category(kind, parsed),
            files: extract_files(parsed, &state.working_dir),
            usage: extract_usage(parsed),
            checkpoint_id: None,
        }
    }

    fn record_source_path(&mut self, path: &Path, line_number: u64, line: &str) {
        self.source_paths
            .entry(path.to_path_buf())
            .and_modify(|source| {
                source.last_line = line_number;
                if !source.content.is_empty() {
                    source.content.push('\n');
                }
                source.content.push_str(line);
            })
            .or_insert_with(|| SourcePathAccumulator {
                first_line: line_number,
                last_line: line_number,
                content: line.to_string(),
            });
    }

    fn update_manifest_status(
        &self,
        state: &PipelineState,
        status: FlightSessionStatus,
    ) -> Result<()> {
        let mut manifest =
            read_flight_manifest(state)?.unwrap_or_else(|| FlightManifest::new(&state.run_id));
        let source_paths = self
            .source_paths
            .iter()
            .map(|(path, source)| FlightSourcePath {
                path: path.clone(),
                first_line: source.first_line,
                last_line: source.last_line,
                content_hash: sha256_text(&source.content),
            })
            .collect::<Vec<_>>();
        if let Some(session) = manifest
            .sessions
            .iter_mut()
            .find(|session| session.flight_session_id == self.flight_session_id)
        {
            session.status = status;
            session.completed_at = Some(Utc::now());
            session.source_paths = source_paths;
        }
        write_flight_manifest(state, &manifest)
    }
}

fn provider_descriptor(home: &Path, provider_name: &str) -> Result<Option<ProviderDescriptor>> {
    let registry = ProviderRegistry::with_overrides(home)
        .map_err(|err| DeadreckonError::InvalidInput(format!("provider registry: {err}")))?;
    Ok(registry.get(provider_name).cloned())
}

fn mark_superseded_sessions(manifest: &mut FlightManifest, deadreckon_turn: u32) {
    for session in &mut manifest.sessions {
        if session.deadreckon_turn >= deadreckon_turn
            && !matches!(session.status, FlightSessionStatus::Superseded)
        {
            session.status = FlightSessionStatus::Superseded;
            session.completed_at.get_or_insert_with(Utc::now);
        }
    }
}

fn next_attempt(manifest: &FlightManifest, deadreckon_turn: u32) -> u32 {
    manifest
        .sessions
        .iter()
        .filter(|session| session.deadreckon_turn == deadreckon_turn)
        .map(|session| session.attempt)
        .max()
        .unwrap_or(0)
        + 1
}

fn initial_log_cursors(ingest: &IngestDescriptor) -> Result<BTreeMap<PathBuf, LogCursor>> {
    let mut cursors = BTreeMap::new();
    for path in discover_log_files(ingest)? {
        let line_count = read_log_lines(&path)?.len() as u64;
        cursors.insert(path, LogCursor { line_count });
    }
    Ok(cursors)
}

fn discover_log_files(ingest: &IngestDescriptor) -> Result<Vec<PathBuf>> {
    let roots = ingest_roots(ingest);
    let mut files = Vec::new();
    for root in roots {
        collect_log_files(ingest, &root, 0, &mut files)?;
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn ingest_roots(ingest: &IngestDescriptor) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(env_var) = ingest.env_var.as_deref()
        && let Some(value) = std::env::var_os(env_var)
    {
        roots.extend(std::env::split_paths(&value));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    roots.extend(ingest.default_dirs.iter().map(|path| match home.as_ref() {
        Some(home) => expand_home_path(path, home),
        None => path.clone(),
    }));

    let mut expanded = Vec::new();
    for root in roots {
        if ingest.watch_subdirs.is_empty() {
            expanded.push(root);
        } else {
            expanded.extend(ingest.watch_subdirs.iter().map(|subdir| root.join(subdir)));
        }
    }
    expanded.sort();
    expanded.dedup();
    expanded
}

fn collect_log_files(
    ingest: &IngestDescriptor,
    root: &Path,
    depth: usize,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    if root.is_file() {
        if matches_ingest_file(ingest, root) {
            files.push(root.to_path_buf());
        }
        return Ok(());
    }
    if !root.exists() {
        return Ok(());
    }
    if depth > MAX_LOG_SCAN_DEPTH {
        return Ok(());
    }
    for entry in fs::read_dir(root).with_path(root)? {
        let entry = entry.with_path(root)?;
        let path = entry.path();
        let file_type = entry.file_type().with_path(&path)?;
        if file_type.is_file() {
            if matches_ingest_file(ingest, &path) {
                files.push(path);
            }
        } else if file_type.is_dir() && !ingest.shallow_watch {
            collect_log_files(ingest, &path, depth + 1, files)?;
        }
    }
    Ok(())
}

fn matches_ingest_file(ingest: &IngestDescriptor, path: &Path) -> bool {
    if let Some(glob) = ingest.file_glob.as_deref() {
        return matches_simple_glob(path, glob);
    }
    match ingest.storage {
        Some(IngestStorage::Jsonl) => {
            path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        }
        Some(IngestStorage::Json) => path.extension().and_then(|ext| ext.to_str()) == Some("json"),
        Some(IngestStorage::JsonOrJsonl | IngestStorage::OpenCodeStorage) | None => {
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("json" | "jsonl")
            )
        }
    }
}

fn matches_simple_glob(path: &Path, glob: &str) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if let Some(extension) = glob.strip_prefix("*.") {
        return path.extension().and_then(|ext| ext.to_str()) == Some(extension);
    }
    file_name == glob
}

fn modified_since_provider_start(path: &Path, start: SystemTime) -> Result<bool> {
    let metadata = fs::metadata(path).with_path(path)?;
    let modified = metadata.modified().with_path(path)?;
    let threshold = start
        .checked_sub(Duration::from_secs(2))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    Ok(modified >= threshold)
}

fn read_log_lines(path: &Path) -> Result<Vec<String>> {
    let raw = fs::read_to_string(path).with_path(path)?;
    Ok(raw.lines().map(ToString::to_string).collect())
}

fn parse_log_line(line: &str) -> Value {
    serde_json::from_str(line).unwrap_or_else(|_| Value::String(line.to_string()))
}

fn classify_provider_event(value: &Value, raw: &str) -> FlightEventKind {
    let label = extract_string(value, &["kind", "type", "event", "event_type"])
        .unwrap_or_else(|| raw.to_string())
        .to_ascii_lowercase();
    if label.contains("error") || label.contains("fail") {
        FlightEventKind::Error
    } else if label.contains("warn") {
        FlightEventKind::Warning
    } else if label.contains("token") || label.contains("usage") {
        FlightEventKind::Tokens
    } else if label.contains("todo") {
        FlightEventKind::Todo
    } else if label.contains("tool")
        || label.contains("function")
        || label.contains("command")
        || label.contains("edit")
    {
        FlightEventKind::Tool
    } else if label.contains("result")
        || label.contains("complete")
        || label.contains("finish")
        || label.contains("response")
    {
        FlightEventKind::Result
    } else if label.contains("think") || label.contains("reason") {
        FlightEventKind::Thinking
    } else {
        FlightEventKind::Agent
    }
}

fn tool_category(kind: FlightEventKind, value: &Value) -> Option<String> {
    if kind != FlightEventKind::Tool {
        return None;
    }
    extract_string(value, &["category", "tool_category"])
        .or_else(|| extract_string(value, &["kind", "type"]))
}

fn event_summary(value: &Value, raw: &str) -> String {
    extract_string(
        value,
        &[
            "summary", "message", "content", "text", "delta", "type", "kind",
        ],
    )
    .map(|summary| truncate_summary(&summary))
    .unwrap_or_else(|| truncate_summary(raw))
}

fn extract_string(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(value) = map.get(*key).and_then(Value::as_str)
                    && !value.trim().is_empty()
                {
                    return Some(value.trim().to_string());
                }
            }
            for child in map.values() {
                if let Some(value) = extract_string(child, keys) {
                    return Some(value);
                }
            }
            None
        }
        Value::Array(values) => values.iter().find_map(|value| extract_string(value, keys)),
        Value::String(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => None,
    }
}

fn extract_usage(value: &Value) -> Option<FlightUsage> {
    let input_tokens = find_u64(value, &["input_tokens", "prompt_tokens"]);
    let output_tokens = find_u64(value, &["output_tokens", "completion_tokens"]);
    if input_tokens.is_none() && output_tokens.is_none() {
        return None;
    }
    Some(FlightUsage {
        input_tokens: input_tokens.unwrap_or(0),
        output_tokens: output_tokens.unwrap_or(0),
        context_window: find_u64(value, &["context_window"]),
    })
}

fn find_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(value) = map.get(*key).and_then(Value::as_u64) {
                    return Some(value);
                }
            }
            map.values().find_map(|value| find_u64(value, keys))
        }
        Value::Array(values) => values.iter().find_map(|value| find_u64(value, keys)),
        _ => None,
    }
}

fn extract_files(value: &Value, working_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_file_values(None, value, working_dir, &mut files);
    files.sort();
    files.dedup();
    files.truncate(32);
    files
}

fn collect_file_values(
    key: Option<&str>,
    value: &Value,
    working_dir: &Path,
    files: &mut Vec<PathBuf>,
) {
    if files.len() >= 32 {
        return;
    }
    match value {
        Value::String(value) => {
            if key.is_some_and(file_key_matches)
                && let Some(path) = normalize_file_path(value, working_dir)
            {
                files.push(path);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_file_values(key, value, working_dir, files);
            }
        }
        Value::Object(map) => {
            for (key, value) in map {
                collect_file_values(Some(key), value, working_dir, files);
            }
        }
        _ => {}
    }
}

fn file_key_matches(key: &str) -> bool {
    matches!(
        key,
        "file" | "files" | "filename" | "filenames" | "path" | "paths" | "uri"
    ) || key.ends_with("_file")
        || key.ends_with("_path")
}

fn normalize_file_path(value: &str, working_dir: &Path) -> Option<PathBuf> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > 512
        || trimmed.contains('\n')
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
    {
        return None;
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        return path
            .strip_prefix(working_dir)
            .ok()
            .map(Path::to_path_buf)
            .filter(|path| path.components().next().is_some());
    }
    Some(path)
}

fn truncate_summary(value: &str) -> String {
    let mut out = value.trim().replace('\n', " ");
    if out.len() > MAX_SUMMARY_CHARS {
        out.truncate(MAX_SUMMARY_CHARS);
        out.push_str("...");
    }
    out
}

fn next_checkpoint_id(state: &PipelineState) -> Result<String> {
    let max = list_checkpoint_manifests(state)?
        .iter()
        .filter_map(|manifest| checkpoint_number(&manifest.checkpoint_id))
        .max()
        .unwrap_or(0);
    Ok(format!("cp-{:06}", max + 1))
}

fn checkpoint_number(checkpoint_id: &str) -> Option<u32> {
    checkpoint_id.strip_prefix("cp-")?.parse().ok()
}

fn expand_home_path(path: &Path, home: &Path) -> PathBuf {
    if path == Path::new("~") {
        return home.to_path_buf();
    }
    if let Ok(rest) = path.strip_prefix("~") {
        return home.join(rest);
    }
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use deadreckon_core::flight::flight_manifest_path;
    use deadreckon_core::paths::DeadreckonPaths;
    use deadreckon_core::state::{RunOptions, create_run};
    use tempfile::TempDir;

    #[test]
    fn provider_flight_recorder_captures_log_rows_and_checkpoint() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = temp.path().join("cwd");
        fs::create_dir_all(&cwd).expect("cwd");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "record flight".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: Some("cli:test-flight".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        fs::create_dir_all(paths.home().join("providers.d")).expect("providers");
        let provider_logs = temp.path().join("provider-logs");
        fs::create_dir_all(&provider_logs).expect("logs");
        fs::write(
            paths.home().join("providers.d/test-flight.toml"),
            format!(
                r#"
id = "cli:test-flight"
display_name = "Test Flight"
kind = "cli"
default_binary = "test-flight"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["{{prompt}}"]

[ingest]
default_dirs = ["{}"]
schema = "test-flight"
file_glob = "*.jsonl"
storage = "jsonl"
"#,
                provider_logs.display()
            ),
        )
        .expect("descriptor");
        deadreckon_core::snapshot_working(&state, 0).expect("snapshot");

        let mut recorder =
            ProviderFlightRecorder::start(&state, "cli:test-flight", paths.home(), 1)
                .expect("start")
                .expect("recorder");
        let source = state.working_dir.join("src/lib.rs");
        fs::create_dir_all(source.parent().expect("parent")).expect("src");
        fs::write(&source, "pub fn value() -> u8 { 1 }\n").expect("source");
        fs::write(
            provider_logs.join("session.jsonl"),
            r#"{"type":"tool_call","tool_name":"write_file","path":"src/lib.rs","message":"wrote source","usage":{"input_tokens":7,"output_tokens":3}}"#,
        )
        .expect("provider log");
        let checkpoint = recorder
            .finish(&state, FlightSessionStatus::Completed)
            .expect("finish")
            .expect("checkpoint");

        let manifest = read_flight_manifest(&state)
            .expect("manifest")
            .expect("manifest exists");
        assert_eq!(manifest.sessions.len(), 1);
        assert_eq!(manifest.sessions[0].status, FlightSessionStatus::Completed);
        assert_eq!(manifest.sessions[0].source_paths.len(), 1);

        let events = read_flight_events(&state).expect("events");
        assert!(events.iter().any(|event| {
            event.kind == FlightEventKind::Tool && event.files == vec![PathBuf::from("src/lib.rs")]
        }));
        assert!(events.iter().any(|event| {
            event.kind == FlightEventKind::Checkpoint
                && event.checkpoint_id.as_deref() == Some(checkpoint.as_str())
        }));
        let checkpoints = list_checkpoint_manifests(&state).expect("checkpoints");
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].checkpoint_id, checkpoint);
    }

    #[test]
    fn provider_flight_recorder_poll_captures_quiet_file_checkpoint() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = temp.path().join("cwd");
        fs::create_dir_all(&cwd).expect("cwd");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "quiet checkpoint".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: Some("cli:test-flight".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        fs::create_dir_all(paths.home().join("providers.d")).expect("providers");
        fs::write(
            paths.home().join("providers.d/test-flight.toml"),
            r#"
id = "cli:test-flight"
display_name = "Test Flight"
kind = "cli"
default_binary = "test-flight"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["{prompt}"]
"#,
        )
        .expect("descriptor");
        let mut manifest = FlightManifest::new(state.run_id.clone());
        manifest.checkpoint_policy.quiet_ms = 0;
        write_flight_manifest(&state, &manifest).expect("manifest");
        deadreckon_core::snapshot_working(&state, 0).expect("snapshot");
        let mut recorder =
            ProviderFlightRecorder::start(&state, "cli:test-flight", paths.home(), 1)
                .expect("start")
                .expect("recorder");
        let source = state.working_dir.join("src/lib.rs");
        fs::create_dir_all(source.parent().expect("parent")).expect("src");
        fs::write(&source, "pub fn value() -> u8 { 7 }\n").expect("source");
        recorder.poll(&state).expect("observe change");
        recorder.poll(&state).expect("quiet checkpoint");
        let checkpoints = list_checkpoint_manifests(&state).expect("checkpoints");
        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].trigger, CheckpointTrigger::FileQuiet);
    }

    #[test]
    fn provider_flight_recorder_supersedes_later_resume_sessions() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = temp.path().join("cwd");
        fs::create_dir_all(&cwd).expect("cwd");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "resume flight".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: Some("cli:test-flight".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        fs::create_dir_all(paths.home().join("providers.d")).expect("providers");
        fs::write(
            paths.home().join("providers.d/test-flight.toml"),
            r#"
id = "cli:test-flight"
display_name = "Test Flight"
kind = "cli"
default_binary = "test-flight"
subscription = true

[auth]
kind = "subscription"

[exec_template]
args_template = ["{prompt}"]
"#,
        )
        .expect("descriptor");
        let mut manifest = FlightManifest::new(state.run_id.clone());
        manifest.sessions.push(FlightSession {
            flight_session_id: "flight-turn-2-attempt-1".to_string(),
            provider: "cli:test-flight".to_string(),
            schema: "test-flight".to_string(),
            deadreckon_turn: 2,
            attempt: 1,
            status: FlightSessionStatus::Completed,
            started_at: Utc::now(),
            completed_at: Some(Utc::now()),
            source_paths: Vec::new(),
        });
        write_flight_manifest(&state, &manifest).expect("manifest");

        let recorder = ProviderFlightRecorder::start(&state, "cli:test-flight", paths.home(), 2)
            .expect("start")
            .expect("recorder");
        assert_eq!(recorder.attempt, 2);
        let manifest = read_flight_manifest(&state)
            .expect("manifest")
            .expect("manifest exists");
        assert_eq!(manifest.sessions[0].status, FlightSessionStatus::Superseded);
        assert_eq!(manifest.sessions[1].attempt, 2);
    }

    #[test]
    fn non_cli_descriptor_does_not_start_flight_recorder() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = temp.path().join("cwd");
        fs::create_dir_all(&cwd).expect("cwd");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "http flight".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: Some("openai".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        let recorder =
            ProviderFlightRecorder::start(&state, "openai", paths.home(), 1).expect("start");
        assert!(recorder.is_none());
        assert!(!flight_manifest_path(&state).exists());
    }
}
