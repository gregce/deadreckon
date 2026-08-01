use std::collections::hash_map::DefaultHasher;
use std::fs::OpenOptions;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Instant;

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_yaml::Value as YamlValue;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::paths::DeadreckonPaths;
use crate::state::{PipelineState, append_json_line};
use crate::tamper::AcceptanceTamperVerdict;

pub const ACCEPTANCE_MARKER: &str = "turn-acceptance.json";
pub const ACCEPTANCE_PROGRESS_JSONL: &str = "acceptance-progress.jsonl";
pub const ACCEPTANCE_SPEC: &str = "acceptance.yaml";
pub const PARENT_REPAIR_MANIFEST_JSON: &str = "proofs/parent-repair.json";
pub const PARENT_REPAIR_CANDIDATE_JSON: &str = "proofs/parent-repair-candidate.json";
pub const GATE_KEY_ENV: &str = "DEADRECKON_GATE_KEY";
pub const GATE_CONTAINED_ENV: &str = "DEADRECKON_GATE_CONTAINED";
pub const GATE_SANDBOX_BACKEND_ENV: &str = "DEADRECKON_GATE_SANDBOX_BACKEND";
pub const GATE_EVALUATION_SCHEMA_VERSION: u32 = 1;
const GATE_NONCE: &str = "gate/nonce";
const GATE_KEY_BYTES: usize = 32;
const V2_CANONICAL_MAGIC: &[u8] = b"deadreckon.acceptance-marker.v2\0";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceProofKind {
    NativeGate,
    SyntheticController,
    DerivedRollup,
    #[default]
    LegacyUnknown,
}

impl AcceptanceProofKind {
    fn canonical_name(self) -> &'static str {
        match self {
            Self::NativeGate => "native_gate",
            Self::SyntheticController => "synthetic_controller",
            Self::DerivedRollup => "derived_rollup",
            Self::LegacyUnknown => "legacy_unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceSignatureStrength {
    HmacSha256,
    LegacyWeak,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptanceContainment {
    pub contained: bool,
    pub sandbox_backend: String,
}

impl AcceptanceContainment {
    pub fn contained(backend: impl Into<String>) -> Self {
        Self {
            contained: true,
            sandbox_backend: backend.into(),
        }
    }

    pub fn uncontained(backend: impl Into<String>) -> Self {
        Self {
            contained: false,
            sandbox_backend: backend.into(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct MarkerSigningIdentity {
    issuer: &'static str,
    proof_kind: AcceptanceProofKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AcceptanceMarker {
    pub schema_version: u32,
    pub run_id: String,
    pub status: String,
    pub produced_by: String,
    #[serde(default)]
    pub issuer: String,
    #[serde(default)]
    pub proof_kind: AcceptanceProofKind,
    pub checked_at: DateTime<Utc>,
    pub working_dir: PathBuf,
    #[serde(default)]
    pub contained: bool,
    #[serde(default)]
    pub sandbox_backend: String,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub check_count: usize,
    #[serde(default)]
    pub checks: Vec<AcceptanceCheckResult>,
}

impl AcceptanceMarker {
    pub fn signature_strength(&self) -> AcceptanceSignatureStrength {
        if self.schema_version >= 2 {
            AcceptanceSignatureStrength::HmacSha256
        } else {
            AcceptanceSignatureStrength::LegacyWeak
        }
    }

    pub fn is_native_gate_proof(&self) -> bool {
        self.schema_version >= 2
            && self.proof_kind == AcceptanceProofKind::NativeGate
            && self.issuer == "dr-gate"
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceSpec {
    pub name: Option<String>,
    #[serde(default)]
    pub checks: Vec<AcceptanceCheck>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AcceptanceCheck {
    CargoTest {
        #[serde(default)]
        args: Vec<String>,
        #[serde(default = "default_must_pass")]
        must_pass: bool,
    },
    FileExists {
        path: String,
        #[serde(default = "default_must_pass")]
        must_pass: bool,
    },
    ContentMatch {
        path: String,
        pattern: String,
        #[serde(default = "default_must_pass")]
        must_pass: bool,
    },
    BuildSuccess {
        cwd: String,
        #[serde(default = "default_must_pass")]
        must_pass: bool,
    },
    Shell {
        command: String,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default = "default_must_pass")]
        must_pass: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct AcceptanceCheckResult {
    pub kind: String,
    pub passed: bool,
    pub must_pass: bool,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceProgressEntry {
    pub checked_at: DateTime<Utc>,
    pub status: String,
    pub index: usize,
    pub total: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<AcceptanceCheckResult>,
}

/// Keyless output from the deterministic evaluator.
///
/// This document crosses the sandbox boundary as captured stdout. It carries
/// no authority by itself: the trusted signing phase re-reads the approved
/// contract, validates every result against it, and recomputes tamper evidence
/// before it can write a marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GateEvaluation {
    pub schema_version: u32,
    pub run_id: String,
    pub working_dir: PathBuf,
    pub contract_sha256: String,
    /// Digest of the immutable gate toolchain approved for this Job.
    ///
    /// This value is echoed by the keyless evaluator and compared with trusted
    /// Job authority before the signing key is read. Older compatibility
    /// evaluations have no such identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_evaluator_sha256: Option<String>,
    pub results: Vec<AcceptanceCheckResult>,
    pub tamper: crate::tamper::AcceptanceTamper,
}

pub fn marker_path(state: &PipelineState) -> PathBuf {
    state.run_root.join("proofs").join(ACCEPTANCE_MARKER)
}

pub fn marker_path_for_run_root(run_root: &Path) -> PathBuf {
    run_root.join("proofs").join(ACCEPTANCE_MARKER)
}

pub fn acceptance_progress_path_for_run_root(run_root: &Path) -> PathBuf {
    run_root.join("proofs").join(ACCEPTANCE_PROGRESS_JSONL)
}

pub fn acceptance_spec_path_for_run_root(run_root: &Path) -> PathBuf {
    run_root.join(ACCEPTANCE_SPEC)
}

pub fn parent_repair_manifest_path_for_run_root(run_root: &Path) -> PathBuf {
    run_root.join(PARENT_REPAIR_MANIFEST_JSON)
}

pub fn parent_repair_candidate_path_for_run_root(run_root: &Path) -> PathBuf {
    run_root.join(PARENT_REPAIR_CANDIDATE_JSON)
}

pub fn gate_nonce_path_for_run_root(run_root: &Path) -> PathBuf {
    run_root.join(GATE_NONCE)
}

pub fn gate_key_path(paths: &DeadreckonPaths, run_id: &str) -> PathBuf {
    paths
        .home()
        .join("gate-keys")
        .join(format!("{}.key", gate_key_file_stem(run_id)))
}

pub fn gate_key_path_for_run_root(run_root: &Path, run_id: &str) -> Result<PathBuf> {
    let home = run_root
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or_else(|| {
            DeadreckonError::InvalidInput(format!(
                "cannot infer DeadReckon home from run root {}",
                run_root.display()
            ))
        })?;
    Ok(gate_key_path(&DeadreckonPaths::from_home(home), run_id))
}

fn ensure_private_gate_key_store(paths: &DeadreckonPaths) -> Result<PathBuf> {
    let store = paths.home().join("gate-keys");
    std::fs::create_dir_all(&store).with_path(&store)?;
    let metadata = std::fs::symlink_metadata(&store).with_path(&store)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(DeadreckonError::InvalidInput(format!(
            "protected gate key store {} is not a real directory",
            store.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // This is a local, per-user keyring. Tightening an older store is a
        // safe in-place migration and keeps existing per-run keys usable.
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o700))
            .with_path(&store)?;
    }
    Ok(store)
}

pub fn create_gate_key(paths: &DeadreckonPaths, run_id: &str) -> Result<Vec<u8>> {
    ensure_private_gate_key_store(paths)?;
    let path = gate_key_path(paths, run_id);
    match std::fs::symlink_metadata(&path) {
        Ok(_) => return read_gate_key_at_path(&path, run_id),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(DeadreckonError::Io { path, source });
        }
    }

    // UUID v4 uses the platform CSPRNG but reserves version/variant bits. Hash
    // three independent UUIDs so the stored 32-byte key has at least 256 bits
    // of random input without adding a second OS-random dependency surface.
    let mut derivation = Sha256::new();
    for _ in 0..3 {
        derivation.update(Uuid::new_v4().as_bytes());
    }
    let key = derivation.finalize().to_vec();
    match write_gate_key(paths, run_id, &key) {
        Ok(()) => Ok(key),
        // A concurrent creator may have won the create_new race. Reuse only
        // the protected, valid key it wrote; never replace key material.
        Err(DeadreckonError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::AlreadyExists =>
        {
            read_gate_key_at_path(&path, run_id)
        }
        Err(error) => Err(error),
    }
}

pub fn write_gate_key(paths: &DeadreckonPaths, run_id: &str, key: &[u8]) -> Result<()> {
    if key.len() != GATE_KEY_BYTES {
        return Err(DeadreckonError::InvalidInput(format!(
            "gate key must be {GATE_KEY_BYTES} bytes, got {}",
            key.len()
        )));
    }
    let path = gate_key_path(paths, run_id);
    ensure_private_gate_key_store(paths)?;

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).with_path(&path)?;
    file.write_all(hex_encode(key).as_bytes())
        .with_path(&path)?;
    file.sync_all().with_path(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).with_path(&path)?;
    }
    Ok(())
}

pub fn read_gate_key(paths: &DeadreckonPaths, run_id: &str) -> Result<Vec<u8>> {
    read_gate_key_at_path(&gate_key_path(paths, run_id), run_id)
}

pub fn read_gate_key_for_run_root(run_root: &Path, run_id: &str) -> Result<Vec<u8>> {
    let path = gate_key_path_for_run_root(run_root, run_id)?;
    read_gate_key_at_path(&path, run_id)
}

pub fn decode_gate_key(value: &str) -> Result<Vec<u8>> {
    let decoded = hex_decode(value.trim()).map_err(DeadreckonError::InvalidInput)?;
    if decoded.len() != GATE_KEY_BYTES {
        return Err(DeadreckonError::InvalidInput(format!(
            "gate key must decode to {GATE_KEY_BYTES} bytes, got {}",
            decoded.len()
        )));
    }
    Ok(decoded)
}

pub fn encode_gate_key(key: &[u8]) -> Result<String> {
    require_gate_key_length(key)?;
    Ok(hex_encode(key))
}

fn read_gate_key_at_path(path: &Path, run_id: &str) -> Result<Vec<u8>> {
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!("gate key path {} has no parent", path.display()))
    })?;
    let parent_metadata = match std::fs::symlink_metadata(parent) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(DeadreckonError::InvalidInput(format!(
                "protected gate key is missing for run {run_id}; acceptance cannot be verified\ntry: deadreckon verdict {run_id}"
            )));
        }
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: parent.to_path_buf(),
                source,
            });
        }
    };
    if !parent_metadata.file_type().is_dir() || parent_metadata.file_type().is_symlink() {
        return Err(DeadreckonError::InvalidInput(format!(
            "protected gate key store {} is not a real directory; acceptance cannot be verified",
            parent.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if parent_metadata.permissions().mode() & 0o077 != 0 {
            return Err(DeadreckonError::InvalidInput(format!(
                "protected gate key store {} is accessible to other users; acceptance cannot be verified",
                parent.display()
            )));
        }
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(DeadreckonError::InvalidInput(format!(
                "protected gate key is missing for run {run_id}; acceptance cannot be verified\ntry: deadreckon verdict {run_id}"
            )));
        }
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(DeadreckonError::InvalidInput(format!(
            "protected gate key for run {run_id} is not a regular file; acceptance cannot be verified\ntry: deadreckon verdict {run_id}"
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(DeadreckonError::InvalidInput(format!(
                "protected gate key for run {run_id} is accessible to other users; acceptance cannot be verified\ntry: deadreckon verdict {run_id}"
            )));
        }
    }

    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Err(DeadreckonError::InvalidInput(format!(
                "protected gate key is missing for run {run_id}; acceptance cannot be verified\ntry: deadreckon verdict {run_id}"
            )));
        }
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    decode_gate_key(&raw).map_err(|err| {
        DeadreckonError::InvalidInput(format!(
            "protected gate key for run {run_id} is unreadable: {err}\ntry: deadreckon verdict {run_id}"
        ))
    })
}

pub fn validate_acceptance_marker(state: &PipelineState) -> Result<AcceptanceMarker> {
    validate_acceptance_marker_inner(state, None)
}

pub(crate) fn validate_acceptance_marker_with_parent_repair_bytes(
    state: &PipelineState,
    parent_repair: Option<&[u8]>,
    parent_repair_candidate: Option<&[u8]>,
) -> Result<AcceptanceMarker> {
    validate_acceptance_marker_inner(
        state,
        Some(ParentRepairBoundBytes {
            manifest: parent_repair,
            candidate: parent_repair_candidate,
        }),
    )
}

fn validate_acceptance_marker_inner(
    state: &PipelineState,
    parent_repair: Option<ParentRepairBoundBytes<'_>>,
) -> Result<AcceptanceMarker> {
    // AS-BUILT §8/§17: completion is accepted only from an external marker
    // written by a binary runner and bound to this run_id.
    let path = marker_path(state);
    let raw = std::fs::read(&path).with_path(&path)?;
    let marker: AcceptanceMarker = serde_json::from_slice(&raw).with_json_path(&path)?;
    if marker.run_id != state.run_id {
        return Err(DeadreckonError::InvalidInput(format!(
            "acceptance marker run_id {} does not match {}",
            marker.run_id, state.run_id
        )));
    }
    if marker.status != "pass" {
        return Err(DeadreckonError::InvalidInput(
            "acceptance marker does not record pass status".to_string(),
        ));
    }
    match marker.schema_version {
        1 => validate_legacy_marker_signature(&state.run_root, &marker)?,
        2 => {
            let key = read_gate_key_for_run_root(&state.run_root, &state.run_id)?;
            verify_v2_marker_signature_with_parent_repair(
                &state.run_root,
                &marker,
                &key,
                parent_repair,
            )?;
        }
        version => {
            return Err(DeadreckonError::InvalidInput(format!(
                "unsupported acceptance marker schema {version}"
            )));
        }
    }
    Ok(marker)
}

pub fn write_acceptance_marker(
    run_root: &Path,
    run_id: String,
    working_dir: PathBuf,
    check_count: usize,
) -> Result<AcceptanceMarker> {
    let checks = (0..check_count)
        .map(|idx| AcceptanceCheckResult {
            kind: "legacy".to_string(),
            passed: true,
            must_pass: true,
            detail: format!("legacy check {}", idx + 1),
            command: None,
            cwd: None,
            duration_ms: None,
            stdout: None,
            stderr: None,
        })
        .collect::<Vec<_>>();
    write_acceptance_marker_with_results(run_root, run_id, working_dir, checks)
}

pub fn write_acceptance_marker_with_results(
    run_root: &Path,
    run_id: String,
    working_dir: PathBuf,
    checks: Vec<AcceptanceCheckResult>,
) -> Result<AcceptanceMarker> {
    let key = read_gate_key_for_run_root(run_root, &run_id)?;
    write_acceptance_marker_with_context_and_key(
        run_root,
        run_id,
        working_dir,
        checks,
        &key,
        MarkerSigningIdentity {
            issuer: "deadreckon-controller",
            proof_kind: AcceptanceProofKind::SyntheticController,
        },
        AcceptanceContainment::uncontained("synthetic"),
    )
}

pub fn write_native_acceptance_marker_with_results_and_key(
    run_root: &Path,
    run_id: String,
    working_dir: PathBuf,
    checks: Vec<AcceptanceCheckResult>,
    gate_key: &[u8],
    containment: AcceptanceContainment,
) -> Result<AcceptanceMarker> {
    write_acceptance_marker_with_context_and_key(
        run_root,
        run_id,
        working_dir,
        checks,
        gate_key,
        MarkerSigningIdentity {
            issuer: "dr-gate",
            proof_kind: AcceptanceProofKind::NativeGate,
        },
        containment,
    )
}

fn write_acceptance_marker_with_context_and_key(
    run_root: &Path,
    run_id: String,
    working_dir: PathBuf,
    checks: Vec<AcceptanceCheckResult>,
    gate_key: &[u8],
    identity: MarkerSigningIdentity,
    containment: AcceptanceContainment,
) -> Result<AcceptanceMarker> {
    if gate_key.len() != GATE_KEY_BYTES {
        return Err(DeadreckonError::InvalidInput(format!(
            "gate key must be {GATE_KEY_BYTES} bytes, got {}",
            gate_key.len()
        )));
    }
    let proofs = run_root.join("proofs");
    std::fs::create_dir_all(&proofs).with_path(&proofs)?;
    let mut marker = AcceptanceMarker {
        schema_version: 2,
        run_id,
        status: "pass".to_string(),
        produced_by: identity.issuer.to_string(),
        issuer: identity.issuer.to_string(),
        proof_kind: identity.proof_kind,
        checked_at: Utc::now(),
        working_dir,
        contained: containment.contained,
        sandbox_backend: containment.sandbox_backend,
        signature: String::new(),
        check_count: checks.len(),
        checks,
    };
    marker.signature = v2_marker_signature(run_root, &marker, gate_key)?;
    std::fs::write(
        proofs.join(ACCEPTANCE_MARKER),
        serde_json::to_vec_pretty(&marker).map_err(|source| DeadreckonError::Json {
            path: proofs.join(ACCEPTANCE_MARKER),
            source,
        })?,
    )
    .with_path(proofs.join(ACCEPTANCE_MARKER))?;
    Ok(marker)
}

pub fn run_acceptance_gate_and_write_marker(
    run_root: &Path,
    run_id: &str,
    working_dir: &Path,
) -> Result<AcceptanceMarker> {
    // Compatibility callers still use the in-process gate. Materializing a
    // generated default is a trusted-controller action; the keyless evaluator
    // itself only ever reads an already-approved contract.
    compiled_acceptance_checks(run_root, working_dir)?;
    let evaluation = evaluate_gate(run_id, run_root, working_dir)?;
    let key = read_gate_key_for_run_root(run_root, run_id)?;
    sign_gate_evaluation_with_key(
        run_root,
        run_id,
        working_dir,
        evaluation,
        &key,
        AcceptanceContainment::uncontained("none"),
    )
}

/// Evaluate an already-approved acceptance contract without reading signing
/// material or writing outside the working directory.
///
/// The caller is responsible for running this function inside the resolved
/// sandbox and transporting the returned value directly to a trusted signer.
pub fn evaluate_gate(run_id: &str, run_root: &Path, working_dir: &Path) -> Result<GateEvaluation> {
    evaluate_gate_with_identity(run_id, run_root, working_dir, None)
}

/// Evaluate an approved contract while echoing the trusted evaluator identity
/// supplied by the controller.
///
/// The echo is not authority by itself. The controller and signer compare it
/// with immutable Job policy before signing any result.
pub fn evaluate_gate_with_identity(
    run_id: &str,
    run_root: &Path,
    working_dir: &Path,
    gate_evaluator_sha256: Option<String>,
) -> Result<GateEvaluation> {
    let canonical_working_dir = canonical_working_dir(working_dir)?;
    let (checks, contract_sha256) = approved_acceptance_contract(run_root)?;
    let mut results = Vec::with_capacity(checks.len());
    for check in checks.iter().cloned() {
        results.push(evaluate_check(working_dir, check)?);
    }
    let tamper = crate::tamper::evaluate(run_id, run_root, working_dir, &checks)?;
    Ok(GateEvaluation {
        schema_version: GATE_EVALUATION_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        working_dir: canonical_working_dir,
        contract_sha256,
        gate_evaluator_sha256,
        results,
        tamper,
    })
}

/// Validate a keyless evaluation against current trusted inputs.
///
/// This is deliberately side-effect free. Signing performs this validation
/// first, then persists reconstructed evidence and the marker.
pub fn validate_gate_evaluation(
    run_id: &str,
    run_root: &Path,
    working_dir: &Path,
    evaluation: &GateEvaluation,
) -> Result<()> {
    let checks = validated_gate_evaluation_checks(run_id, run_root, working_dir, evaluation)?;
    ensure_gate_evaluation_accepted(evaluation, &checks)
}

/// Validate that a keyless evaluation is bound to the approved contract and
/// current filesystem evidence without requiring its checks to have passed.
///
/// Independent inspection commands need trustworthy failure results too. This
/// performs the same structural, contract, result, and tamper validation as
/// signing, but deliberately does not apply the acceptance decision.
pub fn validate_gate_evaluation_integrity(
    run_id: &str,
    run_root: &Path,
    working_dir: &Path,
    evaluation: &GateEvaluation,
) -> Result<()> {
    validated_gate_evaluation_checks(run_id, run_root, working_dir, evaluation).map(drop)
}

/// Validate, persist trusted evidence, and sign a keyless gate evaluation.
///
/// No acceptance checks are executed in this phase. The signing key therefore
/// never shares a process with repository-controlled check subprocesses.
pub fn sign_gate_evaluation_with_key(
    run_root: &Path,
    run_id: &str,
    working_dir: &Path,
    evaluation: GateEvaluation,
    gate_key: &[u8],
    containment: AcceptanceContainment,
) -> Result<AcceptanceMarker> {
    require_gate_key_length(gate_key)?;
    validate_signing_containment(&containment)?;
    let checks = validated_gate_evaluation_checks(run_id, run_root, working_dir, &evaluation)?;
    crate::tamper::write_acceptance_tamper(run_root, &evaluation.tamper)?;
    write_reconstructed_acceptance_progress(run_root, &evaluation.results)?;
    ensure_gate_evaluation_accepted(&evaluation, &checks)?;
    write_native_acceptance_marker_with_results_and_key(
        run_root,
        run_id.to_string(),
        evaluation.working_dir,
        evaluation.results,
        gate_key,
        containment,
    )
}

fn validate_signing_containment(containment: &AcceptanceContainment) -> Result<()> {
    let backend = containment.sandbox_backend.as_str();
    let coherent = if containment.contained {
        matches!(backend, "sandbox-exec" | "bwrap" | "docker")
    } else {
        backend == "none"
    };
    if coherent {
        Ok(())
    } else {
        Err(DeadreckonError::InvalidInput(format!(
            "acceptance containment is incoherent: contained={} backend={backend}",
            containment.contained
        )))
    }
}

pub fn evaluate_acceptance(
    run_root: &Path,
    working_dir: &Path,
) -> Result<Vec<AcceptanceCheckResult>> {
    let results = evaluate_acceptance_checks(run_root, working_dir)?;
    if let Some(failed) = results
        .iter()
        .find(|result| result.must_pass && !result.passed)
    {
        return Err(DeadreckonError::InvalidInput(format!(
            "acceptance check failed: {}",
            failed.detail
        )));
    }
    Ok(results)
}

pub fn evaluate_acceptance_checks(
    run_root: &Path,
    working_dir: &Path,
) -> Result<Vec<AcceptanceCheckResult>> {
    evaluate_acceptance_checks_inner(run_root, working_dir, None)
}

pub fn evaluate_acceptance_checks_with_progress(
    run_root: &Path,
    working_dir: &Path,
) -> Result<Vec<AcceptanceCheckResult>> {
    let progress_path = acceptance_progress_path_for_run_root(run_root);
    match std::fs::remove_file(&progress_path) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: progress_path,
                source,
            });
        }
    }
    evaluate_acceptance_checks_inner(run_root, working_dir, Some(&progress_path))
}

pub fn compiled_acceptance_checks(
    run_root: &Path,
    working_dir: &Path,
) -> Result<Vec<AcceptanceCheck>> {
    let spec_path = acceptance_spec_path_for_run_root(run_root);
    if spec_path.exists() {
        // An operator spec (or a previously-generated one) always wins verbatim.
        let raw = std::fs::read_to_string(&spec_path).with_path(&spec_path)?;
        return parse_acceptance_checks(&raw);
    }
    // No operator spec: detect the project kind, compile a real default, and
    // persist it as the auditable generated spec before returning.
    let kind = crate::acceptance_defaults::detect_project_kind(working_dir);
    let checks = crate::acceptance_defaults::default_checks_for(&kind, working_dir);
    write_generated_spec(&spec_path, &kind, &checks)?;
    Ok(checks)
}

fn approved_acceptance_contract(run_root: &Path) -> Result<(Vec<AcceptanceCheck>, String)> {
    let spec_path = acceptance_spec_path_for_run_root(run_root);
    let metadata = std::fs::symlink_metadata(&spec_path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            DeadreckonError::InvalidInput(format!(
                "approved acceptance contract is missing at {}; the trusted controller must materialize it before evaluation",
                spec_path.display()
            ))
        } else {
            DeadreckonError::Io {
                path: spec_path.clone(),
                source,
            }
        }
    })?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(DeadreckonError::InvalidInput(format!(
            "approved acceptance contract at {} must be a regular, non-symlink file",
            spec_path.display()
        )));
    }
    let raw = std::fs::read_to_string(&spec_path).with_path(&spec_path)?;
    let contract_sha256 = crate::flight::sha256_text(&raw);
    Ok((parse_acceptance_checks(&raw)?, contract_sha256))
}

fn canonical_working_dir(working_dir: &Path) -> Result<PathBuf> {
    working_dir
        .canonicalize()
        .map_err(|source| DeadreckonError::Io {
            path: working_dir.to_path_buf(),
            source,
        })
}

fn validated_gate_evaluation_checks(
    run_id: &str,
    run_root: &Path,
    working_dir: &Path,
    evaluation: &GateEvaluation,
) -> Result<Vec<AcceptanceCheck>> {
    if evaluation.schema_version != GATE_EVALUATION_SCHEMA_VERSION {
        return Err(invalid_gate_evaluation(format!(
            "unsupported schema {}; expected {}",
            evaluation.schema_version, GATE_EVALUATION_SCHEMA_VERSION
        )));
    }
    if evaluation.run_id != run_id {
        return Err(invalid_gate_evaluation(format!(
            "run id {} does not match {run_id}",
            evaluation.run_id
        )));
    }
    let canonical_working_dir = canonical_working_dir(working_dir)?;
    if evaluation.working_dir != canonical_working_dir {
        return Err(invalid_gate_evaluation(format!(
            "working directory {} does not match {}",
            evaluation.working_dir.display(),
            canonical_working_dir.display()
        )));
    }
    let (checks, contract_sha256) = approved_acceptance_contract(run_root)?;
    if evaluation.contract_sha256 != contract_sha256 {
        return Err(invalid_gate_evaluation(format!(
            "contract digest {} does not match {contract_sha256}",
            evaluation.contract_sha256
        )));
    }
    if evaluation.results.len() != checks.len() {
        return Err(invalid_gate_evaluation(format!(
            "result count {} does not match approved check count {}",
            evaluation.results.len(),
            checks.len()
        )));
    }
    for (index, (check, result)) in checks.iter().zip(&evaluation.results).enumerate() {
        validate_result_binding(working_dir, check, result).map_err(|err| {
            invalid_gate_evaluation(format!("result {} is not approved: {err}", index + 1))
        })?;
    }
    if evaluation.tamper.schema_version != 1 {
        return Err(invalid_gate_evaluation(format!(
            "unsupported tamper schema {}",
            evaluation.tamper.schema_version
        )));
    }
    if evaluation.tamper.run_id != run_id {
        return Err(invalid_gate_evaluation(format!(
            "tamper run id {} does not match {run_id}",
            evaluation.tamper.run_id
        )));
    }
    let recomputed = crate::tamper::evaluate(run_id, run_root, working_dir, &checks)?;
    if !same_tamper_facts(&evaluation.tamper, &recomputed) {
        return Err(invalid_gate_evaluation(
            "tamper evidence does not match trusted recomputation",
        ));
    }
    Ok(checks)
}

fn ensure_gate_evaluation_accepted(
    evaluation: &GateEvaluation,
    checks: &[AcceptanceCheck],
) -> Result<()> {
    debug_assert_eq!(evaluation.results.len(), checks.len());
    if evaluation.tamper.verdict == AcceptanceTamperVerdict::Refuse {
        return Err(DeadreckonError::InvalidInput(format!(
            "acceptance refused: {}",
            evaluation.tamper.refusal_reasons.join("; ")
        )));
    }
    if let Some(failed) = evaluation
        .results
        .iter()
        .find(|result| result.must_pass && !result.passed)
    {
        return Err(DeadreckonError::InvalidInput(format!(
            "acceptance check failed: {}",
            failed.detail
        )));
    }
    Ok(())
}

fn validate_result_binding(
    working_dir: &Path,
    check: &AcceptanceCheck,
    result: &AcceptanceCheckResult,
) -> std::result::Result<(), String> {
    let (kind, must_pass, command, cwd, process_check) = match check {
        AcceptanceCheck::CargoTest { args, must_pass } => (
            "cargo_test",
            *must_pass,
            Some(format_command("cargo test", args)),
            Some(working_dir.to_path_buf()),
            true,
        ),
        AcceptanceCheck::FileExists { must_pass, .. } => {
            ("file_exists", *must_pass, None, None, false)
        }
        AcceptanceCheck::ContentMatch { must_pass, .. } => {
            ("content_match", *must_pass, None, None, false)
        }
        AcceptanceCheck::BuildSuccess { cwd, must_pass } => (
            "build_success",
            *must_pass,
            Some("cargo build".to_string()),
            Some(render_template(working_dir, cwd)),
            true,
        ),
        AcceptanceCheck::Shell {
            command,
            cwd,
            must_pass,
        } => (
            "shell",
            *must_pass,
            Some(command.clone()),
            Some(
                cwd.as_deref()
                    .map(|cwd| render_template(working_dir, cwd))
                    .unwrap_or_else(|| working_dir.to_path_buf()),
            ),
            true,
        ),
    };
    if result.kind != kind {
        return Err(format!("kind {} does not match {kind}", result.kind));
    }
    if result.must_pass != must_pass {
        return Err(format!(
            "must_pass {} does not match {must_pass}",
            result.must_pass
        ));
    }
    if result.command != command {
        return Err(format!(
            "command {:?} does not match {:?}",
            result.command, command
        ));
    }
    if result.cwd != cwd {
        return Err(format!("cwd {:?} does not match {:?}", result.cwd, cwd));
    }
    if result.detail.trim().is_empty() {
        return Err("detail is empty".to_string());
    }
    if process_check {
        if result.duration_ms.is_none() {
            return Err("process result is missing duration".to_string());
        }
        let success_suffix = format!("; success={}", result.passed);
        if !result.detail.ends_with(&success_suffix) {
            return Err(format!(
                "process detail does not end with {success_suffix:?}"
            ));
        }
    } else if result.duration_ms.is_some() || result.stdout.is_some() || result.stderr.is_some() {
        return Err("non-process result contains process output".to_string());
    }
    match check {
        AcceptanceCheck::FileExists { path, .. } => {
            let exists = render_template(working_dir, path).exists();
            if result.passed != exists {
                return Err(format!(
                    "file existence result {} does not match recomputed {exists}",
                    result.passed
                ));
            }
        }
        AcceptanceCheck::ContentMatch { path, pattern, .. } => {
            let path = render_template(working_dir, path);
            let body = std::fs::read_to_string(path).unwrap_or_default();
            let matched = regex::Regex::new(pattern)
                .map(|regex| regex.is_match(&body))
                .unwrap_or_else(|_| body.contains(pattern));
            if result.passed != matched {
                return Err(format!(
                    "content result {} does not match recomputed {matched}",
                    result.passed
                ));
            }
        }
        AcceptanceCheck::CargoTest { .. }
        | AcceptanceCheck::BuildSuccess { .. }
        | AcceptanceCheck::Shell { .. } => {}
    }
    Ok(())
}

fn same_tamper_facts(
    left: &crate::tamper::AcceptanceTamper,
    right: &crate::tamper::AcceptanceTamper,
) -> bool {
    left.schema_version == right.schema_version
        && left.run_id == right.run_id
        && left.verdict == right.verdict
        && left.spec_modified == right.spec_modified
        && left.lint_findings == right.lint_findings
        && left.covered_files_touched == right.covered_files_touched
        && left.caveats == right.caveats
        && left.refusal_reasons == right.refusal_reasons
}

fn invalid_gate_evaluation(message: impl Into<String>) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!(
        "gate evaluation validation failed: {}",
        message.into()
    ))
}

fn write_reconstructed_acceptance_progress(
    run_root: &Path,
    results: &[AcceptanceCheckResult],
) -> Result<()> {
    let path = acceptance_progress_path_for_run_root(run_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_path(parent)?;
    }
    let total = results.len();
    let mut entries = Vec::with_capacity(total.saturating_mul(2).saturating_add(2));
    entries.push(AcceptanceProgressEntry {
        checked_at: Utc::now(),
        status: "started".to_string(),
        index: 0,
        total,
        result: None,
    });
    for (index, result) in results.iter().cloned().enumerate() {
        let index = index + 1;
        entries.push(AcceptanceProgressEntry {
            checked_at: Utc::now(),
            status: "running".to_string(),
            index,
            total,
            result: None,
        });
        entries.push(AcceptanceProgressEntry {
            checked_at: Utc::now(),
            status: if result.passed {
                "passed".to_string()
            } else {
                "failed".to_string()
            },
            index,
            total,
            result: Some(result),
        });
    }
    entries.push(AcceptanceProgressEntry {
        checked_at: Utc::now(),
        status: if results
            .iter()
            .any(|result| result.must_pass && !result.passed)
        {
            "failed".to_string()
        } else {
            "passed".to_string()
        },
        index: total,
        total,
        result: None,
    });
    let mut bytes = Vec::new();
    for entry in entries {
        serde_json::to_writer(&mut bytes, &entry).map_err(|source| DeadreckonError::Json {
            path: path.clone(),
            source,
        })?;
        bytes.push(b'\n');
    }
    std::fs::write(&path, bytes).with_path(&path)
}

/// Serialize a detected/inferred default contract to the run's acceptance spec
/// path with a provenance comment header, so the operator, `verdict`, and tamper
/// all see exactly the contract that ran.
fn write_generated_spec(
    spec_path: &Path,
    kind: &crate::acceptance_defaults::ProjectKind,
    checks: &[AcceptanceCheck],
) -> Result<()> {
    let spec = AcceptanceSpec {
        name: None,
        checks: checks.to_vec(),
    };
    let body = serde_yaml::to_string(&spec).map_err(|source| {
        DeadreckonError::InvalidInput(format!(
            "failed to serialize generated acceptance spec: {source}"
        ))
    })?;
    let header = format!(
        "# generated by deadreckon detect: {}\n",
        crate::acceptance_defaults::kind_label(kind)
    );
    std::fs::write(spec_path, format!("{header}{body}")).with_path(spec_path)
}

pub fn acceptance_checks_from_yaml(raw: &str) -> Result<Vec<AcceptanceCheck>> {
    parse_acceptance_checks(raw)
}

fn evaluate_acceptance_checks_inner(
    run_root: &Path,
    working_dir: &Path,
    progress_path: Option<&Path>,
) -> Result<Vec<AcceptanceCheckResult>> {
    let spec_path = acceptance_spec_path_for_run_root(run_root);
    if !spec_path.exists() {
        return evaluate_default_acceptance_with_progress(working_dir, progress_path);
    }
    let raw = std::fs::read_to_string(&spec_path).with_path(&spec_path)?;
    let checks = parse_acceptance_checks(&raw)?;
    let total = checks.len();
    emit_acceptance_progress(progress_path, "started", 0, total, None)?;
    let mut results = Vec::new();
    for (idx, check) in checks.into_iter().enumerate() {
        let index = idx + 1;
        emit_acceptance_progress(progress_path, "running", index, total, None)?;
        let result = evaluate_check(working_dir, check)?;
        let status = if result.passed { "passed" } else { "failed" };
        emit_acceptance_progress(progress_path, status, index, total, Some(result.clone()))?;
        results.push(result);
    }
    let status = if results
        .iter()
        .any(|result| result.must_pass && !result.passed)
    {
        "failed"
    } else {
        "passed"
    };
    emit_acceptance_progress(progress_path, status, total, total, None)?;
    Ok(results)
}

fn evaluate_default_acceptance_with_progress(
    working_dir: &Path,
    progress_path: Option<&Path>,
) -> Result<Vec<AcceptanceCheckResult>> {
    emit_acceptance_progress(progress_path, "started", 0, 1, None)?;
    emit_acceptance_progress(progress_path, "running", 1, 1, None)?;
    let results = evaluate_default_acceptance(working_dir)?;
    if let Some(result) = results.first().cloned() {
        let status = if result.passed { "passed" } else { "failed" };
        emit_acceptance_progress(progress_path, status, 1, 1, Some(result))?;
    }
    let status = if results
        .iter()
        .any(|result| result.must_pass && !result.passed)
    {
        "failed"
    } else {
        "passed"
    };
    emit_acceptance_progress(progress_path, status, 1, 1, None)?;
    Ok(results)
}

fn emit_acceptance_progress(
    progress_path: Option<&Path>,
    status: &str,
    index: usize,
    total: usize,
    result: Option<AcceptanceCheckResult>,
) -> Result<()> {
    let Some(progress_path) = progress_path else {
        return Ok(());
    };
    append_json_line(
        progress_path,
        &AcceptanceProgressEntry {
            checked_at: Utc::now(),
            status: status.to_string(),
            index,
            total,
            result,
        },
    )
}

/// The default checks the no-spec (dr-gate) path evaluates — the same detection
/// and compilation as `compiled_acceptance_checks`, so the standalone binary and
/// the in-process compile agree byte-for-byte instead of diverging into a
/// Rust-only special case.
fn default_acceptance_checks(working_dir: &Path) -> Vec<AcceptanceCheck> {
    let kind = crate::acceptance_defaults::detect_project_kind(working_dir);
    crate::acceptance_defaults::default_checks_for(&kind, working_dir)
}

fn evaluate_default_acceptance(working_dir: &Path) -> Result<Vec<AcceptanceCheckResult>> {
    let mut results = Vec::new();
    for check in default_acceptance_checks(working_dir) {
        results.push(evaluate_check(working_dir, check)?);
    }
    Ok(results)
}

fn evaluate_check(working_dir: &Path, check: AcceptanceCheck) -> Result<AcceptanceCheckResult> {
    match check {
        AcceptanceCheck::CargoTest { args, must_pass } => {
            let started = Instant::now();
            let output = gate_check_command("cargo")
                .arg("test")
                .args(&args)
                .current_dir(working_dir)
                .output()
                .map_err(|source| DeadreckonError::Io {
                    path: working_dir.join("Cargo.toml"),
                    source,
                })?;
            Ok(AcceptanceCheckResult {
                kind: "cargo_test".to_string(),
                passed: output.status.success(),
                must_pass,
                detail: format!(
                    "cargo test exited with {}; success={}",
                    output.status,
                    output.status.success()
                ),
                command: Some(format_command("cargo test", &args)),
                cwd: Some(working_dir.to_path_buf()),
                duration_ms: Some(duration_ms(started)),
                stdout: clipped_stdout(&output),
                stderr: clipped_stderr(&output),
            })
        }
        AcceptanceCheck::FileExists { path, must_pass } => {
            let path = render_template(working_dir, &path);
            let exists = path.exists();
            Ok(AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: exists,
                must_pass,
                detail: if exists {
                    format!("{} exists", path.display())
                } else {
                    format!("{} is missing", path.display())
                },
                command: None,
                cwd: None,
                duration_ms: None,
                stdout: None,
                stderr: None,
            })
        }
        AcceptanceCheck::ContentMatch {
            path,
            pattern,
            must_pass,
        } => {
            let path = render_template(working_dir, &path);
            let body = std::fs::read_to_string(&path).unwrap_or_default();
            let matched = regex::Regex::new(&pattern)
                .map(|regex| regex.is_match(&body))
                .unwrap_or_else(|_| body.contains(&pattern));
            Ok(AcceptanceCheckResult {
                kind: "content_match".to_string(),
                passed: matched,
                must_pass,
                detail: if matched {
                    format!("{} matches {:?}", path.display(), pattern)
                } else {
                    format!("{} does not match {:?}", path.display(), pattern)
                },
                command: None,
                cwd: None,
                duration_ms: None,
                stdout: None,
                stderr: None,
            })
        }
        AcceptanceCheck::BuildSuccess { cwd, must_pass } => {
            let cwd = render_template(working_dir, &cwd);
            let started = Instant::now();
            let output = gate_check_command("cargo")
                .arg("build")
                .current_dir(&cwd)
                .output()
                .map_err(|source| DeadreckonError::Io {
                    path: cwd.join("Cargo.toml"),
                    source,
                })?;
            Ok(AcceptanceCheckResult {
                kind: "build_success".to_string(),
                passed: output.status.success(),
                must_pass,
                detail: format!(
                    "cargo build in {} exited with {}; success={}",
                    cwd.display(),
                    output.status,
                    output.status.success()
                ),
                command: Some("cargo build".to_string()),
                cwd: Some(cwd),
                duration_ms: Some(duration_ms(started)),
                stdout: clipped_stdout(&output),
                stderr: clipped_stderr(&output),
            })
        }
        AcceptanceCheck::Shell {
            command,
            cwd,
            must_pass,
        } => {
            let cwd = cwd
                .map(|cwd| render_template(working_dir, &cwd))
                .unwrap_or_else(|| working_dir.to_path_buf());
            let started = Instant::now();
            let output = gate_check_command("sh")
                .arg("-lc")
                .arg(&command)
                .current_dir(&cwd)
                .output()
                .map_err(|source| DeadreckonError::Io {
                    path: cwd.clone(),
                    source,
                })?;
            Ok(AcceptanceCheckResult {
                kind: "shell".to_string(),
                passed: output.status.success(),
                must_pass,
                detail: format!(
                    "shell {:?} in {} exited with {}; success={}",
                    command,
                    cwd.display(),
                    output.status,
                    output.status.success()
                ),
                command: Some(command),
                cwd: Some(cwd),
                duration_ms: Some(duration_ms(started)),
                stdout: clipped_stdout(&output),
                stderr: clipped_stderr(&output),
            })
        }
    }
}

fn gate_check_command(program: &str) -> Command {
    let mut command = Command::new(program);
    // dr-gate receives the protected signing key from its trusted parent. The
    // checks it runs may execute repository-controlled build scripts, so none of
    // the signing context may cross that child-process boundary.
    command.env_remove(GATE_KEY_ENV);
    command.env_remove(GATE_CONTAINED_ENV);
    command.env_remove(GATE_SANDBOX_BACKEND_ENV);
    command
}

fn duration_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn clipped_stdout(output: &Output) -> Option<String> {
    clipped_output(&output.stdout)
}

fn clipped_stderr(output: &Output) -> Option<String> {
    clipped_output(&output.stderr)
}

fn clipped_output(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(clip_text(&text, 4096))
    }
}

fn clip_text(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut clipped = text
        .char_indices()
        .take_while(|(idx, _)| *idx < limit)
        .map(|(_, ch)| ch)
        .collect::<String>();
    clipped.push_str("\n... output truncated ...");
    clipped
}

fn format_command(base: &str, args: &[String]) -> String {
    if args.is_empty() {
        base.to_string()
    } else {
        format!("{base} {}", args.join(" "))
    }
}

fn parse_acceptance_checks(raw: &str) -> Result<Vec<AcceptanceCheck>> {
    let root: YamlValue = serde_yaml::from_str(raw).map_err(|source| {
        DeadreckonError::InvalidInput(format!("invalid acceptance.yaml: {source}"))
    })?;
    let mut checks = Vec::new();
    for item in yaml_seq(yaml_get(&root, "checks")) {
        checks.push(parse_check_value(item, None)?);
    }
    for item in yaml_seq(yaml_get(&root, "required")) {
        checks.push(parse_check_value(item, Some(true))?);
    }
    for item in yaml_seq(yaml_get(&root, "optional")) {
        checks.push(parse_check_value(item, Some(false))?);
    }
    for item in yaml_seq(yaml_get(&root, "tests")) {
        checks.push(parse_shell_check(item, true)?);
    }
    for item in yaml_seq(yaml_get(&root, "file-exists")) {
        checks.push(parse_file_exists_check(item, true)?);
    }
    for item in yaml_seq(yaml_get(&root, "content-match")) {
        checks.push(parse_content_match_check(item, true)?);
    }
    for item in yaml_seq(yaml_get(&root, "build-success")) {
        checks.push(parse_build_success_check(item, true));
    }
    Ok(checks)
}

fn parse_check_value(value: &YamlValue, force_must_pass: Option<bool>) -> Result<AcceptanceCheck> {
    if let Ok(mut check) = serde_yaml::from_value::<AcceptanceCheck>(value.clone()) {
        if let Some(must_pass) = force_must_pass {
            check.set_must_pass(must_pass);
        }
        return Ok(check);
    }
    if let Some(command) = yaml_string(value) {
        return Ok(AcceptanceCheck::Shell {
            command,
            cwd: None,
            must_pass: force_must_pass.unwrap_or(true),
        });
    }
    let Some((kind, body)) = single_key_mapping(value) else {
        return Err(DeadreckonError::InvalidInput(format!(
            "invalid acceptance check: {:?}",
            value
        )));
    };
    let must_pass = force_must_pass.unwrap_or(true);
    match kind.as_str() {
        "file-exists" | "file_exists" => parse_file_exists_check(body, must_pass),
        "content-match" | "content_match" => parse_content_match_check(body, must_pass),
        "build-success" | "build_success" => Ok(parse_build_success_check(body, must_pass)),
        "shell" | "test" => parse_shell_check(body, must_pass),
        "cargo-test" | "cargo_test" => Ok(AcceptanceCheck::CargoTest {
            args: yaml_string(body).map(|arg| vec![arg]).unwrap_or_default(),
            must_pass,
        }),
        other => Err(DeadreckonError::InvalidInput(format!(
            "unknown acceptance check kind {other}"
        ))),
    }
}

fn parse_file_exists_check(value: &YamlValue, must_pass: bool) -> Result<AcceptanceCheck> {
    let path = yaml_string(value)
        .or_else(|| yaml_get(value, "path").and_then(yaml_string))
        .ok_or_else(|| DeadreckonError::InvalidInput("file-exists requires path".to_string()))?;
    Ok(AcceptanceCheck::FileExists { path, must_pass })
}

fn parse_content_match_check(value: &YamlValue, must_pass: bool) -> Result<AcceptanceCheck> {
    let path = yaml_get(value, "path")
        .and_then(yaml_string)
        .ok_or_else(|| DeadreckonError::InvalidInput("content-match requires path".to_string()))?;
    let pattern = yaml_get(value, "pattern")
        .and_then(yaml_string)
        .ok_or_else(|| {
            DeadreckonError::InvalidInput("content-match requires pattern".to_string())
        })?;
    Ok(AcceptanceCheck::ContentMatch {
        path,
        pattern,
        must_pass,
    })
}

fn parse_build_success_check(value: &YamlValue, must_pass: bool) -> AcceptanceCheck {
    let cwd = yaml_string(value)
        .or_else(|| yaml_get(value, "cwd").and_then(yaml_string))
        .unwrap_or_else(|| "{working_dir}".to_string());
    AcceptanceCheck::BuildSuccess { cwd, must_pass }
}

fn parse_shell_check(value: &YamlValue, must_pass: bool) -> Result<AcceptanceCheck> {
    let command = yaml_string(value)
        .or_else(|| yaml_get(value, "command").and_then(yaml_string))
        .ok_or_else(|| DeadreckonError::InvalidInput("shell check requires command".to_string()))?;
    let cwd = yaml_get(value, "cwd").and_then(yaml_string);
    Ok(AcceptanceCheck::Shell {
        command,
        cwd,
        must_pass,
    })
}

fn yaml_get<'a>(value: &'a YamlValue, key: &str) -> Option<&'a YamlValue> {
    value.as_mapping()?.get(YamlValue::String(key.to_string()))
}

fn yaml_seq(value: Option<&YamlValue>) -> Vec<&YamlValue> {
    match value {
        Some(YamlValue::Sequence(values)) => values.iter().collect(),
        Some(value) => vec![value],
        None => Vec::new(),
    }
}

fn yaml_string(value: &YamlValue) -> Option<String> {
    value.as_str().map(ToString::to_string)
}

fn single_key_mapping(value: &YamlValue) -> Option<(String, &YamlValue)> {
    let mapping = value.as_mapping()?;
    if mapping.len() != 1 {
        return None;
    }
    let (key, value) = mapping.iter().next()?;
    Some((key.as_str()?.to_string(), value))
}

impl AcceptanceCheck {
    fn set_must_pass(&mut self, value: bool) {
        match self {
            AcceptanceCheck::CargoTest { must_pass, .. }
            | AcceptanceCheck::FileExists { must_pass, .. }
            | AcceptanceCheck::ContentMatch { must_pass, .. }
            | AcceptanceCheck::BuildSuccess { must_pass, .. }
            | AcceptanceCheck::Shell { must_pass, .. } => *must_pass = value,
        }
    }
}

fn render_template(working_dir: &Path, value: &str) -> PathBuf {
    PathBuf::from(value.replace("{working_dir}", &working_dir.to_string_lossy()))
}

#[cfg(test)]
fn marker_signature(run_root: &Path, marker: &AcceptanceMarker) -> Result<String> {
    match marker.schema_version {
        1 => legacy_marker_signature(run_root, marker),
        2 => {
            let key = read_gate_key_for_run_root(run_root, &marker.run_id)?;
            v2_marker_signature(run_root, marker, &key)
        }
        version => Err(DeadreckonError::InvalidInput(format!(
            "unsupported acceptance marker schema {version}"
        ))),
    }
}

fn validate_legacy_marker_signature(run_root: &Path, marker: &AcceptanceMarker) -> Result<()> {
    if marker.produced_by != "dr-gate" {
        return Err(DeadreckonError::InvalidInput(
            "legacy acceptance marker was not produced by dr-gate".to_string(),
        ));
    }
    let expected = legacy_marker_signature(run_root, marker)?;
    if marker.signature != expected {
        return Err(DeadreckonError::InvalidInput(
            "acceptance marker signature is invalid; forged self-attestation refused".to_string(),
        ));
    }
    Ok(())
}

fn legacy_marker_signature(run_root: &Path, marker: &AcceptanceMarker) -> Result<String> {
    let nonce_path = gate_nonce_path_for_run_root(run_root);
    let nonce = std::fs::read_to_string(&nonce_path).with_path(&nonce_path)?;
    let mut hasher = DefaultHasher::new();
    nonce.trim().hash(&mut hasher);
    marker.schema_version.hash(&mut hasher);
    marker.run_id.hash(&mut hasher);
    marker.status.hash(&mut hasher);
    marker.produced_by.hash(&mut hasher);
    marker.checked_at.to_rfc3339().hash(&mut hasher);
    marker.working_dir.hash(&mut hasher);
    marker.check_count.hash(&mut hasher);
    for check in &marker.checks {
        check.hash(&mut hasher);
    }
    let tamper_path = crate::tamper::acceptance_tamper_path_for_run_root(run_root);
    match std::fs::read(&tamper_path) {
        Ok(bytes) => bytes.hash(&mut hasher),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => "".hash(&mut hasher),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: tamper_path,
                source,
            });
        }
    }
    // A campaign result run carries its gate-verdict roll-up; binding it here means
    // the roll-up cannot be edited after signing to launder a refused leaf into a
    // clean pass. Absent (the normal, non-campaign case) hashes empty.
    let rollup_path = crate::campaign::rollup_path_at_run_root(run_root);
    match std::fs::read(&rollup_path) {
        Ok(bytes) => bytes.hash(&mut hasher),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => "".hash(&mut hasher),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: rollup_path,
                source,
            });
        }
    }
    Ok(format!("{:016x}", hasher.finish()))
}

pub fn canonical_marker_bytes(run_root: &Path, marker: &AcceptanceMarker) -> Result<Vec<u8>> {
    canonical_marker_bytes_with_parent_repair(run_root, marker, None)
}

#[derive(Clone, Copy)]
struct ParentRepairBoundBytes<'a> {
    manifest: Option<&'a [u8]>,
    candidate: Option<&'a [u8]>,
}

fn canonical_marker_bytes_with_parent_repair(
    run_root: &Path,
    marker: &AcceptanceMarker,
    parent_repair_override: Option<ParentRepairBoundBytes<'_>>,
) -> Result<Vec<u8>> {
    if marker.schema_version != 2 {
        return Err(DeadreckonError::InvalidInput(format!(
            "canonical HMAC bytes require marker schema 2, got {}",
            marker.schema_version
        )));
    }
    let tamper = read_optional_bound_bytes(&crate::tamper::acceptance_tamper_path_for_run_root(
        run_root,
    ))?;
    let campaign_rollup =
        read_optional_bound_bytes(&crate::campaign::rollup_path_at_run_root(run_root))?;
    let (parent_repair, parent_repair_candidate) = match parent_repair_override {
        Some(bound) => (
            bound.manifest.unwrap_or_default().to_vec(),
            bound.candidate.unwrap_or_default().to_vec(),
        ),
        None => (
            read_optional_bound_bytes(&parent_repair_manifest_path_for_run_root(run_root))?,
            read_optional_bound_bytes(&parent_repair_candidate_path_for_run_root(run_root))?,
        ),
    };
    let mut checks = Vec::new();
    for check in &marker.checks {
        let bytes = serde_json::to_vec(check).map_err(|source| DeadreckonError::Json {
            path: marker_path_for_run_root(run_root),
            source,
        })?;
        append_sized_bytes(&mut checks, &bytes)?;
    }

    let mut bytes = V2_CANONICAL_MAGIC.to_vec();
    append_canonical_field(
        &mut bytes,
        "schema_version",
        marker.schema_version.to_string().as_bytes(),
    )?;
    append_canonical_field(&mut bytes, "run_id", marker.run_id.as_bytes())?;
    append_canonical_field(&mut bytes, "status", marker.status.as_bytes())?;
    append_canonical_field(&mut bytes, "produced_by", marker.produced_by.as_bytes())?;
    append_canonical_field(&mut bytes, "issuer", marker.issuer.as_bytes())?;
    append_canonical_field(
        &mut bytes,
        "proof_kind",
        marker.proof_kind.canonical_name().as_bytes(),
    )?;
    append_canonical_field(
        &mut bytes,
        "checked_at",
        marker.checked_at.to_rfc3339().as_bytes(),
    )?;
    append_canonical_field(
        &mut bytes,
        "working_dir",
        marker.working_dir.to_string_lossy().as_bytes(),
    )?;
    append_canonical_field(
        &mut bytes,
        "contained",
        if marker.contained { b"true" } else { b"false" },
    )?;
    append_canonical_field(
        &mut bytes,
        "sandbox_backend",
        marker.sandbox_backend.as_bytes(),
    )?;
    append_canonical_field(
        &mut bytes,
        "check_count",
        marker.check_count.to_string().as_bytes(),
    )?;
    append_canonical_field(&mut bytes, "checks", &checks)?;
    append_canonical_field(&mut bytes, "tamper", &tamper)?;
    append_canonical_field(&mut bytes, "campaign_rollup", &campaign_rollup)?;
    // Preserve the exact v2 sequence for markers created before parent
    // repair existed. A repaired parent appends these fields only when the
    // trusted repair controller has materialized them, so deleting, changing
    // or substituting either file invalidates the marker HMAC.
    if !parent_repair.is_empty() {
        append_canonical_field(&mut bytes, "parent_repair", &parent_repair)?;
    }
    if !parent_repair_candidate.is_empty() {
        append_canonical_field(
            &mut bytes,
            "parent_repair_candidate",
            &parent_repair_candidate,
        )?;
    }
    Ok(bytes)
}

pub fn v2_marker_signature(
    run_root: &Path,
    marker: &AcceptanceMarker,
    gate_key: &[u8],
) -> Result<String> {
    require_gate_key_length(gate_key)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(gate_key).map_err(|_| {
        DeadreckonError::InvalidInput("HMAC-SHA-256 refused the gate key".to_string())
    })?;
    mac.update(&canonical_marker_bytes(run_root, marker)?);
    Ok(hex_encode(&mac.finalize().into_bytes()))
}

pub fn verify_v2_marker_signature(
    run_root: &Path,
    marker: &AcceptanceMarker,
    gate_key: &[u8],
) -> Result<()> {
    verify_v2_marker_signature_with_parent_repair(run_root, marker, gate_key, None)
}

fn verify_v2_marker_signature_with_parent_repair(
    run_root: &Path,
    marker: &AcceptanceMarker,
    gate_key: &[u8],
    parent_repair: Option<ParentRepairBoundBytes<'_>>,
) -> Result<()> {
    require_gate_key_length(gate_key)?;
    let signature = hex_decode(&marker.signature).map_err(|reason| {
        DeadreckonError::InvalidInput(format!(
            "acceptance marker signature is invalid: {reason}; forged self-attestation refused"
        ))
    })?;
    let mut mac = Hmac::<Sha256>::new_from_slice(gate_key).map_err(|_| {
        DeadreckonError::InvalidInput("HMAC-SHA-256 refused the gate key".to_string())
    })?;
    mac.update(&canonical_marker_bytes_with_parent_repair(
        run_root,
        marker,
        parent_repair,
    )?);
    mac.verify_slice(&signature).map_err(|_| {
        DeadreckonError::InvalidInput(
            "acceptance marker signature is invalid; forged self-attestation refused".to_string(),
        )
    })
}

fn require_gate_key_length(gate_key: &[u8]) -> Result<()> {
    if gate_key.len() != GATE_KEY_BYTES {
        return Err(DeadreckonError::InvalidInput(format!(
            "gate key must be {GATE_KEY_BYTES} bytes, got {}",
            gate_key.len()
        )));
    }
    Ok(())
}

fn read_optional_bound_bytes(path: &Path) -> Result<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(bytes),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn append_canonical_field(output: &mut Vec<u8>, name: &str, value: &[u8]) -> Result<()> {
    append_sized_bytes(output, name.as_bytes())?;
    append_sized_bytes(output, value)
}

fn append_sized_bytes(output: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len = u32::try_from(value.len()).map_err(|_| {
        DeadreckonError::InvalidInput("acceptance marker field exceeds 4 GiB".to_string())
    })?;
    output.extend_from_slice(&len.to_be_bytes());
    output.extend_from_slice(value);
    Ok(())
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

fn gate_key_file_stem(run_id: &str) -> String {
    let mut output = String::with_capacity(run_id.len());
    for byte in run_id.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_') {
            output.push(char::from(*byte));
        } else {
            output.push('%');
            output.push_str(&hex_encode(&[*byte]));
        }
    }
    output
}

fn hex_decode(value: &str) -> std::result::Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("hex value has odd length".to_string());
    }
    let mut output = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = hex_nibble(pair[0])?;
        let low = hex_nibble(pair[1])?;
        output.push((high << 4) | low);
    }
    Ok(output)
}

fn hex_nibble(value: u8) -> std::result::Result<u8, String> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err("hex value contains a non-hex character".to_string()),
    }
}

#[cfg(test)]
fn canonical_marker_field_labels(bytes: &[u8]) -> Result<Vec<String>> {
    let mut remaining = bytes.strip_prefix(V2_CANONICAL_MAGIC).ok_or_else(|| {
        DeadreckonError::InvalidInput("canonical marker magic is missing".to_string())
    })?;
    let mut labels = Vec::new();
    while !remaining.is_empty() {
        let (label, after_label) = take_sized_bytes(remaining)?;
        let (_, after_value) = take_sized_bytes(after_label)?;
        labels.push(String::from_utf8(label.to_vec()).map_err(|_| {
            DeadreckonError::InvalidInput("canonical marker label is not UTF-8".to_string())
        })?);
        remaining = after_value;
    }
    Ok(labels)
}

#[cfg(test)]
fn take_sized_bytes(bytes: &[u8]) -> Result<(&[u8], &[u8])> {
    let prefix = bytes.get(..4).ok_or_else(|| {
        DeadreckonError::InvalidInput("canonical marker field is truncated".to_string())
    })?;
    let len = u32::from_be_bytes([prefix[0], prefix[1], prefix[2], prefix[3]]) as usize;
    let value = bytes.get(4..4 + len).ok_or_else(|| {
        DeadreckonError::InvalidInput("canonical marker value is truncated".to_string())
    })?;
    Ok((value, &bytes[4 + len..]))
}

#[cfg(test)]
fn v2_test_marker(run_root: &Path) -> AcceptanceMarker {
    AcceptanceMarker {
        schema_version: 2,
        run_id: "canonical-test".to_string(),
        status: "pass".to_string(),
        produced_by: "dr-gate".to_string(),
        issuer: "dr-gate".to_string(),
        proof_kind: AcceptanceProofKind::NativeGate,
        checked_at: DateTime::parse_from_rfc3339("2026-07-29T00:00:00Z")
            .expect("fixture timestamp")
            .with_timezone(&Utc),
        working_dir: run_root.join("working"),
        contained: true,
        sandbox_backend: "seatbelt".to_string(),
        signature: String::new(),
        check_count: 0,
        checks: Vec::new(),
    }
}

fn default_must_pass() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tempfile::TempDir;

    use crate::artifacts::{ProvenanceRecord, append_provenance, snapshot_working};
    use crate::paths::DeadreckonPaths;
    use crate::state::{PipelineState, RunOptions, create_run};
    use crate::tamper::{AcceptanceTamperVerdict, read_acceptance_tamper_for_run_root};

    use super::{
        ACCEPTANCE_MARKER, AcceptanceCheckResult, AcceptanceContainment, AcceptanceMarker,
        AcceptanceProofKind, GateEvaluation, validate_acceptance_marker,
    };

    // ---- Binnacle P1-P4: protected key, cryptographic marker, containment ----

    fn keyless_evaluation_fixture() -> (TempDir, PipelineState, GateEvaluation) {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "two phase gate".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("two-phase-gate".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        std::fs::write(state.working_dir.join("README.md"), "approved\n").expect("readme");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "checks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("acceptance contract");
        let evaluation = super::evaluate_gate(&state.run_id, &state.run_root, &state.working_dir)
            .expect("keyless evaluation");
        (temp, state, evaluation)
    }

    #[test]
    fn keyless_evaluation_echoes_identity_and_legacy_json_defaults_to_absent() {
        let (_temp, state, legacy_evaluation) = keyless_evaluation_fixture();
        assert!(legacy_evaluation.gate_evaluator_sha256.is_none());

        let identity_sha256 = format!("sha256:{}", "a".repeat(64));
        let evaluation = super::evaluate_gate_with_identity(
            &state.run_id,
            &state.run_root,
            &state.working_dir,
            Some(identity_sha256.clone()),
        )
        .expect("identity-bound evaluation");
        assert_eq!(
            evaluation.gate_evaluator_sha256.as_deref(),
            Some(identity_sha256.as_str())
        );

        let mut legacy_json = serde_json::to_value(evaluation).expect("evaluation JSON");
        legacy_json
            .as_object_mut()
            .expect("evaluation object")
            .remove("gate_evaluator_sha256");
        let decoded: GateEvaluation =
            serde_json::from_value(legacy_json).expect("legacy evaluation JSON");
        assert!(decoded.gate_evaluator_sha256.is_none());
    }

    fn legacy_v2_canonical_bytes(marker: &AcceptanceMarker) -> Vec<u8> {
        let mut checks = Vec::new();
        for check in &marker.checks {
            let bytes = serde_json::to_vec(check).expect("serialize check");
            super::append_sized_bytes(&mut checks, &bytes).expect("append check");
        }

        let mut bytes = super::V2_CANONICAL_MAGIC.to_vec();
        super::append_canonical_field(
            &mut bytes,
            "schema_version",
            marker.schema_version.to_string().as_bytes(),
        )
        .expect("schema version");
        super::append_canonical_field(&mut bytes, "run_id", marker.run_id.as_bytes())
            .expect("run id");
        super::append_canonical_field(&mut bytes, "status", marker.status.as_bytes())
            .expect("status");
        super::append_canonical_field(&mut bytes, "produced_by", marker.produced_by.as_bytes())
            .expect("producer");
        super::append_canonical_field(&mut bytes, "issuer", marker.issuer.as_bytes())
            .expect("issuer");
        super::append_canonical_field(
            &mut bytes,
            "proof_kind",
            marker.proof_kind.canonical_name().as_bytes(),
        )
        .expect("proof kind");
        super::append_canonical_field(
            &mut bytes,
            "checked_at",
            marker.checked_at.to_rfc3339().as_bytes(),
        )
        .expect("checked at");
        super::append_canonical_field(
            &mut bytes,
            "working_dir",
            marker.working_dir.to_string_lossy().as_bytes(),
        )
        .expect("working dir");
        super::append_canonical_field(
            &mut bytes,
            "contained",
            if marker.contained { b"true" } else { b"false" },
        )
        .expect("contained");
        super::append_canonical_field(
            &mut bytes,
            "sandbox_backend",
            marker.sandbox_backend.as_bytes(),
        )
        .expect("sandbox backend");
        super::append_canonical_field(
            &mut bytes,
            "check_count",
            marker.check_count.to_string().as_bytes(),
        )
        .expect("check count");
        super::append_canonical_field(&mut bytes, "checks", &checks).expect("checks");
        super::append_canonical_field(&mut bytes, "tamper", b"").expect("tamper");
        super::append_canonical_field(&mut bytes, "campaign_rollup", b"").expect("campaign rollup");
        bytes
    }

    #[test]
    fn keyless_evaluation_is_versioned_and_writes_no_proof_artifacts() {
        let (_temp, state, evaluation) = keyless_evaluation_fixture();

        assert_eq!(
            evaluation.schema_version,
            super::GATE_EVALUATION_SCHEMA_VERSION
        );
        assert_eq!(evaluation.run_id, state.run_id);
        assert_eq!(
            evaluation.working_dir,
            state.working_dir.canonicalize().expect("canonical working")
        );
        assert_eq!(evaluation.results.len(), 1);
        assert!(evaluation.results[0].passed);
        assert_eq!(
            evaluation.contract_sha256,
            crate::flight::sha256_file(&state.run_root.join("acceptance.yaml"))
                .expect("contract digest")
        );
        assert!(!super::marker_path(&state).exists());
        assert!(!super::acceptance_progress_path_for_run_root(&state.run_root).exists());
        assert!(!crate::tamper::acceptance_tamper_path_for_run_root(&state.run_root).exists());
    }

    #[test]
    fn integrity_validation_preserves_a_legitimate_failed_result_for_inspection() {
        let (_temp, state, _) = keyless_evaluation_fixture();
        std::fs::remove_file(state.working_dir.join("README.md")).expect("remove required file");
        let evaluation = super::evaluate_gate(&state.run_id, &state.run_root, &state.working_dir)
            .expect("keyless failed evaluation");

        assert!(!evaluation.results[0].passed);
        super::validate_gate_evaluation_integrity(
            &state.run_id,
            &state.run_root,
            &state.working_dir,
            &evaluation,
        )
        .expect("failed result remains trustworthy evidence");
        let error = super::validate_gate_evaluation(
            &state.run_id,
            &state.run_root,
            &state.working_dir,
            &evaluation,
        )
        .expect_err("failed result is not acceptable for signing");
        assert!(
            error.to_string().contains("acceptance check failed"),
            "{error}"
        );
    }

    #[test]
    fn keyless_evaluation_requires_a_materialized_regular_contract() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "missing approved contract".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("missing-contract".to_string()),
                codebase: None,
            },
        )
        .expect("run");

        let err = super::evaluate_gate(&state.run_id, &state.run_root, &state.working_dir)
            .expect_err("missing contract refused");

        assert!(err.to_string().contains("trusted controller"), "{err}");
        assert!(!state.run_root.join("acceptance.yaml").exists());
    }

    #[test]
    fn signer_rejects_wrong_identity_contract_result_and_tamper() {
        let (_temp, state, evaluation) = keyless_evaluation_fixture();

        let mut wrong_run = evaluation.clone();
        wrong_run.run_id = "other-run".to_string();
        let err = super::validate_gate_evaluation(
            &state.run_id,
            &state.run_root,
            &state.working_dir,
            &wrong_run,
        )
        .expect_err("wrong run refused");
        assert!(err.to_string().contains("run id"), "{err}");

        let mut wrong_path = evaluation.clone();
        wrong_path.working_dir = state.run_root.clone();
        let err = super::validate_gate_evaluation(
            &state.run_id,
            &state.run_root,
            &state.working_dir,
            &wrong_path,
        )
        .expect_err("wrong path refused");
        assert!(err.to_string().contains("working directory"), "{err}");

        let contract_path = state.run_root.join("acceptance.yaml");
        let contract = std::fs::read_to_string(&contract_path).expect("contract");
        std::fs::write(&contract_path, format!("{contract}\n# changed\n"))
            .expect("change contract");
        let err = super::validate_gate_evaluation(
            &state.run_id,
            &state.run_root,
            &state.working_dir,
            &evaluation,
        )
        .expect_err("wrong contract refused");
        assert!(err.to_string().contains("contract digest"), "{err}");
        std::fs::write(&contract_path, contract).expect("restore contract");

        let mut wrong_count = evaluation.clone();
        wrong_count.results.clear();
        let err = super::validate_gate_evaluation(
            &state.run_id,
            &state.run_root,
            &state.working_dir,
            &wrong_count,
        )
        .expect_err("wrong count refused");
        assert!(err.to_string().contains("result count"), "{err}");

        let mut wrong_result = evaluation.clone();
        wrong_result.results[0].passed = false;
        let err = super::validate_gate_evaluation(
            &state.run_id,
            &state.run_root,
            &state.working_dir,
            &wrong_result,
        )
        .expect_err("wrong result refused");
        assert!(err.to_string().contains("recomputed"), "{err}");

        let mut wrong_tamper = evaluation;
        wrong_tamper.tamper.spec_modified = !wrong_tamper.tamper.spec_modified;
        let err = super::validate_gate_evaluation(
            &state.run_id,
            &state.run_root,
            &state.working_dir,
            &wrong_tamper,
        )
        .expect_err("wrong tamper refused");
        assert!(err.to_string().contains("tamper evidence"), "{err}");
    }

    #[test]
    fn trusted_signer_reconstructs_evidence_then_writes_native_marker() {
        let (temp, state, evaluation) = keyless_evaluation_fixture();
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let key = super::read_gate_key(&paths, &state.run_id).expect("gate key");

        let marker = super::sign_gate_evaluation_with_key(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
            evaluation,
            &key,
            AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("sign");

        assert!(marker.contained);
        assert_eq!(marker.sandbox_backend, "sandbox-exec");
        assert!(super::acceptance_progress_path_for_run_root(&state.run_root).is_file());
        assert!(crate::tamper::acceptance_tamper_path_for_run_root(&state.run_root).is_file());
        validate_acceptance_marker(&state).expect("marker validates");
    }

    #[test]
    fn trusted_signer_writes_nothing_for_an_invalid_evaluation() {
        let (temp, state, mut evaluation) = keyless_evaluation_fixture();
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let key = super::read_gate_key(&paths, &state.run_id).expect("gate key");
        evaluation.results[0].passed = false;

        let err = super::sign_gate_evaluation_with_key(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
            evaluation,
            &key,
            AcceptanceContainment::uncontained("none"),
        )
        .expect_err("invalid evaluation refused");

        assert!(err.to_string().contains("recomputed"), "{err}");
        assert!(!super::marker_path(&state).exists());
        assert!(!super::acceptance_progress_path_for_run_root(&state.run_root).exists());
        assert!(!crate::tamper::acceptance_tamper_path_for_run_root(&state.run_root).exists());
    }

    #[test]
    fn gate_key_is_written_outside_the_run_root() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "protected gate key".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("outside-key".to_string()),
                codebase: None,
            },
        )
        .expect("run");

        let key_path = super::gate_key_path(&paths, &state.run_id);
        assert!(key_path.exists());
        assert!(!key_path.starts_with(&state.run_root));
        assert_eq!(
            super::read_gate_key(&paths, &state.run_id)
                .expect("key")
                .len(),
            32
        );
        assert!(!super::gate_nonce_path_for_run_root(&state.run_root).exists());
    }

    #[test]
    fn creating_the_same_run_identity_reuses_its_protected_gate_key() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));

        let first = super::create_gate_key(&paths, "same-run").expect("first key");
        let second = super::create_gate_key(&paths, "same-run").expect("reused key");

        assert_eq!(second, first);
        assert_eq!(
            super::read_gate_key(&paths, "same-run").expect("stored key"),
            first
        );
    }

    #[test]
    fn existing_invalid_gate_key_is_never_replaced() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let key_path = super::gate_key_path(&paths, "invalid-existing");
        std::fs::create_dir_all(key_path.parent().expect("key parent")).expect("key directory");
        std::fs::write(&key_path, "not-a-key").expect("invalid key fixture");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                key_path.parent().expect("key parent"),
                std::fs::Permissions::from_mode(0o700),
            )
            .expect("private key-store permissions");
            std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600))
                .expect("private fixture permissions");
        }

        let error = super::create_gate_key(&paths, "invalid-existing")
            .expect_err("invalid existing key must fail closed");

        assert!(error.to_string().contains("unreadable"));
        assert_eq!(
            std::fs::read_to_string(&key_path).expect("fixture remains"),
            "not-a-key"
        );
    }

    #[test]
    fn gate_key_path_cannot_escape_the_key_store() {
        let paths = DeadreckonPaths::from_home("/tmp/deadreckon-home");
        let path = super::gate_key_path(&paths, "../../outside");

        assert!(path.starts_with(paths.home().join("gate-keys")));
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("%2e%2e%2f%2e%2e%2foutside.key")
        );
    }

    #[cfg(unix)]
    #[test]
    fn gate_key_file_is_owner_read_write_only() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "private gate key".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("private-key".to_string()),
                codebase: None,
            },
        )
        .expect("run");

        let mode = std::fs::metadata(super::gate_key_path(&paths, &state.run_id))
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let directory_mode = std::fs::metadata(paths.home().join("gate-keys"))
            .expect("key-store metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(directory_mode, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn permissive_gate_key_store_refuses_historical_verification() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let key = super::create_gate_key(&paths, "permissive-store").expect("key");
        let store = paths.home().join("gate-keys");
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o755))
            .expect("weaken fixture store");

        let error = super::read_gate_key(&paths, "permissive-store")
            .expect_err("permissive store must fail closed");
        assert!(error.to_string().contains("accessible to other users"));
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn missing_gate_key_refuses_validation_rather_than_passing() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "missing key refuses".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("missing-key".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        let key = super::read_gate_key(&paths, &state.run_id).expect("key");
        super::write_native_acceptance_marker_with_results_and_key(
            &state.run_root,
            state.run_id.clone(),
            state.working_dir.clone(),
            Vec::new(),
            &key,
            super::AcceptanceContainment::uncontained("none"),
        )
        .expect("marker");
        std::fs::remove_file(super::gate_key_path(&paths, &state.run_id)).expect("remove key");

        let err = validate_acceptance_marker(&state).expect_err("missing key refuses");
        assert!(err.to_string().contains("missing"), "{err}");
        assert!(err.to_string().contains("deadreckon verdict"), "{err}");
    }

    #[test]
    fn v2_marker_signature_is_hmac_sha256_over_the_canonical_bytes() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "hmac marker".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "seatbelt".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("hmac-marker".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        let key = super::read_gate_key(&paths, &state.run_id).expect("key");
        let marker = super::write_native_acceptance_marker_with_results_and_key(
            &state.run_root,
            state.run_id.clone(),
            state.working_dir.clone(),
            Vec::new(),
            &key,
            super::AcceptanceContainment::contained("seatbelt"),
        )
        .expect("marker");

        assert_eq!(marker.schema_version, 2);
        assert_eq!(marker.signature.len(), 64);
        assert_eq!(
            marker.signature,
            super::v2_marker_signature(&state.run_root, &marker, &key).expect("signature")
        );
        validate_acceptance_marker(&state).expect("marker validates");
    }

    #[test]
    fn v1_marker_still_validates_through_the_legacy_path() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "legacy receipt".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("legacy-marker".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        std::fs::write(
            super::gate_nonce_path_for_run_root(&state.run_root),
            "legacy-secret",
        )
        .expect("legacy nonce");
        let mut marker = AcceptanceMarker {
            schema_version: 1,
            run_id: state.run_id.clone(),
            status: "pass".to_string(),
            produced_by: "dr-gate".to_string(),
            issuer: String::new(),
            proof_kind: AcceptanceProofKind::LegacyUnknown,
            checked_at: Utc::now(),
            working_dir: state.working_dir.clone(),
            contained: false,
            sandbox_backend: String::new(),
            signature: String::new(),
            check_count: 0,
            checks: Vec::new(),
        };
        marker.signature =
            super::legacy_marker_signature(&state.run_root, &marker).expect("legacy signature");
        let path = super::marker_path(&state);
        std::fs::create_dir_all(path.parent().expect("proofs")).expect("proofs");
        std::fs::write(&path, serde_json::to_vec_pretty(&marker).expect("json")).expect("marker");

        let validated = validate_acceptance_marker(&state).expect("legacy validates");
        assert_eq!(
            validated.signature_strength(),
            super::AcceptanceSignatureStrength::LegacyWeak
        );
        assert!(!validated.is_native_gate_proof());
    }

    #[test]
    fn v1_marker_is_reported_as_legacy_not_verified() {
        let mut marker = super::v2_test_marker(std::path::Path::new("/tmp/legacy-strength"));
        marker.schema_version = 1;
        marker.issuer.clear();
        marker.proof_kind = AcceptanceProofKind::LegacyUnknown;

        assert_eq!(
            marker.signature_strength(),
            super::AcceptanceSignatureStrength::LegacyWeak
        );
        assert!(!marker.is_native_gate_proof());
    }

    #[test]
    fn signature_comparison_is_constant_time() {
        let temp = TempDir::new().expect("tempdir");
        let marker = super::v2_test_marker(temp.path());
        let key = [7_u8; 32];
        let valid = super::v2_marker_signature(temp.path(), &marker, &key).expect("signature");
        let mut invalid = super::hex_decode(&valid).expect("hex");
        invalid[31] ^= 1;
        let mut tampered = marker;
        tampered.signature = super::hex_encode(&invalid);

        let err = super::verify_v2_marker_signature(temp.path(), &tampered, &key)
            .expect_err("hmac verify rejects");
        assert!(err.to_string().contains("signature is invalid"), "{err}");
    }

    #[test]
    fn marker_bytes_are_canonical_and_field_order_is_pinned() {
        let temp = TempDir::new().expect("tempdir");
        let marker = super::v2_test_marker(temp.path());
        let canonical =
            super::canonical_marker_bytes(temp.path(), &marker).expect("canonical bytes");
        let labels = super::canonical_marker_field_labels(&canonical).expect("labels");
        assert_eq!(
            labels,
            [
                "schema_version",
                "run_id",
                "status",
                "produced_by",
                "issuer",
                "proof_kind",
                "checked_at",
                "working_dir",
                "contained",
                "sandbox_backend",
                "check_count",
                "checks",
                "tamper",
                "campaign_rollup",
            ]
        );
    }

    #[test]
    fn no_parent_repair_preserves_legacy_v2_canonical_bytes() {
        let temp = TempDir::new().expect("tempdir");
        let marker = super::v2_test_marker(temp.path());

        let canonical =
            super::canonical_marker_bytes(temp.path(), &marker).expect("canonical bytes");

        assert_eq!(canonical, legacy_v2_canonical_bytes(&marker));
    }

    #[test]
    fn adding_parent_repair_proof_bytes_invalidates_an_existing_marker_hmac() {
        let temp = TempDir::new().expect("tempdir");
        let key = [17_u8; 32];
        let mut marker = super::v2_test_marker(temp.path());
        let baseline = super::canonical_marker_bytes(temp.path(), &marker).expect("baseline bytes");
        marker.signature =
            super::v2_marker_signature(temp.path(), &marker, &key).expect("baseline signature");
        super::verify_v2_marker_signature(temp.path(), &marker, &key).expect("baseline validates");

        let proofs = temp.path().join("proofs");
        std::fs::create_dir_all(&proofs).expect("proofs");
        for (path, contents) in [
            (
                super::parent_repair_manifest_path_for_run_root(temp.path()),
                br#"{"round":1,"kind":"manifest"}"#.as_slice(),
            ),
            (
                super::parent_repair_candidate_path_for_run_root(temp.path()),
                br#"{"round":1,"kind":"candidate"}"#.as_slice(),
            ),
        ] {
            std::fs::write(&path, contents).expect("repair proof");
            let changed =
                super::canonical_marker_bytes(temp.path(), &marker).expect("changed bytes");
            assert_ne!(changed, baseline);
            let error = super::verify_v2_marker_signature(temp.path(), &marker, &key)
                .expect_err("added repair proof must invalidate prior signature");
            assert!(
                error.to_string().contains("signature is invalid"),
                "{error}"
            );

            std::fs::remove_file(&path).expect("remove repair proof");
            assert_eq!(
                super::canonical_marker_bytes(temp.path(), &marker).expect("restored bytes"),
                baseline
            );
            super::verify_v2_marker_signature(temp.path(), &marker, &key)
                .expect("removing the post-sign addition restores the original bytes");
        }
    }

    #[test]
    fn mutating_or_removing_signed_parent_repair_proof_bytes_invalidates_the_hmac() {
        let temp = TempDir::new().expect("tempdir");
        let proofs = temp.path().join("proofs");
        std::fs::create_dir_all(&proofs).expect("proofs");
        let manifest_path = super::parent_repair_manifest_path_for_run_root(temp.path());
        let candidate_path = super::parent_repair_candidate_path_for_run_root(temp.path());
        let manifest = br#"{"round":1,"kind":"manifest"}"#.to_vec();
        let candidate = br#"{"round":1,"kind":"candidate"}"#.to_vec();
        std::fs::write(&manifest_path, &manifest).expect("manifest");
        std::fs::write(&candidate_path, &candidate).expect("candidate");

        let key = [23_u8; 32];
        let mut marker = super::v2_test_marker(temp.path());
        let signed_bytes =
            super::canonical_marker_bytes(temp.path(), &marker).expect("signed bytes");
        marker.signature =
            super::v2_marker_signature(temp.path(), &marker, &key).expect("signature");
        super::verify_v2_marker_signature(temp.path(), &marker, &key)
            .expect("repair-bound marker validates");

        for (path, original, changed) in [
            (
                manifest_path,
                manifest,
                br#"{"round":2,"kind":"manifest"}"#.to_vec(),
            ),
            (
                candidate_path,
                candidate,
                br#"{"round":2,"kind":"candidate"}"#.to_vec(),
            ),
        ] {
            std::fs::write(&path, &changed).expect("mutate repair proof");
            assert_ne!(
                super::canonical_marker_bytes(temp.path(), &marker).expect("mutated bytes"),
                signed_bytes
            );
            let error = super::verify_v2_marker_signature(temp.path(), &marker, &key)
                .expect_err("mutated repair proof must invalidate signature");
            assert!(
                error.to_string().contains("signature is invalid"),
                "{error}"
            );

            std::fs::write(&path, &original).expect("restore repair proof");
            assert_eq!(
                super::canonical_marker_bytes(temp.path(), &marker).expect("restored bytes"),
                signed_bytes
            );
            super::verify_v2_marker_signature(temp.path(), &marker, &key)
                .expect("restored repair proof validates");

            std::fs::remove_file(&path).expect("remove repair proof");
            assert_ne!(
                super::canonical_marker_bytes(temp.path(), &marker).expect("removed bytes"),
                signed_bytes
            );
            let error = super::verify_v2_marker_signature(temp.path(), &marker, &key)
                .expect_err("removed repair proof must invalidate signature");
            assert!(
                error.to_string().contains("signature is invalid"),
                "{error}"
            );

            std::fs::write(&path, &original).expect("restore removed repair proof");
            assert_eq!(
                super::canonical_marker_bytes(temp.path(), &marker).expect("fully restored bytes"),
                signed_bytes
            );
            super::verify_v2_marker_signature(temp.path(), &marker, &key)
                .expect("fully restored repair proof validates");
        }
    }

    #[test]
    fn editing_contained_after_signing_invalidates_the_marker() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "containment is authority".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "seatbelt".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("containment-bound".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        let key = super::read_gate_key(&paths, &state.run_id).expect("key");
        let mut marker = super::write_native_acceptance_marker_with_results_and_key(
            &state.run_root,
            state.run_id.clone(),
            state.working_dir.clone(),
            Vec::new(),
            &key,
            super::AcceptanceContainment::contained("seatbelt"),
        )
        .expect("marker");
        marker.contained = false;
        std::fs::write(
            super::marker_path(&state),
            serde_json::to_vec_pretty(&marker).expect("json"),
        )
        .expect("rewrite");

        let err = validate_acceptance_marker(&state).expect_err("containment edit rejected");
        assert!(err.to_string().contains("signature"), "{err}");
    }

    #[test]
    fn marker_records_the_resolved_backend_not_the_requested_one() {
        let temp = TempDir::new().expect("tempdir");
        let marker = super::write_native_acceptance_marker_with_results_and_key(
            temp.path(),
            "resolved-backend".to_string(),
            temp.path().join("working"),
            Vec::new(),
            &[9_u8; 32],
            super::AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("marker");

        assert!(marker.contained);
        assert_eq!(marker.sandbox_backend, "sandbox-exec");
    }

    #[test]
    fn sandbox_fallback_to_none_records_contained_false() {
        let temp = TempDir::new().expect("tempdir");
        let marker = super::write_native_acceptance_marker_with_results_and_key(
            temp.path(),
            "fallback-none".to_string(),
            temp.path().join("working"),
            Vec::new(),
            &[11_u8; 32],
            super::AcceptanceContainment::uncontained("none"),
        )
        .expect("marker");

        assert!(!marker.contained);
        assert_eq!(marker.sandbox_backend, "none");
    }

    #[test]
    fn synthetic_marker_is_not_native_gate_proof() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "synthetic controller result".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("synthetic-proof".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        let marker = super::write_acceptance_marker(
            &state.run_root,
            state.run_id.clone(),
            state.working_dir.clone(),
            1,
        )
        .expect("synthetic marker");

        assert_eq!(
            marker.proof_kind,
            super::AcceptanceProofKind::SyntheticController
        );
        assert!(!marker.is_native_gate_proof());
    }

    // ---- P5: detection wired into compiled_acceptance_checks + spec persisted ----

    #[test]
    fn compiled_checks_persist_generated_spec_for_node() {
        let temp = TempDir::new().expect("tempdir");
        let run_root = temp.path().join("run");
        std::fs::create_dir_all(&run_root).expect("run_root");
        let working = temp.path().join("work");
        std::fs::create_dir_all(&working).expect("work");
        std::fs::write(
            working.join("package.json"),
            r#"{"scripts":{"test":"jest"}}"#,
        )
        .expect("package.json");

        let checks = super::compiled_acceptance_checks(&run_root, &working).expect("compile");
        assert!(matches!(
            checks.as_slice(),
            [super::AcceptanceCheck::Shell { command, .. }] if command == "npm test"
        ));

        let spec_path = super::acceptance_spec_path_for_run_root(&run_root);
        let written = std::fs::read_to_string(&spec_path).expect("generated spec written");
        assert!(written.contains("# generated by deadreckon detect: node"));
        assert!(written.contains("npm test"));
    }

    #[test]
    fn operator_spec_overrides_detection() {
        let temp = TempDir::new().expect("tempdir");
        let run_root = temp.path().join("run");
        std::fs::create_dir_all(&run_root).expect("run_root");
        let working = temp.path().join("work");
        std::fs::create_dir_all(&working).expect("work");
        // Node tree, but operator already wrote a different spec.
        std::fs::write(
            working.join("package.json"),
            r#"{"scripts":{"test":"jest"}}"#,
        )
        .expect("package.json");
        let spec_path = super::acceptance_spec_path_for_run_root(&run_root);
        let operator =
            "# operator\nchecks:\n- kind: shell\n  command: ./my-checks.sh\n  must_pass: true\n";
        std::fs::write(&spec_path, operator).expect("operator spec");

        let checks = super::compiled_acceptance_checks(&run_root, &working).expect("compile");
        assert!(matches!(
            checks.as_slice(),
            [super::AcceptanceCheck::Shell { command, .. }] if command == "./my-checks.sh"
        ));
        // The operator spec is not overwritten by detection.
        assert_eq!(std::fs::read_to_string(&spec_path).expect("spec"), operator);
    }

    #[test]
    fn generated_spec_roundtrips_through_parse_acceptance_checks() {
        let temp = TempDir::new().expect("tempdir");
        let run_root = temp.path().join("run");
        std::fs::create_dir_all(&run_root).expect("run_root");
        let working = temp.path().join("work");
        std::fs::create_dir_all(&working).expect("work");
        std::fs::write(working.join("go.mod"), "module example.com/x\n").expect("go.mod");

        let compiled = super::compiled_acceptance_checks(&run_root, &working).expect("compile");
        // Re-reading the persisted spec yields the same checks.
        let reparsed = super::compiled_acceptance_checks(&run_root, &working).expect("reparse");
        assert_eq!(compiled, reparsed);
        assert!(matches!(
            reparsed.as_slice(),
            [super::AcceptanceCheck::Shell { command, .. }] if command == "go test ./..."
        ));
    }

    // ---- P6: dr-gate default eval routes through default_checks_for ----

    #[test]
    fn dr_gate_default_eval_matches_compiled_checks_for_python() {
        let temp = TempDir::new().expect("tempdir");
        let working = temp.path().join("work");
        std::fs::create_dir_all(&working).expect("work");
        std::fs::write(working.join("pyproject.toml"), "[project]\nname = \"x\"\n")
            .expect("pyproject");
        std::fs::write(
            working.join("test_app.py"),
            "def test_ok():\n    assert True\n",
        )
        .expect("test file");

        let dr_gate = super::default_acceptance_checks(&working);
        let in_process = crate::acceptance_defaults::default_checks_for(
            &crate::acceptance_defaults::ProjectKind::Python,
            &working,
        );
        assert_eq!(dr_gate, in_process);
        assert!(matches!(
            dr_gate.as_slice(),
            [super::AcceptanceCheck::Shell { command, .. }] if command == "python -m pytest -q"
        ));
    }

    #[test]
    fn dr_gate_default_eval_node_runs_test_not_fileexists() {
        let temp = TempDir::new().expect("tempdir");
        let working = temp.path().join("work");
        std::fs::create_dir_all(&working).expect("work");
        std::fs::write(
            working.join("package.json"),
            r#"{"scripts":{"test":"jest"}}"#,
        )
        .expect("package.json");

        let checks = super::default_acceptance_checks(&working);
        // The dr-gate default path attempts the real test command, never the
        // hollow FileExists "working directory exists".
        assert!(matches!(
            checks.as_slice(),
            [super::AcceptanceCheck::Shell { command, .. }] if command == "npm test"
        ));
        assert!(
            !checks
                .iter()
                .any(|c| matches!(c, super::AcceptanceCheck::FileExists { .. }))
        );
    }

    #[test]
    fn acceptance_subprocesses_cannot_inherit_the_gate_key() {
        let command = super::gate_check_command("sh");
        let removed = command
            .get_envs()
            .filter_map(|(name, value)| value.is_none().then_some(name))
            .collect::<Vec<_>>();

        assert!(removed.contains(&std::ffi::OsStr::new(super::GATE_KEY_ENV)));
        assert!(removed.contains(&std::ffi::OsStr::new(super::GATE_CONTAINED_ENV)));
        assert!(removed.contains(&std::ffi::OsStr::new(super::GATE_SANDBOX_BACKEND_ENV)));
    }

    #[test]
    fn rejects_agent_written_marker_with_wrong_run_id() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "gate".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
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
        let proofs = state.run_root.join("proofs");
        std::fs::create_dir_all(&proofs).expect("proofs");
        let marker = AcceptanceMarker {
            schema_version: 1,
            run_id: "wrong-run".to_string(),
            status: "pass".to_string(),
            produced_by: "agent".to_string(),
            issuer: String::new(),
            proof_kind: AcceptanceProofKind::LegacyUnknown,
            checked_at: Utc::now(),
            working_dir: state.working_dir.clone(),
            contained: false,
            sandbox_backend: String::new(),
            signature: "forged".to_string(),
            check_count: 0,
            checks: Vec::new(),
        };
        std::fs::write(
            proofs.join(ACCEPTANCE_MARKER),
            serde_json::to_vec_pretty(&marker).expect("json"),
        )
        .expect("write marker");
        let err = validate_acceptance_marker(&state).expect_err("reject");
        assert!(err.to_string().contains("does not match"));
    }

    #[test]
    fn acceptance_yaml_parsed_and_evaluated() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "spec".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
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
        std::fs::write(state.working_dir.join("notes.md"), "dead reckoning").expect("notes");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            r#"
name: fixture
checks:
  - kind: file_exists
    path: "{working_dir}/notes.md"
  - kind: content_match
    path: "{working_dir}/notes.md"
    pattern: "dead reckoning"
"#,
        )
        .expect("spec");
        let results =
            super::evaluate_acceptance(&state.run_root, &state.working_dir).expect("acceptance");
        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.passed));
    }

    #[test]
    fn acceptance_yaml_required_optional_and_shell_evaluated() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "spec-v2".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
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
        std::fs::write(state.working_dir.join("notes.md"), "dead reckoning").expect("notes");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            r#"
required:
  - file-exists: "{working_dir}/notes.md"
  - content-match:
      path: "{working_dir}/notes.md"
      pattern: "dead reckoning"
  - shell:
      command: "test -f notes.md"
optional:
  - shell: "exit 7"
tests:
  - "test -f notes.md"
"#,
        )
        .expect("spec");

        let results =
            super::evaluate_acceptance(&state.run_root, &state.working_dir).expect("acceptance");

        assert_eq!(results.len(), 5);
        assert!(
            results
                .iter()
                .filter(|result| result.must_pass)
                .all(|result| result.passed)
        );
        assert!(
            results
                .iter()
                .any(|result| !result.must_pass && !result.passed)
        );
    }

    #[test]
    fn acceptance_required_failure_blocks_optional_success() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "spec-fail".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
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
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            r#"
required:
  - file-exists: "{working_dir}/missing.txt"
optional:
  - shell: "exit 0"
"#,
        )
        .expect("spec");

        let err = super::evaluate_acceptance(&state.run_root, &state.working_dir)
            .expect_err("required failure");

        assert!(err.to_string().contains("acceptance check failed"));
    }

    #[test]
    fn acceptance_checks_collect_failure_evidence_without_short_circuiting() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "spec-fail-evidence".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
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
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            r#"
checks:
  - kind: shell
    command: "echo first-failed >&2; exit 4"
  - kind: shell
    command: "echo second-ran"
"#,
        )
        .expect("spec");

        let results =
            super::evaluate_acceptance_checks(&state.run_root, &state.working_dir).expect("checks");

        assert_eq!(results.len(), 2);
        assert!(!results[0].passed);
        assert!(
            results[0]
                .stderr
                .as_deref()
                .is_some_and(|stderr| stderr.contains("first-failed"))
        );
        assert!(results[1].passed);
        assert!(
            results[1]
                .stdout
                .as_deref()
                .is_some_and(|stdout| stdout.contains("second-ran"))
        );
    }

    #[test]
    fn acceptance_progress_jsonl_records_running_and_result_rows() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "spec-progress".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
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
        std::fs::write(state.working_dir.join("notes.md"), "dead reckoning").expect("notes");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            r#"
checks:
  - kind: file_exists
    path: "{working_dir}/notes.md"
  - kind: shell
    command: "test -f notes.md"
"#,
        )
        .expect("spec");

        let results =
            super::evaluate_acceptance_checks_with_progress(&state.run_root, &state.working_dir)
                .expect("checks");
        let raw = std::fs::read_to_string(super::acceptance_progress_path_for_run_root(
            &state.run_root,
        ))
        .expect("progress");
        let progress = raw
            .lines()
            .map(|line| serde_json::from_str::<super::AcceptanceProgressEntry>(line).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(results.len(), 2);
        assert!(
            progress
                .iter()
                .any(|entry| entry.status == "running" && entry.index == 1 && entry.total == 2),
            "{progress:?}"
        );
        assert_eq!(
            progress
                .iter()
                .filter(|entry| entry.result.as_ref().is_some_and(|result| result.passed))
                .count(),
            2
        );
        assert_eq!(
            progress.last().map(|entry| entry.status.as_str()),
            Some("passed")
        );
    }

    #[test]
    fn content_match_accepts_regex_patterns() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "regex".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
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
        std::fs::write(state.working_dir.join("app.txt"), "version 12").expect("app");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            r#"
checks:
  - kind: content_match
    path: "{working_dir}/app.txt"
    pattern: 'version \d+'
"#,
        )
        .expect("spec");

        let results =
            super::evaluate_acceptance(&state.run_root, &state.working_dir).expect("acceptance");

        assert!(results[0].passed);
    }

    #[test]
    fn marker_signature_includes_check_results() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "marker-checks".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
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
        let mut marker = super::write_acceptance_marker_with_results(
            &state.run_root,
            state.run_id.clone(),
            state.working_dir.clone(),
            vec![AcceptanceCheckResult {
                kind: "shell".to_string(),
                passed: true,
                must_pass: true,
                detail: "original".to_string(),
                command: None,
                cwd: None,
                duration_ms: None,
                stdout: None,
                stderr: None,
            }],
        )
        .expect("marker");
        marker.checks[0].detail = "tampered".to_string();
        std::fs::write(
            super::marker_path(&state),
            serde_json::to_vec_pretty(&marker).expect("json"),
        )
        .expect("tamper");

        let err = validate_acceptance_marker(&state).expect_err("tamper rejected");

        assert!(err.to_string().contains("signature"));
    }

    #[test]
    fn gate_refuse_writes_tamper_file_and_no_marker() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "gate refuse tamper".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "checks:\n  - kind: shell\n    command: \"cargo test || true\"\n",
        )
        .expect("spec");

        let err = super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect_err("refuse");
        let tamper = read_acceptance_tamper_for_run_root(&state.run_root)
            .expect("tamper")
            .expect("tamper record");

        assert!(err.to_string().contains("acceptance refused"));
        assert_eq!(tamper.verdict, AcceptanceTamperVerdict::Refuse);
        assert!(
            !super::marker_path(&state).exists(),
            "refused gate must not write marker"
        );
    }

    #[test]
    fn gate_caveat_writes_signed_marker_and_caveat_record() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "gate caveat tamper".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::write(state.working_dir.join("README.md"), "before\n").expect("readme");
        snapshot_working(&state, 0).expect("snapshot");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "checks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("spec");
        std::fs::write(state.working_dir.join("README.md"), "after\n").expect("edit readme");
        append_provenance(
            &state,
            &ProvenanceRecord {
                timestamp: Utc::now(),
                prompt_id: "p1".to_string(),
                model: "fixture".to_string(),
                tool_call_id: "tool".to_string(),
                session_id: "session".to_string(),
                files: vec![state.working_dir.join("README.md")],
            },
        )
        .expect("provenance");

        super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect("caveat signs");
        let tamper = read_acceptance_tamper_for_run_root(&state.run_root)
            .expect("tamper")
            .expect("tamper record");

        assert_eq!(tamper.verdict, AcceptanceTamperVerdict::Caveat);
        assert!(
            tamper
                .caveats
                .iter()
                .any(|caveat| caveat.contains("README.md")),
            "{tamper:?}"
        );
        validate_acceptance_marker(&state).expect("signed caveat validates");
    }

    #[test]
    fn forged_tamper_file_fails_marker_signature_validation() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "forged tamper".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::write(state.working_dir.join("README.md"), "before\n").expect("readme");
        snapshot_working(&state, 0).expect("snapshot");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "checks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("spec");
        std::fs::write(state.working_dir.join("README.md"), "after\n").expect("edit readme");
        append_provenance(
            &state,
            &ProvenanceRecord {
                timestamp: Utc::now(),
                prompt_id: "p1".to_string(),
                model: "fixture".to_string(),
                tool_call_id: "tool".to_string(),
                session_id: "session".to_string(),
                files: vec![state.working_dir.join("README.md")],
            },
        )
        .expect("provenance");
        super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect("caveat signs");
        let tamper_path = crate::tamper::acceptance_tamper_path_for_run_root(&state.run_root);
        std::fs::write(
            tamper_path,
            r#"{"schema_version":1,"run_id":"forged","verdict":"clean"}"#,
        )
        .expect("forge");

        let err = validate_acceptance_marker(&state).expect_err("tamper rejected");

        assert!(err.to_string().contains("signature"));
    }

    #[test]
    fn clean_run_signs_and_validates_unchanged() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "clean gate".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::write(state.working_dir.join("README.md"), "ok\n").expect("readme");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "checks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("spec");

        super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect("clean signs");
        let tamper = read_acceptance_tamper_for_run_root(&state.run_root)
            .expect("tamper")
            .expect("tamper record");

        assert_eq!(tamper.verdict, AcceptanceTamperVerdict::Clean);
        validate_acceptance_marker(&state).expect("clean marker validates");
    }

    #[test]
    fn gate_signature_unchanged_with_all_seams_active() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "seam sidecars do not bind gate".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::write(state.working_dir.join("README.md"), "ok\n").expect("readme");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "checks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("spec");
        std::fs::write(
            state.run_root.join("seams.json"),
            r#"{"schema_version":1,"no_seams":false,"kinds":{"policy":{"source":"external"},"catalog":{"source":"external"},"hooks":{"source":"external"},"event_sink":{"source":"external"}}}"#,
        )
        .expect("seams");
        std::fs::write(
            state.run_root.join("compaction.jsonl"),
            "{\"schema_version\":1,\"turn\":2,\"context_window\":200000}\n",
        )
        .expect("compaction");

        let marker = super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect("clean signs with seam sidecars");
        let signature = marker.signature;
        validate_acceptance_marker(&state).expect("marker validates before sidecar edit");

        std::fs::write(state.run_root.join("seams.json"), "{\"tampered\":true}\n")
            .expect("edit seams");
        std::fs::write(
            state.run_root.join("compaction.jsonl"),
            "{\"schema_version\":1,\"turn\":99}\n",
        )
        .expect("edit compaction");
        let validated =
            validate_acceptance_marker(&state).expect("sidecars are not signature inputs");
        let expected = super::marker_signature(&state.run_root, &validated).expect("signature");

        assert_eq!(validated.signature, signature);
        assert_eq!(validated.signature, expected);
    }

    #[test]
    fn self_attest_attempt_fails() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "forged".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
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
        std::fs::write(
            super::gate_nonce_path_for_run_root(&state.run_root),
            "legacy-self-attest-secret",
        )
        .expect("legacy nonce");
        let proofs = state.run_root.join("proofs");
        std::fs::create_dir_all(&proofs).expect("proofs");
        let marker = AcceptanceMarker {
            schema_version: 1,
            run_id: state.run_id.clone(),
            status: "pass".to_string(),
            produced_by: "dr-gate".to_string(),
            issuer: String::new(),
            proof_kind: AcceptanceProofKind::LegacyUnknown,
            checked_at: Utc::now(),
            working_dir: state.working_dir.clone(),
            contained: false,
            sandbox_backend: String::new(),
            signature: "agent-forged".to_string(),
            check_count: 1,
            checks: Vec::new(),
        };
        std::fs::write(
            proofs.join(ACCEPTANCE_MARKER),
            serde_json::to_vec_pretty(&marker).expect("json"),
        )
        .expect("write marker");
        let err = validate_acceptance_marker(&state).expect_err("reject forged");
        assert!(err.to_string().contains("signature"));
    }

    #[test]
    fn deleting_a_covered_test_file_must_not_yield_a_signed_marker() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "deleted test hollow pass".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::create_dir_all(state.working_dir.join("src")).expect("src");
        std::fs::create_dir_all(state.working_dir.join("tests")).expect("tests");
        std::fs::write(
            state.working_dir.join("Cargo.toml"),
            "[package]\nname = \"gate_fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n",
        )
        .expect("cargo");
        std::fs::write(
            state.working_dir.join("src/lib.rs"),
            "pub fn ok() -> bool { true }\n",
        )
        .expect("lib");
        std::fs::write(
            state.working_dir.join("tests/auth_test.rs"),
            "#[test]\nfn expired_token_is_rejected() { assert!(!gate_fixture::ok()); }\n",
        )
        .expect("test");
        snapshot_working(&state, 0).expect("snapshot");

        std::fs::remove_file(state.working_dir.join("tests/auth_test.rs")).expect("delete test");

        let err = super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect_err("deleted covered test refused");

        assert!(err.to_string().contains("acceptance refused"), "{err}");
        assert!(
            !super::marker_path(&state).exists(),
            "deleted covered test must not produce a signed marker"
        );
    }

    #[test]
    fn editing_acceptance_yaml_during_run_must_not_yield_a_signed_marker() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "edited acceptance hollow pass".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::write(state.working_dir.join("README.md"), "ok\n").expect("readme");
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "name: edited\nchecks:\n  - kind: shell\n    command: \"true\"\n    cwd: \"{working_dir}\"\n",
        )
        .expect("acceptance");
        append_provenance(
            &state,
            &ProvenanceRecord {
                timestamp: Utc::now(),
                prompt_id: "p1".to_string(),
                model: "fixture".to_string(),
                tool_call_id: "tool".to_string(),
                session_id: "session".to_string(),
                files: vec![state.run_root.join("acceptance.yaml")],
            },
        )
        .expect("provenance");

        let err = super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect_err("edited acceptance refused");

        assert!(err.to_string().contains("acceptance refused"), "{err}");
        assert!(
            !super::marker_path(&state).exists(),
            "edited acceptance.yaml must not produce a signed marker"
        );
    }

    #[test]
    fn suppression_pattern_in_shell_check_must_not_yield_a_signed_marker() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "suppressed shell hollow pass".to_string(),
                cwd: temp.path().to_path_buf(),
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
        std::fs::write(
            state.run_root.join("acceptance.yaml"),
            "name: suppressed\nchecks:\n  - kind: shell\n    command: \"cargo test || true\"\n    cwd: \"{working_dir}\"\n",
        )
        .expect("acceptance");

        let err = super::run_acceptance_gate_and_write_marker(
            &state.run_root,
            &state.run_id,
            &state.working_dir,
        )
        .expect_err("suppression refused");

        assert!(err.to_string().contains("acceptance refused"), "{err}");
        assert!(
            !super::marker_path(&state).exists(),
            "suppression-pattern check must not produce a signed marker"
        );
    }
}
