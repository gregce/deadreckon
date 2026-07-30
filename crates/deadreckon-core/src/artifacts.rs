use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
#[cfg(test)]
use deadreckon_protocol::spend_kind_loop;
use deadreckon_protocol::{LedgerItem, SpendRecord, TraceRecord};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use similar::TextDiff;
use walkdir::WalkDir;

use crate::artifact_policy::{is_deliverable_workspace_path, is_promotable_workspace_path};
use crate::error::{DeadreckonError, IoContext, Result};
use crate::ledger_io::append_ledger_item;
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileDelta {
    pub path: PathBuf,
    pub added: usize,
    pub removed: usize,
    pub status: FileDeltaStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unified_diff: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileDeltaStatus {
    Added,
    Removed,
    Modified,
}

pub fn append_spend(state: &PipelineState, record: &SpendRecord) -> Result<()> {
    // REPORT.md: Live Context & Spend Meter is durable JSONL, not terminal-only UI.
    append_ledger_item(&state.run_root, LedgerItem::Spend(record.clone()))
}

pub fn append_provenance(state: &PipelineState, record: &ProvenanceRecord) -> Result<()> {
    // REPORT.md: Prompt-To-Code Provenance Audit Trail records prompt, model,
    // tool call, session, and changed files per coding turn.
    append_json_line(&state.run_root.join("provenance.jsonl"), record)
}

pub fn append_trace(state: &PipelineState, record: &TraceRecord) -> Result<()> {
    // REPORT.md: Agent Observability keeps local traces exportable as JSONL.
    append_ledger_item(&state.run_root, LedgerItem::Trace(record.clone()))
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

/// Compare two materialized work trees without first copying either tree into
/// a run snapshot.
///
/// Durable graph completion uses this to judge the merged result against the
/// operator-approved source tree. The ordinary single-run path continues to
/// use turn snapshots.
pub fn diff_working_trees(before: &Path, after: &Path) -> Result<DiffSummary> {
    diff_snapshots(before, after)
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
    !is_deliverable_workspace_path(path)
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
    copy_tree_with_filter(from, to, |_| true)
}

pub fn copy_deliverable_tree(from: &Path, to: &Path) -> Result<()> {
    copy_classified_tree(from, to, is_deliverable_workspace_path)
}

pub fn copy_promotable_tree(from: &Path, to: &Path) -> Result<()> {
    copy_classified_tree(from, to, is_promotable_workspace_path)
}

/// Copy one filesystem artifact without changing or following its identity.
///
/// Graph composition and result seeding already have an allowlisted relative
/// path. This leaf-level primitive preserves regular/executable permissions
/// and symbolic-link targets while refusing directories and special files.
pub fn copy_artifact_path(source: &Path, target: &Path) -> Result<()> {
    if let Ok(metadata) = fs::symlink_metadata(target)
        && metadata.file_type().is_dir()
        && fs::read_dir(target)
            .with_path(target)?
            .next()
            .transpose()
            .with_path(target)?
            .is_some()
    {
        return Err(DeadreckonError::InvalidInput(format!(
            "artifact destination hierarchy is not empty: {}",
            target.display()
        )));
    }
    let metadata = fs::symlink_metadata(source).with_path(source)?;
    let file_type = metadata.file_type();
    if file_type.is_file() {
        copy_regular_file(source, target, &metadata)
    } else if file_type.is_symlink() {
        copy_symbolic_link(source, target)
    } else {
        Err(DeadreckonError::InvalidInput(format!(
            "unsupported filesystem artifact while copying {}",
            source.display()
        )))
    }
}

/// Remove an artifact leaf without following symbolic links.
///
/// Directories are accepted for callers replacing a prior hierarchy, but a
/// symbolic link to a directory is removed as the link itself.
pub fn remove_artifact_path(path: &Path) -> Result<()> {
    remove_copy_target_if_present(path)
}

fn copy_classified_tree(from: &Path, to: &Path, include: fn(&Path) -> bool) -> Result<()> {
    copy_tree_with_filter(from, to, include)
}

fn copy_tree_with_filter(from: &Path, to: &Path, include: fn(&Path) -> bool) -> Result<()> {
    ensure_copy_destination_root(to)?;
    let source_metadata = match fs::symlink_metadata(from) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: from.to_path_buf(),
                source,
            });
        }
    };
    if !source_metadata.file_type().is_dir() {
        return Err(DeadreckonError::InvalidInput(format!(
            "copy source is not a directory: {}",
            from.display()
        )));
    }

    // WalkDir does not follow links by default; make the security boundary
    // explicit because snapshots and provider-evidence archives must preserve
    // links themselves rather than importing whatever they point at.
    for entry in WalkDir::new(from)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry
                .path()
                .strip_prefix(from)
                .ok()
                .is_none_or(|relative| relative.as_os_str().is_empty() || include(relative))
        })
    {
        let entry = entry.map_err(|source| DeadreckonError::Io {
            path: from.to_path_buf(),
            source: source.into(),
        })?;
        let relative = entry.path().strip_prefix(from).map_err(|err| {
            DeadreckonError::InvalidInput(format!("copy source prefix error: {err}"))
        })?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = to.join(relative);
        let metadata = fs::symlink_metadata(entry.path()).with_path(entry.path())?;
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            // A checked-out Git submodule also appears as a directory here.
            // Filesystem copying treats it as a directory tree and never
            // misrepresents it as a symbolic link or a Git `160000` entry.
            ensure_directory_target(&target)?;
        } else if file_type.is_file() {
            copy_regular_file(entry.path(), &target, &metadata)?;
        } else if file_type.is_symlink() {
            copy_symbolic_link(entry.path(), &target)?;
        } else {
            return Err(DeadreckonError::InvalidInput(format!(
                "unsupported filesystem entry while copying {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn ensure_copy_destination_root(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(DeadreckonError::InvalidInput(format!(
            "copy destination is not a directory: {}",
            path.display()
        ))),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).with_path(path)
        }
        Err(source) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn ensure_directory_target(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => {
            remove_copy_target(path)?;
            fs::create_dir(path).with_path(path)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).with_path(path)
        }
        Err(source) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn copy_regular_file(source: &Path, target: &Path, metadata: &fs::Metadata) -> Result<()> {
    if let Some(parent) = target.parent() {
        ensure_directory_target(parent)?;
    }
    remove_copy_target_if_present(target)?;
    fs::copy(source, target).with_path(target)?;
    // `fs::copy` is platform-specific about metadata. Setting the source
    // permissions explicitly keeps the executable bit on Unix and the
    // supported permission representation elsewhere.
    fs::set_permissions(target, metadata.permissions()).with_path(target)
}

fn copy_symbolic_link(source: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        ensure_directory_target(parent)?;
    }
    remove_copy_target_if_present(target)?;
    let link_target = fs::read_link(source).with_path(source)?;
    create_symbolic_link(source, &link_target, target)
}

#[cfg(unix)]
fn create_symbolic_link(_source: &Path, link_target: &Path, target: &Path) -> Result<()> {
    std::os::unix::fs::symlink(link_target, target).with_path(target)
}

#[cfg(windows)]
fn create_symbolic_link(source: &Path, link_target: &Path, target: &Path) -> Result<()> {
    use std::os::windows::fs::FileTypeExt as _;

    // Windows requires the link kind at creation time. The reparse-point kind
    // is available from symlink metadata, so dangling links never need to be
    // followed merely to preserve them.
    let source_type = fs::symlink_metadata(source).with_path(source)?.file_type();
    if source_type.is_symlink_dir() {
        std::os::windows::fs::symlink_dir(link_target, target).with_path(target)
    } else if source_type.is_symlink_file() {
        std::os::windows::fs::symlink_file(link_target, target).with_path(target)
    } else {
        Err(DeadreckonError::InvalidInput(format!(
            "unsupported symbolic-link target while copying {}",
            source.display()
        )))
    }
}

#[cfg(not(any(unix, windows)))]
fn create_symbolic_link(source: &Path, _link_target: &Path, _target: &Path) -> Result<()> {
    Err(DeadreckonError::InvalidInput(format!(
        "symbolic-link copies are unsupported on this platform: {}",
        source.display()
    )))
}

fn remove_copy_target_if_present(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => remove_copy_target(path),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn remove_copy_target(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).with_path(path)?;
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        fs::remove_dir_all(path).with_path(path)
    } else if file_type.is_symlink() {
        remove_symbolic_link(path)
    } else {
        fs::remove_file(path).with_path(path)
    }
}

#[cfg(unix)]
fn remove_symbolic_link(path: &Path) -> Result<()> {
    fs::remove_file(path).with_path(path)
}

#[cfg(windows)]
fn remove_symbolic_link(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::IsADirectory => {
            fs::remove_dir(path).with_path(path)
        }
        Err(source) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(not(any(unix, windows)))]
fn remove_symbolic_link(path: &Path) -> Result<()> {
    fs::remove_file(path).with_path(path)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use crate::DeadreckonPaths;
    use crate::state::{RunOptions, create_run};

    use super::{
        copy_artifact_path, copy_deliverable_tree, copy_promotable_tree, copy_tree, diff_snapshots,
        diff_working_trees, inventory_files, restore_snapshot, snapshot_diff, snapshot_working,
    };

    #[test]
    fn graph_source_result_diff_carries_changed_content_without_snapshots() {
        let temp = TempDir::new().expect("tempdir");
        let source = temp.path().join("source");
        let result = temp.path().join("result");
        fs::create_dir_all(&source).expect("source");
        fs::create_dir_all(&result).expect("result");
        fs::write(source.join("answer.txt"), "before\n").expect("source file");
        fs::write(result.join("answer.txt"), "after\n").expect("result file");
        fs::write(result.join("receipt.txt"), "evidence\n").expect("added file");

        let diff = diff_working_trees(&source, &result).expect("tree diff");
        assert_eq!(diff.files_changed, 2);
        let answer = diff
            .files
            .iter()
            .find(|file| file.path == Path::new("answer.txt"))
            .expect("answer diff");
        let unified = answer.unified_diff.as_deref().expect("text diff");
        assert!(unified.contains("-before"), "{unified}");
        assert!(unified.contains("+after"), "{unified}");
    }

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
    fn provider_private_paths_are_evidence_only_not_semantic_changes() {
        let temp = TempDir::new().expect("tempdir");
        let before = temp.path().join("before");
        let after = temp.path().join("after");
        fs::create_dir_all(before.join(".specstory/history")).expect("before private");
        fs::create_dir_all(after.join(".specstory/history")).expect("after private");
        fs::create_dir_all(after.join("src")).expect("source");
        fs::write(before.join(".specstory/history/session.md"), "before").expect("before");
        fs::write(after.join(".specstory/history/session.md"), "after").expect("after");
        fs::write(after.join("src/lib.rs"), "pub fn value() -> u8 { 42 }\n").expect("source");

        let diff = diff_working_trees(&before, &after).expect("diff");

        assert_eq!(diff.files.len(), 1);
        assert_eq!(diff.files[0].path, Path::new("src/lib.rs"));
    }

    #[test]
    fn deliverable_copy_keeps_source_and_run_docs_but_omits_provider_evidence() {
        let temp = TempDir::new().expect("tempdir");
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir_all(source.join("src")).expect("source");
        fs::create_dir_all(source.join("docs")).expect("docs");
        fs::create_dir_all(source.join(".specstory/history")).expect("private");
        fs::write(source.join("src/lib.rs"), "source\n").expect("source");
        fs::write(source.join("docs/RUN-AS-BUILT.md"), "run docs\n").expect("docs");
        fs::write(source.join("implementation-notes.html"), "notes\n").expect("notes");
        fs::write(source.join(".specstory/history/session.md"), "private\n").expect("private");

        copy_deliverable_tree(&source, &destination).expect("copy");

        assert!(destination.join("src/lib.rs").exists());
        assert!(destination.join("docs/RUN-AS-BUILT.md").exists());
        assert!(destination.join("implementation-notes.html").exists());
        assert!(!destination.join(".specstory").exists());
    }

    #[test]
    fn promotable_copy_keeps_lifecycle_metadata_but_omits_provider_evidence() {
        let temp = TempDir::new().expect("tempdir");
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir_all(source.join(".deadreckon")).expect("lifecycle");
        fs::create_dir_all(source.join(".specstory/history")).expect("private");
        fs::write(source.join(".deadreckon/codebase.json"), "{}\n").expect("lifecycle");
        fs::write(source.join(".specstory/history/session.md"), "private\n").expect("private");

        copy_promotable_tree(&source, &destination).expect("copy");

        assert!(destination.join(".deadreckon/codebase.json").exists());
        assert!(!destination.join(".specstory").exists());
    }

    #[cfg(unix)]
    #[test]
    fn deliverable_copy_preserves_executable_files_and_symlinks_without_following() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let temp = TempDir::new().expect("tempdir");
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir_all(source.join("bin")).expect("bin");
        fs::create_dir_all(source.join(".specstory/history")).expect("evidence");
        let executable = source.join("bin/run");
        fs::write(&executable, "#!/bin/sh\n").expect("executable");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).expect("mode");
        fs::write(source.join(".specstory/history/session.md"), "private\n").expect("evidence");
        symlink(
            "../.specstory/history/session.md",
            source.join("bin/session"),
        )
        .expect("symlink");

        copy_deliverable_tree(&source, &destination).expect("copy");

        assert_ne!(
            fs::metadata(destination.join("bin/run"))
                .expect("metadata")
                .permissions()
                .mode()
                & 0o111,
            0
        );
        assert!(
            fs::symlink_metadata(destination.join("bin/session"))
                .expect("link metadata")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(destination.join("bin/session")).expect("link target"),
            Path::new("../.specstory/history/session.md")
        );
        assert!(!destination.join(".specstory").exists());
    }

    #[cfg(unix)]
    #[test]
    fn evidence_snapshot_preserves_nested_symlink_without_importing_its_target() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let outside = temp.path().join("outside");
        fs::create_dir_all(source.join(".specstory/history")).expect("evidence");
        fs::create_dir_all(&outside).expect("outside");
        fs::write(outside.join("secret.txt"), "outside\n").expect("outside file");
        symlink(
            "../../../outside",
            source.join(".specstory/history/external"),
        )
        .expect("symlink");

        copy_tree(&source, &destination).expect("copy");

        let copied_link = destination.join(".specstory/history/external");
        assert!(
            fs::symlink_metadata(&copied_link)
                .expect("metadata")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(&copied_link).expect("link target"),
            Path::new("../../../outside")
        );
        assert!(!destination.join("outside").exists());
    }

    #[cfg(unix)]
    #[test]
    fn tree_copy_fails_closed_on_special_files() {
        use std::os::unix::net::UnixListener;

        let temp = TempDir::new().expect("tempdir");
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&source).expect("source");
        let _listener = UnixListener::bind(source.join("provider.sock")).expect("socket");

        let error = copy_tree(&source, &destination).expect_err("special file refused");

        assert!(
            error.to_string().contains("unsupported filesystem entry"),
            "{error}"
        );
    }

    #[test]
    fn artifact_copy_refuses_to_replace_a_nonempty_directory() {
        let temp = TempDir::new().expect("tempdir");
        let source = temp.path().join("source.txt");
        let target = temp.path().join("target");
        fs::write(&source, "replacement\n").expect("source");
        fs::create_dir(&target).expect("target directory");
        fs::write(target.join("preserved.txt"), "preserved\n").expect("preserved");

        let error = copy_artifact_path(&source, &target).expect_err("hierarchy refused");

        assert!(
            error
                .to_string()
                .contains("artifact destination hierarchy is not empty"),
            "{error}"
        );
        assert_eq!(
            fs::read_to_string(target.join("preserved.txt")).expect("preserved remains"),
            "preserved\n"
        );
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
                .any(|file| file.path == Path::new("image.bin") && file.unified_diff.is_none())
        );
    }
}
