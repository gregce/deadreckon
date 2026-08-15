//! Trusted, bounded workspace traversal for snapshots and artifact indexes.
//!
//! A capture policy is frozen before the first provider turn. Later walks use
//! those trusted ignore rules rather than agent-editable ignore files, while
//! Git-tracked paths remain eligible even when an ignore or generated-output
//! hint would otherwise prune them.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use ignore::WalkBuilder;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::artifact_policy::{
    WorkspacePathClass, classify_workspace_path, is_checkpointable_workspace_path,
    is_deliverable_workspace_path, is_promotable_workspace_path, is_recoverable_workspace_path,
    runtime_output_root,
};
use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::git::run_git;
use crate::state::{PipelineState, atomic_write_json};

pub const WORKSPACE_CAPTURE_POLICY_JSON: &str = "workspace-capture-policy.json";
pub const SOURCE_HYDRATION_MANIFEST_JSON: &str = "source-hydration-manifest.json";
pub const WORKSPACE_BLOBS_DIR: &str = "workspace-blobs";
pub const WORKSPACE_CAPTURE_POLICY_VERSION: u32 = 1;
pub const WORKSPACE_CAPTURE_MANIFEST_VERSION: u32 = 1;
pub const FROZEN_GIT_HYDRATION_VERSION: u32 = 1;

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
    /// Controller-owned Git identity captured before provider work begins.
    ///
    /// Older version-1 policies predate this explicit record. Compatibility
    /// captures may use their legacy frozen fields, but strict lifecycle paths
    /// must require this record rather than silently consulting the live,
    /// agent-visible Git control plane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frozen_git_hydration: Option<FrozenGitHydration>,
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
pub struct FrozenGitHydration {
    pub schema_version: u32,
    pub frozen_at: DateTime<Utc>,
    /// True when `root` was a Git worktree at the trusted freeze boundary.
    /// A false value is still an affirmative controller observation, allowing
    /// strict capture of plain directories without treating them as a missing
    /// hydration record.
    pub repository: bool,
    pub tracked_paths: Vec<EncodedWorkspacePath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index_sha256: Option<String>,
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
    Promotable,
    ResultCandidate,
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
    PromotionCandidate,
    ResultCandidate,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialization: Option<CaptureMaterialization>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureMaterialization {
    pub regular_files: u64,
    pub symbolic_links: u64,
    pub materialized_bytes: u64,
    pub new_blobs: u64,
    pub reused_blobs: u64,
    pub hardlinks: u64,
    pub copy_fallbacks: u64,
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
    scope_root: PathBuf,
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

/// Capture Git hydration authority while the workspace is still trusted.
///
/// This is the only API in this module that reads Git's live HEAD, index, and
/// tracked-path set for capture classification. Later workspace captures must
/// consume this frozen record and must not merge information from an
/// agent-editable `.git` directory or file.
pub fn freeze_git_hydration(root: &Path) -> Result<FrozenGitHydration> {
    let frozen_at = Utc::now();
    let repository = git_repository_detected(root)?;
    if !repository {
        return Ok(FrozenGitHydration {
            schema_version: FROZEN_GIT_HYDRATION_VERSION,
            frozen_at,
            repository: false,
            tracked_paths: Vec::new(),
            head: None,
            index_sha256: None,
        });
    }

    let tracked_paths = git_tracked_paths(root)?
        .iter()
        .map(|path| EncodedWorkspacePath::from_path(path))
        .collect();
    let (head, index_sha256) = git_hydration_identity(root);
    let hydration = FrozenGitHydration {
        schema_version: FROZEN_GIT_HYDRATION_VERSION,
        frozen_at,
        repository: true,
        tracked_paths,
        head,
        index_sha256,
    };
    validate_frozen_git_hydration(&hydration)?;
    Ok(hydration)
}

pub fn freeze_workspace_capture_policy(root: &Path) -> Result<WorkspaceCapturePolicy> {
    let frozen_at = Utc::now();
    let hydration = freeze_git_hydration(root)?;
    let mut discovery = discover_policy_inputs(root);
    freeze_git_exclude_sources(root, &mut discovery);
    let output_roots = discover_output_roots(root, &discovery.markers, &mut discovery.warnings);
    Ok(WorkspaceCapturePolicy {
        schema_version: WORKSPACE_CAPTURE_POLICY_VERSION,
        frozen_at,
        ignores: discovery.ignores,
        frozen_git_hydration: Some(hydration.clone()),
        frozen_tracked_paths: hydration.tracked_paths.clone(),
        output_roots,
        budgets: CaptureBudgets::default(),
        warnings: discovery.warnings,
        frozen_git_head: hydration.head.clone(),
        frozen_git_index_sha256: hydration.index_sha256,
    })
}

/// Freeze the operator-visible result proposal after provider quiescence.
///
/// Only project-local ignore files are read from the final workspace. Git's
/// tracked-path identity remains the controller-owned admission observation,
/// and neither host-global rules, `.git/info/exclude`, nor ecosystem output
/// discovery can become result-selection authority.
pub fn freeze_result_projection_policy(
    root: &Path,
    admission: &WorkspaceCapturePolicy,
) -> Result<WorkspaceCapturePolicy> {
    let hydration = require_frozen_git_hydration(admission)?.clone();
    let discovery = discover_policy_inputs(root);
    Ok(WorkspaceCapturePolicy {
        schema_version: WORKSPACE_CAPTURE_POLICY_VERSION,
        frozen_at: Utc::now(),
        ignores: discovery.ignores,
        frozen_git_hydration: Some(hydration.clone()),
        frozen_tracked_paths: hydration.tracked_paths.clone(),
        output_roots: Vec::new(),
        budgets: admission.budgets,
        warnings: discovery.warnings,
        frozen_git_head: hydration.head.clone(),
        frozen_git_index_sha256: hydration.index_sha256,
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
    let mut policy: WorkspaceCapturePolicy = serde_json::from_slice(&raw).with_json_path(&path)?;
    if policy.schema_version != WORKSPACE_CAPTURE_POLICY_VERSION {
        return Err(DeadreckonError::InvalidInput(format!(
            "unsupported workspace capture policy version {} at {}",
            policy.schema_version,
            path.display()
        )));
    }
    if policy.frozen_git_hydration.is_none() {
        let repository = policy.frozen_git_index_sha256.is_some()
            || policy.frozen_git_head.is_some()
            || !policy.frozen_tracked_paths.is_empty();
        policy.frozen_git_hydration = Some(FrozenGitHydration {
            schema_version: FROZEN_GIT_HYDRATION_VERSION,
            frozen_at: policy.frozen_at,
            repository,
            tracked_paths: policy.frozen_tracked_paths.clone(),
            head: policy.frozen_git_head.clone(),
            index_sha256: policy.frozen_git_index_sha256.clone(),
        });
    }
    require_frozen_git_hydration(&policy)?;
    Ok(policy)
}

pub fn ensure_workspace_capture_policy(state: &PipelineState) -> Result<WorkspaceCapturePolicy> {
    let path = workspace_capture_policy_path(&state.run_root);
    if path.is_file() {
        return read_workspace_capture_policy(&state.run_root);
    }
    if state.turn != 0
        || !matches!(
            state.status,
            crate::state::RunStatus::Pending | crate::state::RunStatus::Planned
        )
    {
        return Err(DeadreckonError::InvalidInput(format!(
            "trusted workspace capture policy is missing at {}; refusing to reconstruct Git hydration after provider work may have begun",
            path.display()
        )));
    }
    let policy = freeze_workspace_capture_policy(&state.working_dir)?;
    write_workspace_capture_policy(&state.run_root, &policy)?;
    Ok(policy)
}

/// Read a policy that was durably frozen by the controller before provider
/// work. Unlike `ensure_workspace_capture_policy`, this never manufactures a
/// policy from a potentially agent-mutated workspace.
pub fn require_workspace_capture_policy(state: &PipelineState) -> Result<WorkspaceCapturePolicy> {
    let path = workspace_capture_policy_path(&state.run_root);
    if !path.is_file() {
        return Err(DeadreckonError::InvalidInput(format!(
            "trusted workspace capture policy is missing at {}; strict capture refuses to inspect live Git state",
            path.display()
        )));
    }
    let policy = read_workspace_capture_policy(&state.run_root)?;
    require_frozen_git_hydration(&policy)?;
    Ok(policy)
}

/// Require the explicit controller-owned Git hydration observation.
///
/// A record with `repository = false` is valid: it proves the controller saw
/// a plain directory at the freeze boundary. Absence is not equivalent; it can
/// mean an older or late-created policy and therefore fails closed.
pub fn require_frozen_git_hydration(
    policy: &WorkspaceCapturePolicy,
) -> Result<&FrozenGitHydration> {
    let hydration = policy.frozen_git_hydration.as_ref().ok_or_else(|| {
        DeadreckonError::InvalidInput(
            "workspace capture policy has no controller-frozen Git hydration record; strict capture refuses to inspect live Git state"
                .to_string(),
        )
    })?;
    validate_frozen_git_hydration(hydration)?;
    if policy.frozen_tracked_paths != hydration.tracked_paths
        || policy.frozen_git_head != hydration.head
        || policy.frozen_git_index_sha256 != hydration.index_sha256
    {
        return Err(DeadreckonError::InvalidInput(
            "workspace capture policy disagrees with its controller-frozen Git hydration record"
                .to_string(),
        ));
    }
    Ok(hydration)
}

pub fn capture_workspace(
    root: &Path,
    policy: &WorkspaceCapturePolicy,
    projection: CaptureProjection,
    purpose: CapturePurpose,
) -> Result<WorkspaceCapturePlan> {
    capture_workspace_inner(root, policy, projection, purpose, false, None)
}

/// Capture using only an explicit controller-frozen Git hydration record.
///
/// Strict run, snapshot, gate, and promotion paths should use this entrypoint.
/// The compatibility `capture_workspace` wrapper also never reads live Git,
/// but permits older policies whose frozen fields predate the explicit record.
pub fn capture_workspace_strict(
    root: &Path,
    policy: &WorkspaceCapturePolicy,
    projection: CaptureProjection,
    purpose: CapturePurpose,
) -> Result<WorkspaceCapturePlan> {
    capture_workspace_inner(root, policy, projection, purpose, true, None)
}

/// Strict capture clamped to the enclosing Job work boundary.
pub fn capture_workspace_strict_bounded(
    root: &Path,
    policy: &WorkspaceCapturePolicy,
    projection: CaptureProjection,
    purpose: CapturePurpose,
    boundary: &crate::git::WorkBoundaryScope,
) -> Result<WorkspaceCapturePlan> {
    capture_workspace_inner(root, policy, projection, purpose, true, Some(boundary))
}

fn capture_workspace_inner(
    root: &Path,
    policy: &WorkspaceCapturePolicy,
    projection: CaptureProjection,
    purpose: CapturePurpose,
    require_hydration: bool,
    boundary: Option<&crate::git::WorkBoundaryScope>,
) -> Result<WorkspaceCapturePlan> {
    let inherited_boundary = boundary
        .is_none()
        .then(crate::git::inherited_work_boundary)
        .flatten();
    let boundary = boundary.or(inherited_boundary.as_ref());
    if let Some(boundary) = boundary {
        boundary.check()?;
    }
    let started_at = Utc::now();
    let started = Instant::now();
    let policy_sha256 = policy_sha256(policy)?;
    let explicit_hydration = if require_hydration {
        Some(require_frozen_git_hydration(policy)?)
    } else {
        policy.frozen_git_hydration.as_ref()
    };
    if let Some(hydration) = explicit_hydration {
        validate_frozen_git_hydration(hydration)?;
    }
    let tracked = decode_paths(
        explicit_hydration.map_or(policy.frozen_tracked_paths.as_slice(), |hydration| {
            hydration.tracked_paths.as_slice()
        }),
    )?;
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
        if let Some(boundary) = boundary {
            boundary.check()?;
        }
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
    let capture_deadline = started
        .checked_add(Duration::from_millis(policy.budgets.max_traversal_millis))
        .unwrap_or_else(Instant::now);
    for (path, reason) in pruned {
        if let Some(boundary) = boundary {
            boundary.check()?;
        }
        let summary = summarize_omitted_subtree(root, &path, capture_deadline);
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
    let (repository, head, index_sha256) = explicit_hydration.map_or_else(
        || {
            (
                !tracked.is_empty()
                    || policy.frozen_git_head.is_some()
                    || policy.frozen_git_index_sha256.is_some(),
                policy.frozen_git_head.clone(),
                policy.frozen_git_index_sha256.clone(),
            )
        },
        |hydration| {
            (
                hydration.repository,
                hydration.head.clone(),
                hydration.index_sha256.clone(),
            )
        },
    );
    if let Some(boundary) = boundary {
        boundary.check()?;
    }
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
            git: repository.then_some(GitHydrationState {
                head,
                index_sha256,
                tracked_files: tracked.len() as u64,
            }),
            materialization: None,
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

/// Remove one real directory tree without following symlinks, checking an
/// inherited Job boundary between entries. A boundary interruption leaves a
/// recoverable partial staging tree instead of blocking the controller.
pub fn remove_captured_directory_tree(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(DeadreckonError::InvalidInput(format!(
            "refusing to remove capture tree {} because it is not a real directory",
            path.display()
        )));
    }
    let boundary = crate::git::inherited_work_boundary();
    remove_captured_directory_tree_inner(path, boundary.as_ref())
}

fn remove_captured_directory_tree_inner(
    path: &Path,
    boundary: Option<&crate::git::WorkBoundaryScope>,
) -> Result<()> {
    if let Some(boundary) = boundary {
        boundary.check()?;
    }
    for entry in fs::read_dir(path).with_path(path)? {
        if let Some(boundary) = boundary {
            boundary.check()?;
        }
        let entry = entry.with_path(path)?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child).with_path(&child)?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            remove_captured_directory_tree_inner(&child, boundary)?;
        } else {
            fs::remove_file(&child).with_path(&child)?;
        }
    }
    if let Some(boundary) = boundary {
        boundary.check()?;
    }
    fs::remove_dir(path).with_path(path)
}

/// Materialize the selected leaves without following symbolic links.
///
/// The capture plan has already established the trusted traversal boundary.
/// Materialization therefore creates only parent directories required by an
/// admitted leaf and refuses a destination hierarchy containing a symlink.
pub fn materialize_capture_plan(plan: &WorkspaceCapturePlan, destination: &Path) -> Result<()> {
    let inherited_boundary = crate::git::inherited_work_boundary();
    let boundary = inherited_boundary.as_ref();
    if let Some(boundary) = boundary {
        boundary.check()?;
    }
    ensure_materialization_root(destination)?;
    for entry in &plan.entries {
        if let Some(boundary) = boundary {
            boundary.check()?;
        }
        materialize_capture_entry(entry, destination, None, boundary)?;
    }
    if let Some(boundary) = boundary {
        boundary.check()?;
    }
    Ok(())
}

/// Materialize selected leaves under the enclosing Job work boundary.
pub fn materialize_capture_plan_bounded(
    plan: &WorkspaceCapturePlan,
    destination: &Path,
    boundary: &crate::git::WorkBoundaryScope,
) -> Result<()> {
    boundary.check()?;
    ensure_materialization_root(destination)?;
    for entry in &plan.entries {
        boundary.check()?;
        materialize_capture_entry(entry, destination, None, Some(boundary))?;
    }
    boundary.check()
}

/// Materialize a capture through a run-scoped whole-file content store.
///
/// Blob keys bind content and permission identity. Snapshot leaves hard-link
/// to the immutable evidence blob when the filesystem permits it, falling
/// back to an ordinary copy across device or platform boundaries.
pub fn materialize_capture_plan_with_blob_store(
    plan: &WorkspaceCapturePlan,
    destination: &Path,
    blob_root: &Path,
) -> Result<CaptureMaterialization> {
    let inherited_boundary = crate::git::inherited_work_boundary();
    let boundary = inherited_boundary.as_ref();
    if let Some(boundary) = boundary {
        boundary.check()?;
    }
    ensure_materialization_root(destination)?;
    ensure_materialization_root(blob_root)?;
    let mut stats = CaptureMaterialization::default();
    for entry in &plan.entries {
        if let Some(boundary) = boundary {
            boundary.check()?;
        }
        let entry_stats = materialize_capture_entry(entry, destination, Some(blob_root), boundary)?;
        stats.add(&entry_stats);
    }
    if let Some(boundary) = boundary {
        boundary.check()?;
    }
    Ok(stats)
}

/// Materialize a content-addressed capture under the enclosing Job cutoff.
pub fn materialize_capture_plan_with_blob_store_bounded(
    plan: &WorkspaceCapturePlan,
    destination: &Path,
    blob_root: &Path,
    boundary: &crate::git::WorkBoundaryScope,
) -> Result<CaptureMaterialization> {
    boundary.check()?;
    ensure_materialization_root(destination)?;
    ensure_materialization_root(blob_root)?;
    let mut stats = CaptureMaterialization::default();
    for entry in &plan.entries {
        boundary.check()?;
        let entry_stats =
            materialize_capture_entry(entry, destination, Some(blob_root), Some(boundary))?;
        stats.add(&entry_stats);
    }
    boundary.check()?;
    Ok(stats)
}

/// Materialize one already-admitted leaf through the same content store.
///
/// Checkpoint deltas use this after their bounded working index has admitted
/// the exact path.
pub fn materialize_capture_entry_with_blob_store(
    source: &Path,
    kind: CaptureEntryKind,
    size: u64,
    destination_root: &Path,
    relative: &Path,
    blob_root: &Path,
) -> Result<CaptureMaterialization> {
    let inherited_boundary = crate::git::inherited_work_boundary();
    let boundary = inherited_boundary.as_ref();
    if let Some(boundary) = boundary {
        boundary.check()?;
    }
    ensure_materialization_root(destination_root)?;
    ensure_materialization_root(blob_root)?;
    materialize_capture_entry(
        &CaptureEntry {
            relative: relative.to_path_buf(),
            source: source.to_path_buf(),
            kind,
            size,
            tracked: false,
        },
        destination_root,
        Some(blob_root),
        boundary,
    )
}

impl CaptureMaterialization {
    pub fn add(&mut self, other: &Self) {
        self.regular_files = self.regular_files.saturating_add(other.regular_files);
        self.symbolic_links = self.symbolic_links.saturating_add(other.symbolic_links);
        self.materialized_bytes = self
            .materialized_bytes
            .saturating_add(other.materialized_bytes);
        self.new_blobs = self.new_blobs.saturating_add(other.new_blobs);
        self.reused_blobs = self.reused_blobs.saturating_add(other.reused_blobs);
        self.hardlinks = self.hardlinks.saturating_add(other.hardlinks);
        self.copy_fallbacks = self.copy_fallbacks.saturating_add(other.copy_fallbacks);
    }
}

fn materialize_capture_entry(
    entry: &CaptureEntry,
    destination: &Path,
    blob_root: Option<&Path>,
    boundary: Option<&crate::git::WorkBoundaryScope>,
) -> Result<CaptureMaterialization> {
    if let Some(boundary) = boundary {
        boundary.check()?;
    }
    let target = destination.join(&entry.relative);
    ensure_materialization_parent(destination, &entry.relative)?;
    remove_materialization_target(&target)?;
    let mut stats = CaptureMaterialization::default();
    match entry.kind {
        CaptureEntryKind::RegularFile => {
            stats.regular_files = 1;
            stats.materialized_bytes = entry.size;
            if let Some(blob_root) = blob_root {
                materialize_regular_file_from_blob(
                    entry, &target, blob_root, &mut stats, boundary,
                )?;
            } else {
                let metadata = fs::symlink_metadata(&entry.source).with_path(&entry.source)?;
                copy_regular_file(&entry.source, &target, boundary)?;
                fs::set_permissions(&target, metadata.permissions()).with_path(&target)?;
            }
        }
        CaptureEntryKind::SymbolicLink => {
            stats.symbolic_links = 1;
            stats.materialized_bytes = entry.size;
            let link_target = fs::read_link(&entry.source).with_path(&entry.source)?;
            create_materialized_symlink(&entry.source, &link_target, &target)?;
        }
    }
    Ok(stats)
}

fn materialize_regular_file_from_blob(
    entry: &CaptureEntry,
    target: &Path,
    blob_root: &Path,
    stats: &mut CaptureMaterialization,
    boundary: Option<&crate::git::WorkBoundaryScope>,
) -> Result<()> {
    let metadata = fs::symlink_metadata(&entry.source).with_path(&entry.source)?;
    let permission_key = permission_identity(&metadata);
    let blob_dir = blob_root.join("sha256").join(permission_key);
    fs::create_dir_all(&blob_dir).with_path(&blob_dir)?;
    let mut source = fs::File::open(&entry.source).with_path(&entry.source)?;
    let mut temp = NamedTempFile::new_in(&blob_dir).with_path(&blob_dir)?;
    let mut hasher = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if let Some(boundary) = boundary {
            boundary.check()?;
        }
        let read = source.read(&mut buffer).with_path(&entry.source)?;
        if read == 0 {
            break;
        }
        temp.write_all(&buffer[..read]).with_path(temp.path())?;
        hasher.update(&buffer[..read]);
        copied = copied.saturating_add(read as u64);
    }
    if copied != entry.size {
        return Err(DeadreckonError::InvalidInput(format!(
            "workspace file changed size during capture: {} (planned {}, copied {})",
            entry.relative.display(),
            entry.size,
            copied
        )));
    }
    fs::set_permissions(temp.path(), metadata.permissions()).with_path(temp.path())?;
    temp.as_file_mut().sync_all().with_path(temp.path())?;
    let digest = hex_encode(hasher.finalize().as_slice());
    let blob = blob_dir.join(digest);
    let reused = if blob.is_file() {
        let blob_metadata = fs::symlink_metadata(&blob).with_path(&blob)?;
        if blob_metadata.len() != copied {
            return Err(DeadreckonError::InvalidInput(format!(
                "workspace blob store collision or corruption at {}",
                blob.display()
            )));
        }
        true
    } else {
        match temp.persist_noclobber(&blob) {
            Ok(_) => false,
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => true,
            Err(error) => {
                return Err(DeadreckonError::Io {
                    path: blob,
                    source: error.error,
                });
            }
        }
    };
    if reused {
        stats.reused_blobs = 1;
    } else {
        stats.new_blobs = 1;
    }
    if fs::hard_link(&blob, target).is_ok() {
        stats.hardlinks = 1;
    } else {
        copy_regular_file(&blob, target, boundary)?;
        fs::set_permissions(target, metadata.permissions()).with_path(target)?;
        stats.copy_fallbacks = 1;
    }
    Ok(())
}

fn copy_regular_file(
    source_path: &Path,
    target_path: &Path,
    boundary: Option<&crate::git::WorkBoundaryScope>,
) -> Result<u64> {
    let mut source = fs::File::open(source_path).with_path(source_path)?;
    let mut target = fs::File::create(target_path).with_path(target_path)?;
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if let Some(boundary) = boundary {
            boundary.check()?;
        }
        let read = source.read(&mut buffer).with_path(source_path)?;
        if read == 0 {
            break;
        }
        target.write_all(&buffer[..read]).with_path(target_path)?;
        copied = copied.saturating_add(read as u64);
    }
    if let Some(boundary) = boundary {
        boundary.check()?;
    }
    Ok(copied)
}

#[cfg(unix)]
fn permission_identity(metadata: &fs::Metadata) -> String {
    use std::os::unix::fs::PermissionsExt as _;

    format!("mode-{:04o}", metadata.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn permission_identity(metadata: &fs::Metadata) -> String {
    if metadata.permissions().readonly() {
        "readonly".to_string()
    } else {
        "writable".to_string()
    }
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
    // Root policy is the common greenfield case and must not depend on how
    // quickly an arbitrary generated subtree can consume the bounded walk.
    // Nested policy files remain bounded discovery inputs below.
    for (name, kind) in [
        (".gitignore", FrozenIgnoreKind::Gitignore),
        (".ignore", FrozenIgnoreKind::Ignore),
    ] {
        let relative = Path::new(name);
        if root.join(relative).is_file()
            && let Some(source) = frozen_ignore_file(root, relative, kind, &mut discovery.warnings)
        {
            discovery.ignores.push(source);
        }
    }
    let root_owned = root.to_path_buf();
    let mut builder = WalkBuilder::new(root);
    builder
        // Discovery must see the ignore files and ecosystem markers it is
        // freezing. Applying live ignore rules here could hide those inputs
        // before they become trusted policy (and parent-directory ignores are
        // especially inappropriate for an explicitly selected source root).
        .standard_filters(false)
        .hidden(false)
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
            if relative.components().count() == 1 {
                continue;
            }
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
    let bazel_roots = markers
        .iter()
        .filter(|marker| {
            marker.file_name().is_some_and(|name| {
                matches!(
                    name.to_str(),
                    Some("MODULE.bazel" | "WORKSPACE" | "WORKSPACE.bazel")
                )
            })
        })
        .filter_map(|marker| marker.parent())
        .collect::<BTreeSet<_>>();
    for bazel_root in bazel_roots {
        if let Ok(Some(path)) = bazel_output_path(&root.join(bazel_root))
            && let Some(relative) =
                output_path_relative_to_root(root, &root.join(bazel_root), &path)
        {
            roots.insert(relative, GeneratedOutputSource::Bazel);
        }
    }
    let cargo_roots = markers
        .iter()
        .filter(|marker| {
            marker
                .file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name == "Cargo.toml")
        })
        .filter_map(|marker| marker.parent())
        .collect::<BTreeSet<_>>();
    for cargo_root in cargo_roots {
        let project_root = root.join(cargo_root);
        match cargo_metadata_target_directory(&project_root) {
            Ok(Some(path)) => {
                if let Some(relative) = output_path_relative_to_root(root, &project_root, &path) {
                    roots.insert(relative, GeneratedOutputSource::CargoMetadata);
                }
            }
            Ok(None) => {}
            Err(error) => warnings.push(format!(
                "cargo metadata output discovery skipped for {}: {error}",
                project_root.display()
            )),
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

fn output_path_relative_to_root(root: &Path, project_root: &Path, path: &Path) -> Option<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    let relative = absolute.strip_prefix(root).ok().or_else(|| {
        let canonical_root = root.canonicalize().ok()?;
        absolute.strip_prefix(canonical_root).ok()
    })?;
    (!relative.as_os_str().is_empty()).then(|| relative.to_path_buf())
}

fn bazel_output_path(root: &Path) -> Result<Option<PathBuf>> {
    let Some(raw) = bounded_command_stdout(
        root,
        "bazel",
        &["info", "output_path"],
        Duration::from_millis(ECOSYSTEM_QUERY_MILLIS),
        MAX_ECOSYSTEM_QUERY_BYTES,
    )?
    else {
        return Ok(None);
    };
    let value = String::from_utf8_lossy(&raw).trim().to_string();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(value)))
    }
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
        let scope_root = root.join(&base);
        let mut builder = GitignoreBuilder::new(&scope_root);
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
            scope_root,
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
    if context.projection != CaptureProjection::ResultCandidate
        && (context
            .output_roots
            .iter()
            .any(|root| relative == root || relative.starts_with(root))
            || runtime_output_root(relative).is_some())
    {
        record_pruned(context, relative, CaptureOmissionReason::GeneratedOutput);
        return false;
    }
    if context.projection == CaptureProjection::ResultCandidate
        && !is_dir
        && relative
            .file_name()
            .is_some_and(|name| matches!(name.to_str(), Some(".gitignore" | ".ignore")))
    {
        return true;
    }
    if frozen_ignored(&context.ignores, entry.path(), is_dir)
        && !ignore_protected_capture_path(context.projection, relative)
    {
        record_pruned(context, relative, CaptureOmissionReason::FrozenIgnore);
        return false;
    }
    true
}

fn ignore_protected_capture_path(projection: CaptureProjection, relative: &Path) -> bool {
    match (projection, classify_workspace_path(relative)) {
        // Undo must retain DeadReckon's small workspace-side lifecycle record
        // even when a repository or enclosing Git checkout ignores
        // `.deadreckon/`. Otherwise restore clears the record and loses the
        // codebase lineage needed by later lifecycle commands.
        (CaptureProjection::Recoverable, WorkspacePathClass::LifecycleMetadata) => true,
        (CaptureProjection::Promotable, WorkspacePathClass::LifecycleMetadata) => true,
        (CaptureProjection::ResultCandidate, WorkspacePathClass::LifecycleMetadata) => true,
        // The semantic read-only guard must also see protected evidence and
        // lifecycle paths; an ignore rule is not authority to mutate them.
        (
            CaptureProjection::WorkspaceGuard,
            WorkspacePathClass::EvidenceOnly | WorkspacePathClass::LifecycleMetadata,
        ) => true,
        _ => false,
    }
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
        CaptureProjection::Promotable => {
            is_promotable_workspace_path(relative)
                || (tracked_or_parent
                    && classify_workspace_path(relative) == WorkspacePathClass::RuntimeOnly
                    && runtime_output_root(relative).is_some())
        }
        CaptureProjection::ResultCandidate => {
            match classify_workspace_path(relative) {
                WorkspacePathClass::EvidenceOnly => false,
                // A project may intentionally track a lifecycle-looking path
                // such as `.deadreckon/acceptance.yaml`; admission-tracked
                // paths remain part of the result. Untracked controller
                // lifecycle files beside it are run state, not operator
                // output, and must never enter the sealed candidate.
                WorkspacePathClass::LifecycleMetadata => tracked_or_parent,
                WorkspacePathClass::Deliverable | WorkspacePathClass::RuntimeOnly => true,
            }
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
        if !path.starts_with(&source.scope_root) {
            continue;
        }
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

fn validate_frozen_git_hydration(hydration: &FrozenGitHydration) -> Result<()> {
    if hydration.schema_version != FROZEN_GIT_HYDRATION_VERSION {
        return Err(DeadreckonError::InvalidInput(format!(
            "unsupported frozen Git hydration version {}",
            hydration.schema_version
        )));
    }
    if !hydration.repository {
        if !hydration.tracked_paths.is_empty()
            || hydration.head.is_some()
            || hydration.index_sha256.is_some()
        {
            return Err(DeadreckonError::InvalidInput(
                "non-repository frozen Git hydration record contains repository state".to_string(),
            ));
        }
        return Ok(());
    }
    let index_sha256 = hydration.index_sha256.as_deref().ok_or_else(|| {
        DeadreckonError::InvalidInput(
            "repository frozen Git hydration record has no index identity".to_string(),
        )
    })?;
    if !is_prefixed_sha256(index_sha256) {
        return Err(DeadreckonError::InvalidInput(
            "repository frozen Git hydration record has an invalid index identity".to_string(),
        ));
    }
    if hydration
        .head
        .as_deref()
        .is_some_and(|head| !is_hex_digest(head, 40) && !is_hex_digest(head, 64))
    {
        return Err(DeadreckonError::InvalidInput(
            "repository frozen Git hydration record has an invalid HEAD identity".to_string(),
        ));
    }
    let mut seen = BTreeSet::new();
    for encoded in &hydration.tracked_paths {
        let path = encoded.to_path_buf()?;
        if path.as_os_str().is_empty()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            || is_git_control_path(&path)
        {
            return Err(DeadreckonError::InvalidInput(format!(
                "frozen Git hydration contains an unsafe tracked path: {}",
                path.display()
            )));
        }
        if !seen.insert(path.clone()) {
            return Err(DeadreckonError::InvalidInput(format!(
                "frozen Git hydration contains a duplicate tracked path: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn is_hex_digest(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_prefixed_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|digest| is_hex_digest(digest, 64))
}

fn git_repository_detected(root: &Path) -> Result<bool> {
    let output = run_git(root, &["rev-parse", "--is-inside-work-tree"])?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim() == "true");
    }
    if fs::symlink_metadata(root.join(".git")).is_ok() {
        return Err(DeadreckonError::InvalidInput(format!(
            "could not freeze Git hydration for repository control path at {}: {}",
            root.join(".git").display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(false)
}

fn git_tracked_paths(root: &Path) -> Result<BTreeSet<PathBuf>> {
    let output = run_git(root, &["ls-files", "-z", "--cached", "--"])?;
    if !output.status.success() {
        return Err(DeadreckonError::InvalidInput(format!(
            "could not freeze Git tracked paths at {}: {}",
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
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

fn summarize_omitted_subtree(
    root: &Path,
    relative: &Path,
    capture_deadline: Instant,
) -> SubtreeSummary {
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
        if Instant::now() >= capture_deadline
            || files >= OMITTED_SUMMARY_FILES
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
    use std::time::{Duration, Instant};

    use tempfile::TempDir;

    use super::{
        CaptureBudgets, CaptureOmissionReason, CaptureProjection, CapturePurpose,
        EncodedWorkspacePath, GeneratedOutputSource, capture_workspace, capture_workspace_strict,
        capture_workspace_strict_bounded, freeze_result_projection_policy,
        freeze_workspace_capture_policy, materialize_capture_plan,
    };

    fn init_git_fixture(root: &Path) {
        let output = crate::git::run_git(root, &["init", "-q"]).expect("git init");
        assert!(output.status.success());
    }

    #[test]
    fn result_policy_uses_final_local_ignores_and_admission_tracked_paths() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let root = temp.path();
        init_git_fixture(root);
        std::fs::write(root.join("tracked.txt"), "tracked\n").expect("tracked");
        let add = crate::git::run_git(root, &["add", "tracked.txt"]).expect("git add");
        assert!(add.status.success());
        let admission = freeze_workspace_capture_policy(root).expect("admission");
        std::fs::create_dir_all(root.join("invented-cache")).expect("cache");
        std::fs::write(root.join("invented-cache/lock"), "volatile\n").expect("lock");
        std::fs::write(root.join(".gitignore"), "/tracked.txt\n/invented-cache\n").expect("ignore");

        let policy = freeze_result_projection_policy(root, &admission).expect("result policy");
        let capture = capture_workspace_strict(
            root,
            &policy,
            CaptureProjection::ResultCandidate,
            CapturePurpose::ResultCandidate,
        )
        .expect("capture");
        assert!(
            capture
                .entries
                .iter()
                .any(|entry| entry.relative == Path::new("tracked.txt"))
        );
        assert!(
            !capture
                .entries
                .iter()
                .any(|entry| entry.relative == Path::new("invented-cache/lock"))
        );
        assert!(
            capture
                .entries
                .iter()
                .any(|entry| entry.relative == Path::new(".gitignore"))
        );
    }

    #[test]
    fn result_policy_refuses_late_global_and_git_exclude_authority() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let root = temp.path();
        init_git_fixture(root);
        std::fs::write(root.join("source.txt"), "source\n").expect("source");
        let admission = freeze_workspace_capture_policy(root).expect("admission");
        let git_dir = root.join(".git/info");
        std::fs::create_dir_all(&git_dir).expect("info");
        std::fs::write(git_dir.join("exclude"), "/source.txt\n").expect("exclude");

        let result = freeze_result_projection_policy(root, &admission).expect("result");
        assert!(result.ignores.iter().all(|source| matches!(
            source.kind,
            super::FrozenIgnoreKind::Gitignore | super::FrozenIgnoreKind::Ignore
        )));
    }

    #[test]
    fn result_candidate_does_not_consult_runtime_root_names() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let root = temp.path();
        std::fs::create_dir_all(root.join(".next/server")).expect("output");
        std::fs::write(root.join(".next/server/app.js"), "deliver me\n").expect("file");
        let admission = freeze_workspace_capture_policy(root).expect("admission");
        let policy = freeze_result_projection_policy(root, &admission).expect("result");
        let capture = capture_workspace_strict(
            root,
            &policy,
            CaptureProjection::ResultCandidate,
            CapturePurpose::ResultCandidate,
        )
        .expect("capture");
        assert!(
            capture
                .entries
                .iter()
                .any(|entry| entry.relative == Path::new(".next/server/app.js"))
        );
    }

    #[test]
    fn tracked_path_wins_over_late_ignore() {
        result_policy_uses_final_local_ignores_and_admission_tracked_paths();
    }

    #[test]
    fn result_candidate_keeps_tracked_lifecycle_path_but_drops_controller_siblings() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let root = temp.path();
        init_git_fixture(root);
        std::fs::create_dir_all(root.join(".deadreckon/docs")).expect("lifecycle directory");
        std::fs::write(
            root.join(".deadreckon/acceptance.yaml"),
            "name: project contract\nchecks: []\n",
        )
        .expect("tracked contract");
        let add = crate::git::run_git(root, &["add", "-f", ".deadreckon/acceptance.yaml"])
            .expect("git add");
        assert!(add.status.success());
        let admission = freeze_workspace_capture_policy(root).expect("admission");

        std::fs::write(root.join(".deadreckon/codebase.json"), "{}\n")
            .expect("controller lifecycle file");
        std::fs::write(root.join(".deadreckon/docs/polish.json"), "{}\n")
            .expect("controller docs file");

        let policy = freeze_result_projection_policy(root, &admission).expect("result policy");
        let capture = capture_workspace_strict(
            root,
            &policy,
            CaptureProjection::ResultCandidate,
            CapturePurpose::ResultCandidate,
        )
        .expect("capture");
        let paths = capture
            .entries
            .iter()
            .map(|entry| entry.relative.as_path())
            .collect::<Vec<_>>();

        assert!(paths.contains(&Path::new(".deadreckon/acceptance.yaml")));
        assert!(!paths.contains(&Path::new(".deadreckon/codebase.json")));
        assert!(!paths.contains(&Path::new(".deadreckon/docs/polish.json")));
    }

    #[test]
    fn late_project_ignore_is_not_yet_a_verified_result_boundary() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let root = temp.path();
        std::fs::write(root.join("source.txt"), "source\n").expect("source");
        let policy = freeze_workspace_capture_policy(root).expect("admission policy");
        std::fs::create_dir_all(root.join("invented-cache")).expect("cache");
        std::fs::write(root.join("invented-cache/lock"), "volatile\n").expect("lock");
        std::fs::write(root.join(".gitignore"), "/invented-cache\n").expect("late ignore");

        let capture = capture_workspace_strict(
            root,
            &policy,
            CaptureProjection::Deliverable,
            CapturePurpose::DeliverableIndex,
        )
        .expect("historical capture");
        assert!(
            capture
                .entries
                .iter()
                .any(|entry| entry.relative == Path::new("invented-cache/lock")),
            "the characterization must stay explicit until Holdfast replaces this boundary"
        );
    }

    #[test]
    fn receipt_and_promotion_currently_can_select_different_trees() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let root = temp.path();
        std::fs::write(root.join("source.txt"), "source\n").expect("source");
        let frozen = freeze_workspace_capture_policy(root).expect("frozen policy");
        std::fs::write(root.join("late.txt"), "late\n").expect("late");
        std::fs::write(root.join(".gitignore"), "/late.txt\n").expect("late ignore");

        let live = freeze_workspace_capture_policy(root).expect("live policy");
        let receipt_capture = capture_workspace_strict(
            root,
            &live,
            CaptureProjection::Deliverable,
            CapturePurpose::DeliverableIndex,
        )
        .expect("receipt-like live capture");
        let promotion_capture = capture_workspace_strict(
            root,
            &frozen,
            CaptureProjection::Promotable,
            CapturePurpose::PromotionCandidate,
        )
        .expect("promotion-like frozen capture");
        assert!(
            !receipt_capture
                .entries
                .iter()
                .any(|entry| entry.relative == Path::new("late.txt"))
        );
        assert!(
            promotion_capture
                .entries
                .iter()
                .any(|entry| entry.relative == Path::new("late.txt"))
        );
    }
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
    fn strict_capture_stops_at_the_inherited_job_boundary() {
        let temp = TempDir::new().expect("temp");
        fs::write(temp.path().join("app.swift"), "print(\"ready\")\n").expect("source");
        let policy = freeze_workspace_capture_policy(temp.path()).expect("freeze");
        let boundary = crate::git::WorkBoundaryScope::new(
            Instant::now(),
            Duration::from_secs(3),
            || false,
            "workspace snapshot",
        );

        let error = capture_workspace_strict_bounded(
            temp.path(),
            &policy,
            CaptureProjection::Recoverable,
            CapturePurpose::TurnSnapshot,
            &boundary,
        )
        .expect_err("expired Job boundary must stop capture");

        assert!(matches!(
            error,
            crate::DeadreckonError::ProcessBoundary {
                kind: crate::ProcessBoundaryKind::WorkExpired,
                ..
            }
        ));
    }

    #[test]
    fn materialization_inherits_cancellation_before_copying() {
        let temp = TempDir::new().expect("temp");
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir_all(&source).expect("source directory");
        fs::write(source.join("large.bin"), vec![7_u8; 256 * 1024]).expect("source file");
        let policy = freeze_workspace_capture_policy(&source).expect("freeze");
        let plan = capture_workspace_strict(
            &source,
            &policy,
            CaptureProjection::Recoverable,
            CapturePurpose::TurnSnapshot,
        )
        .expect("capture plan");
        let boundary = crate::git::WorkBoundaryScope::new(
            Instant::now() + Duration::from_secs(3),
            Duration::from_secs(3),
            || true,
            "workspace snapshot",
        );

        let error = crate::git::with_git_command_scope(boundary, || {
            materialize_capture_plan(&plan, &destination)
        })
        .expect_err("cancelled Job boundary must stop materialization");

        assert!(matches!(
            error,
            crate::DeadreckonError::ProcessBoundary {
                kind: crate::ProcessBoundaryKind::Cancelled,
                ..
            }
        ));
        assert!(!destination.join("large.bin").exists());
    }

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
    fn sibling_nested_ignore_files_only_apply_within_their_own_directories() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path();
        let left = root.join("left-app");
        let right = root.join("right-app");
        fs::create_dir_all(&left).expect("left app");
        fs::create_dir_all(&right).expect("right app");
        fs::write(left.join(".gitignore"), "generated.txt\n").expect("left ignore");
        fs::write(right.join(".gitignore"), "cache.txt\n").expect("right ignore");
        fs::write(left.join("generated.txt"), "ignored on the left\n").expect("left generated");
        fs::write(right.join("generated.txt"), "kept on the right\n").expect("right source");

        let policy = freeze_workspace_capture_policy(root).expect("freeze");
        let plan = capture_workspace(
            root,
            &policy,
            CaptureProjection::Source,
            CapturePurpose::SourceHydration,
        )
        .expect("capture sibling projects");

        assert!(
            plan.entries
                .iter()
                .all(|entry| entry.relative != Path::new("left-app/generated.txt"))
        );
        assert!(
            plan.entries
                .iter()
                .any(|entry| entry.relative == Path::new("right-app/generated.txt"))
        );
    }

    #[test]
    fn agent_staged_build_output_does_not_change_frozen_hydration() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path();
        git(root, &["init", "-q"]);
        fs::write(root.join("app.swift"), "print(\"ready\")\n").expect("source");
        git(root, &["add", "app.swift"]);
        let policy = freeze_workspace_capture_policy(root).expect("freeze");
        let frozen = policy
            .frozen_git_hydration
            .as_ref()
            .expect("explicit hydration")
            .clone();

        fs::create_dir_all(root.join(".build")).expect("build directory");
        fs::write(root.join(".build/agent-staged.o"), b"generated").expect("generated");
        git(root, &["add", "-f", ".build/agent-staged.o"]);

        let plan = capture_workspace_strict(
            root,
            &policy,
            CaptureProjection::Deliverable,
            CapturePurpose::DeliverableIndex,
        )
        .expect("strict capture");

        assert!(
            plan.entries
                .iter()
                .all(|entry| entry.relative != Path::new(".build/agent-staged.o"))
        );
        let captured = plan.manifest.git.expect("Git hydration manifest");
        assert_eq!(captured.head, frozen.head);
        assert_eq!(captured.index_sha256, frozen.index_sha256);
        assert_eq!(captured.tracked_files, frozen.tracked_paths.len() as u64);
    }

    #[test]
    fn strict_capture_fails_closed_without_explicit_hydration_record() {
        let temp = TempDir::new().expect("temp");
        fs::write(temp.path().join("source.txt"), "source\n").expect("source");
        let mut policy = freeze_workspace_capture_policy(temp.path()).expect("freeze");
        assert!(
            policy
                .frozen_git_hydration
                .as_ref()
                .is_some_and(|hydration| !hydration.repository)
        );

        policy.frozen_git_hydration = None;
        let error = capture_workspace_strict(
            temp.path(),
            &policy,
            CaptureProjection::Recoverable,
            CapturePurpose::TurnSnapshot,
        )
        .expect_err("missing hydration must fail closed");
        assert!(
            error
                .to_string()
                .contains("controller-frozen Git hydration")
        );

        capture_workspace(
            temp.path(),
            &policy,
            CaptureProjection::Recoverable,
            CapturePurpose::TurnSnapshot,
        )
        .expect("legacy compatibility capture");
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
    fn ignored_lifecycle_metadata_remains_recoverable_but_not_deliverable() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path();
        fs::create_dir_all(root.join(".deadreckon/docs")).expect("lifecycle docs");
        fs::write(root.join(".gitignore"), ".deadreckon/\n").expect("ignore");
        fs::write(root.join(".deadreckon/codebase.json"), "{}\n").expect("codebase");
        fs::write(root.join(".deadreckon/docs/RUN-NARRATIVE.md"), "# Run\n").expect("narrative");
        let policy = freeze_workspace_capture_policy(root).expect("freeze");

        let recoverable = capture_workspace(
            root,
            &policy,
            CaptureProjection::Recoverable,
            CapturePurpose::TurnSnapshot,
        )
        .expect("recoverable");
        assert!(
            recoverable
                .entries
                .iter()
                .any(|entry| { entry.relative == Path::new(".deadreckon/codebase.json") })
        );
        assert!(
            recoverable
                .entries
                .iter()
                .any(|entry| { entry.relative == Path::new(".deadreckon/docs/RUN-NARRATIVE.md") })
        );

        let deliverable = capture_workspace(
            root,
            &policy,
            CaptureProjection::Deliverable,
            CapturePurpose::DeliverableIndex,
        )
        .expect("deliverable");
        assert!(
            deliverable
                .entries
                .iter()
                .all(|entry| !entry.relative.starts_with(".deadreckon"))
        );
    }

    #[test]
    fn suspicious_generated_subtree_is_manifested_without_hiding_tracked_files() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path();
        git(root, &["init", "-q"]);
        fs::create_dir_all(root.join("generated-cache")).expect("generated cache");
        for index in 0..4 {
            fs::write(
                root.join(format!("generated-cache/{index}.o")),
                vec![index as u8; 8],
            )
            .expect("generated object");
        }
        git(root, &["add", "generated-cache/0.o"]);
        let mut policy = freeze_workspace_capture_policy(root).expect("freeze");
        policy.budgets.suspicious_files = 3;
        policy.budgets.suspicious_bytes = 3;
        policy.budgets.suspicious_generated_percent = 75;

        let plan = capture_workspace(
            root,
            &policy,
            CaptureProjection::Deliverable,
            CapturePurpose::DeliverableIndex,
        )
        .expect("capture");

        assert!(plan.manifest.partial);
        assert!(
            plan.entries.iter().any(|entry| {
                entry.relative == Path::new("generated-cache/0.o") && entry.tracked
            })
        );
        assert!(
            plan.entries
                .iter()
                .all(|entry| { entry.tracked || !entry.relative.starts_with("generated-cache") })
        );
        assert!(plan.manifest.omissions.iter().any(|omission| {
            omission.path == Path::new("generated-cache")
                && omission.reason == CaptureOmissionReason::SuspiciousGeneratedSubtree
                && omission.file_count == 4
        }));
    }

    #[test]
    fn ecosystem_policy_uses_configured_cargo_target_directory() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path();
        fs::create_dir_all(root.join("src")).expect("src");
        fs::create_dir_all(root.join(".cargo")).expect("cargo config");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"capture-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("manifest");
        fs::write(root.join("src/lib.rs"), "pub fn fixture() {}\n").expect("source");
        fs::write(
            root.join(".cargo/config.toml"),
            "[build]\ntarget-dir = \"custom-target\"\n",
        )
        .expect("cargo target config");

        let direct = super::cargo_metadata_target_directory(root).expect("direct cargo metadata");
        assert!(
            direct.is_some_and(|path| path.ends_with("custom-target")),
            "unexpected direct Cargo target"
        );
        let discovery = super::discover_policy_inputs(root);
        assert!(
            discovery
                .markers
                .iter()
                .any(|marker| marker == Path::new("Cargo.toml")),
            "markers={:?}",
            discovery.markers
        );
        let policy = freeze_workspace_capture_policy(root).expect("policy");

        assert!(
            policy.output_roots.iter().any(|output| {
                output.source == GeneratedOutputSource::CargoMetadata
                    && output
                        .path
                        .to_path_buf()
                        .is_ok_and(|path| path == Path::new("custom-target"))
            }),
            "outputs={:?} warnings={:?}",
            policy.output_roots,
            policy.warnings
        );
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
