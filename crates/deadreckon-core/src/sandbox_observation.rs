//! Authenticated evidence that the strict gate's resolved sandbox boundary was
//! actively exercised by DeadReckon's trusted controller.

use std::fs::{self, File};
use std::io::Read as _;
use std::path::{Path, PathBuf};

use deadreckon_protocol::{
    JobAuthority, JobSchemaVersion, SandboxBoundaryObservation, SandboxBoundaryObservationIssuer,
};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::flight::{build_deliverable_file_index, sha256_file};
use crate::paths::DeadreckonPaths;
use crate::state::{PipelineState, atomic_write_json};
use crate::{acceptance_spec_path_for_run_root, read_gate_key};

pub const SANDBOX_BOUNDARY_OBSERVATION_JSON: &str = "sandbox-boundary-observation.json";
const OBSERVATION_MAGIC: &[u8] = b"deadreckon.sandbox-boundary-observation.v1\0";

/// Authenticate and atomically persist the latest controller-produced
/// observation for one strict Job result.
///
/// A prior regular observation may be replaced by a later attempt. Any
/// non-regular target is refused so a symlink cannot redirect trusted output.
pub fn seal_sandbox_boundary_observation(
    paths: &DeadreckonPaths,
    state: &PipelineState,
    authority: &JobAuthority,
    observation: &SandboxBoundaryObservation,
) -> Result<SandboxBoundaryObservation> {
    validate_observation_fields(paths, state, authority, observation, None)?;
    let mut sealed = observation.clone();
    sealed.schema_version = JobSchemaVersion::CURRENT;
    sealed.signature.clear();
    let key = read_gate_key(paths, authority.job_id.as_ref())?;
    sealed.signature = sign_observation(&sealed, &key)?;

    let path = paths.job_sandbox_boundary_observation(authority.job_id.as_ref());
    refuse_non_regular_existing_target(&path, authority.job_id.as_ref())?;
    atomic_write_json(&path, &sealed)?;

    let persisted =
        validate_sandbox_boundary_observation(paths, state, authority, &sealed.sandbox_backend)?;
    if persisted != sealed {
        return Err(observation_error(
            authority.job_id.as_ref(),
            "persisted observation differs from the controller-produced bytes",
        ));
    }
    Ok(sealed)
}

/// Re-read and authenticate the protected observation, then bind it to current
/// Job authority, current result bytes, and the exact gate backend.
pub fn validate_sandbox_boundary_observation(
    paths: &DeadreckonPaths,
    state: &PipelineState,
    authority: &JobAuthority,
    expected_backend: &str,
) -> Result<SandboxBoundaryObservation> {
    let path = paths.job_sandbox_boundary_observation(authority.job_id.as_ref());
    let raw = read_stable_regular_file(&path, authority.job_id.as_ref())?;
    let observation: SandboxBoundaryObservation =
        serde_json::from_slice(&raw).with_json_path(&path)?;
    validate_observation_fields(
        paths,
        state,
        authority,
        &observation,
        Some(expected_backend),
    )?;
    let key = read_gate_key(paths, authority.job_id.as_ref())?;
    verify_observation_signature(&observation, &key)?;
    Ok(observation)
}

/// Digest the exact stable bytes that a completion receipt must bind.
pub fn sandbox_boundary_observation_sha256(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<String> {
    let path = paths.job_sandbox_boundary_observation(job_id);
    let raw = read_stable_regular_file(&path, job_id)?;
    Ok(sha256_bytes(&raw))
}

/// Compute the same deliverable tree projection used by strict completion.
pub fn sandbox_boundary_result_tree_sha256(state: &PipelineState) -> Result<String> {
    let mut index = build_deliverable_file_index(&state.working_dir)?;
    if state.promoted_library_dir.as_deref() == Some(state.working_dir.as_path()) {
        index.files.remove(Path::new("manifest.json"));
        index.files.remove(Path::new(".materialized-to"));
    }
    Ok(index.tree_hash())
}

fn validate_observation_fields(
    paths: &DeadreckonPaths,
    state: &PipelineState,
    authority: &JobAuthority,
    observation: &SandboxBoundaryObservation,
    expected_backend: Option<&str>,
) -> Result<()> {
    let job_id = authority.job_id.as_ref();
    let authority_path = paths.job_authority(job_id);
    let contract_path = acceptance_spec_path_for_run_root(&state.run_root);
    if observation.schema_version != JobSchemaVersion::CURRENT
        || observation.job_id != authority.job_id
        || observation.run_id != authority.run_id
        || state.run_id != authority.run_id.as_ref()
        || observation.issuer != SandboxBoundaryObservationIssuer::DeadreckonController
        || observation.attempt == 0
        || Uuid::parse_str(&observation.probe_id).is_err()
        || Uuid::parse_str(&observation.outer_launch_id).is_err()
        || observation.sandbox_requested != authority.sandbox_requested
        || !matches!(
            observation.sandbox_backend.as_str(),
            "sandbox-exec" | "bwrap" | "docker"
        )
        || !observation.contained
        || !observation.gate_key_read_denied
        || !observation.proof_write_denied
        || !observation.control_write_denied
        || !observation.operator_capture_read_denied
        || !observation.operator_capture_write_denied
        || !observation.signing_env_scrubbed
    {
        return Err(observation_error(
            job_id,
            "observation identity, issuer, launch, backend, or denial results are invalid",
        ));
    }
    if expected_backend.is_some_and(|expected| observation.sandbox_backend != expected) {
        return Err(observation_error(
            job_id,
            "observed sandbox backend does not match the deterministic gate",
        ));
    }
    require_digest(
        &observation.authority_sha256,
        &sha256_file(&authority_path)?,
        job_id,
        "Job authority",
    )?;
    require_digest(
        &observation.contract_sha256,
        &sha256_file(&contract_path)?,
        job_id,
        "approved contract",
    )?;
    require_digest(
        &observation.result_tree_sha256,
        &sandbox_boundary_result_tree_sha256(state)?,
        job_id,
        "result tree",
    )?;
    for (label, digest) in [
        ("authority", observation.authority_sha256.as_str()),
        ("contract", observation.contract_sha256.as_str()),
        ("result tree", observation.result_tree_sha256.as_str()),
        ("probe program", observation.probe_sha256.as_str()),
    ] {
        require_sha256(digest, job_id, label)?;
    }
    Ok(())
}

fn refuse_non_regular_existing_target(path: &Path, job_id: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            Ok(())
        }
        Ok(_) => Err(observation_error(
            job_id,
            "observation target is not a regular non-symlink file",
        )),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn read_stable_regular_file(path: &Path, job_id: &str) -> Result<Vec<u8>> {
    let before = fs::symlink_metadata(path).with_path(path)?;
    if !before.file_type().is_file() || before.file_type().is_symlink() {
        return Err(observation_error(
            job_id,
            "observation is not a regular non-symlink file",
        ));
    }
    let mut file = File::open(path).with_path(path)?;
    let opened = file.metadata().with_path(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).with_path(path)?;
    let after = file.metadata().with_path(path)?;
    let post_path = fs::symlink_metadata(path).with_path(path)?;
    if !stable_metadata_matches(&before, &opened)
        || !stable_metadata_matches(&opened, &after)
        || !stable_metadata_matches(&after, &post_path)
        || u64::try_from(bytes.len()).ok() != Some(after.len())
    {
        return Err(observation_error(
            job_id,
            "observation changed while its trusted bytes were read",
        ));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn stable_metadata_matches(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.file_type().is_file()
        && right.file_type().is_file()
        && left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

#[cfg(not(unix))]
fn stable_metadata_matches(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.file_type().is_file()
        && right.file_type().is_file()
        && left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
}

fn sign_observation(observation: &SandboxBoundaryObservation, key: &[u8]) -> Result<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
        observation_error(
            observation.job_id.as_ref(),
            "HMAC-SHA-256 refused the observation key",
        )
    })?;
    mac.update(&canonical_observation_bytes(observation)?);
    Ok(hex_encode(&mac.finalize().into_bytes()))
}

fn verify_observation_signature(
    observation: &SandboxBoundaryObservation,
    key: &[u8],
) -> Result<()> {
    let signature = hex_decode(&observation.signature).map_err(|detail| {
        observation_error(
            observation.job_id.as_ref(),
            &format!("observation signature is not valid hex: {detail}"),
        )
    })?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
        observation_error(
            observation.job_id.as_ref(),
            "HMAC-SHA-256 refused the observation key",
        )
    })?;
    mac.update(&canonical_observation_bytes(observation)?);
    mac.verify_slice(&signature).map_err(|_| {
        observation_error(
            observation.job_id.as_ref(),
            "observation signature verification failed",
        )
    })
}

fn canonical_observation_bytes(observation: &SandboxBoundaryObservation) -> Result<Vec<u8>> {
    let mut unsigned = observation.clone();
    unsigned.signature.clear();
    let encoded = serde_json::to_vec(&unsigned).map_err(|source| DeadreckonError::Json {
        path: PathBuf::from(SANDBOX_BOUNDARY_OBSERVATION_JSON),
        source,
    })?;
    let len = u64::try_from(encoded.len()).map_err(|_| {
        observation_error(
            observation.job_id.as_ref(),
            "observation is too large to sign",
        )
    })?;
    let mut bytes = OBSERVATION_MAGIC.to_vec();
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(&encoded);
    Ok(bytes)
}

fn require_digest(expected: &str, actual: &str, job_id: &str, label: &str) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(observation_error(
            job_id,
            &format!("{label} digest changed (expected {expected}, found {actual})"),
        ))
    }
}

fn require_sha256(value: &str, job_id: &str, label: &str) -> Result<()> {
    let digest = value.strip_prefix("sha256:").filter(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if digest.is_some() {
        Ok(())
    } else {
        Err(observation_error(
            job_id,
            &format!("{label} digest is not sha256:<64 lowercase hex characters>"),
        ))
    }
}

fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn hex_decode(value: &str) -> std::result::Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("odd length".to_string());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0])?;
            let low = hex_nibble(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(value: u8) -> std::result::Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err("non-hex character".to_string()),
    }
}

fn observation_error(job_id: &str, detail: &str) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!(
        "sandbox boundary observation for {job_id} is invalid: {detail}"
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::Utc;
    use deadreckon_protocol::{
        AuthorityAcceptedBy, JobAuthority, JobId, JobSchemaVersion, RunId,
        SandboxBoundaryObservation, SandboxBoundaryObservationIssuer, SemanticJudgeMode,
    };
    use tempfile::TempDir;
    use uuid::Uuid;

    use super::{
        sandbox_boundary_result_tree_sha256, seal_sandbox_boundary_observation,
        validate_sandbox_boundary_observation,
    };
    use crate::flight::{sha256_file, sha256_text};
    use crate::state::{RunOptions, atomic_write_json, create_run};
    use crate::{DeadreckonPaths, PipelineState};

    struct Fixture {
        _temp: TempDir,
        paths: DeadreckonPaths,
        state: PipelineState,
        authority: JobAuthority,
    }

    fn fixture() -> Fixture {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "prove the strict sandbox".to_string(),
                cwd: source,
                sandbox: "sandbox-exec".to_string(),
                provider: None,
                skill_name: "test".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(30.0),
                run_id: Some("sandbox-observation-job".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        fs::write(state.working_dir.join("result.txt"), "result\n").expect("result");
        let contract = crate::acceptance_spec_path_for_run_root(&state.run_root);
        fs::write(
            &contract,
            "name: result\nchecks:\n  - file_exists: result.txt\n",
        )
        .expect("contract");
        fs::create_dir_all(paths.job_dir(&state.run_id)).expect("job dir");
        let authority = JobAuthority {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId(state.run_id.clone()),
            run_id: RunId(state.run_id.clone()),
            approved_at: Utc::now(),
            accepted_by: AuthorityAcceptedBy::Operator,
            goal_sha256: sha256_text(&state.goal),
            contract_sha256: sha256_file(&contract).expect("contract digest"),
            effective_policy_sha256: sha256_text("policy"),
            launch_plan_sha256: sha256_text("launch"),
            source_tree_sha256: sha256_text("source"),
            source_revision: None,
            sandbox_requested: "sandbox-exec".to_string(),
            semantic_judge_mode: SemanticJudgeMode::Required,
        };
        atomic_write_json(&paths.job_authority(&state.run_id), &authority).expect("authority");
        Fixture {
            _temp: temp,
            paths,
            state,
            authority,
        }
    }

    fn observation(fixture: &Fixture) -> SandboxBoundaryObservation {
        SandboxBoundaryObservation {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: fixture.authority.job_id.clone(),
            run_id: fixture.authority.run_id.clone(),
            observed_at: Utc::now(),
            issuer: SandboxBoundaryObservationIssuer::DeadreckonController,
            probe_id: Uuid::new_v4().to_string(),
            attempt: 1,
            outer_launch_id: Uuid::new_v4().to_string(),
            authority_sha256: sha256_file(
                &fixture
                    .paths
                    .job_authority(fixture.authority.job_id.as_ref()),
            )
            .expect("authority digest"),
            contract_sha256: fixture.authority.contract_sha256.clone(),
            result_tree_sha256: sandbox_boundary_result_tree_sha256(&fixture.state)
                .expect("result tree"),
            sandbox_requested: fixture.authority.sandbox_requested.clone(),
            sandbox_backend: "sandbox-exec".to_string(),
            contained: true,
            gate_key_read_denied: true,
            proof_write_denied: true,
            control_write_denied: true,
            operator_capture_read_denied: true,
            operator_capture_write_denied: true,
            signing_env_scrubbed: true,
            probe_sha256: sha256_text("fixed controller probe"),
            signature: String::new(),
        }
    }

    #[test]
    fn signed_observation_round_trips_and_binds_current_result() {
        let fixture = fixture();
        let sealed = seal_sandbox_boundary_observation(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &observation(&fixture),
        )
        .expect("seal observation");
        assert_eq!(
            validate_sandbox_boundary_observation(
                &fixture.paths,
                &fixture.state,
                &fixture.authority,
                "sandbox-exec",
            )
            .expect("validate observation"),
            sealed
        );

        fs::write(fixture.state.working_dir.join("result.txt"), "mutated\n").expect("mutate");
        let error = validate_sandbox_boundary_observation(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            "sandbox-exec",
        )
        .expect_err("result mutation must invalidate observation");
        assert!(error.to_string().contains("result tree digest changed"));
    }

    #[test]
    fn missing_forged_mutated_foreign_and_backend_mismatch_fail_closed() {
        let fixture = fixture();
        let path = fixture
            .paths
            .job_sandbox_boundary_observation(fixture.authority.job_id.as_ref());
        let missing = validate_sandbox_boundary_observation(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            "sandbox-exec",
        )
        .expect_err("missing observation");
        assert!(
            missing
                .to_string()
                .contains(path.to_string_lossy().as_ref())
        );

        let mut forged = observation(&fixture);
        forged.signature = "00".repeat(32);
        atomic_write_json(&path, &forged).expect("forged observation");
        let error = validate_sandbox_boundary_observation(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            "sandbox-exec",
        )
        .expect_err("forged signature");
        assert!(
            error
                .to_string()
                .contains("observation signature verification failed")
        );

        let sealed = seal_sandbox_boundary_observation(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &observation(&fixture),
        )
        .expect("seal observation");
        let mut mutated = sealed.clone();
        mutated.observed_at += chrono::TimeDelta::seconds(1);
        atomic_write_json(&path, &mutated).expect("mutated observation");
        let error = validate_sandbox_boundary_observation(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            "sandbox-exec",
        )
        .expect_err("mutated signed fields");
        assert!(
            error
                .to_string()
                .contains("observation signature verification failed")
        );

        let mut foreign = sealed.clone();
        foreign.job_id = JobId("other-job".to_string());
        atomic_write_json(&path, &foreign).expect("foreign observation");
        let error = validate_sandbox_boundary_observation(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            "sandbox-exec",
        )
        .expect_err("foreign observation");
        assert!(error.to_string().contains("identity"));

        atomic_write_json(&path, &sealed).expect("restore observation");
        let error = validate_sandbox_boundary_observation(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            "docker",
        )
        .expect_err("backend mismatch");
        assert!(error.to_string().contains("does not match"));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_observation_is_refused_for_read_and_replacement() {
        use std::os::unix::fs::symlink;

        let fixture = fixture();
        let path = fixture
            .paths
            .job_sandbox_boundary_observation(fixture.authority.job_id.as_ref());
        let target = fixture.paths.job_dir("target").join("observation.json");
        fs::create_dir_all(target.parent().expect("target parent")).expect("target dir");
        atomic_write_json(&target, &observation(&fixture)).expect("target");
        symlink(&target, &path).expect("symlink");

        let read_error = validate_sandbox_boundary_observation(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            "sandbox-exec",
        )
        .expect_err("symlink read");
        assert!(read_error.to_string().contains("non-symlink"));

        let write_error = seal_sandbox_boundary_observation(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &observation(&fixture),
        )
        .expect_err("symlink replacement");
        assert!(write_error.to_string().contains("non-symlink"));
    }
}
