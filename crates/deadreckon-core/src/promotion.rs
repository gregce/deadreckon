use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::codebase::{
    CodebaseMode, CodebaseRecord, read_codebase_record, read_run_codebase_record,
    read_trusted_codebase_record, write_codebase_record,
};
use crate::completion::validate_completion_receipt;
use crate::error::{DeadreckonError, IoContext, Result};
use crate::events::emit_event;
use crate::gate::validate_acceptance_marker;
use crate::paths::DeadreckonPaths;
use crate::state::{PipelineState, save_state};
use crate::workspace_capture::{
    CaptureProjection, CapturePurpose, capture_workspace_strict, materialize_capture_plan,
    require_workspace_capture_policy,
};
use deadreckon_protocol::RunEventKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromotionManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub scope: String,
    pub goal: String,
    pub promoted_at: DateTime<Utc>,
    pub source_working_dir: PathBuf,
    pub provenance_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_tree_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_policy_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_files: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromotionCheckpoint {
    CandidateReady,
    CandidateValidated,
    Published,
    PublishedValidated,
    StateSaved,
}

pub fn promote_completed_run(
    paths: &DeadreckonPaths,
    state: &mut PipelineState,
) -> Result<PathBuf> {
    promote_completed_run_with_hook(paths, state, |_, _| Ok(()))
}

/// Promote under the same cumulative Job work boundary used by verification.
/// Partially prepared candidates remain in their recoverable staging paths if
/// the boundary is reached; they are never published after expiry.
pub fn promote_completed_run_bounded(
    paths: &DeadreckonPaths,
    state: &mut PipelineState,
    scope: crate::git::WorkBoundaryScope,
) -> Result<PathBuf> {
    crate::git::with_git_command_scope(scope, || promote_completed_run(paths, state))
}

fn promote_completed_run_with_hook(
    paths: &DeadreckonPaths,
    state: &mut PipelineState,
    mut hook: impl FnMut(PromotionCheckpoint, &Path) -> Result<()>,
) -> Result<PathBuf> {
    let strict = is_strict_job(paths, state);
    let library_dir = paths.library_dir(&state.scope, &state.run_id);
    let parent = library_parent(&library_dir)?;
    let staging = legacy_staging_path(parent, &state.run_id);
    let candidate = sanitized_staging_path(parent, &state.run_id);
    let recovery_artifact_exists = library_dir.exists() || staging.exists() || candidate.exists();
    if strict && recovery_artifact_exists {
        // Older versions could publish (and even remove the run-owned
        // workspace) before saving state. Validate the existing candidate or
        // library through strict recovery first instead of requiring the stale
        // `working_dir` to remain readable.
        recover_promotion_with_hook(paths, state, &mut hook)?;
        validate_completion_receipt(paths, state)?;
    } else {
        if strict {
            validate_completion_receipt(paths, state)?;
        } else {
            validate_acceptance_marker(state)?;
        }
        recover_promotion_with_hook(paths, state, &mut hook)?;
    }
    if library_dir.exists() {
        return Ok(library_dir);
    }

    fs::create_dir_all(parent).with_path(parent)?;
    if !state.working_dir.exists() {
        return Err(DeadreckonError::NotFound(format!(
            "working directory {}",
            state.working_dir.display()
        )));
    }
    if staging.exists() {
        return Err(DeadreckonError::InvalidInput(format!(
            "legacy promotion staging {} remains after recovery",
            staging.display()
        )));
    }
    let codebase = promotion_codebase_record(paths, state, &state.working_dir, false)?;
    let source_working_dir = expected_source_working_dir(state, &codebase)?;
    prepare_candidate(
        state,
        &codebase,
        &state.working_dir,
        &source_working_dir,
        &candidate,
    )?;
    hook(PromotionCheckpoint::CandidateReady, &candidate)?;
    validate_strict_candidate(paths, state, &candidate)?;
    hook(PromotionCheckpoint::CandidateValidated, &candidate)?;
    publish_candidate(&candidate, &library_dir)?;
    hook(PromotionCheckpoint::Published, &library_dir)?;
    validate_strict_candidate(paths, state, &library_dir)?;
    hook(PromotionCheckpoint::PublishedValidated, &library_dir)?;
    finalize_published_library(state, &library_dir, &codebase, &mut hook)?;
    remove_temporary_tree(&staging)?;
    remove_temporary_tree(&candidate)?;
    Ok(library_dir)
}

pub fn recover_promotion(paths: &DeadreckonPaths, state: &mut PipelineState) -> Result<()> {
    recover_promotion_with_hook(paths, state, &mut |_, _| Ok(()))
}

fn recover_promotion_with_hook(
    paths: &DeadreckonPaths,
    state: &mut PipelineState,
    hook: &mut impl FnMut(PromotionCheckpoint, &Path) -> Result<()>,
) -> Result<()> {
    let library_dir = paths.library_dir(&state.scope, &state.run_id);
    let parent = library_parent(&library_dir)?;
    let staging = legacy_staging_path(parent, &state.run_id);
    let candidate = sanitized_staging_path(parent, &state.run_id);
    let strict = is_strict_job(paths, state);

    // A strict recovery must obtain routing and ownership only from the
    // control-plane copy. In particular, neither an agent-visible workspace
    // nor a crash-staging tree is allowed to redirect publication or cleanup.
    let codebase = promotion_codebase_record(
        paths,
        state,
        if library_dir.exists() {
            &library_dir
        } else if candidate.exists() {
            &candidate
        } else if staging.exists() {
            &staging
        } else {
            &state.working_dir
        },
        true,
    )?;
    let source_working_dir = expected_source_working_dir(state, &codebase)?;

    if library_dir.exists() {
        validate_manifest(state, &library_dir, &source_working_dir)?;
        if strict {
            validate_strict_candidate(paths, state, &library_dir)?;
        }
        finalize_published_library(state, &library_dir, &codebase, hook)?;
        remove_temporary_tree(&staging)?;
        remove_temporary_tree(&candidate)?;
        return Ok(());
    }
    if !candidate.exists() && !staging.exists() {
        return Ok(());
    }

    fs::create_dir_all(parent).with_path(parent)?;
    if candidate.exists()
        && let Err(error) = validate_manifest(state, &candidate, &source_working_dir)
    {
        if !source_working_dir.exists() && !staging.exists() {
            return Err(error);
        }
        remove_temporary_tree(&candidate)?;
    }
    if !candidate.exists() {
        let source = if source_working_dir.exists() {
            source_working_dir.as_path()
        } else if staging.exists() {
            staging.as_path()
        } else {
            return Ok(());
        };
        prepare_candidate(state, &codebase, source, &source_working_dir, &candidate)?;
        hook(PromotionCheckpoint::CandidateReady, &candidate)?;
    }
    validate_manifest(state, &candidate, &source_working_dir)?;
    if strict {
        validate_strict_candidate(paths, state, &candidate)?;
    }
    hook(PromotionCheckpoint::CandidateValidated, &candidate)?;
    publish_candidate(&candidate, &library_dir)?;
    hook(PromotionCheckpoint::Published, &library_dir)?;
    if strict {
        validate_strict_candidate(paths, state, &library_dir)?;
    }
    hook(PromotionCheckpoint::PublishedValidated, &library_dir)?;
    finalize_published_library(state, &library_dir, &codebase, hook)?;
    remove_temporary_tree(&staging)?;
    remove_temporary_tree(&candidate)?;
    Ok(())
}

fn is_strict_job(paths: &DeadreckonPaths, state: &PipelineState) -> bool {
    // Once any durable Job control directory exists, missing or damaged Job
    // files must fail closed through receipt validation rather than silently
    // downgrading the run to the legacy acceptance-marker path.
    fs::symlink_metadata(paths.job_dir(&state.run_id)).is_ok()
}

fn promotion_codebase_record(
    paths: &DeadreckonPaths,
    state: &PipelineState,
    fallback_workspace: &Path,
    recovery: bool,
) -> Result<CodebaseRecord> {
    if is_strict_job(paths, state) {
        return read_trusted_codebase_record(&state.run_root).map_err(|error| {
            DeadreckonError::InvalidInput(format!(
                "strict promotion {} requires its trusted run-root codebase record: {error}",
                state.run_id
            ))
        });
    }
    if recovery {
        read_trusted_codebase_record(&state.run_root)
            .or_else(|_| read_codebase_record(fallback_workspace))
    } else {
        read_run_codebase_record(&state.run_root, fallback_workspace)
    }
}

fn expected_source_working_dir(
    state: &PipelineState,
    codebase: &CodebaseRecord,
) -> Result<PathBuf> {
    match codebase.mode {
        CodebaseMode::Fresh | CodebaseMode::Copy => Ok(state.run_root.join("working")),
        CodebaseMode::InPlace => codebase.source_path.clone().ok_or_else(|| {
            DeadreckonError::InvalidInput(format!(
                "in-place promotion {} has no trusted source path",
                state.run_id
            ))
        }),
        CodebaseMode::Worktree => codebase.worktree_path.clone().ok_or_else(|| {
            DeadreckonError::InvalidInput(format!(
                "worktree promotion {} has no trusted worktree path",
                state.run_id
            ))
        }),
    }
}

fn prepare_candidate(
    state: &PipelineState,
    codebase: &CodebaseRecord,
    source: &Path,
    source_working_dir: &Path,
    candidate: &Path,
) -> Result<()> {
    remove_temporary_tree(candidate)?;
    let policy = require_workspace_capture_policy(state)?;
    let capture = capture_workspace_strict(
        source,
        &policy,
        CaptureProjection::Promotable,
        CapturePurpose::PromotionCandidate,
    )?;
    capture.require_complete("promotion candidate")?;
    materialize_capture_plan(&capture, candidate)?;
    write_codebase_record(candidate, codebase)?;
    write_manifest(state, candidate, source_working_dir.to_path_buf(), &policy)
}

fn publish_candidate(candidate: &Path, library_dir: &Path) -> Result<()> {
    if library_dir.exists() {
        return Err(DeadreckonError::InvalidInput(format!(
            "refusing to replace existing promotion library {}",
            library_dir.display()
        )));
    }
    fs::rename(candidate, library_dir).with_path(candidate)
}

fn validate_strict_candidate(
    paths: &DeadreckonPaths,
    state: &PipelineState,
    candidate: &Path,
) -> Result<()> {
    if !is_strict_job(paths, state) {
        return Ok(());
    }
    let mut candidate_state = state.clone();
    candidate_state.working_dir = candidate.to_path_buf();
    candidate_state.promoted_library_dir = Some(candidate.to_path_buf());
    validate_completion_receipt(paths, &candidate_state).map(|_| ())
}

fn finalize_published_library(
    state: &mut PipelineState,
    library_dir: &Path,
    codebase: &CodebaseRecord,
    hook: &mut impl FnMut(PromotionCheckpoint, &Path) -> Result<()>,
) -> Result<()> {
    let source_working_dir = expected_source_working_dir(state, codebase)?;
    let manifest = validate_manifest(state, library_dir, &source_working_dir)?;
    let newly_recorded = state.working_dir != library_dir
        || state.promoted_library_dir.as_deref() != Some(library_dir);
    state.working_dir = library_dir.to_path_buf();
    state.promoted_library_dir = Some(library_dir.to_path_buf());
    if newly_recorded {
        emit_event(
            state,
            None,
            RunEventKind::RunPromoted {
                library_dir: library_dir.to_path_buf(),
            },
        )?;
    }
    // State must durably point at the published result before the only
    // run-owned working copy can be removed.
    save_state(state)?;
    hook(PromotionCheckpoint::StateSaved, library_dir)?;
    cleanup_owned_working(state, library_dir, &manifest)
}

fn cleanup_owned_working(
    state: &PipelineState,
    library_dir: &Path,
    manifest: &PromotionManifest,
) -> Result<()> {
    let owned_working = state.run_root.join("working");
    if manifest.source_working_dir != owned_working || owned_working == library_dir {
        return Ok(());
    }
    let metadata = match fs::symlink_metadata(&owned_working) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: owned_working,
                source,
            });
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(DeadreckonError::InvalidInput(format!(
            "refusing to clean run-owned working path {} because it is not a real directory",
            owned_working.display()
        )));
    }
    remove_real_directory_tree(&owned_working)
}

fn validate_manifest(
    state: &PipelineState,
    library_dir: &Path,
    expected_source_working_dir: &Path,
) -> Result<PromotionManifest> {
    let directory_metadata = fs::symlink_metadata(library_dir).with_path(library_dir)?;
    if directory_metadata.file_type().is_symlink() || !directory_metadata.is_dir() {
        return Err(DeadreckonError::InvalidInput(format!(
            "promotion library {} is not a real directory",
            library_dir.display()
        )));
    }
    let path = library_dir.join("manifest.json");
    let metadata = fs::symlink_metadata(&path).with_path(&path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(DeadreckonError::InvalidInput(format!(
            "promotion manifest {} is not a regular file",
            path.display()
        )));
    }
    let raw = fs::read(&path).with_path(&path)?;
    let manifest: PromotionManifest =
        serde_json::from_slice(&raw).map_err(|source| DeadreckonError::Json {
            path: path.clone(),
            source,
        })?;
    let expected_provenance = provenance_hash(&state.run_root.join("provenance.jsonl"))?;
    if !matches!(manifest.schema_version, 1 | 2)
        || manifest.run_id != state.run_id
        || manifest.scope != state.scope
        || manifest.goal != state.goal
        || manifest.source_working_dir != expected_source_working_dir
        || manifest.provenance_hash != expected_provenance
    {
        return Err(DeadreckonError::InvalidInput(format!(
            "promotion manifest {} does not match run {}, its trusted source, and provenance",
            path.display(),
            state.run_id
        )));
    }
    if manifest.schema_version == 2 {
        let policy = require_workspace_capture_policy(state)?;
        let payload = promotable_payload_identity(library_dir, &policy)?;
        if manifest.payload_tree_sha256.as_deref() != Some(payload.tree_sha256.as_str())
            || manifest.capture_policy_sha256.as_deref()
                != Some(payload.capture_policy_sha256.as_str())
            || manifest.payload_files != Some(payload.files)
            || manifest.payload_bytes != Some(payload.bytes)
        {
            return Err(DeadreckonError::InvalidInput(format!(
                "promotion payload at {} does not match its trusted capture manifest",
                library_dir.display()
            )));
        }
    }
    Ok(manifest)
}

fn library_parent(library_dir: &Path) -> Result<&Path> {
    library_dir.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "library path has no parent: {}",
            library_dir.display()
        ))
    })
}

fn legacy_staging_path(parent: &Path, run_id: &str) -> PathBuf {
    parent.join(format!(".{run_id}.promoting"))
}

fn sanitized_staging_path(parent: &Path, run_id: &str) -> PathBuf {
    parent.join(format!(".{run_id}.promoting.sanitized"))
}

fn remove_temporary_tree(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(DeadreckonError::InvalidInput(format!(
                "refusing to remove promotion temporary path {} because it is not a real directory",
                path.display()
            )))
        }
        Ok(_) => remove_real_directory_tree(path),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn remove_real_directory_tree(path: &Path) -> Result<()> {
    crate::workspace_capture::remove_captured_directory_tree(path)
}

fn write_manifest(
    state: &PipelineState,
    library_dir: &Path,
    source_working_dir: PathBuf,
    policy: &crate::WorkspaceCapturePolicy,
) -> Result<()> {
    fs::create_dir_all(library_dir).with_path(library_dir)?;
    let payload = promotable_payload_identity(library_dir, policy)?;
    let manifest = PromotionManifest {
        schema_version: 2,
        run_id: state.run_id.clone(),
        scope: state.scope.clone(),
        goal: state.goal.clone(),
        promoted_at: Utc::now(),
        source_working_dir,
        provenance_hash: provenance_hash(&state.run_root.join("provenance.jsonl"))?,
        payload_tree_sha256: Some(payload.tree_sha256),
        capture_policy_sha256: Some(payload.capture_policy_sha256),
        payload_files: Some(payload.files),
        payload_bytes: Some(payload.bytes),
    };
    let path = library_dir.join("manifest.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&manifest).map_err(|source| DeadreckonError::Json {
            path: path.clone(),
            source,
        })?,
    )
    .with_path(path)
}

struct PromotablePayloadIdentity {
    tree_sha256: String,
    capture_policy_sha256: String,
    files: u64,
    bytes: u64,
}

fn promotable_payload_identity(
    root: &Path,
    policy: &crate::WorkspaceCapturePolicy,
) -> Result<PromotablePayloadIdentity> {
    let capture = capture_workspace_strict(
        root,
        policy,
        CaptureProjection::Promotable,
        CapturePurpose::PromotionCandidate,
    )?;
    capture.require_complete("promotion payload verification")?;
    let capture_policy_sha256 = capture.manifest.policy_sha256.clone();
    let mut index = crate::flight::artifact_file_index_from_capture(capture)?;
    index.files.remove(Path::new("manifest.json"));
    index.files.remove(Path::new(".materialized-to"));
    let files = index.files.len() as u64;
    let bytes = index
        .files
        .values()
        .map(|fingerprint| fingerprint.size)
        .sum();
    Ok(PromotablePayloadIdentity {
        tree_sha256: index.tree_hash(),
        capture_policy_sha256,
        files,
        bytes,
    })
}

fn provenance_hash(path: &Path) -> Result<String> {
    let mut hasher = DefaultHasher::new();
    match fs::read(path) {
        Ok(bytes) => bytes.hash(&mut hasher),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => 0_u8.hash(&mut hasher),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    }
    Ok(format!("{:016x}", hasher.finish()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use chrono::Utc;
    use deadreckon_protocol::{
        AuthorityAcceptedBy, GoalCoverage, GoalCoverageStatus, Job, JobAuthority, JobId, JobPolicy,
        JobSchemaVersion, JobShape, RunId, SandboxBoundaryObservation,
        SandboxBoundaryObservationIssuer, SemanticDecision, SemanticJudgeMode, SemanticJudgment,
    };
    use tempfile::TempDir;
    use uuid::Uuid;

    use crate::codebase::{
        CODEBASE_RECORD_VERSION, CodebaseMode, CodebaseRecord, TRUSTED_CODEBASE_RECORD,
        write_codebase_record, write_trusted_codebase_record,
    };
    use crate::completion::{SEMANTIC_JUDGMENT_JSON, seal_completion_receipt};
    use crate::error::DeadreckonError;
    use crate::flight::{build_deliverable_file_index, sha256_file, sha256_text};
    use crate::gate::{
        AcceptanceCheckResult, AcceptanceContainment, read_gate_key, validate_acceptance_marker,
        write_acceptance_marker, write_native_acceptance_marker_with_results_and_key,
    };
    use crate::job::{load_job, write_job};
    use crate::paths::DeadreckonPaths;
    use crate::state::{RunOptions, atomic_write_json, create_run, load_state};

    use super::{
        PromotionCheckpoint, PromotionManifest, promote_completed_run,
        promote_completed_run_with_hook, recover_promotion,
    };

    struct Fixture {
        _temp: TempDir,
        paths: DeadreckonPaths,
        state: crate::PipelineState,
        external_source: PathBuf,
    }

    fn codebase_record(mode: CodebaseMode, external_source: &Path) -> CodebaseRecord {
        CodebaseRecord {
            schema_version: CODEBASE_RECORD_VERSION,
            mode,
            source_path: (!matches!(mode, CodebaseMode::Fresh))
                .then(|| external_source.to_path_buf()),
            source_git_root: (mode == CodebaseMode::Worktree)
                .then(|| external_source.to_path_buf()),
            branch_name: (mode == CodebaseMode::Worktree).then(|| "deadreckon/test".to_string()),
            base_ref: (mode == CodebaseMode::Worktree).then(|| "main".to_string()),
            base_sha: (mode == CodebaseMode::Worktree).then(|| "approved-base".to_string()),
            parent_branch: None,
            worktree_path: (mode == CodebaseMode::Worktree).then(|| external_source.to_path_buf()),
            dirty_files_seeded: false,
            head_was_detached: false,
            created_at: Utc::now(),
            deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
            doc_polish_hash: None,
        }
    }

    fn fixture(mode: CodebaseMode) -> Fixture {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let external_source = temp.path().join("external-source");
        fs::create_dir_all(&external_source).expect("external source");
        fs::write(external_source.join("operator.txt"), "operator-owned\n").expect("operator file");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "promote".to_string(),
                cwd: external_source.clone(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: None,
                codebase: Some(codebase_record(mode, &external_source)),
            },
        )
        .expect("run");
        fs::write(state.working_dir.join("result.txt"), "verified result\n").expect("result");
        write_acceptance_marker(
            &state.run_root,
            state.run_id.clone(),
            state.working_dir.clone(),
            1,
        )
        .expect("marker");
        Fixture {
            _temp: temp,
            paths,
            state,
            external_source,
        }
    }

    fn strict_fixture() -> Fixture {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let external_source = temp.path().join("source");
        fs::create_dir_all(&external_source).expect("source");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "ship verified change".to_string(),
                cwd: external_source.clone(),
                sandbox: "sandbox-exec".to_string(),
                provider: Some("judge".to_string()),
                skill_name: "deadreckon".to_string(),
                max_spend_usd: Some(2.0),
                max_wall_seconds: Some(60.0),
                run_id: Some("strict-job".to_string()),
                codebase: Some(codebase_record(CodebaseMode::Fresh, &external_source)),
            },
        )
        .expect("run");
        fs::write(state.working_dir.join("result.txt"), "verified\n").expect("result");
        let contract_path = crate::acceptance_spec_path_for_run_root(&state.run_root);
        fs::write(
            &contract_path,
            "name: result\nchecks:\n  - kind: file_exists\n    path: result.txt\n    must_pass: true\n",
        )
        .expect("contract");
        fs::create_dir_all(paths.job_dir(&state.run_id)).expect("job dir");
        let launch_path = paths.job_launch_plan(&state.run_id);
        fs::write(
            &launch_path,
            "{\"schema\":1,\"goal\":\"ship verified change\"}\n",
        )
        .expect("launch");
        let policy = JobPolicy {
            max_spend_usd: 2.0,
            max_wall_seconds: 60,
            max_attempts: 3,
            deadline: None,
            semantic_judge: SemanticJudgeMode::Required,
            execution: Some(deadreckon_protocol::JobExecutionPolicy::workspace_only(
                "sandbox-exec",
            )),
        };
        let authority = JobAuthority {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(state.run_id.clone()),
            run_id: RunId(state.run_id.clone()),
            approved_at: Utc::now(),
            accepted_by: AuthorityAcceptedBy::Operator,
            goal_sha256: sha256_text(&state.goal),
            contract_sha256: sha256_file(&contract_path).expect("contract digest"),
            effective_policy_sha256: sha256_text(
                &serde_json::to_string(&policy).expect("policy json"),
            ),
            launch_plan_sha256: sha256_file(&launch_path).expect("launch digest"),
            source_tree_sha256: build_deliverable_file_index(&state.working_dir)
                .expect("source index")
                .tree_hash(),
            source_revision: None,
            sandbox_requested: "sandbox-exec".to_string(),
            semantic_judge_mode: SemanticJudgeMode::Required,
            gate_evaluator_sha256: None,
        };
        atomic_write_json(&paths.job_authority(&state.run_id), &authority).expect("authority");
        let job = Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(state.run_id.clone()),
            scope: state.scope.clone(),
            goal: state.goal.clone(),
            shape: JobShape::Single,
            created_at: Utc::now(),
            source_cwd: state.cwd.clone(),
            launch_plan_sha256: authority.launch_plan_sha256.clone(),
            authority_sha256: sha256_file(&paths.job_authority(&state.run_id))
                .expect("authority digest"),
            policy,
        };
        write_job(&paths, &job).expect("job");
        let key = read_gate_key(&paths, &state.run_id).expect("gate key");
        let marker = write_native_acceptance_marker_with_results_and_key(
            &state.run_root,
            state.run_id.clone(),
            state.working_dir.clone(),
            vec![AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: true,
                must_pass: true,
                detail: "result exists".to_string(),
                command: None,
                cwd: None,
                duration_ms: None,
                stdout: None,
                stderr: None,
            }],
            &key,
            AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("marker");
        let judgment = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(state.run_id.clone()),
            run_id: RunId(state.run_id.clone()),
            judged_at: Utc::now(),
            provider: "judge".to_string(),
            model: "judge-model".to_string(),
            decision: SemanticDecision::Achieved,
            summary: "the result satisfies the approved goal".to_string(),
            goal_coverage: vec![GoalCoverage {
                claim: state.goal.clone(),
                status: GoalCoverageStatus::Met,
                evidence: vec!["deterministic-gate".to_string()],
            }],
            missing: Vec::new(),
            input_sha256: sha256_text("evidence"),
            spend_usd: 0.01,
        };
        atomic_write_json(&state.run_root.join(SEMANTIC_JUDGMENT_JSON), &judgment)
            .expect("judgment");
        seal_boundary_observation(&paths, &state, &authority);
        seal_completion_receipt(&paths, &state, &authority, &marker, &judgment)
            .expect("seal receipt");
        Fixture {
            _temp: temp,
            paths,
            state,
            external_source,
        }
    }

    fn strict_worktree_fixture() -> Fixture {
        let fixture = strict_fixture();
        let working = fixture.state.working_dir.clone();
        git_ok(&working, &["init", "-q"]);
        git_ok(
            &working,
            &["config", "user.email", "deadreckon@example.invalid"],
        );
        git_ok(&working, &["config", "user.name", "DeadReckon Test"]);
        fs::write(working.join(".git/info/exclude"), ".deadreckon/\n").expect("git exclude");
        git_ok(&working, &["add", "-A"]);
        git_ok(&working, &["commit", "-q", "-m", "approved base"]);
        let base = git_stdout(&working, &["rev-parse", "HEAD"]);
        fs::write(working.join("result.txt"), "verified worktree result\n").expect("result change");
        git_ok(&working, &["add", "result.txt"]);
        git_ok(&working, &["commit", "-q", "-m", "deliver result"]);
        let branch = git_stdout(&working, &["symbolic-ref", "--short", "HEAD"]);
        let git_root = fixture.external_source.join("result-repository");
        let working_arg = working.to_string_lossy().into_owned();
        let git_root_arg = git_root.to_string_lossy().into_owned();
        git_ok(
            &fixture.external_source,
            &["clone", "-q", &working_arg, &git_root_arg],
        );
        let record = CodebaseRecord {
            schema_version: CODEBASE_RECORD_VERSION,
            mode: CodebaseMode::Worktree,
            source_path: Some(fixture.external_source.clone()),
            source_git_root: Some(git_root),
            branch_name: Some(branch),
            base_ref: Some(base.clone()),
            base_sha: Some(base.clone()),
            parent_branch: None,
            worktree_path: Some(working.clone()),
            dirty_files_seeded: false,
            head_was_detached: false,
            created_at: Utc::now(),
            deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
            doc_polish_hash: None,
        };
        write_trusted_codebase_record(&fixture.state.run_root, &record)
            .expect("trusted worktree record");
        write_codebase_record(&working, &record).expect("workspace worktree record");
        let authority_path = fixture.paths.job_authority(&fixture.state.run_id);
        let mut authority: JobAuthority =
            serde_json::from_slice(&fs::read(&authority_path).expect("authority"))
                .expect("authority json");
        authority.source_revision = Some(base);
        atomic_write_json(&authority_path, &authority).expect("updated authority");
        let mut job = load_job(&fixture.paths, &fixture.state.run_id).expect("job");
        job.authority_sha256 = sha256_file(&authority_path).expect("authority digest");
        atomic_write_json(&fixture.paths.job_json(&fixture.state.run_id), &job)
            .expect("updated job");
        let marker = validate_acceptance_marker(&fixture.state).expect("marker");
        let judgment_path = fixture.state.run_root.join(SEMANTIC_JUDGMENT_JSON);
        let judgment: SemanticJudgment =
            serde_json::from_slice(&fs::read(&judgment_path).expect("judgment"))
                .expect("judgment json");
        seal_boundary_observation(&fixture.paths, &fixture.state, &authority);
        seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &authority,
            &marker,
            &judgment,
        )
        .expect("reseal worktree receipt");
        fixture
    }

    fn seal_boundary_observation(
        paths: &DeadreckonPaths,
        state: &crate::PipelineState,
        authority: &JobAuthority,
    ) {
        let observation = SandboxBoundaryObservation {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: authority.job_id.clone(),
            run_id: authority.run_id.clone(),
            observed_at: Utc::now(),
            issuer: SandboxBoundaryObservationIssuer::DeadreckonController,
            probe_id: Uuid::new_v4().to_string(),
            attempt: 1,
            outer_launch_id: Uuid::new_v4().to_string(),
            authority_sha256: sha256_file(&paths.job_authority(authority.job_id.as_ref()))
                .expect("authority digest"),
            contract_sha256: authority.contract_sha256.clone(),
            result_tree_sha256: crate::sandbox_boundary_result_tree_sha256(state)
                .expect("result tree"),
            sandbox_requested: authority.sandbox_requested.clone(),
            sandbox_backend: "sandbox-exec".to_string(),
            contained: true,
            gate_key_read_denied: true,
            proof_write_denied: true,
            control_write_denied: true,
            operator_capture_read_denied: true,
            operator_capture_write_denied: true,
            signing_env_scrubbed: true,
            probe_sha256: sha256_text("fixed controller probe"),
            gate_evaluator_sha256: authority.gate_evaluator_sha256.clone(),
            signature: String::new(),
        };
        crate::seal_sandbox_boundary_observation(paths, state, authority, &observation)
            .expect("sandbox boundary observation");
    }

    fn git_ok(cwd: &Path, args: &[&str]) {
        let output = crate::git::run_git(cwd, args).expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(cwd: &Path, args: &[&str]) -> String {
        let output = crate::git::run_git(cwd, args).expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn crash_error(checkpoint: PromotionCheckpoint) -> DeadreckonError {
        DeadreckonError::InvalidInput(format!("injected crash at {checkpoint:?}"))
    }

    #[test]
    fn promotion_recovery_is_idempotent_across_each_durable_crash_window() {
        for crash_point in [
            PromotionCheckpoint::CandidateValidated,
            PromotionCheckpoint::PublishedValidated,
            PromotionCheckpoint::StateSaved,
        ] {
            let mut fixture = fixture(CodebaseMode::Fresh);
            let state_path = fixture.state.state_path();
            let original_working = fixture.state.working_dir.clone();
            let library = fixture
                .paths
                .library_dir(&fixture.state.scope, &fixture.state.run_id);
            let error = promote_completed_run_with_hook(
                &fixture.paths,
                &mut fixture.state,
                |checkpoint, _| {
                    if checkpoint == crash_point {
                        Err(crash_error(checkpoint))
                    } else {
                        Ok(())
                    }
                },
            )
            .expect_err("failpoint must interrupt promotion");
            assert!(error.to_string().contains("injected crash"));

            let persisted = load_state(&state_path).expect("persisted state");
            if crash_point == PromotionCheckpoint::StateSaved {
                assert_eq!(persisted.working_dir, library);
                assert!(original_working.exists());
            } else {
                assert_eq!(persisted.working_dir, original_working);
            }

            let mut restarted = persisted;
            recover_promotion(&fixture.paths, &mut restarted).expect("recover");
            recover_promotion(&fixture.paths, &mut restarted).expect("idempotent retry");
            let recovered = load_state(&state_path).expect("recovered state");
            assert_eq!(recovered.working_dir, library);
            assert_eq!(
                recovered.promoted_library_dir.as_deref(),
                Some(library.as_path())
            );
            assert!(library.join("manifest.json").is_file());
            assert_eq!(
                fs::read_to_string(library.join("result.txt")).expect("result"),
                "verified result\n"
            );
            assert!(!original_working.exists());
        }
    }

    #[test]
    fn promotion_retains_lifecycle_metadata_but_excludes_specstory_and_runtime_output() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "promote classified result".to_string(),
                cwd: source,
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
        fs::create_dir_all(state.working_dir.join(".specstory/history")).expect("private");
        fs::create_dir_all(state.working_dir.join("target/debug")).expect("runtime");
        fs::write(state.working_dir.join("result.txt"), "deliverable\n").expect("result");
        fs::write(
            state.working_dir.join(".specstory/history/session.md"),
            "private\n",
        )
        .expect("private");
        fs::write(state.working_dir.join("target/debug/output"), "runtime\n").expect("runtime");
        write_acceptance_marker(
            &state.run_root,
            state.run_id.clone(),
            state.working_dir.clone(),
            1,
        )
        .expect("marker");

        let promoted = promote_completed_run(&paths, &mut state).expect("promote");

        assert!(promoted.join("result.txt").exists());
        assert!(promoted.join(".deadreckon/codebase.json").exists());
        assert!(!promoted.join(".specstory").exists());
        assert!(!promoted.join("target").exists());
    }

    #[test]
    fn promotion_refuses_to_bless_an_existing_library_without_a_valid_manifest() {
        for manifest in [None, Some(b"not-json".as_slice())] {
            let mut fixture = fixture(CodebaseMode::Fresh);
            let library = fixture
                .paths
                .library_dir(&fixture.state.scope, &fixture.state.run_id);
            fs::create_dir_all(&library).expect("library");
            fs::write(library.join("orphan.txt"), "untrusted\n").expect("orphan");
            if let Some(manifest) = manifest {
                fs::write(library.join("manifest.json"), manifest).expect("invalid manifest");
            }

            promote_completed_run(&fixture.paths, &mut fixture.state)
                .expect_err("orphan library must fail closed");

            if manifest.is_none() {
                assert!(!library.join("manifest.json").exists());
            }
            assert!(fixture.state.run_root.join("working").exists());
            assert_ne!(fixture.state.working_dir, library);
        }
    }

    #[test]
    fn recovery_reconstructs_legacy_staging_before_publishing_it() {
        let mut fixture = fixture(CodebaseMode::Fresh);
        fs::create_dir_all(fixture.state.working_dir.join(".specstory/history")).expect("private");
        fs::write(
            fixture
                .state
                .working_dir
                .join(".specstory/history/session.md"),
            "provider evidence\n",
        )
        .expect("private");
        fs::create_dir_all(fixture.state.working_dir.join("target/debug")).expect("runtime");
        fs::write(
            fixture.state.working_dir.join("target/debug/output"),
            "runtime\n",
        )
        .expect("runtime");

        let library = fixture
            .paths
            .library_dir(&fixture.state.scope, &fixture.state.run_id);
        let parent = library.parent().expect("library parent");
        fs::create_dir_all(parent).expect("library parent");
        let staging = parent.join(format!(".{}.promoting", fixture.state.run_id));
        fs::rename(&fixture.state.working_dir, &staging).expect("legacy staging");

        recover_promotion(&fixture.paths, &mut fixture.state).expect("recover");

        assert_eq!(
            fs::read_to_string(library.join("result.txt")).expect("result"),
            "verified result\n"
        );
        assert!(library.join(".deadreckon/codebase.json").is_file());
        assert!(library.join("manifest.json").is_file());
        assert!(!library.join(".specstory").exists());
        assert!(!library.join("target").exists());
        assert!(!staging.exists());
        assert!(
            !parent
                .join(format!(".{}.promoting.sanitized", fixture.state.run_id))
                .exists()
        );
    }

    #[test]
    fn recovery_preserves_operator_directories_for_copy_in_place_and_worktree() {
        for mode in [
            CodebaseMode::Copy,
            CodebaseMode::InPlace,
            CodebaseMode::Worktree,
        ] {
            let mut fixture = fixture(mode);
            let state_path = fixture.state.state_path();
            let original_working = fixture.state.working_dir.clone();
            let external_source = fixture.external_source.clone();
            promote_completed_run_with_hook(&fixture.paths, &mut fixture.state, |checkpoint, _| {
                if checkpoint == PromotionCheckpoint::StateSaved {
                    Err(crash_error(checkpoint))
                } else {
                    Ok(())
                }
            })
            .expect_err("crash after state save");

            assert!(external_source.join("operator.txt").is_file());
            let mut restarted = load_state(&state_path).expect("restart state");
            recover_promotion(&fixture.paths, &mut restarted).expect("recover");
            assert!(external_source.join("operator.txt").is_file());
            match mode {
                CodebaseMode::Copy => assert!(!original_working.exists()),
                CodebaseMode::InPlace | CodebaseMode::Worktree => {
                    assert!(original_working.join("result.txt").is_file())
                }
                CodebaseMode::Fresh => unreachable!(),
            }
        }
    }

    #[test]
    fn recovery_uses_manifest_source_only_after_matching_trusted_ownership() {
        let mut fixture = fixture(CodebaseMode::InPlace);
        let external = fixture.state.working_dir.clone();
        let library = fixture
            .paths
            .library_dir(&fixture.state.scope, &fixture.state.run_id);
        promote_completed_run_with_hook(&fixture.paths, &mut fixture.state, |checkpoint, _| {
            if checkpoint == PromotionCheckpoint::PublishedValidated {
                Err(crash_error(checkpoint))
            } else {
                Ok(())
            }
        })
        .expect_err("crash after publish");
        let manifest_path = library.join("manifest.json");
        let mut manifest: PromotionManifest =
            serde_json::from_slice(&fs::read(&manifest_path).expect("manifest")).expect("json");
        let owned_working = fixture.state.run_root.join("working");
        fs::create_dir_all(&owned_working).expect("owned-looking path");
        fs::write(owned_working.join("sentinel"), "keep\n").expect("sentinel");
        manifest.source_working_dir = owned_working.clone();
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("manifest json"),
        )
        .expect("tamper manifest");

        recover_promotion(&fixture.paths, &mut fixture.state)
            .expect_err("trusted source mismatch must fail");

        assert!(external.join("operator.txt").is_file());
        assert!(owned_working.join("sentinel").is_file());
    }

    #[test]
    fn promotion_manifest_binds_the_exact_materialized_payload() {
        let mut fixture = fixture(CodebaseMode::Fresh);
        let library =
            promote_completed_run(&fixture.paths, &mut fixture.state).expect("initial promotion");
        let manifest: PromotionManifest =
            serde_json::from_slice(&fs::read(library.join("manifest.json")).expect("manifest"))
                .expect("manifest json");
        assert_eq!(manifest.schema_version, 2);
        assert!(manifest.payload_tree_sha256.is_some());
        assert!(manifest.capture_policy_sha256.is_some());

        fs::write(library.join("result.txt"), "tampered after publication\n")
            .expect("tamper payload");
        let error = recover_promotion(&fixture.paths, &mut fixture.state)
            .expect_err("payload tamper must fail closed");
        assert!(
            error
                .to_string()
                .contains("does not match its trusted capture manifest"),
            "{error}"
        );
    }

    #[test]
    fn strict_promotion_validates_the_exact_candidate_before_and_after_publish() {
        let mut before = strict_fixture();
        let before_library = before
            .paths
            .library_dir(&before.state.scope, &before.state.run_id);
        promote_completed_run_with_hook(&before.paths, &mut before.state, |checkpoint, path| {
            if checkpoint == PromotionCheckpoint::CandidateReady {
                fs::write(path.join("result.txt"), "tampered before validation\n")
                    .expect("tamper candidate");
            }
            Ok(())
        })
        .expect_err("candidate tamper must invalidate receipt");
        assert!(!before_library.exists());

        let mut after = strict_fixture();
        let state_path = after.state.state_path();
        let original_working = after.state.working_dir.clone();
        let after_library = after
            .paths
            .library_dir(&after.state.scope, &after.state.run_id);
        promote_completed_run_with_hook(&after.paths, &mut after.state, |checkpoint, path| {
            if checkpoint == PromotionCheckpoint::Published {
                fs::write(path.join("result.txt"), "tampered after publish\n")
                    .expect("tamper library");
            }
            Ok(())
        })
        .expect_err("published tamper must invalidate receipt");
        let persisted = load_state(&state_path).expect("state");
        assert_eq!(persisted.working_dir, original_working);
        assert!(after_library.exists());
        recover_promotion(&after.paths, &mut after.state)
            .expect_err("recovery must continue refusing tampered library");
    }

    #[test]
    fn strict_recovery_never_falls_back_to_workspace_or_staging_routing() {
        let mut fixture = strict_fixture();
        let library = fixture
            .paths
            .library_dir(&fixture.state.scope, &fixture.state.run_id);
        promote_completed_run_with_hook(&fixture.paths, &mut fixture.state, |checkpoint, _| {
            if checkpoint == PromotionCheckpoint::CandidateValidated {
                Err(crash_error(checkpoint))
            } else {
                Ok(())
            }
        })
        .expect_err("crash with validated staging");
        fs::remove_file(fixture.state.run_root.join(TRUSTED_CODEBASE_RECORD))
            .expect("remove trusted record");

        let error = recover_promotion(&fixture.paths, &mut fixture.state)
            .expect_err("strict recovery must fail closed");

        assert!(
            error
                .to_string()
                .contains("trusted run-root codebase record")
        );
        assert!(!library.exists());
        assert!(
            library
                .parent()
                .expect("parent")
                .join(format!(".{}.promoting.sanitized", fixture.state.run_id))
                .exists()
        );
    }

    #[test]
    fn strict_worktree_recovery_validates_the_candidate_after_the_checkout_is_gone() {
        let mut fixture = strict_worktree_fixture();
        let stale_state_path = fixture.state.state_path();
        let original_worktree = fixture.state.working_dir.clone();
        let library = fixture
            .paths
            .library_dir(&fixture.state.scope, &fixture.state.run_id);
        promote_completed_run_with_hook(&fixture.paths, &mut fixture.state, |checkpoint, _| {
            if checkpoint == PromotionCheckpoint::CandidateValidated {
                Err(crash_error(checkpoint))
            } else {
                Ok(())
            }
        })
        .expect_err("crash with a receipt-validated candidate");
        fs::remove_dir_all(&original_worktree).expect("simulate old cleanup ordering");
        let mut stale_state = load_state(&stale_state_path).expect("stale state");
        assert_eq!(stale_state.working_dir, original_worktree);

        let promoted = promote_completed_run(&fixture.paths, &mut stale_state)
            .expect("recover before stale validation");

        assert_eq!(promoted, library);
        assert_eq!(
            fs::read_to_string(library.join("result.txt")).expect("published result"),
            "verified worktree result\n"
        );
        crate::validate_completion_receipt(&fixture.paths, &stale_state)
            .expect("candidate tree remains bound to the signed Git result");
    }

    #[test]
    fn untampered_strict_promotion_remains_receipt_valid_after_publication() {
        let mut fixture = strict_fixture();
        let library =
            promote_completed_run(&fixture.paths, &mut fixture.state).expect("strict promotion");

        crate::validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect("receipt remains valid");
        assert_eq!(fixture.state.working_dir, library);
        assert!(!fixture.state.run_root.join("working").exists());
    }
}
