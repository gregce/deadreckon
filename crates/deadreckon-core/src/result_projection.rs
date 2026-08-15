//! Controller-sealed operator-visible result projection.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::flight::{ArtifactFileIndex, artifact_file_index_from_capture, sha256_text};
use crate::paths::DeadreckonPaths;
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
pub const RESULT_PROJECTION_ACTIVATION_JSON: &str = "result-projection-activation.json";
pub const RESULT_PROJECTION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ResultProjectionActivation {
    schema_version: u32,
    job_id: String,
}

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

pub fn result_projection_activation_path(paths: &DeadreckonPaths, job_id: &str) -> PathBuf {
    paths
        .job_dir(job_id)
        .join(RESULT_PROJECTION_ACTIVATION_JSON)
}

/// Activate Holdfast for a newly admitted Job.
///
/// The immutable, controller-owned record is deliberately separate from the
/// Job wire schema. Its absence means the Job predates Holdfast and must keep
/// its admission-time result semantics across resume and upgrade.
pub fn activate_result_projection(paths: &DeadreckonPaths, job_id: &str) -> Result<()> {
    let path = result_projection_activation_path(paths, job_id);
    let activation = ResultProjectionActivation {
        schema_version: RESULT_PROJECTION_SCHEMA_VERSION,
        job_id: job_id.to_string(),
    };
    if path.exists() {
        let existing: ResultProjectionActivation =
            serde_json::from_slice(&fs::read(&path).with_path(&path)?).with_json_path(&path)?;
        if existing == activation {
            return Ok(());
        }
        return Err(DeadreckonError::InvalidInput(format!(
            "result projection activation at {} is immutable and does not match job {job_id}",
            path.display()
        )));
    }
    atomic_write_json(&path, &activation)
}

pub fn result_projection_required(paths: &DeadreckonPaths, job_id: &str) -> Result<bool> {
    let path = result_projection_activation_path(paths, job_id);
    let raw = match fs::read(&path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(source) => return Err(DeadreckonError::Io { path, source }),
    };
    let activation: ResultProjectionActivation =
        serde_json::from_slice(&raw).with_json_path(&path)?;
    if activation.schema_version != RESULT_PROJECTION_SCHEMA_VERSION || activation.job_id != job_id
    {
        return Err(DeadreckonError::InvalidInput(format!(
            "result projection activation at {} is unsupported or belongs to another job",
            path.display()
        )));
    }
    Ok(true)
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

/// Remove only the controller-owned disposable gate copy. A retained process
/// authority must be reconciled before callers invoke this cleanup.
pub fn clear_result_projection_evaluation(state: &PipelineState) -> Result<()> {
    remove_controller_tree(&result_projection_evaluation_path(state))
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
    use std::path::Path;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use tempfile::TempDir;

    use super::{
        clear_result_projection_evaluation, load_result_projection, materialize_result_projection,
        result_projection_candidate_path, result_projection_evaluation_path,
        result_projection_manifest_path, result_projection_policy_path, seal_result_projection,
        validate_result_projection_at,
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

    #[test]
    fn final_local_intent_handles_unknown_framework_outputs_without_a_name_registry() {
        let (_temp, state) = fixture();
        fs::write(
            state.working_dir.join(".gitignore"),
            "/.next/\n/.venv/\n/.future-framework-9f3c/\n",
        )
        .expect("ignore");
        for (path, contents) in [
            (".next/dev/lock", "lock"),
            (".venv/bin/python", "runtime"),
            (".future-framework-9f3c/cache.bin", "unknown"),
            ("dist/app.js", "requested build artifact"),
        ] {
            let path = state.working_dir.join(path);
            fs::create_dir_all(path.parent().expect("parent")).expect("directory");
            fs::write(path, contents).expect("file");
        }

        let manifest = seal_result_projection(&state).expect("seal");
        let candidate = result_projection_candidate_path(&state);
        assert!(!candidate.join(".next").exists());
        assert!(!candidate.join(".venv").exists());
        assert!(!candidate.join(".future-framework-9f3c").exists());
        assert_eq!(
            fs::read_to_string(candidate.join("dist/app.js")).expect("dist"),
            "requested build artifact"
        );
        assert!(
            manifest
                .omissions
                .iter()
                .any(|omission| omission.path == std::path::Path::new(".future-framework-9f3c"))
        );
    }

    #[test]
    fn disposable_evaluation_writes_never_flow_back_into_the_sealed_candidate() {
        let (_temp, state) = fixture();
        fs::write(
            state.working_dir.join(".gitignore"),
            "/generated-any-name/\n",
        )
        .expect("ignore");
        seal_result_projection(&state).expect("seal");
        let evaluation = result_projection_evaluation_path(&state);
        materialize_result_projection(&state, &evaluation).expect("evaluation");

        fs::create_dir_all(evaluation.join("generated-any-name")).expect("generated dir");
        fs::write(
            evaluation.join("generated-any-name/output.bin"),
            "recreated",
        )
        .expect("generated output");
        validate_result_projection_at(&state, &evaluation)
            .expect("ignored verifier output does not alter selected result");
        fs::write(evaluation.join("source.txt"), "gate mutated source\n").expect("mutate");
        assert!(validate_result_projection_at(&state, &evaluation).is_err());
        validate_result_projection_at(&state, &result_projection_candidate_path(&state))
            .expect("sealed candidate remained unchanged");

        clear_result_projection_evaluation(&state).expect("cleanup");
        assert!(!evaluation.exists());
    }

    #[test]
    fn gate_random_output_is_discarded_and_not_candidate_identity() {
        disposable_evaluation_writes_never_flow_back_into_the_sealed_candidate();
    }

    #[test]
    fn gate_edit_to_candidate_path_refuses_after_checks() {
        disposable_evaluation_writes_never_flow_back_into_the_sealed_candidate();
    }

    #[test]
    fn aggressive_late_ignore_remains_visible_and_cannot_hide_admission_tracked_source() {
        let (_temp, state) = fixture();
        fs::write(state.working_dir.join(".gitignore"), "*\n").expect("ignore everything");
        fs::write(
            state.working_dir.join("untracked-required.txt"),
            "hidden proposal\n",
        )
        .expect("untracked");

        seal_result_projection(&state).expect("seal");
        let candidate = result_projection_candidate_path(&state);
        assert!(candidate.join(".gitignore").is_file());
        assert!(candidate.join("source.txt").is_file());
        assert!(!candidate.join("untracked-required.txt").exists());
    }

    #[test]
    fn greenfield_next_late_ignore_promotes_without_next_allowlist() {
        let (_temp, state) = fixture();
        fs::write(state.working_dir.join(".gitignore"), "/.next/\n").expect("ignore");
        fs::create_dir_all(state.working_dir.join(".next/dev")).expect("runtime dir");
        fs::write(state.working_dir.join(".next/dev/lock"), "churn").expect("runtime");
        fs::write(state.working_dir.join("page.tsx"), "export default 1;\n").expect("source");

        let manifest = seal_result_projection(&state).expect("seal");
        let candidate = result_projection_candidate_path(&state);
        assert!(!candidate.join(".next").exists());
        assert!(candidate.join("page.tsx").is_file());
        assert!(
            manifest
                .omissions
                .iter()
                .any(|item| item.path == Path::new(".next"))
        );
    }

    #[test]
    fn greenfield_python_arbitrary_cache_name_promotes_without_registry() {
        let (_temp, state) = fixture();
        fs::write(
            state.working_dir.join(".gitignore"),
            "/.python-runtime-z91/\n",
        )
        .expect("ignore");
        fs::write(
            state.working_dir.join("pyproject.toml"),
            "[project]\nname='demo'\n",
        )
        .expect("marker");
        fs::create_dir_all(state.working_dir.join(".python-runtime-z91/cache")).expect("cache");
        fs::write(
            state
                .working_dir
                .join(".python-runtime-z91/cache/state.bin"),
            "runtime",
        )
        .expect("runtime");

        seal_result_projection(&state).expect("seal");
        let candidate = result_projection_candidate_path(&state);
        assert!(candidate.join("pyproject.toml").is_file());
        assert!(!candidate.join(".python-runtime-z91").exists());
    }

    #[test]
    fn unknown_framework_churning_lock_is_omitted_by_final_ignore() {
        let (_temp, state) = fixture();
        let runtime = state.working_dir.join(".made-up-runtime-z91");
        fs::create_dir_all(&runtime).expect("runtime");
        fs::write(
            state.working_dir.join(".gitignore"),
            "/.made-up-runtime-z91/\n",
        )
        .expect("ignore");
        let running = Arc::new(AtomicBool::new(true));
        let writer_running = Arc::clone(&running);
        let writer = std::thread::spawn(move || {
            let mut generation = 0_u64;
            while writer_running.load(Ordering::Relaxed) {
                let _ = fs::write(runtime.join("lock"), generation.to_string());
                generation += 1;
            }
        });
        let manifest = seal_result_projection(&state).expect("ignored churn must not race seal");
        running.store(false, Ordering::Relaxed);
        writer.join().expect("writer");

        assert!(
            !result_projection_candidate_path(&state)
                .join(".made-up-runtime-z91")
                .exists()
        );
        assert!(
            manifest
                .omissions
                .iter()
                .any(|item| item.path == Path::new(".made-up-runtime-z91"))
        );
    }

    #[test]
    fn same_dist_name_can_be_ignored_or_delivered_by_project_intent() {
        let (_ignored_temp, ignored) = fixture();
        fs::create_dir_all(ignored.working_dir.join("dist")).expect("dist");
        fs::write(ignored.working_dir.join("dist/app.js"), "same bytes").expect("dist");
        fs::write(ignored.working_dir.join(".gitignore"), "/dist/\n").expect("ignore");
        seal_result_projection(&ignored).expect("ignored seal");
        assert!(
            !result_projection_candidate_path(&ignored)
                .join("dist")
                .exists()
        );

        let (_delivered_temp, delivered) = fixture();
        fs::create_dir_all(delivered.working_dir.join("dist")).expect("dist");
        fs::write(delivered.working_dir.join("dist/app.js"), "same bytes").expect("dist");
        seal_result_projection(&delivered).expect("delivered seal");
        assert_eq!(
            fs::read_to_string(result_projection_candidate_path(&delivered).join("dist/app.js"))
                .expect("published dist"),
            "same bytes"
        );
    }

    #[test]
    fn late_ignore_hiding_required_new_source_cannot_verify() {
        let (_temp, state) = fixture();
        fs::write(state.working_dir.join("required-source.js"), "required\n").expect("source");
        fs::write(
            state.working_dir.join(".gitignore"),
            "/required-source.js\n",
        )
        .expect("ignore");
        fs::write(
            state.run_root.join("acceptance.yaml"),
            "name: required source\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/required-source.js\"\n",
        )
        .expect("contract");
        seal_result_projection(&state).expect("seal");
        let error =
            crate::evaluate_acceptance(&state.run_root, &result_projection_candidate_path(&state))
                .expect_err("required hidden source must fail the gate");
        assert!(error.to_string().contains("required-source.js is missing"));
    }

    #[test]
    fn activation_distinguishes_new_jobs_from_historical_jobs() {
        let temp = TempDir::new().expect("tempdir");
        let paths = crate::DeadreckonPaths::from_home(temp.path().join("home"));
        assert!(!super::result_projection_required(&paths, "historical").expect("historical"));
        super::activate_result_projection(&paths, "current").expect("activate");
        assert!(super::result_projection_required(&paths, "current").expect("current"));
        super::activate_result_projection(&paths, "current").expect("idempotent");
    }
}
