use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::artifacts::{append_trace, copy_tree, inventory_files};
use crate::codebase::{CodebaseMode, read_codebase_record};
use crate::error::{DeadreckonError, IoContext, Result};
use crate::git::run_git;
use crate::state::{PipelineState, append_json_line};
use deadreckon_protocol::TraceRecord;

pub const DOCS_DIR: &str = ".deadreckon/docs";
pub const PUBLIC_DOCS_DIR: &str = "docs";
pub const RUN_NARRATIVE: &str = "RUN-NARRATIVE.md";
pub const RUN_AS_BUILT: &str = "RUN-AS-BUILT.md";
pub const RUN_DECISIONS: &str = "RUN-DECISIONS.md";
pub const AS_BUILT_DELTA: &str = "AS-BUILT-DELTA.md";
pub const INCREMENTAL_JSONL: &str = "_incremental.jsonl";
pub const POLISH_JSON: &str = "polish.json";
pub const IMPLEMENTATION_NOTES_HTML: &str = "implementation-notes.html";

const IMPLEMENTATION_NOTE_SECTIONS: [(&str, &str); 4] = [
    ("design-decisions", "Design decisions"),
    ("deviations", "Deviations"),
    ("tradeoffs", "Tradeoffs"),
    ("open-questions", "Open questions"),
];
const ILL_REGEX_PATTERN: &str = r"(?i)\bi['’]?ll\s+([a-z][a-z0-9_-]*(?:\s+[a-z0-9_./-]+){0,5})";
const DECISION_MARKER_PATTERNS: &[&str] = &[
    r"(?i)\b(let me consider|let me think|i'll go with|i'll choose)\b",
    r"(?i)\b(option [123]|alternatives?:|either .* or)\b",
    r"(?i)\b(instead of|rather than|actually,?\s*let)\b",
    r"(?i)\bdecision\b.*\b(chose|pick|go(?:ing)? with)\b",
];

#[derive(Debug, Clone)]
pub struct FrontmatterFields {
    pub last_updated: DateTime<Utc>,
    pub doc_writer: String,
    pub parent_run_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TurnDocInput {
    pub turn: u32,
    pub tool_kind: String,
    pub latency_ms: Option<u128>,
    pub files: Vec<PathBuf>,
    pub outcome: String,
    pub response_text: String,
    pub tool_stdout: Option<String>,
    pub tool_stderr: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnRecord {
    pub turn: u32,
    pub title: String,
    pub tool_kind: String,
    pub latency_ms: Option<u128>,
    pub files: Vec<FileChange>,
    pub outcome: String,
    #[serde(default)]
    pub response_full: String,
    #[serde(default)]
    pub response_summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_stderr: Option<String>,
    pub trace_link: String,
    pub snapshot_link: String,
    pub commit_sha: Option<String>,
    #[serde(default)]
    pub response_text: String,
    pub decision_candidate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileChange {
    pub path: String,
    pub adds: u32,
    pub dels: u32,
    pub largest_hunk_excerpt: String,
    pub is_new: bool,
    pub is_binary: bool,
}

impl<'de> Deserialize<'de> for FileChange {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if let Some(path) = value.as_str() {
            return Ok(Self::for_path(path.to_string()));
        }
        #[derive(Deserialize)]
        struct FileChangeObject {
            path: String,
            #[serde(default)]
            adds: u32,
            #[serde(default)]
            dels: u32,
            #[serde(default)]
            largest_hunk_excerpt: String,
            #[serde(default)]
            is_new: bool,
            #[serde(default)]
            is_binary: bool,
        }
        let object: FileChangeObject =
            serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        Ok(Self {
            path: object.path,
            adds: object.adds,
            dels: object.dels,
            largest_hunk_excerpt: object.largest_hunk_excerpt,
            is_new: object.is_new,
            is_binary: object.is_binary,
        })
    }
}

impl FileChange {
    pub fn for_path(path: String) -> Self {
        Self {
            path,
            adds: 0,
            dels: 0,
            largest_hunk_excerpt: String::new(),
            is_new: false,
            is_binary: false,
        }
    }
}

impl TurnRecord {
    pub fn file_paths(&self) -> Vec<String> {
        self.files
            .iter()
            .map(|file| file.path.clone())
            .filter(|file| is_documentable_path(file))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase {
    pub index: usize,
    pub title: String,
    pub start_turn: u32,
    pub end_turn: u32,
    pub commit_sha: Option<String>,
    pub files: Vec<String>,
    pub turns: Vec<TurnRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocsStatus {
    Polished,
    Incremental,
    Failed,
    NotAvailable,
}

impl std::fmt::Display for DocsStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Polished => "polished",
            Self::Incremental => "incremental",
            Self::Failed => "failed",
            Self::NotAvailable => "n/a",
        };
        f.write_str(value)
    }
}

pub fn docs_dir(working_dir: &Path) -> PathBuf {
    working_dir.join(DOCS_DIR)
}

pub fn public_docs_dir(working_dir: &Path) -> PathBuf {
    working_dir.join(PUBLIC_DOCS_DIR)
}

pub fn narrative_path(working_dir: &Path) -> PathBuf {
    docs_dir(working_dir).join(RUN_NARRATIVE)
}

pub fn as_built_path(working_dir: &Path) -> PathBuf {
    docs_dir(working_dir).join(RUN_AS_BUILT)
}

pub fn decisions_path(working_dir: &Path) -> PathBuf {
    docs_dir(working_dir).join(RUN_DECISIONS)
}

pub fn delta_path(working_dir: &Path) -> PathBuf {
    docs_dir(working_dir).join(AS_BUILT_DELTA)
}

pub fn incremental_path(working_dir: &Path) -> PathBuf {
    docs_dir(working_dir).join(INCREMENTAL_JSONL)
}

pub fn polish_path(working_dir: &Path) -> PathBuf {
    docs_dir(working_dir).join(POLISH_JSON)
}

pub fn implementation_notes_path(working_dir: &Path) -> PathBuf {
    working_dir.join(IMPLEMENTATION_NOTES_HTML)
}

pub fn public_doc_path(working_dir: &Path, file_name: &str) -> PathBuf {
    public_docs_dir(working_dir).join(file_name)
}

pub fn ensure_docs_started(state: &PipelineState) -> Result<()> {
    ensure_implementation_notes_started(state)?;
    let dir = docs_dir(&state.working_dir);
    fs::create_dir_all(&dir).with_path(&dir)?;
    let incremental = incremental_path(&state.working_dir);
    if !incremental.exists() {
        fs::write(&incremental, "").with_path(&incremental)?;
    }
    rewrite_templated_docs(state, "templated only")
}

pub fn ensure_implementation_notes_started(state: &PipelineState) -> Result<()> {
    let path = implementation_notes_path(&state.working_dir);
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_path(parent)?;
    }
    write_file(path, implementation_notes_template(state))
}

pub fn frontmatter(state: &PipelineState, fields: &FrontmatterFields) -> String {
    let mut lines = Vec::new();
    lines.push(format!("# {}", title_from_goal(&state.goal)));
    lines.push(String::new());
    lines.push(format!("**Date:** {}", state.started_at.to_rfc3339()));
    lines.push(format!(
        "**Last updated:** {}",
        fields.last_updated.to_rfc3339()
    ));
    lines.push(format!("**Status:** {}", state.status));
    lines.push(format!("**Run ID:** `{}`", state.run_id));
    lines.push(format!("**Goal:** {}", state.goal));
    if let Some(parent) = fields.parent_run_id.as_deref() {
        lines.push(format!("**Parent run:** `{}`", short_id(parent)));
    }
    lines.extend(commit_or_working_lines(state));
    lines.push(format!("**Owner:** {}", owner_line(state)));
    lines.push(format!(
        "**Provider:** {}",
        state.provider.as_deref().unwrap_or("unconfigured")
    ));
    lines.push(format!("**Sandbox:** {}", state.sandbox));
    lines.push(format!("**Spend:** {}", spend_line(state)));
    lines.push(format!("**Doc-writer:** {}", fields.doc_writer));
    lines.push(String::new());
    lines.join("\n")
}

pub fn append_turn_doc(state: &PipelineState, input: TurnDocInput) -> Result<TurnRecord> {
    ensure_docs_started(state)?;
    let normalized_files = normalize_files(state, &input.files);
    let file_changes = capture_diff_samples(state, input.turn, &input.files)?;
    let title = auto_title(
        &input.response_text,
        &input.tool_kind,
        &normalized_files,
        input.turn,
    );
    let response_full = capture_response_full(&input.response_text);
    let response_summary = capture_response_summary(&response_full);
    let record = TurnRecord {
        turn: input.turn,
        title,
        tool_kind: input.tool_kind,
        latency_ms: input.latency_ms,
        files: file_changes,
        outcome: input.outcome,
        response_full,
        response_summary,
        tool_stdout: input.tool_stdout.as_deref().map(capture_tool_stdio),
        tool_stderr: input.tool_stderr.as_deref().map(capture_tool_stdio),
        trace_link: format!("../traces.jsonl#turn-{}", input.turn),
        snapshot_link: format!("../../snapshots/turn-{}/", input.turn),
        commit_sha: current_worktree_sha(state),
        decision_candidate: is_decision_candidate(&input.response_text),
        response_text: input.response_text,
    };
    append_json_line(&incremental_path(&state.working_dir), &record)?;
    rewrite_templated_docs(state, "templated only")?;
    Ok(record)
}

pub fn capture_response_full(response: &str) -> String {
    cap_utf8(response, 50 * 1024)
}

pub fn capture_response_summary(response: &str) -> String {
    let paragraph = response
        .split("\n\n")
        .find(|part| !part.trim().is_empty())
        .unwrap_or(response)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    cap_on_word_boundary(&paragraph, 280)
}

pub fn capture_tool_stdio(value: &str) -> String {
    cap_utf8(value, 10 * 1024)
}

pub fn capture_diff_samples(
    state: &PipelineState,
    turn: u32,
    files: &[PathBuf],
) -> Result<Vec<FileChange>> {
    let previous_root = state
        .run_root
        .join("snapshots")
        .join(format!("turn-{}", turn.saturating_sub(1)));
    let mut changes = Vec::new();
    let mut seen = BTreeSet::new();
    for path in files {
        let Some(relative) = normalize_file(state, path) else {
            continue;
        };
        if !seen.insert(relative.clone()) {
            continue;
        }
        let current = if path.is_absolute() {
            path.clone()
        } else {
            state.working_dir.join(path)
        };
        let previous = previous_root.join(&relative);
        changes.push(diff_sample_for_file(&relative, &previous, &current)?);
    }
    Ok(changes)
}

pub fn read_turn_records(working_dir: &Path) -> Result<Vec<TurnRecord>> {
    let path = incremental_path(working_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&path).with_path(&path)?;
    let mut records = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        records.push(
            serde_json::from_str(line).map_err(|source| DeadreckonError::Json {
                path: path.clone(),
                source,
            })?,
        );
    }
    Ok(records)
}

pub fn rewrite_templated_docs(state: &PipelineState, doc_writer: &str) -> Result<()> {
    fs::create_dir_all(docs_dir(&state.working_dir)).with_path(docs_dir(&state.working_dir))?;
    let records = read_turn_records(&state.working_dir)?;
    let parent = parent_run_id(&state.working_dir);
    let fields = FrontmatterFields {
        last_updated: Utc::now(),
        doc_writer: doc_writer.to_string(),
        parent_run_id: parent.clone(),
    };
    write_file(
        narrative_path(&state.working_dir),
        render_narrative(state, &fields, &records, parent.as_deref()),
    )?;
    write_file(
        as_built_path(&state.working_dir),
        render_as_built(state, &fields, &records),
    )?;
    write_file(
        decisions_path(&state.working_dir),
        render_decisions(state, &fields, &records),
    )?;
    maybe_write_delta(state, &records)?;
    Ok(())
}

pub fn publish_docs_for_promotion(state: &PipelineState) -> Result<()> {
    publish_docs_for_promotion_with_commit(state, true)
}

/// Publish the operator-facing run documents without invoking Git.
///
/// Durable runs use this form because provider-backed polishing happens after
/// a coding turn. The runtime then routes the final document write through the
/// same trusted Git-control sanitizer as every other result change.
pub fn publish_docs_for_promotion_uncommitted(state: &PipelineState) -> Result<()> {
    publish_docs_for_promotion_with_commit(state, false)
}

fn publish_docs_for_promotion_with_commit(state: &PipelineState, commit_docs: bool) -> Result<()> {
    let internal = docs_dir(&state.working_dir);
    if !internal.is_dir() {
        return Ok(());
    }
    let public = public_docs_dir(&state.working_dir);
    fs::create_dir_all(&public).with_path(&public)?;
    for name in [RUN_NARRATIVE, RUN_AS_BUILT, RUN_DECISIONS, AS_BUILT_DELTA] {
        let source = internal.join(name);
        if source.exists() {
            let dest = public.join(name);
            if name == RUN_DECISIONS {
                let mut raw = fs::read_to_string(&source).with_path(&source)?;
                if implementation_notes_path(&state.working_dir).exists() {
                    raw = raw.replace(
                        &format!("](../../{IMPLEMENTATION_NOTES_HTML})"),
                        &format!("](../{IMPLEMENTATION_NOTES_HTML})"),
                    );
                }
                write_file(dest, raw)?;
            } else {
                fs::copy(&source, dest).with_path(&source)?;
            }
        }
    }
    if delta_path(&state.working_dir).exists() {
        let named = format!("AS-BUILT-DELTA-{}.md", short_id(&state.run_id));
        fs::copy(delta_path(&state.working_dir), public.join(named))
            .with_path(delta_path(&state.working_dir))?;
    }
    mirror_trace_file(state)?;
    if commit_docs {
        commit_docs_if_worktree(state)?;
    }
    Ok(())
}

pub fn docs_status_for_state(state: &PipelineState) -> DocsStatus {
    let path = polish_path(&state.working_dir);
    if path.exists()
        && let Ok(raw) = fs::read_to_string(&path)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw)
    {
        return match value.get("status").and_then(serde_json::Value::as_str) {
            Some("polished") => DocsStatus::Polished,
            Some("incremental") | Some("skipped") => DocsStatus::Incremental,
            Some(_) => DocsStatus::Failed,
            None => DocsStatus::Incremental,
        };
    }
    if narrative_path(&state.working_dir).exists()
        || public_doc_path(&state.working_dir, RUN_NARRATIVE).exists()
    {
        DocsStatus::Incremental
    } else {
        DocsStatus::NotAvailable
    }
}

pub fn doc_path_for_kind(working_dir: &Path, kind: DocKind) -> Option<PathBuf> {
    let file = kind.file_name();
    let public = public_doc_path(working_dir, file);
    if public.exists() {
        return Some(public);
    }
    let internal = docs_dir(working_dir).join(file);
    if internal.exists() {
        return Some(internal);
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    Narrative,
    AsBuilt,
    Decisions,
    Delta,
}

impl DocKind {
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Narrative => RUN_NARRATIVE,
            Self::AsBuilt => RUN_AS_BUILT,
            Self::Decisions => RUN_DECISIONS,
            Self::Delta => AS_BUILT_DELTA,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImplementationNotesStatus {
    Current,
    Missing,
    MissingSections(Vec<&'static str>),
    Stale {
        notes_turn: Option<u32>,
        implementation_turn: u32,
    },
}

impl ImplementationNotesStatus {
    pub fn is_current(&self) -> bool {
        matches!(self, Self::Current)
    }

    pub fn reason(&self) -> String {
        match self {
            Self::Current => "implementation notes are current".to_string(),
            Self::Missing => format!("{IMPLEMENTATION_NOTES_HTML} is missing"),
            Self::MissingSections(sections) => {
                format!(
                    "{IMPLEMENTATION_NOTES_HTML} is missing required section IDs or headings for: {}",
                    sections.join(", ")
                )
            }
            Self::Stale {
                notes_turn,
                implementation_turn,
            } => format!(
                "{IMPLEMENTATION_NOTES_HTML} is stale; latest notes turn {}, latest implementation turn {implementation_turn}",
                notes_turn
                    .map(|turn| turn.to_string())
                    .unwrap_or_else(|| "none".to_string())
            ),
        }
    }
}

pub fn check_implementation_notes_current(
    state: &PipelineState,
) -> Result<ImplementationNotesStatus> {
    let path = implementation_notes_path(&state.working_dir);
    if !path.exists() {
        return Ok(ImplementationNotesStatus::Missing);
    }
    let raw = fs::read_to_string(&path).with_path(&path)?;
    let missing = missing_implementation_note_sections(&raw);
    if !missing.is_empty() {
        return Ok(ImplementationNotesStatus::MissingSections(missing));
    }
    let records = read_turn_records(&state.working_dir)?;
    let notes_turn = latest_notes_turn(&records);
    let implementation_turn = latest_implementation_turn(&records);
    if let Some(implementation_turn) = implementation_turn
        && notes_turn
            .map(|notes_turn| notes_turn < implementation_turn)
            .unwrap_or(true)
    {
        return Ok(ImplementationNotesStatus::Stale {
            notes_turn,
            implementation_turn,
        });
    }
    Ok(ImplementationNotesStatus::Current)
}

pub fn coalesce_into_phases(records: &[TurnRecord]) -> Vec<Phase> {
    if records.is_empty() {
        return Vec::new();
    }
    let mut groups: Vec<Vec<TurnRecord>> = Vec::new();
    let mut current = vec![records[0].clone()];
    for turn in records.iter().skip(1) {
        let overlap = file_overlap(&current, turn);
        let same_kind_run = same_tool_kind_consecutive_count(&current, &turn.tool_kind);
        if overlap > 0.5 || same_kind_run > 0 && same_kind_run < 3 {
            current.push(turn.clone());
        } else {
            groups.push(current);
            current = vec![turn.clone()];
        }
    }
    groups.push(current);
    while groups.len() > 8 {
        let idx = smallest_neighbor_index(&groups);
        let next = groups.remove(idx + 1);
        groups[idx].extend(next);
    }
    groups
        .into_iter()
        .enumerate()
        .map(|(index, turns)| phase_from_group(index + 1, turns))
        .collect()
}

pub fn is_decision_candidate(response: &str) -> bool {
    if response.chars().count() < 200 {
        return false;
    }
    decision_markers()
        .iter()
        .any(|regex| regex.is_match(response))
}

pub fn auto_title(response: &str, tool_kind: &str, files: &[String], turn: u32) -> String {
    if let Some(capture) = ill_regex().captures(response)
        && let Some(value) = capture.get(1)
    {
        return title_case_words(value.as_str(), 6);
    }
    if let Some(file) = files.first() {
        let basename = Path::new(file)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(file);
        return match tool_kind {
            "write_file" => format!("Write {basename}"),
            "bash" => format!("Run {basename}"),
            "cli_subagent" => format!("Edit {basename}"),
            other => format!("{} {}", title_case_words(other, 2), basename),
        };
    }
    match tool_kind {
        "bash" => "Run command".to_string(),
        "done" => "Complete run".to_string(),
        _ => format!("Turn {turn}"),
    }
}

pub fn changed_doc_files(state: &PipelineState) -> Result<Vec<String>> {
    let records = read_turn_records(&state.working_dir)?;
    Ok(records
        .iter()
        .flat_map(TurnRecord::file_paths)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

pub fn missing_files_in_narrative(state: &PipelineState) -> Result<Vec<String>> {
    let narrative = fs::read_to_string(narrative_path(&state.working_dir))
        .or_else(|_| fs::read_to_string(public_doc_path(&state.working_dir, RUN_NARRATIVE)))
        .unwrap_or_default();
    Ok(changed_doc_files(state)?
        .into_iter()
        .filter(|file| {
            let basename = Path::new(file)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(file);
            !narrative.contains(file) && !narrative.contains(basename)
        })
        .collect())
}

pub fn source_layout(records: &[TurnRecord], working_dir: &Path) -> String {
    let files = all_files(records);
    let mut out = String::new();
    out.push_str("| Layer | Responsibilities | Key entrypoints |\n| --- | --- | --- |\n");
    let component_rows = component_rows(records);
    if component_rows.is_empty() {
        out.push_str("| (none inferred) | No mapped component paths changed | - |\n");
    } else {
        for row in component_rows {
            out.push_str(&format!(
                "| {} | {} | `{}` |\n",
                row.layer, row.responsibility, row.entrypoint
            ));
        }
    }
    if let Some(topology) = topology_for_files(working_dir, &files) {
        out.push_str("\n```text\n");
        out.push_str(&topology);
        out.push_str("\n```\n");
    }
    out
}

pub fn diff_samples_markdown(records: &[TurnRecord]) -> String {
    let mut out = String::new();
    for record in records {
        let files = record
            .files
            .iter()
            .filter(|file| is_documentable_path(&file.path))
            .collect::<Vec<_>>();
        if files.is_empty() {
            continue;
        }
        out.push_str(&format!("### Turn {}\n\n", record.turn));
        for file in files {
            out.push_str(&format!(
                "- `{}`: +{}/-{}{}\n",
                file.path,
                file.adds,
                file.dels,
                if file.is_binary { " (binary)" } else { "" }
            ));
            if !file.largest_hunk_excerpt.trim().is_empty() {
                out.push_str("  ```diff\n");
                out.push_str(&file.largest_hunk_excerpt);
                out.push_str("\n  ```\n");
            }
        }
    }
    if out.trim().is_empty() {
        "No diff samples recorded.".to_string()
    } else {
        out
    }
}

pub fn tool_stdio_markdown(records: &[TurnRecord]) -> String {
    let mut out = String::new();
    for record in records {
        if record.tool_stdout.is_none() && record.tool_stderr.is_none() {
            continue;
        }
        out.push_str(&format!(
            "### Turn {} `{}`\n\n",
            record.turn, record.tool_kind
        ));
        if let Some(stdout) = record.tool_stdout.as_deref()
            && !stdout.trim().is_empty()
        {
            out.push_str("stdout:\n\n```text\n");
            out.push_str(stdout.trim());
            out.push_str("\n```\n\n");
        }
        if let Some(stderr) = record.tool_stderr.as_deref()
            && !stderr.trim().is_empty()
        {
            out.push_str("stderr:\n\n```text\n");
            out.push_str(stderr.trim());
            out.push_str("\n```\n\n");
        }
    }
    if out.trim().is_empty() {
        "No bash stdout/stderr captured.".to_string()
    } else {
        out
    }
}

pub fn append_docs_warning(state: &PipelineState, message: &str) -> Result<()> {
    append_trace(
        state,
        &TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: state.turn,
            event: "docs.warning".to_string(),
            latency_ms: None,
            detail: json!({ "message": message }),
        },
    )
}

pub fn append_parent_narrative_update(parent: &PipelineState, child: &PipelineState) -> Result<()> {
    for path in [
        parent.working_dir.join("docs/RUN-NARRATIVE.md"),
        parent.working_dir.join(".deadreckon/docs/RUN-NARRATIVE.md"),
    ] {
        if !path.exists() {
            continue;
        }
        let mut raw = fs::read_to_string(&path).with_path(&path)?;
        if raw.contains(&format!("child run `{}`", child.run_id)) {
            continue;
        }
        raw.push_str("\n\n## Updates since\n\n");
        raw.push_str(&format!(
            "- Extended by child run `{}` for goal: {}. Child narrative: `library/{}/{}/docs/RUN-NARRATIVE.md`.\n",
            child.run_id, child.goal, child.scope, child.run_id
        ));
        fs::write(&path, raw).with_path(path)?;
    }
    Ok(())
}

fn render_narrative(
    state: &PipelineState,
    fields: &FrontmatterFields,
    records: &[TurnRecord],
    parent_run_id: Option<&str>,
) -> String {
    let mut out = frontmatter(state, fields);
    if let Some(parent) = parent_run_id {
        out.push_str(&format!(
            "> **Reading order:** Start with the parent narrative in `library/{}/{}/docs/RUN-NARRATIVE.md`, then read the \"Updates since the parent run\" section below.\n\n",
            state.scope, parent
        ));
    }
    out.push_str("## Goal\n\n");
    out.push_str(&state.goal);
    out.push_str("\n\n## Why now\n\n");
    out.push_str("This run was meant to leave a usable project handoff: what changed, which files matter, how the work was checked, and what still needs attention.\n\n");
    out.push_str("## High-level approach\n\n");
    out.push_str(&high_level_approach(records));
    out.push_str("\n\n## What shipped in this run\n\n");
    let phases = coalesce_into_phases(records);
    if phases.is_empty() {
        out.push_str("- No turn activity recorded yet.\n\n");
    } else {
        for phase in phases {
            out.push_str(&format!(
                "### Phase {} - {} ({})\n\n",
                phase.index,
                phase.title,
                phase
                    .commit_sha
                    .as_deref()
                    .map(|sha| format!("commit `{sha}`"))
                    .unwrap_or_else(|| "commit `-`".to_string())
            ));
            out.push_str(&phase_paragraph(&phase));
            out.push_str("\n\n");
            out.push_str("| File | Change | Largest hunk |\n| --- | ---: | --- |\n");
            for file in phase_file_changes(&phase) {
                out.push_str(&format!(
                    "| `{}` | +{} / -{} | {} |\n",
                    file.path,
                    file.adds,
                    file.dels,
                    hunk_inline(&file.largest_hunk_excerpt)
                ));
            }
            if phase.files.is_empty() {
                out.push_str("| - | +0 / -0 | no file changes recorded |\n");
            }
            out.push('\n');
            for turn in &phase.turns {
                out.push_str(&render_turn_section(turn));
            }
            out.push('\n');
        }
    }
    if let Some(parent) = parent_run_id {
        out.push_str("## Updates since the parent run\n\n");
        let files = records
            .iter()
            .flat_map(TurnRecord::file_paths)
            .collect::<BTreeSet<_>>();
        if files.is_empty() {
            out.push_str(&format!(
                "- Extends parent `{}`; no changed files recorded yet.\n\n",
                short_id(parent)
            ));
        } else {
            for file in files {
                out.push_str(&format!(
                    "- `{file}` changed after parent `{}`. Trace: [turns](../traces.jsonl).\n",
                    short_id(parent)
                ));
            }
            out.push('\n');
        }
    }
    out.push_str("## Open threads\n\n");
    let open_threads = open_threads(records);
    if open_threads.is_empty() {
        out.push_str("- No open threads recorded by deadreckon.\n\n");
    } else {
        for thread in open_threads {
            out.push_str(&format!("- {thread}\n"));
        }
        out.push('\n');
    }
    out.push_str("## Cross-references\n\n");
    out.push_str(&format!(
        "- Traces: `traces.jsonl` ({} turns)\n",
        records.len()
    ));
    out.push_str("- Provenance: `provenance.jsonl`\n");
    out.push_str(&format!(
        "- Snapshots: `snapshots/turn-0/` through `snapshots/turn-{}`\n",
        state.turn
    ));
    if let Ok(record) = read_codebase_record(&state.working_dir)
        && let Some(branch) = record.branch_name.as_deref()
    {
        out.push_str(&format!(
            "- Branch: `{branch}` at `{}`\n",
            current_worktree_sha(state).unwrap_or_else(|| "-".to_string())
        ));
    }
    out.push_str("- Acceptance: `proofs/turn-acceptance.json`\n");
    out
}

fn render_turn_section(record: &TurnRecord) -> String {
    let mut out = String::new();
    let files = record
        .files
        .iter()
        .filter(|file| is_documentable_path(&file.path))
        .collect::<Vec<_>>();
    out.push_str(&format!(
        "\n#### Turn {} - {} ({})\n\n",
        record.turn,
        record.title,
        record
            .commit_sha
            .as_deref()
            .map(|sha| format!("commit `{sha}`"))
            .unwrap_or_else(|| "commit `-`".to_string())
    ));
    out.push_str(&format!(
        "- Tool: {} ({})\n",
        record.tool_kind,
        record
            .latency_ms
            .map(|ms| format!("{ms}ms"))
            .unwrap_or_else(|| "latency not recorded".to_string())
    ));
    if files.is_empty() && record.files.is_empty() {
        out.push_str("- Files: none recorded\n");
    } else if files.is_empty() {
        out.push_str("- Files: generated/vendor artifacts omitted from the handoff\n");
    } else {
        out.push_str(&format!(
            "- Files: {}\n",
            files
                .iter()
                .map(|file| format!("`{}`", file.path))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        for file in files {
            out.push_str(&format!(
                "  - `{}`: +{} / -{}{}\n",
                file.path,
                file.adds,
                file.dels,
                if file.is_binary { " (binary)" } else { "" }
            ));
            if !file.largest_hunk_excerpt.trim().is_empty() {
                out.push_str("    ```diff\n");
                for line in file.largest_hunk_excerpt.lines().take(3) {
                    out.push_str("    ");
                    out.push_str(line);
                    out.push('\n');
                }
                out.push_str("    ```\n");
            }
        }
    }
    out.push_str(&format!("- Outcome: {}\n", record.outcome));
    out.push_str(&format!(
        "- Trace: [turn {}]({})\n",
        record.turn, record.trace_link
    ));
    out.push_str(&format!("- Snapshot: `{}`\n", record.snapshot_link));
    out
}

fn render_turn_citations(records: &[TurnRecord]) -> String {
    let mut out = String::new();
    if records.is_empty() {
        out.push_str("- No turn citations recorded yet.\n");
    } else {
        for record in records {
            out.push_str(&format!(
                "- Turn {}: [trace]({}) and snapshot `{}`\n",
                record.turn, record.trace_link, record.snapshot_link
            ));
        }
    }
    out
}

fn implementation_notes_template(state: &PipelineState) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Implementation notes</title>
</head>
<body>
  <h1>Implementation notes</h1>
  <dl>
    <dt>Run</dt><dd>{}</dd>
    <dt>Goal</dt><dd>{}</dd>
    <dt>Last updated</dt><dd>{}</dd>
  </dl>
  <section id="design-decisions"><h2>Design decisions</h2><p>None.</p></section>
  <section id="deviations"><h2>Deviations</h2><p>None.</p></section>
  <section id="tradeoffs"><h2>Tradeoffs</h2><p>None.</p></section>
  <section id="open-questions"><h2>Open questions</h2><p>None.</p></section>
</body>
</html>
"#,
        html_escape(&state.run_id),
        html_escape(&state.goal),
        Utc::now().to_rfc3339()
    )
}

fn missing_implementation_note_sections(raw: &str) -> Vec<&'static str> {
    let lower = raw.to_ascii_lowercase();
    IMPLEMENTATION_NOTE_SECTIONS
        .iter()
        .filter_map(|(id, heading)| {
            let has_id =
                lower.contains(&format!("id=\"{id}\"")) || lower.contains(&format!("id='{id}'"));
            let has_heading = lower.contains(&heading.to_ascii_lowercase());
            (!has_id || !has_heading).then_some(*heading)
        })
        .collect()
}

fn latest_notes_turn(records: &[TurnRecord]) -> Option<u32> {
    records
        .iter()
        .filter(|record| {
            record
                .files
                .iter()
                .any(|file| is_implementation_notes_file(&file.path))
        })
        .map(|record| record.turn)
        .max()
}

fn latest_implementation_turn(records: &[TurnRecord]) -> Option<u32> {
    records
        .iter()
        .filter(|record| {
            record.files.iter().any(|file| {
                is_documentable_path(&file.path) && !is_implementation_notes_file(&file.path)
            })
        })
        .map(|record| record.turn)
        .max()
}

fn is_implementation_notes_file(path: &str) -> bool {
    path.trim_start_matches("./").replace('\\', "/") == IMPLEMENTATION_NOTES_HTML
}

fn implementation_note_markdown_sections(working_dir: &Path) -> BTreeMap<&'static str, String> {
    let path = implementation_notes_path(working_dir);
    let raw = fs::read_to_string(path).unwrap_or_default();
    IMPLEMENTATION_NOTE_SECTIONS
        .iter()
        .map(|(id, heading)| {
            let value = extract_html_section(&raw, id)
                .map(|section| html_fragment_to_markdown(&section))
                .filter(|section| !section.trim().is_empty())
                .unwrap_or_else(|| "None.".to_string());
            (*heading, value)
        })
        .collect()
}

fn extract_html_section(raw: &str, id: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let id_double = format!("id=\"{id}\"");
    let id_single = format!("id='{id}'");
    let id_index = lower.find(&id_double).or_else(|| lower.find(&id_single))?;
    let start_tag = lower[..id_index].rfind("<section")?;
    let content_start = lower[start_tag..].find('>')? + start_tag + 1;
    let content_end = lower[content_start..].find("</section>")? + content_start;
    Some(raw[content_start..content_end].to_string())
}

fn html_fragment_to_markdown(fragment: &str) -> String {
    let mut prepared = fragment
        .replace("\r\n", "\n")
        .replace("<li>", "\n- ")
        .replace("</li>", "\n")
        .replace("</p>", "\n\n")
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("</div>", "\n")
        .replace("</section>", "\n");
    prepared = strip_html_tags(&prepared);
    let decoded = decode_html_entities(&prepared);
    let lines = decoded
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            !IMPLEMENTATION_NOTE_SECTIONS
                .iter()
                .any(|(_, heading)| line.eq_ignore_ascii_case(heading))
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        "None.".to_string()
    } else {
        lines.join("\n")
    }
}

fn strip_html_tags(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn html_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn decode_html_entities(raw: &str) -> String {
    raw.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn render_as_built(
    state: &PipelineState,
    fields: &FrontmatterFields,
    records: &[TurnRecord],
) -> String {
    let mut out = frontmatter(state, fields);
    out.push_str(&format!(
        "**Subject:** run `{}` changed the user-authored files listed below.\n\n",
        short_id(&state.run_id)
    ));
    out.push_str("This document describes the subsystem changed by this run. For chronology, see [`RUN-NARRATIVE.md`](./RUN-NARRATIVE.md).\n\n");
    out.push_str("## System overview\n\n");
    out.push_str(&high_level_approach(records));
    out.push_str("\n\n**What's load-bearing:** The changed entrypoints listed below are the files future maintainers should inspect first.\n\n");
    out.push_str("**Where the seams are:** The boundaries are inferred from source paths, manifests, tests, and docs; generated, vendor, cache, and build-output paths are intentionally left out.\n\n");
    out.push_str("## Components (changed in this run)\n\n");
    out.push_str("| Layer | Responsibilities | Key entrypoints |\n| --- | --- | --- |\n");
    let files = all_files(records);
    let rows = component_rows(records);
    if rows.is_empty() {
        out.push_str("| (none inferred) | No mapped component paths changed | - |\n");
    } else {
        for row in rows {
            out.push_str(&format!(
                "| {} | {} | `{}` |\n",
                row.layer, row.responsibility, row.entrypoint
            ));
        }
    }
    out.push_str("\n## Process / data flow\n\n");
    if let Some(topology) = topology_for_files(&state.working_dir, &files) {
        out.push_str("```text\n");
        out.push_str(&topology);
        out.push_str("\n```\n\n");
    } else {
        out.push_str("No multi-directory process topology was inferred for this run.\n\n");
    }
    out.push_str("## File-system layout (changed/added paths)\n\n```text\n");
    if files.is_empty() {
        out.push_str("(no changed paths recorded)\n");
    } else {
        for file in &files {
            out.push_str(file);
            out.push('\n');
        }
    }
    out.push_str("```\n\n## External interactions\n\n");
    out.push_str("- Provider: ");
    out.push_str(state.provider.as_deref().unwrap_or("unconfigured"));
    out.push_str("\n- Sandbox: ");
    out.push_str(&state.sandbox);
    out.push_str("\n\n## Cross-references\n\n");
    out.push_str("- Narrative: [`RUN-NARRATIVE.md`](./RUN-NARRATIVE.md)\n");
    out.push_str("- Decisions: [`RUN-DECISIONS.md`](./RUN-DECISIONS.md)\n");
    out.push_str("\n### Turn citations\n\n");
    out.push_str(&render_turn_citations(records));
    out
}

fn render_decisions(
    state: &PipelineState,
    fields: &FrontmatterFields,
    records: &[TurnRecord],
) -> String {
    let mut out = frontmatter(state, fields);
    out.push_str(&format!(
        "This document captures implementation decisions and spec interpretation for run `{}`.\n\n",
        short_id(&state.run_id)
    ));
    if implementation_notes_path(&state.working_dir).exists() {
        out.push_str(&format!(
            "Live working copy: [`{IMPLEMENTATION_NOTES_HTML}`](../../{IMPLEMENTATION_NOTES_HTML})\n\n"
        ));
    }
    let notes = implementation_note_markdown_sections(&state.working_dir);
    for (_, heading) in IMPLEMENTATION_NOTE_SECTIONS {
        out.push_str(&format!("## {heading}\n\n"));
        out.push_str(notes.get(heading).map(String::as_str).unwrap_or("None."));
        out.push_str("\n\n");
    }
    out.push_str("## Multi-alternative decision details\n\n");
    let decisions = records
        .iter()
        .filter(|record| record.decision_candidate)
        .collect::<Vec<_>>();
    if decisions.is_empty() {
        out.push_str("No multi-alternative decisions detected in this run.\n\n");
        out.push_str("## Turn citations\n\n");
        out.push_str(&render_turn_citations(records));
        return out;
    }
    for (idx, record) in decisions.into_iter().enumerate() {
        out.push_str(&format!(
            "### Decision {} - {} (turn {})\n\n",
            idx + 1,
            record.title,
            record.turn
        ));
        out.push_str("**Considered:** alternatives detected in provider reasoning\n");
        out.push_str(&format!("**Chose:** {}\n", record.title));
        out.push_str(&format!(
            "**Why:** {}\n",
            one_line(&record.response_text, 600)
        ));
        out.push_str(&format!(
            "**Trace:** [turn {}](../traces.jsonl)\n",
            record.turn
        ));
        let files = record.file_paths();
        if files.is_empty() {
            out.push_str("**Files affected:** none recorded\n\n");
        } else {
            out.push_str(&format!(
                "**Files affected:** {}\n\n",
                files
                    .iter()
                    .map(|file| format!("`{file}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    out.push_str("## Turn citations\n\n");
    out.push_str(&render_turn_citations(records));
    out
}

fn maybe_write_delta(state: &PipelineState, records: &[TurnRecord]) -> Result<()> {
    let files = all_files(records);
    if should_emit_delta(state, &files)? {
        let mut out = frontmatter(
            state,
            &FrontmatterFields {
                last_updated: Utc::now(),
                doc_writer: "templated only".to_string(),
                parent_run_id: parent_run_id(&state.working_dir),
            },
        );
        out.push_str(&format!(
            "> deadreckon proposes (run {}): update the project AS-BUILT for this change set.\n\n",
            short_id(&state.run_id)
        ));
        out.push_str("### Proposed new section: \"Run-produced changes\"\n\n");
        out.push_str(&format!(
            "> deadreckon proposes (run {}):\n>\n> This run changed: {}.\n",
            short_id(&state.run_id),
            files
                .iter()
                .map(|file| format!("`{file}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        write_file(delta_path(&state.working_dir), out)?;
    } else {
        let path = delta_path(&state.working_dir);
        if path.exists() {
            fs::remove_file(&path).with_path(&path)?;
        }
    }
    Ok(())
}

pub fn should_emit_delta(state: &PipelineState, files: &[String]) -> Result<bool> {
    let Ok(record) = read_codebase_record(&state.working_dir) else {
        return Ok(false);
    };
    if record.mode != CodebaseMode::Worktree {
        return Ok(false);
    }
    if !source_has_as_built(&state.working_dir, files) {
        return Ok(false);
    }
    if files.len() >= 3 {
        return Ok(true);
    }
    for file in files {
        let path = state.working_dir.join(file);
        if let Ok(raw) = fs::read_to_string(path)
            && (raw.contains("pub fn ") || raw.contains("pub struct ") || raw.contains("export "))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn source_has_as_built(working_dir: &Path, files: &[String]) -> bool {
    ["AS-BUILT-ARCHITECTURE.md", "AS-BUILT.md"]
        .iter()
        .any(|name| working_dir.join(name).exists())
        || files.iter().any(|file| {
            let parent = Path::new(file).parent().unwrap_or_else(|| Path::new(""));
            ["AS-BUILT-ARCHITECTURE.md", "AS-BUILT.md"]
                .iter()
                .any(|name| working_dir.join(parent).join(name).exists())
        })
}

fn title_from_goal(goal: &str) -> String {
    let first = goal.lines().next().unwrap_or("deadreckon run").trim();
    if first.is_empty() {
        "deadreckon run".to_string()
    } else {
        first.to_string()
    }
}

fn commit_or_working_lines(state: &PipelineState) -> Vec<String> {
    let Ok(record) = read_codebase_record(&state.working_dir) else {
        return Vec::new();
    };
    match record.mode {
        CodebaseMode::Worktree => {
            let base = record
                .base_sha
                .as_deref()
                .map(short_id)
                .unwrap_or_else(|| "-".to_string());
            let head = current_worktree_sha(state).unwrap_or_else(|| "-".to_string());
            let branch = record.branch_name.as_deref().unwrap_or("-");
            let commits = worktree_commit_count(state, record.base_sha.as_deref()).unwrap_or(0);
            let (adds, dels) = diff_numstat(state, record.base_sha.as_deref()).unwrap_or((0, 0));
            vec![format!(
                "**Commit span:** `{base}` ... `{head}` on `{branch}` ({commits} turn commits, +{adds}/-{dels} LoC)"
            )]
        }
        CodebaseMode::Copy => vec![format!("**Working dir:** {}", state.working_dir.display())],
        CodebaseMode::InPlace => vec![format!("**In-place:** {}", state.working_dir.display())],
        CodebaseMode::Fresh => Vec::new(),
    }
}

fn owner_line(state: &PipelineState) -> String {
    let owner = git_output(&state.working_dir, &["config", "user.name"])
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        "{} (with deadreckon + {})",
        owner.trim(),
        state.provider.as_deref().unwrap_or("unconfigured")
    )
}

fn spend_line(state: &PipelineState) -> String {
    if state
        .provider
        .as_deref()
        .is_some_and(|provider| provider.starts_with("cli:"))
    {
        format!("{:.0}s wall (subscription)", state.total_wall_seconds)
    } else {
        format!("${:.2}", state.total_spend_usd)
    }
}

fn parent_run_id(working_dir: &Path) -> Option<String> {
    let path = working_dir.join(".deadreckon/parent.json");
    let raw = fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    value
        .get("parent_run_id")
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

fn normalize_files(state: &PipelineState, files: &[PathBuf]) -> Vec<String> {
    let mut normalized = files
        .iter()
        .filter_map(|path| normalize_file(state, path))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    normalized.sort();
    normalized
}

fn normalize_file(state: &PipelineState, path: &Path) -> Option<String> {
    let relative = if path.is_absolute() {
        path.strip_prefix(&state.working_dir).ok()?.to_path_buf()
    } else {
        path.to_path_buf()
    };
    let normalized = relative.to_string_lossy().replace('\\', "/");
    is_documentable_path(&normalized).then_some(normalized)
}

pub fn is_documentable_path(file: &str) -> bool {
    let path = file.trim_start_matches("./").replace('\\', "/");
    if path.trim().is_empty() {
        return false;
    }
    let file_name = Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&path);
    let file_name_lower = file_name.to_ascii_lowercase();
    let path_lower = path.to_ascii_lowercase();
    if matches!(
        file_name_lower.as_str(),
        "run-narrative.md"
            | "run-as-built.md"
            | "run-decisions.md"
            | "as-built-delta.md"
            | "polish.json"
            | "traces.jsonl"
            | "provenance.jsonl"
            | "spend.jsonl"
            | ".ds_store"
            | "thumbs.db"
            | "ehthumbs.db"
            | "ehthumbs_vista.db"
            | "desktop.ini"
            | "cmakelists.txt.user"
            | "cmakecache.txt"
            | "cmake_install.cmake"
            | "install_manifest.txt"
            | "ctesttestfile.cmake"
            | "compile_commands.json"
            | "cmakeuserpresets.json"
            | "go.work.sum"
            | "package.resolved"
            | "package.pins"
            | "pubspec.lock"
            | "erl_crash.dump"
            | "pip-log.txt"
            | "pip-delete-this-directory.txt"
            | ".coverage"
            | "coverage.xml"
            | "nosetests.xml"
            | ".dmypy.json"
            | "dmypy.json"
            | ".pdm-python"
            | ".pypirc"
            | ".installed.cfg"
            | "manifest"
            | ".rhistory"
            | ".rdata"
            | ".ruserdata"
            | ".renviron"
            | ".httr-oauth"
            | "hs_err_pid"
            | "release.properties"
            | "dependency-reduced-pom.xml"
            | "buildnumber.properties"
            | "gradle-app.setting"
            | ".gradletasknamecache"
            | "pom.xml.tag"
            | "pom.xml.releasebackup"
            | "pom.xml.versionsbackup"
            | "pom.xml.next"
            | "scaffoldingreadme.txt"
            | "testresult.xml"
            | "composer.phar"
            | "composer.lock"
            | "gemfile.lock"
            | ".rvmrc"
            | "modules.order"
            | "module.symvers"
            | "mkfile.old"
            | "dkms.conf"
            | ".flutter-plugins"
            | ".flutter-plugins-dependencies"
            | ".last_build_id"
            | "servicedefinitions.json"
            | ".jekyll-metadata"
            | ".terraform.tfstate.lock.info"
            | ".terraformrc"
            | "terraform.rc"
            | ".env"
            | ".envrc"
            | ".vault_pass"
            | "vault_password_file"
    ) {
        return false;
    }
    let generated_suffixes = [
        ".map",
        ".css.map",
        ".sass.map",
        ".scss.map",
        ".tsbuildinfo",
        ".pyc",
        ".pyo",
        ".pyd",
        ".log",
        ".tmp",
        ".temp",
        ".cache",
        ".swp",
        ".swo",
        ".swn",
        ".bak",
        ".pid",
        ".pid.lock",
        ".lcov",
        ".node",
        ".class",
        ".jar",
        ".war",
        ".nar",
        ".ear",
        ".ctxt",
        ".tasty",
        ".o",
        ".obj",
        ".ko",
        ".lo",
        ".slo",
        ".gch",
        ".pch",
        ".ilk",
        ".pdb",
        ".so",
        ".dylib",
        ".dll",
        ".lai",
        ".la",
        ".a",
        ".lib",
        ".dwo",
        ".exe",
        ".out",
        ".app",
        ".elf",
        ".exp",
        ".hex",
        ".su",
        ".idb",
        ".smod",
        ".nupkg",
        ".snupkg",
        ".binlog",
        ".trx",
        ".beam",
        ".ez",
        ".tfstate",
        ".tfvars",
        ".tfvars.json",
        ".pkrvars.hcl",
        ".box",
    ];
    if generated_suffixes
        .iter()
        .any(|suffix| path_lower.ends_with(suffix))
        || file_name_lower.ends_with('~')
        || file_name_lower.starts_with("npm-debug.log")
        || file_name_lower.starts_with("yarn-debug.log")
        || file_name_lower.starts_with("yarn-error.log")
        || file_name_lower.starts_with("lerna-debug.log")
        || file_name_lower.starts_with("report.") && file_name_lower.ends_with(".json")
        || file_name_lower.starts_with("hs_err_pid")
        || file_name_lower.starts_with("replay_pid")
        || file_name_lower.starts_with("rustc-ice-") && file_name_lower.ends_with(".txt")
        || file_name_lower.starts_with("vite.config.") && file_name_lower.contains(".timestamp-")
        || file_name_lower.starts_with(".pnp.")
        || file_name_lower.starts_with("app.") && file_name_lower.ends_with(".symbols")
        || file_name_lower.starts_with("flutter_") && file_name_lower.ends_with(".png")
        || file_name_lower.starts_with("crash.") && file_name_lower.ends_with(".log")
        || file_name_lower.starts_with(".env.")
        || file_name_lower.ends_with(".tmp")
        || file_name_lower.ends_with(".tar.gz")
        || file_name_lower.ends_with(".dsym.zip")
    {
        return false;
    }
    let generated_segments = [
        ".deadreckon",
        ".stoa",
        ".git",
        ".hg",
        ".svn",
        ".bzr",
        "cvs",
        ".specstory",
        ".claude",
        ".next",
        ".nuxt",
        ".svelte-kit",
        ".astro",
        ".output",
        ".docusaurus",
        ".vuepress",
        ".remix",
        ".turbo",
        ".vite",
        ".vitepress",
        ".cache",
        ".parcel-cache",
        ".sass-cache",
        ".jekyll-cache",
        "node_modules",
        "jspm_packages",
        "web_modules",
        "bower_components",
        ".npm",
        ".pnpm-store",
        ".pnp",
        ".yarn",
        ".nyc_output",
        ".grunt",
        ".serverless",
        ".fusebox",
        ".dynamodb",
        ".firebase",
        "vendor",
        "pods",
        "carthage",
        "dist",
        "build",
        "codecoverage",
        "testresults",
        "benchmarkdotnet.artifacts",
        "out",
        "coverage",
        "cover",
        "htmlcov",
        "target",
        "downloads",
        "eggs",
        ".eggs",
        "sdist",
        "wheels",
        "develop-eggs",
        "__pypackages__",
        "__pycache__",
        ".pytest_cache",
        ".mypy_cache",
        ".ruff_cache",
        ".nox",
        ".tox",
        ".hypothesis",
        ".pybuilder",
        ".pdm-build",
        ".pixi",
        ".pyre",
        ".pytype",
        ".webassets-cache",
        ".scrapy",
        "cython_debug",
        ".venv",
        "venv",
        "env",
        "env.bak",
        "venv.bak",
        ".ipynb_checkpoints",
        ".virtual_documents",
        ".gradle",
        ".kotlin",
        ".bsp",
        ".bloop",
        ".metals",
        ".mtj.tmp",
        "cmakefiles",
        "cmakescripts",
        "_deps",
        "vcpkg_installed",
        ".tmp_versions",
        ".phpunit.result.cache",
        ".phpactor.json",
        ".vs",
        ".yardoc",
        "_yardoc",
        ".bundle",
        ".build",
        ".swiftpm",
        "deriveddata",
        "xcuserdata",
        ".dart_tool",
        ".pub",
        ".pub-cache",
        ".buildlog",
        ".history",
        "ephemeral",
        "flutter_assets",
        ".eunit",
        ".concrete",
        ".rebar",
        ".rebar3",
        "_build",
        "_checkouts",
        "deps",
        ".elixir_ls",
        "_site",
        ".terraform",
        ".ansible",
        ".pulumi",
        "packer_cache",
        ".vagrant",
        "cdk.out",
        ".cdk.staging",
        "tmp",
        "temp",
    ];
    if path.split('/').any(|segment| {
        let lower = segment.to_ascii_lowercase();
        generated_segments.contains(&lower.as_str())
            || lower.starts_with("cmake-build-")
            || lower.starts_with("build-")
            || lower.ends_with(".egg-info")
            || lower.ends_with(".rs.bk")
            || lower.starts_with("mutants.out")
            || lower.ends_with(".dsym")
            || lower.ends_with(".framework")
    }) {
        return false;
    }
    if path_lower.contains("/.vitepress/dist/")
        || path_lower.contains("/.vitepress/cache/")
        || path_lower.contains("/docs/_build/")
        || path_lower.contains("/vendor/bundle/")
        || path_lower.contains("/test/tmp/")
        || path_lower.starts_with("fastlane/test_output/")
        || path_lower.contains("/fastlane/test_output/")
        || path_lower.contains("/generated_plugin_registrant.")
        || path_lower.contains("/generated_plugins.cmake")
    {
        return false;
    }
    true
}

fn diff_sample_for_file(relative: &str, previous: &Path, current: &Path) -> Result<FileChange> {
    if current.exists() && is_binary_file(current)? {
        return Ok(FileChange {
            path: relative.to_string(),
            adds: 0,
            dels: 0,
            largest_hunk_excerpt: String::new(),
            is_new: !previous.exists(),
            is_binary: true,
        });
    }
    let before = fs::read_to_string(previous).unwrap_or_default();
    let after = fs::read_to_string(current).unwrap_or_default();
    let before_lines = before.lines().collect::<Vec<_>>();
    let after_lines = after.lines().collect::<Vec<_>>();
    let common_prefix = before_lines
        .iter()
        .zip(after_lines.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let common_suffix = before_lines[common_prefix..]
        .iter()
        .rev()
        .zip(after_lines[common_prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();
    let before_end = before_lines.len().saturating_sub(common_suffix);
    let after_end = after_lines.len().saturating_sub(common_suffix);
    let removed = &before_lines[common_prefix..before_end];
    let added = &after_lines[common_prefix..after_end];
    let adds = added.len() as u32;
    let dels = removed.len() as u32;
    let old_start = common_prefix + 1;
    let new_start = common_prefix + 1;
    let header = format!(
        "@@ -{},{} +{},{} @@",
        old_start.max(1),
        removed.len(),
        new_start.max(1),
        added.len()
    );
    let mut excerpt_lines = vec![header];
    for line in removed.iter().take(5) {
        excerpt_lines.push(format!("-{line}"));
    }
    for line in added
        .iter()
        .take(5usize.saturating_sub(excerpt_lines.len().saturating_sub(1)))
    {
        excerpt_lines.push(format!("+{line}"));
    }
    Ok(FileChange {
        path: relative.to_string(),
        adds,
        dels,
        largest_hunk_excerpt: if adds == 0 && dels == 0 {
            String::new()
        } else {
            excerpt_lines.join("\n")
        },
        is_new: !previous.exists() && current.exists(),
        is_binary: false,
    })
}

fn is_binary_file(path: &Path) -> Result<bool> {
    match fs::read(path) {
        Ok(bytes) => Ok(bytes.contains(&0) || std::str::from_utf8(&bytes).is_err()),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn all_files(records: &[TurnRecord]) -> Vec<String> {
    records
        .iter()
        .flat_map(TurnRecord::file_paths)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn high_level_approach(records: &[TurnRecord]) -> String {
    if records.is_empty() {
        return "No completed turns have been recorded yet.".to_string();
    }
    let summaries = records
        .iter()
        .take(3)
        .map(|record| {
            let summary = if record.response_summary.trim().is_empty() {
                capture_response_summary(&record.outcome)
            } else {
                record.response_summary.clone()
            };
            format!(
                "turn {} used `{}` and reported {}",
                record.turn, record.tool_kind, summary
            )
        })
        .collect::<Vec<_>>();
    if summaries.is_empty() {
        return "The run has not yet mutated files.".to_string();
    }
    format!(
        "The run advanced through {}. The sections below prioritize the user-authored files that matter for operating and maintaining the result; trace, snapshot, provenance, and diff evidence are linked at the end for audit.",
        summaries.join("; ")
    )
}

fn phase_paragraph(phase: &Phase) -> String {
    let summaries = phase
        .turns
        .iter()
        .map(|turn| {
            let summary = if turn.response_summary.trim().is_empty() {
                capture_response_summary(&turn.outcome)
            } else {
                turn.response_summary.clone()
            };
            format!(
                "turn {} used `{}` to {}",
                turn.turn, turn.tool_kind, summary
            )
        })
        .collect::<Vec<_>>();
    let files = if phase.files.is_empty() {
        "no files".to_string()
    } else {
        phase
            .files
            .iter()
            .map(|file| format!("`{file}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    if summaries.is_empty() {
        format!(
            "Turns {}-{} recorded no provider summary but touched {files}.",
            phase.start_turn, phase.end_turn
        )
    } else {
        format!(
            "Turns {}-{} focused on {files}. {}.",
            phase.start_turn,
            phase.end_turn,
            summaries.join("; ")
        )
    }
}

fn phase_file_changes(phase: &Phase) -> Vec<FileChange> {
    let mut by_path = BTreeMap::<String, FileChange>::new();
    for turn in &phase.turns {
        for file in &turn.files {
            if !is_documentable_path(&file.path) {
                continue;
            }
            by_path
                .entry(file.path.clone())
                .and_modify(|existing| {
                    existing.adds += file.adds;
                    existing.dels += file.dels;
                    if existing.largest_hunk_excerpt.lines().count()
                        < file.largest_hunk_excerpt.lines().count()
                    {
                        existing.largest_hunk_excerpt = file.largest_hunk_excerpt.clone();
                    }
                    existing.is_new |= file.is_new;
                    existing.is_binary |= file.is_binary;
                })
                .or_insert_with(|| file.clone());
        }
    }
    by_path.into_values().collect()
}

fn hunk_inline(excerpt: &str) -> String {
    if excerpt.trim().is_empty() {
        "no textual hunk captured".to_string()
    } else {
        format!(
            "`{}`",
            excerpt
                .lines()
                .take(3)
                .map(str::trim)
                .collect::<Vec<_>>()
                .join(" / ")
                .replace('|', "\\|")
        )
    }
}

fn open_threads(records: &[TurnRecord]) -> Vec<String> {
    let markers = [
        "TODO",
        "out of scope",
        "follow-up",
        "noted but not implemented",
    ];
    let mut threads = BTreeSet::new();
    for record in records {
        for line in record.response_full.lines() {
            if markers.iter().any(|marker| {
                line.to_ascii_lowercase()
                    .contains(&marker.to_ascii_lowercase())
            }) {
                threads.insert(format!("turn {}: {}", record.turn, one_line(line, 180)));
            }
        }
    }
    threads.into_iter().collect()
}

#[derive(Debug, Clone)]
struct ComponentRow {
    layer: String,
    responsibility: String,
    entrypoint: String,
}

fn component_rows(records: &[TurnRecord]) -> Vec<ComponentRow> {
    let mut rows = BTreeMap::<String, ComponentRow>::new();
    for record in records {
        for file in &record.files {
            if !is_documentable_path(&file.path) {
                continue;
            }
            let Some(layer) = layer_for_path(&file.path) else {
                continue;
            };
            rows.entry(format!("{layer}:{}", file.path))
                .or_insert_with(|| ComponentRow {
                    layer: layer.clone(),
                    responsibility: responsibility_for_layer(&layer).to_string(),
                    entrypoint: entrypoint_for_change(file),
                });
        }
    }
    rows.into_values().collect()
}

fn layer_for_path(file: &str) -> Option<String> {
    let path = Path::new(file);
    let first = file.split('/').next().unwrap_or(file);
    if let Some(crate_name) = file
        .strip_prefix("crates/")
        .and_then(|rest| rest.split('/').next())
        .filter(|name| !name.is_empty())
    {
        return Some(format!("Crate {crate_name} (Rust)"));
    }
    if matches!(file, "Cargo.toml" | "Cargo.lock") {
        return Some("Workspace manifest".to_string());
    }
    if let Some(component) = file
        .strip_prefix("src/components/")
        .and_then(|rest| rest.split('/').next())
        .filter(|name| !name.is_empty())
    {
        return Some(format!("Frontend component ({component})"));
    }
    if matches!(first, "src" | "app" | "pages")
        && (file.starts_with("src/pages/")
            || file.starts_with("src/routes/")
            || matches!(first, "app" | "pages"))
    {
        return Some("Frontend route".to_string());
    }
    if file.contains(".test.") || first == "tests" || first == "__tests__" {
        return Some("Tests".to_string());
    }
    if first == "docs" || path.parent().is_none() && file.ends_with(".md") {
        return Some("Documentation".to_string());
    }
    if first == "migrations" || file.ends_with(".sql") {
        return Some("Database migration".to_string());
    }
    if file.starts_with(".github/workflows/") {
        return Some("CI".to_string());
    }
    if matches!(file, "Makefile" | "Justfile") {
        return Some("Build script".to_string());
    }
    if matches!(file, "package.json" | "pnpm-lock.yaml" | "yarn.lock") {
        return Some("Frontend manifest".to_string());
    }
    if file == "pyproject.toml" || file.starts_with("requirements") && file.ends_with(".txt") {
        return Some("Python manifest".to_string());
    }
    if matches!(file, "go.mod" | "go.sum") {
        return Some("Go module".to_string());
    }
    None
}

fn responsibility_for_layer(layer: &str) -> &'static str {
    if layer.starts_with("Crate ") {
        "Rust crate touched by this run"
    } else if layer.starts_with("Frontend component") {
        "User-facing component implementation"
    } else if layer == "Frontend route" {
        "Routable frontend surface"
    } else if layer == "Tests" {
        "Verification surface"
    } else if layer == "Documentation" {
        "Project documentation and run handoff"
    } else if layer.contains("manifest") {
        "Dependency, script, or workspace metadata"
    } else {
        "Changed subsystem surface"
    }
}

fn entrypoint_for_change(file: &FileChange) -> String {
    let line = largest_hunk_new_line(&file.largest_hunk_excerpt).unwrap_or(1);
    format!("{}:{line}", file.path)
}

fn topology_for_files(working_dir: &Path, files: &[String]) -> Option<String> {
    let dirs = files
        .iter()
        .filter_map(|file| file.split_once('/').map(|(dir, _)| dir))
        .filter(|dir| !dir.is_empty())
        .collect::<BTreeSet<_>>();
    if dirs.len() < 3 {
        return None;
    }
    let dirs = dirs.into_iter().take(6).collect::<Vec<_>>();
    let mut out = String::new();
    for dir in &dirs {
        out.push_str("+-----------+   ");
        let _ = dir;
    }
    out.push('\n');
    for dir in &dirs {
        out.push_str(&format!("| {:<9} |-->", format!("{dir}/")));
    }
    out.push('\n');
    for _ in &dirs {
        out.push_str("+-----------+   ");
    }
    out.push('\n');
    for (left_idx, left) in dirs.iter().enumerate() {
        for right in dirs.iter().skip(left_idx + 1) {
            if directory_mentions(working_dir, left, right) {
                out.push_str(&format!("{left}/ -> {right}/\n"));
            }
            if directory_mentions(working_dir, right, left) {
                out.push_str(&format!("{right}/ -> {left}/\n"));
            }
        }
    }
    Some(out.trim_end().to_string())
}

fn directory_mentions(working_dir: &Path, left: &str, right: &str) -> bool {
    let root = working_dir.join(left);
    if !root.is_dir() {
        return false;
    }
    let needle = format!("{right}/");
    inventory_files(&root)
        .unwrap_or_default()
        .into_iter()
        .take(200)
        .any(|path| fs::read_to_string(path).is_ok_and(|raw| raw.contains(&needle)))
}

fn current_worktree_sha(state: &PipelineState) -> Option<String> {
    let Ok(record) = read_codebase_record(&state.working_dir) else {
        return None;
    };
    if record.mode != CodebaseMode::Worktree {
        return None;
    }
    git_output(&state.working_dir, &["rev-parse", "--short", "HEAD"])
        .ok()
        .map(|sha| sha.trim().to_string())
        .filter(|sha| !sha.is_empty())
}

fn worktree_commit_count(state: &PipelineState, base: Option<&str>) -> Result<usize> {
    let Some(base) = base else {
        return Ok(0);
    };
    let output = git_output(
        &state.working_dir,
        &["rev-list", "--count", &format!("{base}..HEAD")],
    )?;
    Ok(output.trim().parse::<usize>().unwrap_or(0))
}

fn diff_numstat(state: &PipelineState, base: Option<&str>) -> Result<(u64, u64)> {
    let Some(base) = base else {
        return Ok((0, 0));
    };
    let output = git_output(
        &state.working_dir,
        &["diff", "--numstat", &format!("{base}..HEAD")],
    )?;
    let mut adds = 0;
    let mut dels = 0;
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        adds += parts
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        dels += parts
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
    }
    Ok((adds, dels))
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = run_git(cwd, args)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(DeadreckonError::InvalidInput(format!(
            "git {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

fn git_status(cwd: &Path, args: &[&str]) -> Result<()> {
    let output = run_git(cwd, args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(DeadreckonError::InvalidInput(format!(
            "git {:?} failed: {}{}",
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

fn commit_docs_if_worktree(state: &PipelineState) -> Result<()> {
    let Ok(record) = read_codebase_record(&state.working_dir) else {
        return Ok(());
    };
    if record.mode != CodebaseMode::Worktree {
        return Ok(());
    }
    git_status(
        &state.working_dir,
        &["config", "user.email", "deadreckon@example.invalid"],
    )?;
    git_status(&state.working_dir, &["config", "user.name", "deadreckon"])?;
    git_status(&state.working_dir, &["add", "docs"])?;
    if git_quiet(&state.working_dir, &["diff", "--cached", "--quiet"])? {
        return Ok(());
    }
    git_status(
        &state.working_dir,
        &[
            "commit",
            "-m",
            &format!(
                "turn docs: deadreckon run docs for {}",
                state.run_id.chars().take(8).collect::<String>()
            ),
        ],
    )
}

fn git_quiet(cwd: &Path, args: &[&str]) -> Result<bool> {
    let output = run_git(cwd, args)?;
    Ok(output.status.success())
}

fn mirror_trace_file(state: &PipelineState) -> Result<()> {
    let metadata_dir = state.working_dir.join(".deadreckon");
    fs::create_dir_all(&metadata_dir).with_path(&metadata_dir)?;
    for name in ["traces.jsonl", "provenance.jsonl", "spend.jsonl"] {
        let source = state.run_root.join(name);
        if source.exists() {
            fs::copy(&source, metadata_dir.join(name)).with_path(&source)?;
        }
    }
    Ok(())
}

fn write_file(path: PathBuf, content: String) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_path(parent)?;
    }
    fs::write(&path, content).with_path(path)
}

fn file_overlap(current: &[TurnRecord], turn: &TurnRecord) -> f64 {
    let turn_files = turn.file_paths();
    if turn_files.is_empty() {
        return 0.0;
    }
    let current_files = current
        .iter()
        .flat_map(TurnRecord::file_paths)
        .collect::<BTreeSet<_>>();
    let overlap = turn_files
        .iter()
        .filter(|file| current_files.contains(*file))
        .count();
    overlap as f64 / turn_files.len() as f64
}

fn same_tool_kind_consecutive_count(current: &[TurnRecord], tool_kind: &str) -> usize {
    current
        .iter()
        .rev()
        .take_while(|record| record.tool_kind == tool_kind)
        .count()
}

fn smallest_neighbor_index(groups: &[Vec<TurnRecord>]) -> usize {
    groups
        .windows(2)
        .enumerate()
        .min_by_key(|(_, pair)| pair[0].len() + pair[1].len())
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn phase_from_group(index: usize, turns: Vec<TurnRecord>) -> Phase {
    let title = turns
        .first()
        .map(|turn| turn.title.clone())
        .unwrap_or_else(|| format!("Phase {index}"));
    let start_turn = turns.first().map(|turn| turn.turn).unwrap_or(0);
    let end_turn = turns.last().map(|turn| turn.turn).unwrap_or(start_turn);
    let commit_sha = turns.iter().find_map(|turn| turn.commit_sha.clone());
    let files = turns
        .iter()
        .flat_map(TurnRecord::file_paths)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Phase {
        index,
        title,
        start_turn,
        end_turn,
        commit_sha,
        files,
        turns,
    }
}

#[allow(
    clippy::expect_used,
    reason = "BUG-tagged static regex invariant; tests compile the patterns explicitly"
)]
fn ill_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(ill_regex_pattern()).expect("BUG: auto-title regex pattern must compile")
    })
}

fn ill_regex_pattern() -> &'static str {
    ILL_REGEX_PATTERN
}

#[allow(
    clippy::expect_used,
    reason = "BUG-tagged static regex invariant; tests compile the patterns explicitly"
)]
fn decision_markers() -> &'static [Regex] {
    static REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();
    REGEXES
        .get_or_init(|| {
            decision_marker_patterns()
                .iter()
                .map(|pattern| {
                    Regex::new(pattern).expect("BUG: decision marker regex pattern must compile")
                })
                .collect()
        })
        .as_slice()
}

fn decision_marker_patterns() -> &'static [&'static str] {
    DECISION_MARKER_PATTERNS
}

fn title_case_words(input: &str, max_words: usize) -> String {
    let words = input
        .split_whitespace()
        .take(max_words)
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>();
    if words.is_empty() {
        input.to_string()
    } else {
        words.join(" ")
    }
}

fn short_id(value: &str) -> String {
    value.chars().take(8).collect()
}

fn one_line(value: &str, limit: usize) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= limit {
        collapsed
    } else {
        format!("{}...", collapsed.chars().take(limit).collect::<String>())
    }
}

fn cap_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    value[..end].to_string()
}

fn cap_on_word_boundary(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out = value.chars().take(max_chars).collect::<String>();
    if let Some(idx) = out.rfind(char::is_whitespace) {
        out.truncate(idx);
    }
    out.trim_end_matches(|ch: char| ch.is_ascii_punctuation() && ch != '.')
        .trim()
        .to_string()
}

fn largest_hunk_new_line(excerpt: &str) -> Option<usize> {
    let header = excerpt.lines().next()?.trim();
    let plus = header
        .split_whitespace()
        .find(|part| part.starts_with('+'))?;
    plus.trim_start_matches('+')
        .split(',')
        .next()?
        .parse::<usize>()
        .ok()
}

pub fn apply_commit_body(state: &PipelineState) -> String {
    let narrative = doc_path_for_kind(&state.working_dir, DocKind::Narrative)
        .and_then(|path| fs::read_to_string(path).ok())
        .unwrap_or_default();
    let summary = section_first_paragraph(&narrative, "High-level approach")
        .or_else(|| section_first_paragraph(&narrative, "Goal"))
        .unwrap_or_else(|| "deadreckon produced this run and recorded its trace.".to_string());
    let phases = narrative
        .lines()
        .filter(|line| line.starts_with("### Phase "))
        .map(|line| {
            let title = line
                .trim_start_matches("### ")
                .split(" (")
                .next()
                .unwrap_or(line);
            format!("- {title}")
        })
        .collect::<Vec<_>>();
    let decisions = fs::read_to_string(
        doc_path_for_kind(&state.working_dir, DocKind::Decisions)
            .unwrap_or_else(|| decisions_path(&state.working_dir)),
    )
    .ok()
    .map(|raw| {
        raw.lines()
            .filter(|line| line.starts_with("## Decision "))
            .count()
    })
    .unwrap_or(0);
    let mut out = String::new();
    out.push_str(&summary);
    out.push_str("\n\nPhases:\n");
    if phases.is_empty() {
        out.push_str("- Phase 1: run completed\n");
    } else {
        for phase in phases {
            out.push_str(&phase);
            out.push('\n');
        }
    }
    out.push_str(&format!(
        "\nDecisions: {decisions} (see docs/RUN-DECISIONS.md)\n"
    ));
    out.push_str("Open threads: 0 (see docs/RUN-NARRATIVE.md#open-threads)\n\n");
    out.push_str("Generated by deadreckon. Trace: docs/RUN-NARRATIVE.md");
    out
}

fn section_first_paragraph(markdown: &str, header: &str) -> Option<String> {
    let marker = format!("## {header}");
    let mut in_section = false;
    let mut paragraph = Vec::new();
    for line in markdown.lines() {
        if line.trim() == marker {
            in_section = true;
            continue;
        }
        if in_section && line.starts_with("## ") {
            break;
        }
        if in_section {
            if line.trim().is_empty() {
                if !paragraph.is_empty() {
                    break;
                }
            } else {
                paragraph.push(line.trim());
            }
        }
    }
    if paragraph.is_empty() {
        None
    } else {
        Some(paragraph.join(" "))
    }
}

pub fn docs_inventory(working_dir: &Path) -> Result<BTreeMap<String, u64>> {
    let mut inventory = BTreeMap::new();
    for root in [docs_dir(working_dir), public_docs_dir(working_dir)] {
        if !root.exists() {
            continue;
        }
        for path in inventory_files(&root)? {
            if let Ok(relative) = path.strip_prefix(working_dir) {
                inventory.insert(
                    relative.to_string_lossy().replace('\\', "/"),
                    fs::metadata(&path)
                        .map(|metadata| metadata.len())
                        .unwrap_or(0),
                );
            }
        }
    }
    Ok(inventory)
}

pub fn copy_public_docs_from_internal(working_dir: &Path) -> Result<()> {
    let internal = docs_dir(working_dir);
    let public = public_docs_dir(working_dir);
    if internal.exists() {
        copy_tree(&internal, &public)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use regex::Regex;

    #[test]
    fn docs_regex_patterns_compile() {
        Regex::new(super::ill_regex_pattern()).expect("auto-title regex should compile");
        for pattern in super::decision_marker_patterns() {
            Regex::new(pattern).expect("decision marker regex should compile");
        }
    }
}
