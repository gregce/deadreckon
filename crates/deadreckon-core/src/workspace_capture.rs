//! Trusted, bounded workspace traversal for snapshots and artifact indexes.
//!
//! A capture policy is frozen before the first provider turn. Later walks use
//! those trusted ignore rules rather than agent-editable ignore files, while
//! Git-tracked paths remain eligible even when an ignore or generated-output
//! hint would otherwise prune them.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::artifact_policy::{
    WorkspacePathClass, classify_workspace_path, is_checkpointable_workspace_path,
    is_deliverable_workspace_path, is_recoverable_workspace_path, runtime_output_root,
};
use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::git::run_git;
use crate::state::{PipelineState, atomic_write_json};

pub const WORKSPACE_CAPTURE_POLICY_JSON: &str = "workspace-capture-policy.json";
pub const SOURCE_HYDRATION_MANIFEST_JSON: &str = "source-hydration-manifest.json";
pub const WORKSPACE_CAPTURE_POLICY_VERSION: u32 = 1;
pub const WORKSPACE_CAPTURE_MANIFEST_VERSION: u32 = 1;

const DEFAULT_MAX_FILES: u64 = 100_000;
const DEFAULT_MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;
const DEFAULT_MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
const DEFAULT_MAX_TRAVERSAL_MILLIS: u64 = 10_000;
const DEFAULT_SUSPICIOUS_FILES: u64 = 2_000;
const DEFAULT_SUSPICIOUS_BYTES: u64 = 128 * 1024 * 1024;
const DEFAULT_SUSPICIOUS_GENERATED_PERCENT: u8 = 60;
const MAX_RECORDED_OMISSIONS: usize = 256;
const MAX_POLICY_DISCOVERY_ENTRIES: u64 = 100_000;
const POLICY_DISCOVERY_MILLIS: u64 = 3_000;
const ECOSYSTEM_QUERY_MILLIS: u64 = 2_000;
const MAX_ECOSYSTEM_QUERY_BYTES: u64 = 2 * 1024 * 1024;
const OMITTED_SUMMARY_FILES: u64 = 50_000;
const OMITTED_SUMMARY_MILLIS: u64 = 2_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCapturePolicy {
    pub schema_version: u32,
    pub frozen_at: DateTime<Utc>,
    pub ignores: Vec<FrozenIgnoreSource>,
    pub frozen_tracked_paths: Vec<EncodedWorkspacePath>,
    pub output_roots: Vec<GeneratedOutputRoot>,
    pub budgets: CaptureBudgets,
    #[serde(default)]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_git_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_git_index_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenIgnoreSource {
    pub kind: FrozenIgnoreKind,
    pub base: EncodedWorkspacePath,
    pub origin: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrozenIgnoreKind {
    GlobalGit,
    GitExclude,
    Gitignore,
    Ignore,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeneratedOutputRoot {
    pub path: EncodedWorkspacePath,
    pub source: GeneratedOutputSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneratedOutputSource {
    CargoMetadata,
    SwiftPackage,
    Bazel,
    CmakeCache,
    Gradle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureBudgets {
    pub max_files: u64,
    pub max_total_bytes: u64,
    pub max_file_bytes: u64,
    pub max_traversal_millis: u64,
    pub suspicious_files: u64,
    pub suspicious_bytes: u64,
    pub suspicious_generated_percent: u8,
}

impl Default for CaptureBudgets {
    fn default() -> Self {
        Self {
            max_files: DEFAULT_MAX_FILES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_traversal_millis: DEFAULT_MAX_TRAVERSAL_MILLIS,
            suspicious_files: DEFAULT_SUSPICIOUS_FILES,
            suspicious_bytes: DEFAULT_SUSPICIOUS_BYTES,
            suspicious_generated_percent: DEFAULT_SUSPICIOUS_GENERATED_PERCENT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "encoding", content = "value", rename_all = "snake_case")]
pub enum EncodedWorkspacePath {
    Utf8(String),
    #[cfg(unix)]
    UnixBytesHex(String),
    #[cfg(windows)]
    WindowsWide(Vec<u16>),
}

impl EncodedWorkspacePath {
    pub fn from_path(path: &Path) -> Self {
        if let Some(value) = path.to_str() {
            return Self::Utf8(value.to_string());
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            return Self::UnixBytesHex(hex_encode(path.as_os_str().as_bytes()));
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt as _;
            return Self::WindowsWide(path.as_os_str().encode_wide().collect());
        }
        #[allow(unreachable_code)]
        Self::Utf8(path.to_string_lossy().into_owned())
    }

    pub fn to_path_buf(&self) -> Result<PathBuf> {
        match self {
            Self::Utf8(value) => Ok(PathBuf::from(value)),
            #[cfg(unix)]
            Self::UnixBytesHex(value) => {
                use std::os::unix::ffi::OsStringExt as _;
                Ok(PathBuf::from(OsString::from_vec(hex_decode(value)?)))
            }
            #[cfg(windows)]
            Self::WindowsWide(value) => {
                use std::os::windows::ffi::OsStringExt as _;
                Ok(PathBuf::from(OsString::from_wide(value)))
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureProjection {
    Source,
    Recoverable,
    Checkpoint,
    Deliverable,
    WorkspaceGuard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapturePurpose {
    SourceHydration,
    TurnSnapshot,
    FlightCheckpoint,
    WorkingInventory,
    DeliverableIndex,
    WorkspaceGuard,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureEntry {
    pub relative: PathBuf,
    pub source: PathBuf,
    pub kind: CaptureEntryKind,
    pub size: u64,
    pub tracked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureEntryKind {
    RegularFile,
    SymbolicLink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCapturePlan {
    pub entries: Vec<CaptureEntry>,
    pub manifest: WorkspaceCaptureManifest,
}

impl WorkspaceCapturePlan {
    pub fn require_complete(&self, operation: &str) -> Result<()> {
        if self.manifest.partial {
            return Err(DeadreckonError::InvalidInput(format!(
                "{operation} refused a partial workspace capture: {}",
                self.manifest.omission_summary()
            )));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCaptureManifest {
    pub schema_version: u32,
    pub policy_sha256: String,
    pub purpose: CapturePurpose,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub partial: bool,
    pub included_files: u64,
    pub included_bytes: u64,
    pub tracked_files: u64,
    pub omissions: Vec<CaptureOmission>,
    #[serde(default)]
    pub omissions_truncated: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitHydrationState>,
}

impl WorkspaceCaptureManifest {
    pub fn omission_summary(&self) -> String {
        let mut labels = self
            .omissions
            .iter()
            .take(3)
            .map(|omission| format!("{} ({})", omission.path.display(), omission.reason.label()))
            .collect::<Vec<_>>();
        if self.omissions.len() > 3 || self.omissions_truncated > 0 {
            labels.push(format!(
                "{} more",
                self.omissions
                    .len()
                    .saturating_sub(3)
                    .saturating_add(self.omissions_truncated as usize)
            ));
        }
        if labels.is_empty() {
            "capture policy reported an unspecified omission".to_string()
        } else {
            labels.join(", ")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureOmission {
    pub path: PathBuf,
    pub reason: CaptureOmissionReason,
    #[serde(default)]
    pub file_count: u64,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata_sha256: Option<String>,
    #[serde(default)]
    pub summary_partial: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureOmissionReason {
    FrozenIgnore,
    GeneratedOutput,
    SuspiciousGeneratedSubtree,
    OversizeFile,
    FileBudget,
    ByteBudget,
    TraversalDeadline,
    UnsupportedEntry,
    IoError,
}

impl CaptureOmissionReason {
    fn label(self) -> &'static str {
        match self {
            Self::FrozenIgnore => "frozen ignore",
            Self::GeneratedOutput => "generated output",
            Self::SuspiciousGeneratedSubtree => "suspicious generated subtree",
            Self::OversizeFile => "oversize file",
            Self::FileBudget => "file budget",
            Self::ByteBudget => "byte budget",
            Self::TraversalDeadline => "traversal deadline",
            Self::UnsupportedEntry => "unsupported entry",
            Self::IoError => "I/O error",
        }
    }

    fn makes_partial(self) -> bool {
        !matches!(self, Self::FrozenIgnore | Self::GeneratedOutput)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHydrationState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_sha256: Option<String>,
    pub tracked_files: u64,
}

#[derive(Debug)]
struct CompiledIgnoreSource {
    kind: FrozenIgnoreKind,
    depth: usize,
    matcher: Gitignore,
}

#[derive(Debug, Clone)]
struct TraversalContext {
    root: PathBuf,
    projection: CaptureProjection,
    ignores: Arc<Vec<CompiledIgnoreSource>>,
    tracked: Arc<BTreeSet<PathBuf>>,
    tracked_directories: Arc<BTreeSet<PathBuf>>,
    output_roots: Arc<BTreeSet<PathBuf>>,
    pruned: Arc<Mutex<BTreeMap<PathBuf, CaptureOmissionReason>>>,
}

#[derive(Debug)]
struct SubtreeAccumulator {
    file_count: u64,
    total_bytes: u64,
    generated_files: u64,
    hasher: Sha256,
}

impl Default for SubtreeAccumulator {
    fn default() -> Self {
        Self {
            file_count: 0,
            total_bytes: 0,
            generated_files: 0,
            hasher: Sha256::new(),
        }
    }
}

#[derive(Debug, Default)]
struct PolicyDiscovery {
    ignores: Vec<FrozenIgnoreSource>,
    markers: Vec<PathBuf>,
    warnings: Vec<String>,
}

pub fn workspace_capture_policy_path(run_root: &Path) -> PathBuf {
    run_root.join(WORKSPACE_CAPTURE_POLICY_JSON)
}

pub fn freeze_workspace_capture_policy(root: &Path) -> Result<WorkspaceCapturePolicy> {
    let mut discovery = discover_policy_inputs(root);
    freeze_git_exclude_sources(root, &mut discovery);
    let tracked = git_tracked_paths(root)?;
    let (git_head, git_index_sha256) = git_hydration_identity(root);
    let output_roots = discover_output_roots(root, &discovery.markers, &mut discovery.warnings);
    Ok(WorkspaceCapturePolicy {
        schema_version: WORKSPACE_CAPTURE_POLICY_VERSION,
        frozen_at: Utc::now(),
        ignores: discovery.ignores,
        frozen_tracked_paths: tracked
            .iter()
            .map(|path| EncodedWorkspacePath::from_path(path))
            .collect(),
        output_roots,
        budgets: CaptureBudgets::default(),
        warnings: discovery.warnings,
        frozen_git_head: git_head,
        frozen_git_index_sha256: git_index_sha256,
    })
}

pub fn write_workspace_capture_policy(
    run_root: &Path,
    policy: &WorkspaceCapturePolicy,
) -> Result<()> {
    atomic_write_json(&workspace_capture_policy_path(run_root), policy)
}

pub fn read_workspace_capture_policy(run_root: &Path) -> Result<WorkspaceCapturePolicy> {
    let path = workspace_capture_policy_path(run_root);
    let raw = fs::read(&path).with_path(&path)?;
    let policy: WorkspaceCapturePolicy = serde_json::from_slice(&raw).with_json_path(&path)?;
    if policy.schema_version != WORKSPACE_CAPTURE_POLICY_VERSION {
        return Err(DeadreckonError::InvalidInput(format!(
            "unsupported workspace capture policy version {} at {}",
            policy.schema_version,
            path.display()
        )));
    }
    Ok(policy)
}

pub fn ensure_workspace_capture_policy(state: &PipelineState) -> Result<WorkspaceCapturePolicy> {
    let path = workspace_capture_policy_path(&state.run_root);
    if path.is_file() {
        return read_workspace_capture_policy(&state.run_root);
    }
    let policy = freeze_workspace_capture_policy(&state.working_dir)?;
    write_workspace_capture_policy(&state.run_root, &policy)?;
    Ok(policy)
}

pub fn capture_workspace(
    root: &Path,
    policy: &WorkspaceCapturePolicy,
    projection: CaptureProjection,
    purpose: CapturePurpose,
) -> Result<WorkspaceCapturePlan> {
    let started_at = Utc::now();
    let started = Instant::now();
    let policy_sha256 = policy_sha256(policy)?;
    let mut tracked = decode_paths(&policy.frozen_tracked_paths)?;
    tracked.extend(git_tracked_paths(root)?);
    let tracked_directories = tracked_parent_directories(&tracked);
    let output_roots = decode_output_roots(&policy.output_roots)?;
    let ignores = Arc::new(compile_ignores(root, &policy.ignores)?);
    let pruned = Arc::new(Mutex::new(BTreeMap::new()));
    let context = TraversalContext {
        root: root.to_path_buf(),
        projection,
        ignores,
        tracked: Arc::new(tracked.clone()),
        tracked_directories: Arc::new(tracked_directories),
        output_roots: Arc::new(output_roots),
        pruned: Arc::clone(&pruned),
    };
    let filter_context = context;
    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(false)
        .hidden(false)
        .follow_links(false)
        .filter_entry(move |entry| filter_capture_entry(&filter_context, entry));

    let mut entries = Vec::new();
    let mut omissions = Vec::new();
    let mut omissions_truncated = 0_u64;
    let mut discovered_files = 0_u64;
    let mut discovered_bytes = 0_u64;
    let mut subtree_stats = BTreeMap::<PathBuf, SubtreeAccumulator>::new();
    let mut stopped = false;
    for item in builder.build() {
        if started.elapsed() > Duration::from_millis(policy.budgets.max_traversal_millis) {
            push_omission(
                &mut omissions,
                &mut omissions_truncated,
                CaptureOmission {
                    path: PathBuf::from("."),
                    reason: CaptureOmissionReason::TraversalDeadline,
                    file_count: discovered_files,
                    total_bytes: discovered_bytes,
                    metadata_sha256: None,
                    summary_partial: true,
                },
            );
            stopped = true;
            break;
        }
        let entry = match item {
            Ok(entry) => entry,
            Err(_error) => {
                push_omission(
                    &mut omissions,
                    &mut omissions_truncated,
                    CaptureOmission {
                        path: PathBuf::from("."),
                        reason: CaptureOmissionReason::IoError,
                        file_count: 0,
                        total_bytes: 0,
                        metadata_sha256: None,
                        summary_partial: true,
                    },
                );
                continue;
            }
        };
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|error| {
            DeadreckonError::InvalidInput(format!("capture path prefix error: {error}"))
        })?;
        if relative.as_os_str().is_empty()
            || entry
                .file_type()
                .is_some_and(|file_type| file_type.is_dir())
        {
            continue;
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                push_omission(
                    &mut omissions,
                    &mut omissions_truncated,
                    CaptureOmission {
                        path: relative.to_path_buf(),
                        reason: CaptureOmissionReason::IoError,
                        file_count: 1,
                        total_bytes: 0,
                        metadata_sha256: None,
                        summary_partial: true,
                    },
                );
                continue;
            }
        };
        let file_type = metadata.file_type();
        let (kind, size) = if file_type.is_file() {
            (CaptureEntryKind::RegularFile, metadata.len())
        } else if file_type.is_symlink() {
            let target = fs::read_link(path).with_path(path)?;
            (CaptureEntryKind::SymbolicLink, path_identity_len(&target))
        } else {
            push_omission(
                &mut omissions,
                &mut omissions_truncated,
                CaptureOmission {
                    path: relative.to_path_buf(),
                    reason: CaptureOmissionReason::UnsupportedEntry,
                    file_count: 1,
                    total_bytes: metadata.len(),
                    metadata_sha256: None,
                    summary_partial: true,
                },
            );
            continue;
        };
        discovered_files = discovered_files.saturating_add(1);
        discovered_bytes = discovered_bytes.saturating_add(size);
        update_subtree_stats(&mut subtree_stats, relative, size, &metadata);
        if size > policy.budgets.max_file_bytes {
            push_omission(
                &mut omissions,
                &mut omissions_truncated,
                CaptureOmission {
                    path: relative.to_path_buf(),
                    reason: CaptureOmissionReason::OversizeFile,
                    file_count: 1,
                    total_bytes: size,
                    metadata_sha256: None,
                    summary_partial: false,
                },
            );
            continue;
        }
        if discovered_files > policy.budgets.max_files {
            push_omission(
                &mut omissions,
                &mut omissions_truncated,
                CaptureOmission {
                    path: relative.to_path_buf(),
                    reason: CaptureOmissionReason::FileBudget,
                    file_count: discovered_files,
                    total_bytes: discovered_bytes,
                    metadata_sha256: None,
                    summary_partial: true,
                },
            );
            stopped = true;
            break;
        }
        if discovered_bytes > policy.budgets.max_total_bytes {
            push_omission(
                &mut omissions,
                &mut omissions_truncated,
                CaptureOmission {
                    path: relative.to_path_buf(),
                    reason: CaptureOmissionReason::ByteBudget,
                    file_count: discovered_files,
                    total_bytes: discovered_bytes,
                    metadata_sha256: None,
                    summary_partial: true,
                },
            );
            stopped = true;
            break;
        }
        entries.push(CaptureEntry {
            relative: relative.to_path_buf(),
            source: path.to_path_buf(),
            kind,
            size,
            tracked: tracked.contains(relative),
        });
    }

    let pruned = pruned
        .lock()
        .map_err(|_| DeadreckonError::InvalidInput("capture prune ledger poisoned".to_string()))?
        .clone();
    for (path, reason) in pruned {
        let summary = summarize_omitted_subtree(root, &path);
        push_omission(
            &mut omissions,
            &mut omissions_truncated,
            CaptureOmission {
                path,
                reason,
                file_count: summary.file_count,
                total_bytes: summary.total_bytes,
                metadata_sha256: Some(summary.metadata_sha256),
                summary_partial: summary.partial,
            },
        );
    }

    if !stopped {
        let suspicious = suspicious_subtrees(&subtree_stats, &policy.budgets);
        if !suspicious.is_empty() {
            entries.retain(|entry| {
                entry.tracked
                    || !suspicious
                        .iter()
                        .any(|(root, _)| entry.relative.starts_with(root))
            });
            for (path, summary) in suspicious {
                push_omission(
                    &mut omissions,
                    &mut omissions_truncated,
                    CaptureOmission {
                        path,
                        reason: CaptureOmissionReason::SuspiciousGeneratedSubtree,
                        file_count: summary.file_count,
                        total_bytes: summary.total_bytes,
                        metadata_sha256: Some(summary.metadata_sha256),
                        summary_partial: false,
                    },
                );
            }
        }
    }
    entries.sort_by(|left, right| left.relative.cmp(&right.relative));
    let included_files = entries.len() as u64;
    let included_bytes = entries.iter().map(|entry| entry.size).sum();
    let tracked_files = entries.iter().filter(|entry| entry.tracked).count() as u64;
    let partial = omissions
        .iter()
        .any(|omission| omission.reason.makes_partial());
    let (head, index_sha256) = git_hydration_identity(root);
    Ok(WorkspaceCapturePlan {
        entries,
        manifest: WorkspaceCaptureManifest {
            schema_version: WORKSPACE_CAPTURE_MANIFEST_VERSION,
            policy_sha256,
            purpose,
            started_at,
            completed_at: Utc::now(),
            partial,
            included_files,
            included_bytes,
            tracked_files,
            omissions,
            omissions_truncated,
            git: (!tracked.is_empty() || head.is_some()).then_some(GitHydrationState {
                head,
                index_sha256,
                tracked_files: tracked.len() as u64,
            }),
        },
    })
}

pub fn write_capture_manifest(path: &Path, manifest: &WorkspaceCaptureManifest) -> Result<()> {
    atomic_write_json(path, manifest)
}

pub fn read_capture_manifest(path: &Path) -> Result<WorkspaceCaptureManifest> {
    let raw = fs::read(path).with_path(path)?;
    serde_json::from_slice(&raw).with_json_path(path)
}

/// Materialize the selected leaves without following symbolic links.
///
/// The capture plan has already established the trusted traversal boundary.
/// Materialization therefore creates only parent directories required by an
/// admitted leaf and refuses a destination hierarchy containing a symlink.
pub fn materialize_capture_plan(plan: &WorkspaceCapturePlan, destination: &Path) -> Result<()> {
    ensure_materialization_root(destination)?;
    for entry in &plan.entries {
        let target = destination.join(&entry.relative);
        ensure_materialization_parent(destination, &entry.relative)?;
        remove_materialization_target(&target)?;
        match entry.kind {
            CaptureEntryKind::RegularFile => {
                let metadata = fs::symlink_metadata(&entry.source).with_path(&entry.source)?;
                fs::copy(&entry.source, &target).with_path(&entry.source)?;
                fs::set_permissions(&target, metadata.permissions()).with_path(&target)?;
            }
            CaptureEntryKind::SymbolicLink => {
                let link_target = fs::read_link(&entry.source).with_path(&entry.source)?;
                create_materialized_symlink(&entry.source, &link_target, &target)?;
            }
        }
    }
    Ok(())
}

fn ensure_materialization_root(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(DeadreckonError::InvalidInput(format!(
            "workspace capture destination is not a directory: {}",
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

fn ensure_materialization_parent(root: &Path, relative: &Path) -> Result<()> {
    let Some(parent) = relative.parent() else {
        return Ok(());
    };
    let mut current = root.to_path_buf();
    for component in parent.components() {
        let Component::Normal(name) = component else {
            return Err(DeadreckonError::InvalidInput(format!(
                "workspace capture contains an unsafe path: {}",
                relative.display()
            )));
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(DeadreckonError::InvalidInput(format!(
                    "workspace capture destination hierarchy is not a directory: {}",
                    current.display()
                )));
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).with_path(&current)?;
            }
            Err(source) => {
                return Err(DeadreckonError::Io {
                    path: current,
                    source,
                });
            }
        }
    }
    Ok(())
}

fn remove_materialization_target(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => fs::remove_dir_all(path).with_path(path),
        Ok(_) => fs::remove_file(path).with_path(path),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(unix)]
fn create_materialized_symlink(_source: &Path, target: &Path, destination: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, destination).with_path(destination)
}

#[cfg(windows)]
fn create_materialized_symlink(source: &Path, target: &Path, destination: &Path) -> Result<()> {
    use std::os::windows::fs::{FileTypeExt as _, symlink_dir, symlink_file};

    let file_type = fs::symlink_metadata(source).with_path(source)?.file_type();
    if file_type.is_symlink_dir() {
        symlink_dir(target, destination).with_path(destination)
    } else {
        symlink_file(target, destination).with_path(destination)
    }
}

#[cfg(not(any(unix, windows)))]
fn create_materialized_symlink(_source: &Path, _target: &Path, destination: &Path) -> Result<()> {
    Err(DeadreckonError::InvalidInput(format!(
        "symbolic-link capture is unsupported on this platform: {}",
        destination.display()
    )))
}

fn discover_policy_inputs(root: &Path) -> PolicyDiscovery {
    let started = Instant::now();
    let mut discovery = PolicyDiscovery::default();
    if !root.is_dir() {
        return discovery;
    }
    let root_owned = root.to_path_buf();
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .parents(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .follow_links(false)
        .filter_entry(move |entry| {
            let relative = entry
                .path()
                .strip_prefix(&root_owned)
                .unwrap_or(entry.path());
            if relative.as_os_str().is_empty() {
                return true;
            }
            !is_git_control_path(relative) && runtime_output_root(relative).is_none()
        });
    let mut seen = 0_u64;
    for entry in builder.build() {
        if seen >= MAX_POLICY_DISCOVERY_ENTRIES
            || started.elapsed() > Duration::from_millis(POLICY_DISCOVERY_MILLIS)
        {
            discovery.warnings.push(
                "ignore/ecosystem discovery reached its bounded scan limit; root and Git-level rules remain frozen"
                    .to_string(),
            );
            break;
        }
        seen = seen.saturating_add(1);
        let Ok(entry) = entry else {
            continue;
        };
        if !entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            continue;
        }
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
        if matches!(name, ".gitignore" | ".ignore") {
            let kind = if name == ".gitignore" {
                FrozenIgnoreKind::Gitignore
            } else {
                FrozenIgnoreKind::Ignore
            };
            if let Some(source) = frozen_ignore_file(root, relative, kind, &mut discovery.warnings)
            {
                discovery.ignores.push(source);
            }
        }
        if is_ecosystem_marker(name) {
            discovery.markers.push(relative.to_path_buf());
        }
    }
    discovery.ignores.sort_by(|left, right| {
        (left.kind, path_depth_encoded(&left.base), &left.origin).cmp(&(
            right.kind,
            path_depth_encoded(&right.base),
            &right.origin,
        ))
    });
    discovery.markers.sort();
    discovery.markers.dedup();
    discovery
}

fn frozen_ignore_file(
    root: &Path,
    relative: &Path,
    kind: FrozenIgnoreKind,
    warnings: &mut Vec<String>,
) -> Option<FrozenIgnoreSource> {
    let path = root.join(relative);
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) => {
            warnings.push(format!(
                "could not freeze ignore file {}: {error}",
                relative.display()
            ));
            return None;
        }
    };
    Some(FrozenIgnoreSource {
        kind,
        base: EncodedWorkspacePath::from_path(relative.parent().unwrap_or_else(|| Path::new(""))),
        origin: relative.to_string_lossy().into_owned(),
        lines: raw.lines().map(str::to_string).collect(),
    })
}

fn freeze_git_exclude_sources(root: &Path, discovery: &mut PolicyDiscovery) {
    if let Some(path) = git_config_path(root, &["rev-parse", "--git-path", "info/exclude"])
        && let Some(source) = frozen_external_ignore_file(
            &path,
            FrozenIgnoreKind::GitExclude,
            "git info/exclude",
            &mut discovery.warnings,
        )
    {
        discovery.ignores.push(source);
    }
    if let Some(path) = global_gitignore_path(root)
        && let Some(source) = frozen_external_ignore_file(
            &path,
            FrozenIgnoreKind::GlobalGit,
            "global Git excludes",
            &mut discovery.warnings,
        )
    {
        discovery.ignores.push(source);
    }
}

fn frozen_external_ignore_file(
    path: &Path,
    kind: FrozenIgnoreKind,
    origin: &str,
    warnings: &mut Vec<String>,
) -> Option<FrozenIgnoreSource> {
    if !path.is_file() {
        return None;
    }
    match fs::read_to_string(path) {
        Ok(raw) => Some(FrozenIgnoreSource {
            kind,
            base: EncodedWorkspacePath::from_path(Path::new("")),
            origin: format!("{origin}: {}", path.display()),
            lines: raw.lines().map(str::to_string).collect(),
        }),
        Err(error) => {
            warnings.push(format!(
                "could not freeze {origin} {}: {error}",
                path.display()
            ));
            None
        }
    }
}

fn global_gitignore_path(root: &Path) -> Option<PathBuf> {
    if let Some(path) = git_config_path(root, &["config", "--path", "--get", "core.excludesFile"]) {
        return Some(path);
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(xdg).join("git/ignore"));
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/git/ignore"))
}

fn git_config_path(root: &Path, args: &[&str]) -> Option<PathBuf> {
    let output = run_git(root, args).ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        None
    } else {
        let path = PathBuf::from(value);
        Some(if path.is_absolute() {
            path
        } else {
            root.join(path)
        })
    }
}

fn discover_output_roots(
    root: &Path,
    markers: &[PathBuf],
    warnings: &mut Vec<String>,
) -> Vec<GeneratedOutputRoot> {
    let mut roots = BTreeMap::<PathBuf, GeneratedOutputSource>::new();
    for marker in markers {
        let parent = marker.parent().unwrap_or_else(|| Path::new(""));
        match marker
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
        {
            "Package.swift" => {
                roots.insert(parent.join(".build"), GeneratedOutputSource::SwiftPackage);
            }
            "MODULE.bazel" | "WORKSPACE" | "WORKSPACE.bazel" => {
                for name in ["bazel-bin", "bazel-out", "bazel-testlogs"] {
                    roots.insert(parent.join(name), GeneratedOutputSource::Bazel);
                }
            }
            "CMakeCache.txt" if !parent.as_os_str().is_empty() => {
                roots.insert(parent.to_path_buf(), GeneratedOutputSource::CmakeCache);
            }
            "build.gradle" | "build.gradle.kts" => {
                roots.insert(parent.join("build"), GeneratedOutputSource::Gradle);
                roots.insert(parent.join(".gradle"), GeneratedOutputSource::Gradle);
            }
            _ => {}
        }
    }
    if markers
        .iter()
        .any(|marker| marker.file_name().is_some_and(|name| name == "Cargo.toml"))
    {
        match cargo_metadata_target_directory(root) {
            Ok(Some(path)) => {
                if let Ok(relative) = path.strip_prefix(root)
                    && !relative.as_os_str().is_empty()
                {
                    roots.insert(relative.to_path_buf(), GeneratedOutputSource::CargoMetadata);
                }
            }
            Ok(None) => {}
            Err(error) => {
                warnings.push(format!("cargo metadata output discovery skipped: {error}"))
            }
        }
    }
    roots
        .into_iter()
        .map(|(path, source)| GeneratedOutputRoot {
            path: EncodedWorkspacePath::from_path(&path),
            source,
        })
        .collect()
}

fn cargo_metadata_target_directory(root: &Path) -> Result<Option<PathBuf>> {
    if !root.join("Cargo.toml").is_file() {
        return Ok(None);
    }
    let Some(raw) = bounded_command_stdout(
        root,
        "cargo",
        &[
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ],
        Duration::from_millis(ECOSYSTEM_QUERY_MILLIS),
        MAX_ECOSYSTEM_QUERY_BYTES,
    )?
    else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_slice(&raw).map_err(|error| {
        DeadreckonError::InvalidInput(format!("cargo metadata returned invalid JSON: {error}"))
    })?;
    Ok(value
        .get("target_directory")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from))
}

fn bounded_command_stdout(
    cwd: &Path,
    program: &str,
    args: &[&str],
    timeout: Duration,
    max_bytes: u64,
) -> Result<Option<Vec<u8>>> {
    let mut stdout = tempfile::tempfile().map_err(|source| DeadreckonError::Io {
        path: PathBuf::from("temporary ecosystem query output"),
        source,
    })?;
    let stdout_child = stdout.try_clone().map_err(|source| DeadreckonError::Io {
        path: PathBuf::from("temporary ecosystem query output"),
        source,
    })?;
    let mut child = match Command::new(program)
        .current_dir(cwd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_child))
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: PathBuf::from(program),
                source,
            });
        }
    };
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|source| DeadreckonError::Io {
            path: PathBuf::from(program),
            source,
        })? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if !status.success() {
        return Ok(None);
    }
    let length = stdout
        .metadata()
        .map_err(|source| DeadreckonError::Io {
            path: PathBuf::from("temporary ecosystem query output"),
            source,
        })?
        .len();
    if length > max_bytes {
        return Ok(None);
    }
    stdout
        .seek(SeekFrom::Start(0))
        .map_err(|source| DeadreckonError::Io {
            path: PathBuf::from("temporary ecosystem query output"),
            source,
        })?;
    let mut raw = Vec::with_capacity(length as usize);
    stdout
        .read_to_end(&mut raw)
        .map_err(|source| DeadreckonError::Io {
            path: PathBuf::from("temporary ecosystem query output"),
            source,
        })?;
    Ok(Some(raw))
}

fn compile_ignores(
    root: &Path,
    sources: &[FrozenIgnoreSource],
) -> Result<Vec<CompiledIgnoreSource>> {
    let mut compiled = Vec::new();
    for source in sources {
        let base = source.base.to_path_buf()?;
        let mut builder = GitignoreBuilder::new(root.join(&base));
        for (index, line) in source.lines.iter().enumerate() {
            builder
                .add_line(Some(PathBuf::from(&source.origin)), line)
                .map_err(|error| {
                    DeadreckonError::InvalidInput(format!(
                        "frozen ignore rule {}:{} is invalid: {error}",
                        source.origin,
                        index + 1
                    ))
                })?;
        }
        let matcher = builder.build().map_err(|error| {
            DeadreckonError::InvalidInput(format!(
                "could not compile frozen ignore source {}: {error}",
                source.origin
            ))
        })?;
        compiled.push(CompiledIgnoreSource {
            kind: source.kind,
            depth: path_depth(&base),
            matcher,
        });
    }
    compiled.sort_by_key(|source| (ignore_precedence(source.kind), source.depth));
    Ok(compiled)
}

fn filter_capture_entry(context: &TraversalContext, entry: &ignore::DirEntry) -> bool {
    let relative = entry
        .path()
        .strip_prefix(&context.root)
        .unwrap_or(entry.path());
    if relative.as_os_str().is_empty() {
        return true;
    }
    let is_dir = entry
        .file_type()
        .is_some_and(|file_type| file_type.is_dir());
    let tracked = context.tracked.contains(relative);
    let tracked_descendant = is_dir && context.tracked_directories.contains(relative);
    if is_git_control_path(relative) {
        return context.projection == CaptureProjection::Recoverable
            && !is_dir
            && relative.components().count() == 1;
    }
    if !projection_boundary_allows(context.projection, relative, tracked || tracked_descendant) {
        record_pruned(context, relative, CaptureOmissionReason::GeneratedOutput);
        return false;
    }
    if tracked || tracked_descendant {
        return true;
    }
    if context
        .output_roots
        .iter()
        .any(|root| relative == root || relative.starts_with(root))
        || runtime_output_root(relative).is_some()
    {
        record_pruned(context, relative, CaptureOmissionReason::GeneratedOutput);
        return false;
    }
    if frozen_ignored(&context.ignores, entry.path(), is_dir) {
        record_pruned(context, relative, CaptureOmissionReason::FrozenIgnore);
        return false;
    }
    true
}

fn projection_boundary_allows(
    projection: CaptureProjection,
    relative: &Path,
    tracked_or_parent: bool,
) -> bool {
    match projection {
        CaptureProjection::Source => {
            if classify_workspace_path(relative) == WorkspacePathClass::RuntimeOnly {
                tracked_or_parent && runtime_output_root(relative).is_some()
            } else {
                true
            }
        }
        CaptureProjection::Recoverable => {
            is_recoverable_workspace_path(relative) || tracked_or_parent
        }
        CaptureProjection::Checkpoint => {
            is_checkpointable_workspace_path(relative)
                || (tracked_or_parent
                    && classify_workspace_path(relative) != WorkspacePathClass::EvidenceOnly
                    && !relative
                        .components()
                        .any(|component| component.as_os_str() == ".deadreckon"))
        }
        CaptureProjection::Deliverable => {
            is_deliverable_workspace_path(relative)
                || (tracked_or_parent
                    && classify_workspace_path(relative) == WorkspacePathClass::RuntimeOnly
                    && runtime_output_root(relative).is_some())
        }
        CaptureProjection::WorkspaceGuard => {
            classify_workspace_path(relative) != WorkspacePathClass::RuntimeOnly
                || (tracked_or_parent && runtime_output_root(relative).is_some())
        }
    }
}

fn frozen_ignored(sources: &[CompiledIgnoreSource], path: &Path, is_dir: bool) -> bool {
    let mut ignored = false;
    for source in sources {
        let matched = source.matcher.matched_path_or_any_parents(path, is_dir);
        if matched.is_ignore() {
            ignored = true;
        } else if matched.is_whitelist() {
            ignored = false;
        }
    }
    ignored
}

fn record_pruned(context: &TraversalContext, relative: &Path, reason: CaptureOmissionReason) {
    if let Ok(mut pruned) = context.pruned.lock() {
        if let Some(existing) = pruned.keys().find(|path| relative.starts_with(path)) {
            let _ = existing;
            return;
        }
        pruned.insert(relative.to_path_buf(), reason);
    }
}

fn git_tracked_paths(root: &Path) -> Result<BTreeSet<PathBuf>> {
    let output = match run_git(root, &["ls-files", "-z", "--cached", "--"]) {
        Ok(output) if output.status.success() => output,
        Ok(_) => return Ok(BTreeSet::new()),
        Err(DeadreckonError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(BTreeSet::new());
        }
        Err(error) => return Err(error),
    };
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
        .map(raw_git_path)
        .collect())
}

fn git_hydration_identity(root: &Path) -> (Option<String>, Option<String>) {
    let head = run_git(root, &["rev-parse", "--verify", "HEAD"])
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty());
    let index_sha256 = run_git(root, &["ls-files", "--stage", "-z", "--"])
        .ok()
        .filter(|output| output.status.success())
        .map(|output| sha256_bytes(&output.stdout));
    (head, index_sha256)
}

fn raw_git_path(raw: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        PathBuf::from(OsString::from_vec(raw.to_vec()))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(raw).into_owned())
    }
}

fn decode_paths(paths: &[EncodedWorkspacePath]) -> Result<BTreeSet<PathBuf>> {
    paths
        .iter()
        .map(EncodedWorkspacePath::to_path_buf)
        .collect()
}

fn decode_output_roots(outputs: &[GeneratedOutputRoot]) -> Result<BTreeSet<PathBuf>> {
    outputs
        .iter()
        .map(|output| output.path.to_path_buf())
        .collect()
}

fn tracked_parent_directories(tracked: &BTreeSet<PathBuf>) -> BTreeSet<PathBuf> {
    let mut directories = BTreeSet::new();
    for path in tracked {
        let mut parent = path.parent();
        while let Some(path) = parent {
            if path.as_os_str().is_empty() {
                break;
            }
            directories.insert(path.to_path_buf());
            parent = path.parent();
        }
    }
    directories
}

fn update_subtree_stats(
    stats: &mut BTreeMap<PathBuf, SubtreeAccumulator>,
    relative: &Path,
    size: u64,
    metadata: &fs::Metadata,
) {
    let Some(parent) = relative.parent() else {
        return;
    };
    let ancestors = parent
        .ancestors()
        .filter(|path| !path.as_os_str().is_empty());
    for ancestor in ancestors.take(3) {
        let accumulator = stats.entry(ancestor.to_path_buf()).or_default();
        accumulator.file_count = accumulator.file_count.saturating_add(1);
        accumulator.total_bytes = accumulator.total_bytes.saturating_add(size);
        if looks_generated(relative, metadata) {
            accumulator.generated_files = accumulator.generated_files.saturating_add(1);
        }
        update_metadata_digest(&mut accumulator.hasher, relative, size);
    }
}

fn suspicious_subtrees(
    stats: &BTreeMap<PathBuf, SubtreeAccumulator>,
    budgets: &CaptureBudgets,
) -> BTreeMap<PathBuf, SubtreeSummary> {
    let mut candidates = stats
        .iter()
        .filter(|(_, summary)| {
            summary.file_count >= budgets.suspicious_files
                && summary.total_bytes >= budgets.suspicious_bytes
                && summary.generated_files.saturating_mul(100)
                    >= summary
                        .file_count
                        .saturating_mul(u64::from(budgets.suspicious_generated_percent))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(path, _)| std::cmp::Reverse(path_depth(path)));
    let mut selected = BTreeMap::new();
    for (path, summary) in candidates {
        if selected
            .keys()
            .any(|chosen: &PathBuf| chosen.starts_with(path))
        {
            continue;
        }
        selected.insert(
            path.clone(),
            SubtreeSummary {
                file_count: summary.file_count,
                total_bytes: summary.total_bytes,
                metadata_sha256: format_sha256(summary.hasher.clone().finalize().as_slice()),
                partial: false,
            },
        );
    }
    selected
}

#[derive(Debug)]
struct SubtreeSummary {
    file_count: u64,
    total_bytes: u64,
    metadata_sha256: String,
    partial: bool,
}

fn summarize_omitted_subtree(root: &Path, relative: &Path) -> SubtreeSummary {
    let absolute = root.join(relative);
    let started = Instant::now();
    let mut files = 0_u64;
    let mut bytes = 0_u64;
    let mut hasher = Sha256::new();
    let mut partial = false;
    if !absolute.exists() {
        return SubtreeSummary {
            file_count: 0,
            total_bytes: 0,
            metadata_sha256: sha256_bytes(&[]),
            partial: false,
        };
    }
    let mut builder = WalkBuilder::new(&absolute);
    builder
        .standard_filters(false)
        .hidden(false)
        .follow_links(false);
    for entry in builder.build() {
        if files >= OMITTED_SUMMARY_FILES
            || started.elapsed() > Duration::from_millis(OMITTED_SUMMARY_MILLIS)
        {
            partial = true;
            break;
        }
        let Ok(entry) = entry else {
            partial = true;
            continue;
        };
        let Some(file_type) = entry.file_type() else {
            partial = true;
            continue;
        };
        if !file_type.is_file() && !file_type.is_symlink() {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            partial = true;
            continue;
        };
        let path = entry.path().strip_prefix(root).unwrap_or(entry.path());
        files = files.saturating_add(1);
        bytes = bytes.saturating_add(metadata.len());
        update_metadata_digest(&mut hasher, path, metadata.len());
    }
    SubtreeSummary {
        file_count: files,
        total_bytes: bytes,
        metadata_sha256: format_sha256(hasher.finalize().as_slice()),
        partial,
    }
}

fn looks_generated(path: &Path, metadata: &fs::Metadata) -> bool {
    const GENERATED_EXTENSIONS: &[&str] = &[
        "a",
        "app",
        "beam",
        "class",
        "d",
        "dll",
        "dylib",
        "exe",
        "jar",
        "o",
        "obj",
        "pdb",
        "pyc",
        "rlib",
        "rmeta",
        "so",
        "swiftdeps",
        "swiftmodule",
        "wasm",
    ];
    if path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            GENERATED_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
        })
    {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn push_omission(
    omissions: &mut Vec<CaptureOmission>,
    truncated: &mut u64,
    omission: CaptureOmission,
) {
    if omissions.len() < MAX_RECORDED_OMISSIONS {
        omissions.push(omission);
    } else {
        *truncated = truncated.saturating_add(1);
    }
}

fn policy_sha256(policy: &WorkspaceCapturePolicy) -> Result<String> {
    let raw = serde_json::to_vec(policy).map_err(|error| {
        DeadreckonError::InvalidInput(format!("could not serialize capture policy: {error}"))
    })?;
    Ok(sha256_bytes(&raw))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format_sha256(hasher.finalize().as_slice())
}

fn update_metadata_digest(hasher: &mut Sha256, path: &Path, size: u64) {
    update_path_identity(hasher, path);
    hasher.update(b"\0");
    hasher.update(size.to_le_bytes());
    hasher.update(b"\0");
}

fn update_path_identity(hasher: &mut Sha256, path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        hasher.update(path.as_os_str().as_bytes());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        for unit in path.as_os_str().encode_wide() {
            hasher.update(unit.to_le_bytes());
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        hasher.update(path.as_os_str().as_encoded_bytes());
    }
}

fn path_identity_len(path: &Path) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        path.as_os_str().as_bytes().len() as u64
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        (path.as_os_str().encode_wide().count() * std::mem::size_of::<u16>()) as u64
    }
    #[cfg(not(any(unix, windows)))]
    {
        path.as_os_str().as_encoded_bytes().len() as u64
    }
}

fn format_sha256(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(7 + bytes.len() * 2);
    output.push_str("sha256:");
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(unix)]
fn hex_decode(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return Err(DeadreckonError::InvalidInput(
            "encoded workspace path has an odd hexadecimal length".to_string(),
        ));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            let pair = std::str::from_utf8(chunk).map_err(|error| {
                DeadreckonError::InvalidInput(format!("invalid encoded workspace path: {error}"))
            })?;
            u8::from_str_radix(pair, 16).map_err(|error| {
                DeadreckonError::InvalidInput(format!("invalid encoded workspace path: {error}"))
            })
        })
        .collect()
}

fn ignore_precedence(kind: FrozenIgnoreKind) -> u8 {
    match kind {
        FrozenIgnoreKind::GlobalGit => 0,
        FrozenIgnoreKind::GitExclude => 1,
        FrozenIgnoreKind::Gitignore => 2,
        FrozenIgnoreKind::Ignore => 3,
    }
}

fn path_depth(path: &Path) -> usize {
    path.components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .count()
}

fn path_depth_encoded(path: &EncodedWorkspacePath) -> usize {
    path.to_path_buf().map_or(0, |path| path_depth(&path))
}

fn is_git_control_path(path: &Path) -> bool {
    path.components()
        .next()
        .is_some_and(|component| component.as_os_str() == ".git")
}

fn is_ecosystem_marker(name: &str) -> bool {
    matches!(
        name,
        "Cargo.toml"
            | "Package.swift"
            | "MODULE.bazel"
            | "WORKSPACE"
            | "WORKSPACE.bazel"
            | "CMakeCache.txt"
            | "build.gradle"
            | "build.gradle.kts"
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{
        CaptureBudgets, CaptureOmissionReason, CaptureProjection, CapturePurpose,
        EncodedWorkspacePath, capture_workspace, freeze_workspace_capture_policy,
    };
    use crate::git::run_git;

    fn git(root: &Path, args: &[&str]) {
        let output = run_git(root, args).expect("git command");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    use std::path::Path;

    #[test]
    fn frozen_ignores_do_not_trust_agent_rewrites_and_tracked_files_override_them() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path();
        git(root, &["init", "-q"]);
        fs::create_dir_all(root.join("build")).expect("build");
        fs::write(root.join(".gitignore"), "build/\n").expect("ignore");
        fs::write(root.join("build/tracked.txt"), "tracked\n").expect("tracked");
        git(root, &["add", "-f", ".gitignore", "build/tracked.txt"]);
        git(
            root,
            &[
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-qm",
                "fixture",
            ],
        );
        let policy = freeze_workspace_capture_policy(root).expect("freeze");

        fs::write(root.join(".gitignore"), "").expect("agent rewrite");
        fs::write(root.join("build/generated.o"), vec![0_u8; 1024]).expect("generated");
        let plan = capture_workspace(
            root,
            &policy,
            CaptureProjection::Deliverable,
            CapturePurpose::DeliverableIndex,
        )
        .expect("capture");

        assert!(
            plan.entries
                .iter()
                .any(|entry| entry.relative == Path::new("build/tracked.txt"))
        );
        assert!(
            !plan
                .entries
                .iter()
                .any(|entry| entry.relative == Path::new("build/generated.o"))
        );
    }

    #[test]
    fn hard_byte_budget_returns_a_reported_partial_capture() {
        let temp = TempDir::new().expect("temp");
        fs::write(temp.path().join("one.bin"), vec![1_u8; 64]).expect("one");
        fs::write(temp.path().join("two.bin"), vec![2_u8; 64]).expect("two");
        let mut policy = freeze_workspace_capture_policy(temp.path()).expect("freeze");
        policy.budgets = CaptureBudgets {
            max_total_bytes: 80,
            ..policy.budgets
        };

        let plan = capture_workspace(
            temp.path(),
            &policy,
            CaptureProjection::Recoverable,
            CapturePurpose::TurnSnapshot,
        )
        .expect("capture");

        assert!(plan.manifest.partial);
        assert!(
            plan.manifest
                .omissions
                .iter()
                .any(|omission| omission.reason == CaptureOmissionReason::ByteBudget)
        );
        assert!(plan.require_complete("fixture snapshot").is_err());
    }

    #[test]
    fn non_utf8_tracked_paths_round_trip_through_the_frozen_policy() {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt as _;

            let path = PathBuf::from(OsString::from_vec(b"tracked-\xff.txt".to_vec()));
            let encoded = EncodedWorkspacePath::from_path(&path);
            assert_eq!(encoded.to_path_buf().expect("decode"), path);
        }
    }

    use std::ffi::OsString;
    use std::path::PathBuf;
}
