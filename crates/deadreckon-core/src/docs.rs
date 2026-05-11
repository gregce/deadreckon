use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::artifacts::{TraceRecord, append_trace, copy_tree, inventory_files};
use crate::codebase::{CodebaseMode, read_codebase_record};
use crate::error::{DeadreckonError, IoContext, Result};
use crate::state::{PipelineState, append_json_line};

pub const DOCS_DIR: &str = ".deadreckon/docs";
pub const PUBLIC_DOCS_DIR: &str = "docs";
pub const RUN_NARRATIVE: &str = "RUN-NARRATIVE.md";
pub const RUN_AS_BUILT: &str = "RUN-AS-BUILT.md";
pub const RUN_DECISIONS: &str = "RUN-DECISIONS.md";
pub const AS_BUILT_DELTA: &str = "AS-BUILT-DELTA.md";
pub const INCREMENTAL_JSONL: &str = "_incremental.jsonl";
pub const POLISH_JSON: &str = "polish.json";

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnRecord {
    pub turn: u32,
    pub title: String,
    pub tool_kind: String,
    pub latency_ms: Option<u128>,
    pub files: Vec<String>,
    pub outcome: String,
    pub trace_link: String,
    pub snapshot_link: String,
    pub commit_sha: Option<String>,
    pub response_text: String,
    pub decision_candidate: bool,
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

pub fn public_doc_path(working_dir: &Path, file_name: &str) -> PathBuf {
    public_docs_dir(working_dir).join(file_name)
}

pub fn ensure_docs_started(state: &PipelineState) -> Result<()> {
    let dir = docs_dir(&state.working_dir);
    fs::create_dir_all(&dir).with_path(&dir)?;
    let incremental = incremental_path(&state.working_dir);
    if !incremental.exists() {
        fs::write(&incremental, "").with_path(&incremental)?;
    }
    rewrite_templated_docs(state, "templated only")
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
    lines.push(format!("**Status:** {} (alpha)", state.status));
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
    let files = normalize_files(state, &input.files);
    let title = auto_title(&input.response_text, &input.tool_kind, &files, input.turn);
    let record = TurnRecord {
        turn: input.turn,
        title,
        tool_kind: input.tool_kind,
        latency_ms: input.latency_ms,
        files,
        outcome: input.outcome,
        trace_link: format!("../traces.jsonl#turn-{}", input.turn),
        snapshot_link: format!("../../snapshots/turn-{}/", input.turn),
        commit_sha: current_worktree_sha(state)?,
        decision_candidate: is_decision_candidate(&input.response_text),
        response_text: input.response_text,
    };
    append_json_line(&incremental_path(&state.working_dir), &record)?;
    rewrite_templated_docs(state, "templated only")?;
    Ok(record)
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
    let internal = docs_dir(&state.working_dir);
    if !internal.is_dir() {
        return Ok(());
    }
    let public = public_docs_dir(&state.working_dir);
    fs::create_dir_all(&public).with_path(&public)?;
    for name in [RUN_NARRATIVE, RUN_AS_BUILT, RUN_DECISIONS, AS_BUILT_DELTA] {
        let source = internal.join(name);
        if source.exists() {
            fs::copy(&source, public.join(name)).with_path(&source)?;
        }
    }
    if delta_path(&state.working_dir).exists() {
        let named = format!("AS-BUILT-DELTA-{}.md", short_id(&state.run_id));
        fs::copy(delta_path(&state.working_dir), public.join(named))
            .with_path(delta_path(&state.working_dir))?;
    }
    mirror_trace_file(state)?;
    commit_docs_if_worktree(state)?;
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
        .flat_map(|record| record.files.iter().cloned())
        .filter(|file| !file.starts_with(".deadreckon/") && !file.starts_with("docs/"))
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
    out.push_str("This run records unattended coding work in a durable form so the changed files, tool activity, and acceptance evidence can be reviewed without replaying raw traces.\n\n");
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
            let files = if phase.files.is_empty() {
                "no file changes recorded".to_string()
            } else {
                phase
                    .files
                    .iter()
                    .map(|file| format!("`{file}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            out.push_str(&format!(
                "- Turns {}-{} changed {}.\n",
                phase.start_turn, phase.end_turn, files
            ));
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
            .flat_map(|record| record.files.iter())
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
    out.push_str("- No open threads recorded by deadreckon.\n\n");
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
            current_worktree_sha(state)
                .ok()
                .flatten()
                .unwrap_or_else(|| "-".to_string())
        ));
    }
    out.push_str("- Acceptance: `proofs/turn-acceptance.json`\n");
    out
}

fn render_turn_section(record: &TurnRecord) -> String {
    let mut out = String::new();
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
    if record.files.is_empty() {
        out.push_str("- Files: none recorded\n");
    } else {
        out.push_str(&format!(
            "- Files: {}\n",
            record
                .files
                .iter()
                .map(|file| format!("`{file}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
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

fn render_as_built(
    state: &PipelineState,
    fields: &FrontmatterFields,
    records: &[TurnRecord],
) -> String {
    let mut out = frontmatter(state, fields);
    out.push_str(&format!(
        "**Subject:** deadreckon run `{}` changed the files listed below.\n\n",
        short_id(&state.run_id)
    ));
    out.push_str("This document describes the subsystem changed by this run. For chronology, see [`RUN-NARRATIVE.md`](./RUN-NARRATIVE.md).\n\n");
    out.push_str("## System overview\n\n");
    out.push_str(&high_level_approach(records));
    out.push_str("\n\n## Components (changed in this run)\n\n");
    out.push_str("| Layer | Responsibilities | Key entrypoints |\n| --- | --- | --- |\n");
    let files = all_files(records);
    if files.is_empty() {
        out.push_str("| Working tree | No files recorded yet | - |\n");
    } else {
        for file in &files {
            out.push_str(&format!(
                "| {} | Changed during run `{}` | `{}` |\n",
                layer_for_file(file),
                short_id(&state.run_id),
                file
            ));
        }
    }
    out.push_str("\n## Process / data flow\n\n");
    out.push_str("```text\nGoal -> provider turn -> tool call -> snapshot/provenance -> docs -> gate -> promotion\n```\n\n");
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
        "This document captures meaningful decisions made during run `{}`.\n\n",
        short_id(&state.run_id)
    ));
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
            "## Decision {} - {} (turn {})\n\n",
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
        if record.files.is_empty() {
            out.push_str("**Files affected:** none recorded\n\n");
        } else {
            out.push_str(&format!(
                "**Files affected:** {}\n\n",
                record
                    .files
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
    let words = first.split_whitespace().take(10).collect::<Vec<_>>();
    if words.is_empty() {
        "deadreckon run".to_string()
    } else {
        words.join(" ")
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
            let head = current_worktree_sha(state)
                .ok()
                .flatten()
                .unwrap_or_else(|| "-".to_string());
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
        .filter_map(|path| {
            path.strip_prefix(&state.working_dir)
                .ok()
                .map(Path::to_path_buf)
                .or_else(|| Some(path.clone()))
        })
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .filter(|path| {
            !path.starts_with(".deadreckon/docs/")
                && !path.starts_with("docs/RUN-")
                && !path.ends_with(POLISH_JSON)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    normalized.sort();
    normalized
}

fn all_files(records: &[TurnRecord]) -> Vec<String> {
    records
        .iter()
        .flat_map(|record| record.files.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn high_level_approach(records: &[TurnRecord]) -> String {
    if records.is_empty() {
        return "No completed turns have been recorded yet.".to_string();
    }
    let mut parts = records
        .iter()
        .take(3)
        .map(|record| format!("turn {} used {}", record.turn, record.tool_kind))
        .collect::<Vec<_>>();
    if parts.is_empty() {
        parts.push("the run has not yet mutated files".to_string());
    }
    format!(
        "deadreckon progressed through {} and tracked the resulting files, traces, snapshots, and provenance.",
        parts.join(", ")
    )
}

fn layer_for_file(file: &str) -> &'static str {
    if file.ends_with(".rs") {
        "Rust"
    } else if file.ends_with(".js") || file.ends_with(".ts") {
        "Frontend/runtime"
    } else if file.ends_with(".md") {
        "Documentation"
    } else if file.contains("test") {
        "Tests"
    } else {
        "Project files"
    }
}

fn current_worktree_sha(state: &PipelineState) -> Result<Option<String>> {
    let Ok(record) = read_codebase_record(&state.working_dir) else {
        return Ok(None);
    };
    if record.mode != CodebaseMode::Worktree {
        return Ok(None);
    }
    Ok(
        git_output(&state.working_dir, &["rev-parse", "--short", "HEAD"])
            .ok()
            .map(|sha| sha.trim().to_string())
            .filter(|sha| !sha.is_empty()),
    )
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
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|source| DeadreckonError::Io {
            path: PathBuf::from("git"),
            source,
        })?;
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
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|source| DeadreckonError::Io {
            path: PathBuf::from("git"),
            source,
        })?;
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
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(|source| DeadreckonError::Io {
            path: PathBuf::from("git"),
            source,
        })?;
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
    if turn.files.is_empty() {
        return 0.0;
    }
    let current_files = current
        .iter()
        .flat_map(|record| record.files.iter())
        .collect::<BTreeSet<_>>();
    let overlap = turn
        .files
        .iter()
        .filter(|file| current_files.contains(file))
        .count();
    overlap as f64 / turn.files.len() as f64
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
        .flat_map(|turn| turn.files.iter().cloned())
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

fn ill_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?i)\bi['’]?ll\s+([a-z][a-z0-9_-]*(?:\s+[a-z0-9_./-]+){0,5})")
            .expect("valid auto-title regex")
    })
}

fn decision_markers() -> &'static [Regex] {
    static REGEXES: OnceLock<Vec<Regex>> = OnceLock::new();
    REGEXES
        .get_or_init(|| {
            [
                r"(?i)\b(let me consider|let me think|i'll go with|i'll choose)\b",
                r"(?i)\b(option [123]|alternatives?:|either .* or)\b",
                r"(?i)\b(instead of|rather than|actually,?\s*let)\b",
                r"(?i)\bdecision\b.*\b(chose|pick|go(?:ing)? with)\b",
            ]
            .iter()
            .map(|pattern| Regex::new(pattern).expect("valid decision marker regex"))
            .collect()
        })
        .as_slice()
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
