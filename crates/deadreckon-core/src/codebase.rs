use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};

pub const CODEBASE_RECORD_VERSION: u32 = 1;
pub const CODEBASE_RECORD_PATH: &str = ".deadreckon/codebase.json";

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
    pub worktree_path: Option<PathBuf>,
    pub dirty_files_seeded: bool,
    pub head_was_detached: bool,
    pub created_at: DateTime<Utc>,
    pub deadreckon_version: String,
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
            worktree_path: None,
            dirty_files_seeded: false,
            head_was_detached: false,
            created_at: Utc::now(),
            deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
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

pub fn find_git_root(path: &Path) -> Result<Option<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|source| DeadreckonError::Io {
            path: PathBuf::from("git"),
            source,
        })?;
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
