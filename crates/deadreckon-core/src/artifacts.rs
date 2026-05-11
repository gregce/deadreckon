use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::error::{DeadreckonError, IoContext, Result};
use crate::state::{PipelineState, append_json_line};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpendRecord {
    pub timestamp: DateTime<Utc>,
    pub turn: u32,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub total_cost_usd: f64,
    pub cap_usd: Option<f64>,
    #[serde(default)]
    pub subscription: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_time_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_time_cap_seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceRecord {
    pub timestamp: DateTime<Utc>,
    pub prompt_id: String,
    pub model: String,
    pub tool_call_id: String,
    pub session_id: String,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceRecord {
    pub timestamp: DateTime<Utc>,
    pub run_id: String,
    pub turn: u32,
    pub event: String,
    pub latency_ms: Option<u128>,
    pub detail: serde_json::Value,
}

pub fn append_spend(state: &PipelineState, record: &SpendRecord) -> Result<()> {
    // REPORT.md: Live Context & Spend Meter is durable JSONL, not terminal-only UI.
    append_json_line(&state.run_root.join("spend.jsonl"), record)
}

pub fn append_provenance(state: &PipelineState, record: &ProvenanceRecord) -> Result<()> {
    // REPORT.md: Prompt-To-Code Provenance Audit Trail records prompt, model,
    // tool call, session, and changed files per coding turn.
    append_json_line(&state.run_root.join("provenance.jsonl"), record)
}

pub fn append_trace(state: &PipelineState, record: &TraceRecord) -> Result<()> {
    // REPORT.md: Agent Observability keeps local traces exportable as JSONL.
    append_json_line(&state.run_root.join("traces.jsonl"), record)
}

pub fn snapshot_working(state: &PipelineState, turn: u32) -> Result<PathBuf> {
    // REPORT.md: Infinite Undo For Agent Edits is implemented as durable
    // per-turn filesystem snapshots.
    // AS-BUILT §9: every mutation boundary gets a filesystem snapshot so a
    // later bounded fix or undo operation has a concrete rollback target.
    let snapshot_dir = state
        .run_root
        .join("snapshots")
        .join(format!("turn-{turn}"));
    if snapshot_dir.exists() {
        fs::remove_dir_all(&snapshot_dir).with_path(&snapshot_dir)?;
    }
    copy_tree(&state.working_dir, &snapshot_dir)?;
    Ok(snapshot_dir)
}

pub fn restore_snapshot(state: &PipelineState, turn: u32) -> Result<()> {
    // REPORT.md: Infinite Undo For Agent Edits restores files from a selected
    // turn snapshot rather than only rewinding chat.
    let snapshot_dir = state
        .run_root
        .join("snapshots")
        .join(format!("turn-{turn}"));
    if !snapshot_dir.exists() {
        return Err(DeadreckonError::NotFound(format!(
            "snapshot turn-{turn} for run {}",
            state.run_id
        )));
    }
    if state.working_dir.exists() {
        fs::remove_dir_all(&state.working_dir).with_path(&state.working_dir)?;
    }
    copy_tree(&snapshot_dir, &state.working_dir)
}

pub fn inventory_files(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        files.push(entry.path().to_path_buf());
    }
    files.sort();
    Ok(files)
}

pub fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).with_path(to)?;
    if !from.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(from).into_iter() {
        let entry = entry.map_err(|source| DeadreckonError::Io {
            path: from.to_path_buf(),
            source: source.into(),
        })?;
        let relative = entry.path().strip_prefix(from).map_err(|err| {
            DeadreckonError::InvalidInput(format!("copy source prefix error: {err}"))
        })?;
        let target = to.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target).with_path(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).with_path(parent)?;
            }
            fs::copy(entry.path(), &target).with_path(&target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use crate::DeadreckonPaths;
    use crate::state::{RunOptions, create_run};

    use super::{inventory_files, restore_snapshot, snapshot_working};

    #[test]
    fn snapshot_and_restore_working_tree() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = std::env::current_dir().expect("cwd");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "snapshot".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");

        fs::write(state.working_dir.join("file.txt"), "one").expect("write");
        snapshot_working(&state, 1).expect("snapshot");
        fs::write(state.working_dir.join("file.txt"), "two").expect("mutate");
        restore_snapshot(&state, 1).expect("restore");

        let restored = fs::read_to_string(state.working_dir.join("file.txt")).expect("read");
        assert_eq!(restored, "one");
        let inventory = inventory_files(&state.working_dir).expect("inventory");
        assert!(inventory.iter().any(|path| path.ends_with("file.txt")));
        assert!(
            inventory
                .iter()
                .any(|path| path.ends_with(".deadreckon/docs/RUN-NARRATIVE.md"))
        );
    }
}
