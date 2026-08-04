use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::git::{run_git, run_git_with_input};
use crate::paths::{DeadreckonPaths, sanitize_slug, workspace_scope};
use crate::workspace_capture::{
    CaptureProjection, CapturePurpose, WorkspaceCaptureManifest, WorkspaceCapturePolicy,
    capture_workspace_strict, freeze_workspace_capture_policy, materialize_capture_plan,
};

pub const CODEBASE_RECORD_VERSION: u32 = 1;
pub const CODEBASE_RECORD_PATH: &str = ".deadreckon/codebase.json";
pub const TRUSTED_CODEBASE_RECORD: &str = "codebase.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodebaseMode {
    Worktree,
    Copy,
    InPlace,
    Fresh,
}

impl std::fmt::Display for CodebaseMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Worktree => "worktree",
            Self::Copy => "copy",
            Self::InPlace => "in-place",
            Self::Fresh => "fresh",
        };
        f.write_str(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodebaseRecord {
    pub schema_version: u32,
    pub mode: CodebaseMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_git_root: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<PathBuf>,
    pub dirty_files_seeded: bool,
    pub head_was_detached: bool,
    pub created_at: DateTime<Utc>,
    pub deadreckon_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc_polish_hash: Option<String>,
}

impl CodebaseRecord {
    pub fn fresh() -> Self {
        Self {
            schema_version: CODEBASE_RECORD_VERSION,
            mode: CodebaseMode::Fresh,
            source_path: None,
            source_git_root: None,
            branch_name: None,
            base_ref: None,
            base_sha: None,
            parent_branch: None,
            worktree_path: None,
            dirty_files_seeded: false,
            head_was_detached: false,
            created_at: Utc::now(),
            deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
            doc_polish_hash: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ModeFlags {
    pub fresh: bool,
    pub worktree: bool,
    pub from: Option<PathBuf>,
    pub in_place: bool,
    pub i_know_its_a_lot: bool,
}

#[derive(Debug, Clone)]
pub struct WorktreeOptions {
    pub run_id: String,
    pub task_key: String,
    pub source_path: PathBuf,
    pub base_ref: Option<String>,
    pub branch_name: Option<String>,
    pub allow_dirty: bool,
    /// Controller-owned paths that may be dirty without being copied into the
    /// isolated worktree. Callers must bind these paths to immutable authority
    /// before using this escape hatch.
    pub allowed_dirty_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewGitState {
    pub git_root: PathBuf,
    pub branch: String,
    pub head_sha: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedMode {
    Worktree {
        source_path: PathBuf,
        git_root: PathBuf,
    },
    Copy {
        source_path: PathBuf,
    },
    InPlace {
        source_path: PathBuf,
    },
    Fresh,
}

impl ResolvedMode {
    pub fn mode(&self) -> CodebaseMode {
        match self {
            Self::Worktree { .. } => CodebaseMode::Worktree,
            Self::Copy { .. } => CodebaseMode::Copy,
            Self::InPlace { .. } => CodebaseMode::InPlace,
            Self::Fresh => CodebaseMode::Fresh,
        }
    }
}

pub fn resolve_mode(flags: &ModeFlags, cwd: &Path, tty: bool) -> Result<ResolvedMode> {
    if flags.fresh {
        return Ok(ResolvedMode::Fresh);
    }
    if flags.in_place {
        if !flags.i_know_its_a_lot && !tty {
            return Err(user_error(
                "--in-place requires --i-know-its-a-lot or interactive confirm",
                "add --i-know-its-a-lot or run in a TTY",
            ));
        }
        return Ok(ResolvedMode::InPlace {
            source_path: canonical_or_self(cwd)?,
        });
    }
    if let Some(source_path) = flags.from.as_ref() {
        return Ok(ResolvedMode::Copy {
            source_path: canonical_or_self(source_path)?,
        });
    }
    if flags.worktree {
        let git_root = find_git_root(cwd)?.ok_or_else(|| {
            user_error(
                &format!("{} is not a git repo", cwd.display()),
                "git init or pass --from .",
            )
        })?;
        return Ok(ResolvedMode::Worktree {
            source_path: canonical_or_self(cwd)?,
            git_root,
        });
    }
    if let Some(git_root) = find_git_root(cwd)? {
        return Ok(ResolvedMode::Worktree {
            source_path: canonical_or_self(cwd)?,
            git_root,
        });
    }
    if tty {
        return Ok(ResolvedMode::Copy {
            source_path: canonical_or_self(cwd)?,
        });
    }
    Err(user_error(
        "non-interactive without a mode flag",
        "--fresh or --from . or git init",
    ))
}

pub fn record_for_resolved_mode(mode: ResolvedMode) -> CodebaseRecord {
    let mut record = CodebaseRecord::fresh();
    match mode {
        ResolvedMode::Fresh => {}
        ResolvedMode::Copy { source_path } => {
            record.mode = CodebaseMode::Copy;
            record.source_path = Some(source_path);
        }
        ResolvedMode::InPlace { source_path } => {
            record.mode = CodebaseMode::InPlace;
            record.source_path = Some(source_path);
        }
        ResolvedMode::Worktree {
            source_path,
            git_root,
        } => {
            record.mode = CodebaseMode::Worktree;
            record.source_path = Some(source_path);
            record.source_git_root = Some(git_root);
        }
    }
    record
}

pub fn prepare_worktree_record(
    paths: &DeadreckonPaths,
    options: WorktreeOptions,
) -> Result<CodebaseRecord> {
    let git_root = find_git_root(&options.source_path)?.ok_or_else(|| {
        user_error(
            &format!("{} is not a git repo", options.source_path.display()),
            "git init or pass --from .",
        )
    })?;
    let base_ref = options
        .base_ref
        .unwrap_or_else(|| current_branch(&git_root).unwrap_or_else(|_| "HEAD".to_string()));
    preflight_worktree_with_allowed_paths(
        &git_root,
        &base_ref,
        options.allow_dirty,
        &options.allowed_dirty_paths,
    )?;
    let branch_name = options.branch_name.unwrap_or_else(|| {
        format!(
            "dr/{}-{}",
            options.task_key.chars().take(32).collect::<String>(),
            options.run_id.chars().take(8).collect::<String>()
        )
    });
    if git_ref_exists(&git_root, &branch_name)? {
        return Err(user_error(
            &format!("branch {branch_name} already exists"),
            "pass --branch-name <other-name>",
        ));
    }
    let scope = workspace_scope(&git_root)?;
    let worktree_path = unique_worktree_path(paths, &scope, &options.run_id);
    let base_sha = git_stdout(&git_root, &["rev-parse", &base_ref])?;
    let mut record = CodebaseRecord::fresh();
    record.mode = CodebaseMode::Worktree;
    record.source_path = Some(canonical_or_self(&options.source_path)?);
    record.source_git_root = Some(git_root);
    record.branch_name = Some(branch_name);
    record.base_ref = Some(base_ref);
    record.base_sha = Some(base_sha);
    record.worktree_path = Some(worktree_path);
    record.dirty_files_seeded = options.allow_dirty;
    Ok(record)
}

pub fn create_worktree(record: &CodebaseRecord) -> Result<()> {
    if record.mode != CodebaseMode::Worktree {
        return Ok(());
    }
    let git_root = required_path(record.source_git_root.as_ref(), "source_git_root")?;
    let worktree_path = required_path(record.worktree_path.as_ref(), "worktree_path")?;
    let branch = required_string(record.branch_name.as_deref(), "branch_name")?;
    let base = required_string(record.base_ref.as_deref(), "base_ref")?;
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent).with_path(parent)?;
    }
    git_status(
        git_root,
        &[
            "worktree",
            "add",
            "-b",
            branch,
            path_str(worktree_path)?,
            base,
        ],
    )?;
    append_git_exclude(git_root, ".deadreckon/")?;
    if record.dirty_files_seeded {
        seed_dirty_files(git_root, worktree_path)?;
    }
    Ok(())
}

pub fn copy_source_to_working(source: &Path, working_dir: &Path) -> Result<()> {
    let policy = freeze_workspace_capture_policy(source)?;
    copy_source_to_working_with_policy(source, working_dir, &policy).map(|_| ())
}

pub fn copy_source_to_working_with_policy(
    source: &Path,
    working_dir: &Path,
    policy: &WorkspaceCapturePolicy,
) -> Result<WorkspaceCaptureManifest> {
    if !source.exists() {
        return Err(DeadreckonError::NotFound(format!(
            "source {}",
            source.display()
        )));
    }
    let plan = capture_workspace_strict(
        source,
        policy,
        CaptureProjection::Source,
        CapturePurpose::SourceHydration,
    )?;
    plan.require_complete("source hydration")?;
    materialize_capture_plan(&plan, working_dir)?;
    Ok(plan.manifest)
}

pub fn preview_git_state(path: &Path) -> Result<Option<PreviewGitState>> {
    let Some(git_root) = find_git_root(path)? else {
        return Ok(None);
    };
    Ok(Some(PreviewGitState {
        branch: current_branch(&git_root).unwrap_or_else(|_| "detached".to_string()),
        head_sha: git_stdout(&git_root, &["rev-parse", "--short", "HEAD"])
            .unwrap_or_else(|_| "no-commits".to_string()),
        git_root,
    }))
}

pub fn current_branch(git_root: &Path) -> Result<String> {
    git_stdout(git_root, &["symbolic-ref", "--short", "-q", "HEAD"]).and_then(|branch| {
        if branch.is_empty() {
            Err(user_error("HEAD is detached", "git switch -c <branch>"))
        } else {
            Ok(branch)
        }
    })
}

pub fn preflight_worktree(git_root: &Path, base_ref: &str, allow_dirty: bool) -> Result<()> {
    preflight_worktree_with_allowed_paths(git_root, base_ref, allow_dirty, &[])
}

fn preflight_worktree_with_allowed_paths(
    git_root: &Path,
    base_ref: &str,
    allow_dirty: bool,
    allowed_dirty_paths: &[PathBuf],
) -> Result<()> {
    let head = git_stdout(git_root, &["rev-parse", "HEAD"])
        .map_err(|_| user_error("git repo has no commits", "git commit -m initial"))?;
    if current_branch(git_root).is_err() {
        let short = head.chars().take(12).collect::<String>();
        return Err(user_error(
            &format!("HEAD is detached at {short}"),
            "git switch -c <branch>",
        ));
    }
    if git_path(git_root, "MERGE_HEAD")?.exists() {
        return Err(user_error(
            "git is in the middle of a merge",
            "git merge --abort",
        ));
    }
    if git_path(git_root, "rebase-merge")?.exists() || git_path(git_root, "rebase-apply")?.exists()
    {
        return Err(user_error(
            "git is in the middle of a rebase",
            "git rebase --abort",
        ));
    }
    let dirty = git_stdout(
        git_root,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    if !allow_dirty && !dirty.trim().is_empty() {
        let allowed = allowed_dirty_paths
            .iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect::<std::collections::BTreeSet<_>>();
        let mut fields = dirty.split_terminator('\0');
        let mut unexpected = Vec::new();
        while let Some(entry) = fields.next() {
            let Some(status) = entry.get(..2) else {
                unexpected.push(format!("  malformed git status entry: {entry}"));
                break;
            };
            let Some(path) = entry.get(3..) else {
                unexpected.push(format!("  malformed git status entry: {entry}"));
                break;
            };
            let rename_or_copy = status.bytes().any(|byte| matches!(byte, b'R' | b'C'));
            if rename_or_copy {
                let original = fields.next().unwrap_or("<missing source path>");
                unexpected.push(format!("  {status} {original} -> {path}"));
            } else if !allowed.contains(path) {
                unexpected.push(format!("  {status} {path}"));
            }
            if unexpected.len() == 12 {
                break;
            }
        }
        if !unexpected.is_empty() {
            return Err(user_error(
                &format!(
                    "working tree has uncommitted changes:\n{}",
                    unexpected.join("\n")
                ),
                "git stash && deadreckon run … (or --allow-dirty)",
            ));
        }
    }
    git_stdout(git_root, &["rev-parse", "--verify", base_ref])?;
    Ok(())
}

pub fn codebase_record_path(working_dir: &Path) -> PathBuf {
    working_dir.join(CODEBASE_RECORD_PATH)
}

pub fn write_codebase_record(working_dir: &Path, record: &CodebaseRecord) -> Result<()> {
    let path = codebase_record_path(working_dir);
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!("codebase record has no parent: {}", path.display()))
    })?;
    std::fs::create_dir_all(parent).with_path(parent)?;
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(record).map_err(|source| DeadreckonError::Json {
            path: path.clone(),
            source,
        })?,
    )
    .with_path(path)
}

pub fn read_codebase_record(working_dir: &Path) -> Result<CodebaseRecord> {
    let path = codebase_record_path(working_dir);
    let data = std::fs::read(&path).with_path(&path)?;
    serde_json::from_slice(&data).with_json_path(path)
}

/// Persist the lifecycle routing record outside the agent-visible workspace.
///
/// The workspace copy remains useful to docs and legacy commands, but delivery
/// and receipt code should prefer this control-plane copy so a provider cannot
/// redirect `finish` by editing `.deadreckon/codebase.json`.
pub fn write_trusted_codebase_record(run_root: &Path, record: &CodebaseRecord) -> Result<()> {
    let path = run_root.join(TRUSTED_CODEBASE_RECORD);
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(record).map_err(|source| DeadreckonError::Json {
            path: path.clone(),
            source,
        })?,
    )
    .with_path(path)
}

pub fn read_trusted_codebase_record(run_root: &Path) -> Result<CodebaseRecord> {
    let path = run_root.join(TRUSTED_CODEBASE_RECORD);
    let data = std::fs::read(&path).with_path(&path)?;
    serde_json::from_slice(&data).with_json_path(path)
}

pub fn read_run_codebase_record(run_root: &Path, working_dir: &Path) -> Result<CodebaseRecord> {
    read_trusted_codebase_record(run_root).or_else(|error| {
        if matches!(
            &error,
            DeadreckonError::Io { source, .. }
                if source.kind() == std::io::ErrorKind::NotFound
        ) {
            read_codebase_record(working_dir)
        } else {
            Err(error)
        }
    })
}

pub fn find_git_root(path: &Path) -> Result<Option<PathBuf>> {
    let output = run_git(path, &["rev-parse", "--show-toplevel"])?;
    if !output.status.success() {
        return Ok(None);
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if root.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(root)))
    }
}

pub fn user_error(message: &str, try_hint: &str) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!("{message}\ntry: {try_hint}"))
}

fn git_ref_exists(git_root: &Path, name: &str) -> Result<bool> {
    let output = run_git(git_root, &["rev-parse", "--verify", name])?;
    Ok(output.status.success())
}

fn git_path(git_root: &Path, name: &str) -> Result<PathBuf> {
    git_stdout(git_root, &["rev-parse", "--git-path", name]).map(PathBuf::from)
}

fn append_git_exclude(git_root: &Path, pattern: &str) -> Result<()> {
    let exclude = git_path(git_root, "info/exclude")?;
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == pattern) {
        return Ok(());
    }
    if let Some(parent) = exclude.parent() {
        std::fs::create_dir_all(parent).with_path(parent)?;
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&exclude)
        .with_path(&exclude)?;
    writeln!(file, "{pattern}").with_path(exclude)
}

fn git_stdout(git_root: &Path, args: &[&str]) -> Result<String> {
    let output = run_git(git_root, args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(DeadreckonError::InvalidInput(if stderr.is_empty() {
            format!("git {:?} failed", args)
        } else {
            stderr
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_status(git_root: &Path, args: &[&str]) -> Result<()> {
    let output = run_git(git_root, args)?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(DeadreckonError::InvalidInput(if stderr.is_empty() {
            format!("git {:?} failed", args)
        } else {
            stderr
        }))
    }
}

fn unique_worktree_path(paths: &DeadreckonPaths, scope: &str, run_id: &str) -> PathBuf {
    let stem = format!(
        "{}-{}",
        sanitize_slug(scope),
        &run_id[..run_id.len().min(8)]
    );
    let root = paths.home().join("worktrees");
    let mut candidate = root.join(&stem);
    let mut suffix = 2;
    while candidate.exists()
        && std::fs::read_dir(&candidate)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(true)
    {
        candidate = root.join(format!("{stem}-{suffix}"));
        suffix += 1;
    }
    candidate
}

fn seed_dirty_files(git_root: &Path, worktree_path: &Path) -> Result<()> {
    apply_dirty_diff(git_root, worktree_path, false)?;
    apply_dirty_diff(git_root, worktree_path, true)?;
    let dirty = git_stdout(git_root, &["status", "--porcelain"])?;
    for line in dirty.lines() {
        if line.len() < 4 {
            continue;
        }
        if !line.starts_with("?? ") {
            continue;
        }
        let relative = line[3..].trim().trim_matches('"');
        if relative.contains(" -> ") {
            continue;
        }
        let source = git_root.join(relative);
        let dest = worktree_path.join(relative);
        if source.is_file() {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).with_path(parent)?;
            }
            std::fs::copy(&source, &dest).with_path(&source)?;
        }
    }
    Ok(())
}

fn apply_dirty_diff(git_root: &Path, worktree_path: &Path, staged: bool) -> Result<()> {
    let mut diff_args = vec!["diff", "--binary"];
    if staged {
        diff_args.push("--cached");
    }
    let output = run_git(git_root, &diff_args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(DeadreckonError::InvalidInput(if stderr.is_empty() {
            format!("git {:?} failed", diff_args)
        } else {
            stderr
        }));
    }
    if output.stdout.is_empty() {
        return Ok(());
    }
    let output = run_git_with_input(
        worktree_path,
        &["apply", "--whitespace=nowarn", "-"],
        &output.stdout,
    )?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(DeadreckonError::InvalidInput(if stderr.is_empty() {
            "git apply dirty diff failed".to_string()
        } else {
            stderr
        }))
    }
}

fn required_path<'a>(value: Option<&'a PathBuf>, field: &str) -> Result<&'a Path> {
    value
        .map(PathBuf::as_path)
        .ok_or_else(|| DeadreckonError::InvalidInput(format!("codebase missing {field}")))
}

fn required_string<'a>(value: Option<&'a str>, field: &str) -> Result<&'a str> {
    value.ok_or_else(|| DeadreckonError::InvalidInput(format!("codebase missing {field}")))
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!("path is not valid UTF-8: {}", path.display()))
    })
}

fn canonical_or_self(path: &Path) -> Result<PathBuf> {
    match path.canonicalize() {
        Ok(path) => Ok(path),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(source) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(test)]
mod trusted_record_tests {
    use tempfile::TempDir;

    use super::{
        CodebaseMode, CodebaseRecord, copy_source_to_working, read_run_codebase_record,
        write_codebase_record, write_trusted_codebase_record,
    };

    #[test]
    fn run_record_prefers_control_plane_copy_over_workspace_tampering() {
        let temp = TempDir::new().expect("tempdir");
        let run_root = temp.path().join("run");
        let working = temp.path().join("working");
        std::fs::create_dir_all(&run_root).expect("run root");
        std::fs::create_dir_all(&working).expect("working");
        let trusted = CodebaseRecord::fresh();
        write_codebase_record(&working, &trusted).expect("workspace record");
        write_trusted_codebase_record(&run_root, &trusted).expect("trusted record");

        let mut tampered = trusted.clone();
        tampered.mode = CodebaseMode::InPlace;
        tampered.source_path = Some(temp.path().join("redirected-source"));
        write_codebase_record(&working, &tampered).expect("tampered workspace record");

        assert_eq!(
            read_run_codebase_record(&run_root, &working).expect("run record"),
            trusted
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_source_preserves_symlinks_and_executable_mode_without_following() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let temp = TempDir::new().expect("tempdir");
        let source = temp.path().join("source");
        let working = temp.path().join("working");
        std::fs::create_dir_all(source.join("bin")).expect("source bin");
        std::fs::create_dir_all(source.join("target")).expect("ignored target");
        std::fs::create_dir_all(source.join(".build/debug")).expect("ignored Swift build");
        std::fs::write(source.join("bin/tool"), "#!/bin/sh\nexit 0\n").expect("tool");
        std::fs::set_permissions(
            source.join("bin/tool"),
            std::fs::Permissions::from_mode(0o751),
        )
        .expect("tool mode");
        symlink("../missing-outside-secret", source.join("outside-link")).expect("source link");
        std::fs::write(source.join("target/ignored"), "ignored\n").expect("ignored file");
        std::fs::write(source.join(".build/debug/App"), "ignored\n")
            .expect("ignored Swift build artifact");

        copy_source_to_working(&source, &working).expect("copy source");

        let copied_link = working.join("outside-link");
        assert!(
            std::fs::symlink_metadata(&copied_link)
                .expect("copied link metadata")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_link(&copied_link).expect("raw copied target"),
            std::path::Path::new("../missing-outside-secret")
        );
        assert_eq!(
            std::fs::metadata(working.join("bin/tool"))
                .expect("copied tool")
                .permissions()
                .mode()
                & 0o777,
            0o751
        );
        assert!(!working.join("target").exists());
        assert!(!working.join(".build").exists());
    }

    #[cfg(unix)]
    #[test]
    fn copy_source_rejects_unsupported_special_files() {
        use std::os::unix::net::UnixListener;

        let temp = TempDir::new().expect("tempdir");
        let source = temp.path().join("source");
        let working = temp.path().join("working");
        std::fs::create_dir_all(&source).expect("source");
        let _listener = UnixListener::bind(source.join("provider.sock")).expect("socket");

        let error =
            copy_source_to_working(&source, &working).expect_err("special file must be refused");

        assert!(
            error
                .to_string()
                .contains("provider.sock (unsupported entry)"),
            "{error}"
        );
        assert!(!working.join("provider.sock").exists());
    }
}
