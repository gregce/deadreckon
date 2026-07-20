use std::collections::BTreeSet;
use std::fs;
use std::path::Component;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
pub use deadreckon_protocol::{SpendRecord, TraceRecord, spend_kind_loop};
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use walkdir::WalkDir;

use crate::error::{DeadreckonError, IoContext, Result};
use crate::state::{PipelineState, append_json_line};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceRecord {
    pub timestamp: DateTime<Utc>,
    pub prompt_id: String,
    pub model: String,
    pub tool_call_id: String,
    pub session_id: String,
    pub files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffSummary {
    pub files_changed: usize,
    pub added: usize,
    pub removed: usize,
    pub files: Vec<FileDelta>,
}

impl DiffSummary {
    fn from_files(files: Vec<FileDelta>) -> Self {
        Self {
            files_changed: files.len(),
            added: files.iter().map(|file| file.added).sum(),
            removed: files.iter().map(|file| file.removed).sum(),
            files,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDelta {
    pub path: PathBuf,
    pub added: usize,
    pub removed: usize,
    pub status: FileDeltaStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unified_diff: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileDeltaStatus {
    Added,
    Removed,
    Modified,
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

pub fn diff_snapshots(a: &Path, b: &Path) -> Result<DiffSummary> {
    let a_files = snapshot_file_set(a)?;
    let b_files = snapshot_file_set(b)?;
    let paths = a_files.union(&b_files).cloned().collect::<Vec<_>>();
    let mut deltas = Vec::new();
    for relative in paths {
        let old_path = a.join(&relative);
        let new_path = b.join(&relative);
        let old = a_files.contains(&relative);
        let new = b_files.contains(&relative);
        match (old, new) {
            (false, true) => {
                deltas.push(file_delta(
                    &relative,
                    FileDeltaStatus::Added,
                    None,
                    Some(&new_path),
                ));
            }
            (true, false) => {
                deltas.push(file_delta(
                    &relative,
                    FileDeltaStatus::Removed,
                    Some(&old_path),
                    None,
                ));
            }
            (true, true) if fs::read(&old_path).ok() != fs::read(&new_path).ok() => {
                deltas.push(file_delta(
                    &relative,
                    FileDeltaStatus::Modified,
                    Some(&old_path),
                    Some(&new_path),
                ));
            }
            _ => {}
        }
    }
    Ok(DiffSummary::from_files(deltas))
}

pub fn snapshot_diff(state: &PipelineState, from: u32, to: u32) -> Result<DiffSummary> {
    let snapshots = state.run_root.join("snapshots");
    diff_snapshots(
        &snapshots.join(format!("turn-{from}")),
        &snapshots.join(format!("turn-{to}")),
    )
}

fn snapshot_file_set(root: &Path) -> Result<BTreeSet<PathBuf>> {
    if !root.exists() {
        return Err(DeadreckonError::NotFound(format!(
            "snapshot {}",
            root.display()
        )));
    }
    let mut paths = BTreeSet::new();
    for entry in WalkDir::new(root).into_iter() {
        let entry = entry.map_err(|source| DeadreckonError::Io {
            path: root.to_path_buf(),
            source: source.into(),
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(root).map_err(|err| {
            DeadreckonError::InvalidInput(format!("snapshot prefix error: {err}"))
        })?;
        if diff_excluded_path(relative) {
            continue;
        }
        paths.insert(relative.to_path_buf());
    }
    Ok(paths)
}

fn diff_excluded_path(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::Normal(name)
                if name == ".git" || name == "target" || name == ".deadreckon"
        )
    })
}

fn file_delta(
    relative: &Path,
    status: FileDeltaStatus,
    old_path: Option<&Path>,
    new_path: Option<&Path>,
) -> FileDelta {
    let old_bytes = old_path.and_then(|path| fs::read(path).ok());
    let new_bytes = new_path.and_then(|path| fs::read(path).ok());
    let old_text = old_bytes
        .as_ref()
        .and_then(|bytes| std::str::from_utf8(bytes).ok());
    let new_text = new_bytes
        .as_ref()
        .and_then(|bytes| std::str::from_utf8(bytes).ok());
    let (added, removed, unified_diff) = match (old_text, new_text) {
        (Some(old), Some(new)) => text_delta(relative, old, new),
        (None, Some(new)) if old_path.is_none() => {
            let added = new.lines().count();
            let diff = TextDiff::from_lines("", new)
                .unified_diff()
                .header("/dev/null", &format!("b/{}", relative.to_string_lossy()))
                .to_string();
            (added, 0, Some(diff))
        }
        (Some(old), None) if new_path.is_none() => {
            let removed = old.lines().count();
            let diff = TextDiff::from_lines(old, "")
                .unified_diff()
                .header(&format!("a/{}", relative.to_string_lossy()), "/dev/null")
                .to_string();
            (0, removed, Some(diff))
        }
        _ => (0, 0, None),
    };
    FileDelta {
        path: relative.to_path_buf(),
        added,
        removed,
        status,
        unified_diff,
    }
}

fn text_delta(relative: &Path, old: &str, new: &str) -> (usize, usize, Option<String>) {
    let diff = TextDiff::from_lines(old, new);
    let added = diff
        .iter_all_changes()
        .filter(|change| change.tag() == similar::ChangeTag::Insert)
        .count();
    let removed = diff
        .iter_all_changes()
        .filter(|change| change.tag() == similar::ChangeTag::Delete)
        .count();
    let unified = diff
        .unified_diff()
        .header(
            &format!("a/{}", relative.to_string_lossy()),
            &format!("b/{}", relative.to_string_lossy()),
        )
        .to_string();
    (added, removed, Some(unified))
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

    use super::{
        diff_snapshots, inventory_files, restore_snapshot, snapshot_diff, snapshot_working,
    };

    #[test]
    fn spend_record_kind_defaults_to_loop_when_absent() {
        // A legacy spend.jsonl row written before `kind` existed must still
        // parse, defaulting to "loop"; a narrator row round-trips as "narrator".
        let legacy = r#"{"timestamp":"2026-06-15T00:00:00Z","turn":3,"provider":"anthropic","model":"claude-sonnet-4-5","input_tokens":10,"output_tokens":20,"cost_usd":0.01,"total_cost_usd":0.05,"cap_usd":null}"#;
        let parsed: super::SpendRecord = serde_json::from_str(legacy).expect("legacy row parses");
        assert_eq!(parsed.kind, "loop");
        assert_eq!(super::spend_kind_loop(), "loop");

        let narrator = super::SpendRecord {
            kind: "narrator".to_string(),
            ..parsed
        };
        let encoded = serde_json::to_string(&narrator).expect("encode");
        let round: super::SpendRecord = serde_json::from_str(&encoded).expect("round-trip");
        assert_eq!(round.kind, "narrator");
    }

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

    #[test]
    fn snapshot_diff_reports_source_file_added_between_turns() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = std::env::current_dir().expect("cwd");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "diff added".to_string(),
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

        snapshot_working(&state, 0).expect("snapshot 0");
        fs::write(state.working_dir.join("src.txt"), "one\ntwo\n").expect("write");
        snapshot_working(&state, 1).expect("snapshot 1");

        let diff = snapshot_diff(&state, 0, 1).expect("diff");

        assert_eq!(diff.files_changed, 1);
        assert_eq!(diff.added, 2);
        assert_eq!(diff.files[0].path, std::path::PathBuf::from("src.txt"));
        assert!(
            diff.files[0]
                .unified_diff
                .as_deref()
                .is_some_and(|text| text.contains("+one"))
        );
    }

    #[test]
    fn snapshot_diff_excludes_target_build_output() {
        let temp = TempDir::new().expect("tempdir");
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::create_dir_all(a.join("target/debug")).expect("target a");
        fs::create_dir_all(b.join("target/debug")).expect("target b");
        fs::create_dir_all(b.join(".deadreckon/docs")).expect("deadreckon b");
        fs::write(a.join("source.rs"), "fn a() {}\n").expect("source a");
        fs::write(b.join("source.rs"), "fn a() {}\nfn b() {}\n").expect("source b");
        fs::write(b.join("target/debug/app"), "binary").expect("target binary");
        fs::write(b.join(".deadreckon/docs/RUN-NARRATIVE.md"), "doc").expect("doc");

        let diff = diff_snapshots(&a, &b).expect("diff");

        assert_eq!(diff.files_changed, 1, "{diff:#?}");
        assert_eq!(diff.files[0].path, std::path::PathBuf::from("source.rs"));
    }

    #[test]
    fn snapshot_diff_handles_binary_and_missing_without_error() {
        let temp = TempDir::new().expect("tempdir");
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::create_dir_all(&a).expect("a");
        fs::create_dir_all(&b).expect("b");
        fs::write(a.join("image.bin"), [0, 159, 146, 150]).expect("binary a");
        fs::write(b.join("image.bin"), [0, 159, 146, 151]).expect("binary b");
        fs::write(a.join("removed.bin"), [255, 0, 1]).expect("removed");

        let diff = diff_snapshots(&a, &b).expect("diff");

        assert_eq!(diff.files_changed, 2, "{diff:#?}");
        assert!(
            diff.files
                .iter()
                .any(|file| file.path == std::path::PathBuf::from("image.bin")
                    && file.unified_diff.is_none())
        );
    }
}
