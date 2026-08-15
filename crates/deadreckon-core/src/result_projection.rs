//! Controller-sealed operator-visible result projection.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::flight::{ArtifactFileIndex, artifact_file_index_from_capture, sha256_text};
use crate::state::{PipelineState, atomic_write_json};
use crate::workspace_capture::{
    CaptureOmission, CaptureProjection, CapturePurpose, WorkspaceCapturePolicy,
    capture_workspace_strict, freeze_result_projection_policy, materialize_capture_plan,
    remove_captured_directory_tree, require_workspace_capture_policy,
};

pub const RESULT_PROJECTION_DIR: &str = "result-projection";
pub const RESULT_PROJECTION_POLICY_JSON: &str = "policy.json";
pub const RESULT_PROJECTION_MANIFEST_JSON: &str = "manifest.json";
pub const RESULT_PROJECTION_CANDIDATE_DIR: &str = "candidate";
pub const RESULT_PROJECTION_EVALUATION_DIR: &str = "evaluation";
pub const RESULT_PROJECTION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultProjectionManifest {
    pub schema_version: u32,
    pub run_id: String,
    pub sealed_at: DateTime<Utc>,
    pub source_working_dir: PathBuf,
    pub admission_policy_sha256: String,
    pub projection_policy_sha256: String,
    pub tree_sha256: String,
    pub included_files: u64,
    pub included_bytes: u64,
    pub omissions: Vec<CaptureOmission>,
    #[serde(default)]
    pub omissions_truncated: u64,
}

#[derive(Debug, Clone)]
pub struct SealedResultProjection {
    pub manifest: ResultProjectionManifest,
    pub policy: WorkspaceCapturePolicy,
    pub candidate: PathBuf,
}

pub fn result_projection_dir(state: &PipelineState) -> PathBuf {
    state.run_root.join(RESULT_PROJECTION_DIR)
}

pub fn result_projection_policy_path(state: &PipelineState) -> PathBuf {
    result_projection_dir(state).join(RESULT_PROJECTION_POLICY_JSON)
}

pub fn result_projection_manifest_path(state: &PipelineState) -> PathBuf {
    result_projection_dir(state).join(RESULT_PROJECTION_MANIFEST_JSON)
}

pub fn result_projection_candidate_path(state: &PipelineState) -> PathBuf {
    result_projection_dir(state).join(RESULT_PROJECTION_CANDIDATE_DIR)
}

pub fn result_projection_evaluation_path(state: &PipelineState) -> PathBuf {
    result_projection_dir(state).join(RESULT_PROJECTION_EVALUATION_DIR)
}

pub fn result_projection_exists(state: &PipelineState) -> bool {
    result_projection_manifest_path(state).is_file()
        && result_projection_policy_path(state).is_file()
        && result_projection_candidate_path(state).is_dir()
}

pub fn seal_result_projection(state: &PipelineState) -> Result<ResultProjectionManifest> {
    let admission = require_workspace_capture_policy(state)?;
    let policy = freeze_result_projection_policy(&state.working_dir, &admission)?;
    let source_capture = capture_workspace_strict(
        &state.working_dir,
        &policy,
        CaptureProjection::ResultCandidate,
        CapturePurpose::ResultCandidate,
    )?;
    source_capture.require_complete("result projection")?;
    let omissions = source_capture.manifest.omissions.clone();
    let omissions_truncated = source_capture.manifest.omissions_truncated;
    let source_index = artifact_file_index_from_capture(source_capture.clone())?;

    let root = result_projection_dir(state);
    fs::create_dir_all(&root).with_path(&root)?;
    let preparing = root.join(".candidate-preparing");
    remove_controller_tree(&preparing)?;
    materialize_capture_plan(&source_capture, &preparing)?;
    let copied_index = index_with_policy(&preparing, &policy)?;
    require_same_index(
        state,
        &source_index,
        &copied_index,
        "candidate materialization",
    )?;

    let candidate = result_projection_candidate_path(state);
    remove_controller_tree(&candidate)?;
    fs::rename(&preparing, &candidate).with_path(&preparing)?;

    let admission_policy_sha256 = policy_digest(&admission)?;
    let projection_policy_sha256 = policy_digest(&policy)?;
    let included_bytes = source_index
        .files
        .values()
        .map(|fingerprint| fingerprint.size)
        .sum();
    let manifest = ResultProjectionManifest {
        schema_version: RESULT_PROJECTION_SCHEMA_VERSION,
        run_id: state.run_id.clone(),
        sealed_at: Utc::now(),
        source_working_dir: state.working_dir.clone(),
        admission_policy_sha256,
        projection_policy_sha256,
        tree_sha256: source_index.tree_hash(),
        included_files: source_index.files.len() as u64,
        included_bytes,
        omissions,
        omissions_truncated,
    };
    atomic_write_json(&result_projection_policy_path(state), &policy)?;
    atomic_write_json(&result_projection_manifest_path(state), &manifest)?;
    validate_result_projection_at(state, &candidate)?;
    Ok(manifest)
}

pub fn load_result_projection(state: &PipelineState) -> Result<SealedResultProjection> {
    let manifest_path = result_projection_manifest_path(state);
    let policy_path = result_projection_policy_path(state);
    let manifest_raw = fs::read(&manifest_path).with_path(&manifest_path)?;
    let manifest: ResultProjectionManifest =
        serde_json::from_slice(&manifest_raw).with_json_path(&manifest_path)?;
    if manifest.schema_version != RESULT_PROJECTION_SCHEMA_VERSION
        || manifest.run_id != state.run_id
    {
        return Err(projection_error(
            state,
            "result projection manifest identity is unsupported or belongs to another run",
        ));
    }
    let policy_raw = fs::read(&policy_path).with_path(&policy_path)?;
    let policy: WorkspaceCapturePolicy =
        serde_json::from_slice(&policy_raw).with_json_path(&policy_path)?;
    let actual_policy_sha256 = policy_digest(&policy)?;
    if actual_policy_sha256 != manifest.projection_policy_sha256 {
        return Err(projection_error(
            state,
            "result projection policy changed after sealing",
        ));
    }
    let admission = require_workspace_capture_policy(state)?;
    if policy_digest(&admission)? != manifest.admission_policy_sha256 {
        return Err(projection_error(
            state,
            "admission capture policy changed after result sealing",
        ));
    }
    Ok(SealedResultProjection {
        manifest,
        policy,
        candidate: result_projection_candidate_path(state),
    })
}

pub fn validate_result_projection_at(
    state: &PipelineState,
    root: &Path,
) -> Result<ResultProjectionManifest> {
    let projection = load_result_projection(state)?;
    let index = index_with_policy(root, &projection.policy)?;
    if index.tree_hash() != projection.manifest.tree_sha256
        || index.files.len() as u64 != projection.manifest.included_files
        || index
            .files
            .values()
            .map(|fingerprint| fingerprint.size)
            .sum::<u64>()
            != projection.manifest.included_bytes
    {
        return Err(projection_error(
            state,
            &format!(
                "result tree at {} does not match the sealed candidate",
                root.display()
            ),
        ));
    }
    Ok(projection.manifest)
}

pub fn materialize_result_projection(
    state: &PipelineState,
    destination: &Path,
) -> Result<ResultProjectionManifest> {
    let projection = load_result_projection(state)?;
    validate_result_projection_at(state, &projection.candidate)?;
    let capture = capture_workspace_strict(
        &projection.candidate,
        &projection.policy,
        CaptureProjection::ResultCandidate,
        CapturePurpose::ResultCandidate,
    )?;
    capture.require_complete("sealed result materialization")?;
    remove_controller_tree(destination)?;
    materialize_capture_plan(&capture, destination)?;
    validate_result_projection_at(state, destination)
}

pub fn result_projection_sha256(state: &PipelineState) -> Result<String> {
    let path = result_projection_manifest_path(state);
    let raw = fs::read_to_string(&path).with_path(&path)?;
    Ok(sha256_text(&raw))
}

pub fn result_projection_index_at(state: &PipelineState, root: &Path) -> Result<ArtifactFileIndex> {
    let projection = load_result_projection(state)?;
    index_with_policy(root, &projection.policy)
}

fn index_with_policy(root: &Path, policy: &WorkspaceCapturePolicy) -> Result<ArtifactFileIndex> {
    let capture = capture_workspace_strict(
        root,
        policy,
        CaptureProjection::ResultCandidate,
        CapturePurpose::ResultCandidate,
    )?;
    artifact_file_index_from_capture(capture)
}

fn policy_digest(policy: &WorkspaceCapturePolicy) -> Result<String> {
    let raw = serde_json::to_string(policy).map_err(|error| {
        DeadreckonError::InvalidInput(format!(
            "could not serialize result projection policy: {error}"
        ))
    })?;
    Ok(sha256_text(&raw))
}

fn require_same_index(
    state: &PipelineState,
    expected: &ArtifactFileIndex,
    actual: &ArtifactFileIndex,
    operation: &str,
) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(projection_error(
            state,
            &format!("{operation} changed the selected result bytes"),
        ))
    }
}

fn remove_controller_tree(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            remove_captured_directory_tree(path)
        }
        Ok(_) => Err(DeadreckonError::InvalidInput(format!(
            "result projection path is not a real directory: {}",
            path.display()
        ))),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn projection_error(state: &PipelineState, detail: &str) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!(
        "result projection for {} is invalid: {detail}",
        state.run_id
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{
        load_result_projection, result_projection_candidate_path, result_projection_manifest_path,
        result_projection_policy_path, seal_result_projection, validate_result_projection_at,
    };

    fn fixture() -> (TempDir, crate::PipelineState) {
        let temp = TempDir::new().expect("tempdir");
        let paths = crate::DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source root");
        let mut state = crate::create_run(
            &paths,
            crate::RunOptions {
                goal: "seal candidate".to_string(),
                cwd: source,
                skill_name: "test".to_string(),
                sandbox: "none".to_string(),
                provider: None,
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        fs::create_dir_all(&state.working_dir).expect("working");
        fs::write(state.working_dir.join("source.txt"), "source\n").expect("source");
        let init = crate::git::run_git(&state.working_dir, &["init", "-q"]).expect("git init");
        assert!(init.status.success());
        let add = crate::git::run_git(&state.working_dir, &["add", "source.txt"]).expect("git add");
        assert!(add.status.success());
        let policy = crate::freeze_workspace_capture_policy(&state.working_dir).expect("policy");
        crate::write_workspace_capture_policy(&state.run_root, &policy).expect("write policy");
        state.turn = 1;
        (temp, state)
    }

    #[test]
    fn sealed_candidate_source_copy_and_manifest_share_one_tree_hash() {
        let (_temp, state) = fixture();
        let manifest = seal_result_projection(&state).expect("seal");
        let candidate = result_projection_candidate_path(&state);
        validate_result_projection_at(&state, &candidate).expect("validate");
        assert!(manifest.included_files >= 1);
        assert_eq!(
            fs::read_to_string(candidate.join("source.txt")).expect("read"),
            "source\n"
        );
    }

    #[test]
    fn candidate_byte_mode_and_symlink_mutation_refuse() {
        let (_temp, state) = fixture();
        seal_result_projection(&state).expect("seal");
        let candidate = result_projection_candidate_path(&state);
        fs::write(candidate.join("source.txt"), "changed\n").expect("mutate");
        assert!(validate_result_projection_at(&state, &candidate).is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::{PermissionsExt, symlink};

            seal_result_projection(&state).expect("reseal for mode");
            let source = candidate.join("source.txt");
            let mut permissions = fs::metadata(&source).expect("metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&source, permissions).expect("chmod");
            assert!(validate_result_projection_at(&state, &candidate).is_err());

            seal_result_projection(&state).expect("reseal for symlink");
            fs::remove_file(&source).expect("remove source");
            symlink("missing-target", &source).expect("symlink");
            assert!(validate_result_projection_at(&state, &candidate).is_err());
        }
    }

    #[test]
    fn projection_policy_or_manifest_mutation_refuses() {
        let (_temp, state) = fixture();
        seal_result_projection(&state).expect("seal");
        let policy_path = result_projection_policy_path(&state);
        let mut policy: serde_json::Value =
            serde_json::from_slice(&fs::read(&policy_path).expect("policy")).expect("json");
        policy["warnings"] = serde_json::json!(["changed"]);
        fs::write(
            &policy_path,
            serde_json::to_vec_pretty(&policy).expect("encode"),
        )
        .expect("write");
        assert!(load_result_projection(&state).is_err());

        seal_result_projection(&state).expect("reseal");
        let manifest_path = result_projection_manifest_path(&state);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).expect("manifest")).expect("json");
        manifest["tree_sha256"] = serde_json::json!("sha256:changed");
        fs::write(
            &manifest_path,
            serde_json::to_vec_pretty(&manifest).expect("encode"),
        )
        .expect("write");
        assert!(
            validate_result_projection_at(&state, &result_projection_candidate_path(&state))
                .is_err()
        );
    }

    #[test]
    fn partial_projection_never_seals() {
        let (_temp, mut state) = fixture();
        let mut admission = crate::read_workspace_capture_policy(&state.run_root).expect("policy");
        admission.budgets.max_files = 0;
        crate::write_workspace_capture_policy(&state.run_root, &admission).expect("write");
        assert!(seal_result_projection(&state).is_err());
        assert!(!result_projection_manifest_path(&state).exists());
        state.turn = 2;
    }

    #[test]
    fn tracked_path_wins_over_late_ignore_in_sealed_candidate() {
        let (_temp, state) = fixture();
        fs::write(state.working_dir.join(".gitignore"), "/source.txt\n").expect("ignore");
        seal_result_projection(&state).expect("seal");
        assert!(
            result_projection_candidate_path(&state)
                .join("source.txt")
                .exists()
        );
    }
}
