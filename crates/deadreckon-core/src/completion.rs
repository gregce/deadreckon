//! Cryptographically bound two-key completion receipts.

use std::fs;
use std::path::{Path, PathBuf};

use deadreckon_protocol::{
    CompletionProofKind, CompletionReceipt, CompletionReceiptIssuer, GoalCoverageStatus,
    JobAuthority, JobOutcome, JobSchemaVersion, SemanticDecision, SemanticJudgment, StopReason,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::flight::{build_working_file_index, sha256_file, sha256_text};
use crate::gate::{AcceptanceMarker, read_gate_key, validate_acceptance_marker};
use crate::job::load_job;
use crate::paths::DeadreckonPaths;
use crate::state::{PipelineState, atomic_write_json};

const RECEIPT_MAGIC: &[u8] = b"deadreckon.completion-receipt.v1\0";
pub const SEMANTIC_JUDGMENT_JSON: &str = "proofs/semantic-judgment.json";

pub fn seal_completion_receipt(
    paths: &DeadreckonPaths,
    state: &PipelineState,
    authority: &JobAuthority,
    marker: &AcceptanceMarker,
    judgment: &SemanticJudgment,
) -> Result<CompletionReceipt> {
    let job = load_job(paths, authority.job_id.as_ref())?;
    if job.job_id != authority.job_id || state.run_id != authority.run_id.as_ref() {
        return Err(completion_error(
            authority.job_id.as_ref(),
            "job, authority, and run identities do not agree",
        ));
    }
    let validated_marker = validate_acceptance_marker(state)?;
    if validated_marker != *marker || !marker.is_native_gate_proof() {
        return Err(completion_error(
            authority.job_id.as_ref(),
            "deterministic proof is not a validated native dr-gate marker",
        ));
    }
    if !marker.contained || marker.sandbox_backend == "none" {
        return Err(completion_error(
            authority.job_id.as_ref(),
            "strict job completion requires a contained deterministic gate",
        ));
    }
    if judgment.job_id != authority.job_id
        || judgment.run_id != authority.run_id
        || judgment.decision != SemanticDecision::Achieved
    {
        return Err(completion_error(
            authority.job_id.as_ref(),
            "semantic judgment is not an achieved decision for this job and run",
        ));
    }
    validate_achieved_judgment(judgment, state.run_id.as_str())?;

    let authority_path = paths.job_authority(authority.job_id.as_ref());
    let launch_path = paths.job_launch_plan(authority.job_id.as_ref());
    let marker_path = crate::marker_path_for_run_root(&state.run_root);
    let semantic_path = state.run_root.join(SEMANTIC_JUDGMENT_JSON);
    verify_authority_inputs(&job, authority, &authority_path, &launch_path, state)?;
    let result_tree_sha256 = result_tree_hash(state)?;
    let mut receipt = CompletionReceipt {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: authority.job_id.clone(),
        run_id: authority.run_id.clone(),
        issued_at: chrono::Utc::now(),
        issuer: CompletionReceiptIssuer::DeadreckonSupervisor,
        proof_kind: CompletionProofKind::TwoKeyCompletion,
        outcome: JobOutcome::Verified,
        stop_reason: StopReason::Verified,
        authority_sha256: sha256_file(&authority_path)?,
        goal_sha256: authority.goal_sha256.clone(),
        contract_sha256: authority.contract_sha256.clone(),
        effective_policy_sha256: authority.effective_policy_sha256.clone(),
        launch_plan_sha256: authority.launch_plan_sha256.clone(),
        source_tree_sha256: authority.source_tree_sha256.clone(),
        source_revision: authority.source_revision.clone(),
        result_tree_sha256,
        result_revision: current_git_revision(&state.working_dir),
        deterministic_marker_sha256: sha256_file(&marker_path)?,
        semantic_judgment_sha256: sha256_file(&semantic_path)?,
        contained: marker.contained,
        sandbox_backend: marker.sandbox_backend.clone(),
        signature: String::new(),
    };
    let key = read_gate_key(paths, &state.run_id)?;
    receipt.signature = sign_receipt(&receipt, &key)?;
    atomic_write_json(&paths.job_receipt(authority.job_id.as_ref()), &receipt)?;
    Ok(receipt)
}

pub fn validate_completion_receipt(
    paths: &DeadreckonPaths,
    state: &PipelineState,
) -> Result<CompletionReceipt> {
    let receipt_path = paths.job_receipt(&state.run_id);
    let raw = fs::read(&receipt_path).with_path(&receipt_path)?;
    let receipt: CompletionReceipt = serde_json::from_slice(&raw).with_json_path(&receipt_path)?;
    if receipt.job_id.as_ref() != state.run_id || receipt.run_id.as_ref() != state.run_id {
        return Err(completion_error(
            &state.run_id,
            "receipt identity does not match the requested run",
        ));
    }
    if receipt.outcome != JobOutcome::Verified
        || receipt.stop_reason != StopReason::Verified
        || receipt.proof_kind != CompletionProofKind::TwoKeyCompletion
        || receipt.issuer != CompletionReceiptIssuer::DeadreckonSupervisor
    {
        return Err(completion_error(
            &state.run_id,
            "receipt is not a supervisor-issued two-key verified result",
        ));
    }

    let authority_path = paths.job_authority(&state.run_id);
    let launch_path = paths.job_launch_plan(&state.run_id);
    let authority_raw = fs::read(&authority_path).with_path(&authority_path)?;
    let authority: JobAuthority =
        serde_json::from_slice(&authority_raw).with_json_path(&authority_path)?;
    let job = load_job(paths, &state.run_id)?;
    verify_authority_inputs(&job, &authority, &authority_path, &launch_path, state)?;
    require_digest(
        &receipt.authority_sha256,
        &sha256_file(&authority_path)?,
        "authority",
        &state.run_id,
    )?;
    require_digest(
        &receipt.launch_plan_sha256,
        &sha256_file(&launch_path)?,
        "launch plan",
        &state.run_id,
    )?;
    require_digest(
        &receipt.deterministic_marker_sha256,
        &sha256_file(&crate::marker_path_for_run_root(&state.run_root))?,
        "deterministic marker",
        &state.run_id,
    )?;
    let semantic_path = state.run_root.join(SEMANTIC_JUDGMENT_JSON);
    require_digest(
        &receipt.semantic_judgment_sha256,
        &sha256_file(&semantic_path)?,
        "semantic judgment",
        &state.run_id,
    )?;
    let semantic_raw = fs::read(&semantic_path).with_path(&semantic_path)?;
    let judgment: SemanticJudgment =
        serde_json::from_slice(&semantic_raw).with_json_path(&semantic_path)?;
    if judgment.decision != SemanticDecision::Achieved
        || judgment.job_id != receipt.job_id
        || judgment.run_id != receipt.run_id
    {
        return Err(completion_error(
            &state.run_id,
            "semantic judgment no longer records achieved for this job",
        ));
    }
    validate_achieved_judgment(&judgment, &state.run_id)?;
    let marker = validate_acceptance_marker(state)?;
    if !marker.is_native_gate_proof()
        || marker.contained != receipt.contained
        || marker.sandbox_backend != receipt.sandbox_backend
    {
        return Err(completion_error(
            &state.run_id,
            "deterministic proof or containment does not match the receipt",
        ));
    }
    require_digest(
        &receipt.result_tree_sha256,
        &result_tree_hash(state)?,
        "result tree",
        &state.run_id,
    )?;
    let key = read_gate_key(paths, &state.run_id)?;
    verify_receipt_signature(&receipt, &key)?;
    Ok(receipt)
}

fn validate_achieved_judgment(judgment: &SemanticJudgment, run_id: &str) -> Result<()> {
    if judgment.goal_coverage.is_empty()
        || !judgment.missing.is_empty()
        || judgment.goal_coverage.iter().any(|coverage| {
            coverage.status != GoalCoverageStatus::Met || coverage.evidence.is_empty()
        })
    {
        return Err(completion_error(
            run_id,
            "semantic achieved judgment lacks complete evidence-backed goal coverage",
        ));
    }
    Ok(())
}

fn verify_authority_inputs(
    job: &deadreckon_protocol::Job,
    authority: &JobAuthority,
    authority_path: &Path,
    launch_path: &Path,
    state: &PipelineState,
) -> Result<()> {
    let job_id = authority.job_id.as_ref();
    require_digest(
        &authority.goal_sha256,
        &sha256_text(&job.goal),
        "approved goal",
        job_id,
    )?;
    require_digest(
        &authority.launch_plan_sha256,
        &sha256_file(launch_path)?,
        "approved launch plan",
        job_id,
    )?;
    require_digest(
        &job.launch_plan_sha256,
        &authority.launch_plan_sha256,
        "job launch authority",
        job_id,
    )?;
    require_digest(
        &job.authority_sha256,
        &sha256_file(authority_path)?,
        "job authority",
        job_id,
    )?;
    let policy_sha256 = sha256_text(&serde_json::to_string(&job.policy).map_err(|source| {
        DeadreckonError::Json {
            path: PathBuf::from("job policy"),
            source,
        }
    })?);
    require_digest(
        &authority.effective_policy_sha256,
        &policy_sha256,
        "approved effective policy",
        job_id,
    )?;
    if authority.semantic_judge_mode != job.policy.semantic_judge {
        return Err(completion_error(
            job_id,
            "semantic judge policy changed after approval",
        ));
    }
    let contract_path = crate::acceptance_spec_path_for_run_root(&state.run_root);
    require_digest(
        &authority.contract_sha256,
        &sha256_file(&contract_path)?,
        "approved contract",
        job_id,
    )
}

fn result_tree_hash(state: &PipelineState) -> Result<String> {
    let mut index = build_working_file_index(&state.working_dir)?;
    // Promotion adds DeadReckon's own library manifest after the result was
    // sealed, and finish appends its delivery ledger. Both are lifecycle
    // metadata, not agent output, so the same receipt remains valid before
    // and after those operations.
    if state.promoted_library_dir.as_deref() == Some(state.working_dir.as_path()) {
        index.files.remove(Path::new("manifest.json"));
        index.files.remove(Path::new(".materialized-to"));
    }
    Ok(index.tree_hash())
}

fn sign_receipt(receipt: &CompletionReceipt, key: &[u8]) -> Result<String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
        completion_error(
            receipt.job_id.as_ref(),
            "HMAC-SHA-256 refused the receipt key",
        )
    })?;
    mac.update(&canonical_receipt_bytes(receipt)?);
    Ok(hex_encode(&mac.finalize().into_bytes()))
}

fn verify_receipt_signature(receipt: &CompletionReceipt, key: &[u8]) -> Result<()> {
    let signature = hex_decode(&receipt.signature).map_err(|detail| {
        completion_error(
            receipt.job_id.as_ref(),
            &format!("receipt signature is not valid hex: {detail}"),
        )
    })?;
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
        completion_error(
            receipt.job_id.as_ref(),
            "HMAC-SHA-256 refused the receipt key",
        )
    })?;
    mac.update(&canonical_receipt_bytes(receipt)?);
    mac.verify_slice(&signature).map_err(|_| {
        completion_error(
            receipt.job_id.as_ref(),
            "receipt signature verification failed",
        )
    })
}

fn canonical_receipt_bytes(receipt: &CompletionReceipt) -> Result<Vec<u8>> {
    let mut unsigned = receipt.clone();
    unsigned.signature.clear();
    let encoded = serde_json::to_vec(&unsigned).map_err(|source| DeadreckonError::Json {
        path: PathBuf::from("receipt.json"),
        source,
    })?;
    let mut bytes = RECEIPT_MAGIC.to_vec();
    let len = u64::try_from(encoded.len())
        .map_err(|_| completion_error(receipt.job_id.as_ref(), "receipt is too large to sign"))?;
    bytes.extend_from_slice(&len.to_be_bytes());
    bytes.extend_from_slice(&encoded);
    Ok(bytes)
}

fn require_digest(expected: &str, actual: &str, label: &str, job_id: &str) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(completion_error(
            job_id,
            &format!("{label} digest changed (expected {expected}, found {actual})"),
        ))
    }
}

fn current_git_revision(working_dir: &Path) -> Option<String> {
    let output = crate::git::run_git(working_dir, &["rev-parse", "HEAD"]).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn completion_error(job_id: &str, detail: &str) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!(
        "completion receipt for {job_id} is invalid: {detail}; try: deadreckon verdict {job_id}"
    ))
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
        .map(|pair| Ok((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?))
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

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::Utc;
    use deadreckon_protocol::{
        AuthorityAcceptedBy, GoalCoverage, GoalCoverageStatus, Job, JobAuthority, JobId, JobPolicy,
        JobSchemaVersion, JobShape, RunId, SemanticDecision, SemanticJudgeMode, SemanticJudgment,
    };
    use tempfile::TempDir;

    use super::{seal_completion_receipt, validate_completion_receipt};
    use crate::flight::{build_working_file_index, sha256_file, sha256_text};
    use crate::gate::{
        AcceptanceCheckResult, AcceptanceContainment, read_gate_key,
        write_native_acceptance_marker_with_results_and_key,
    };
    use crate::job::write_job;
    use crate::paths::DeadreckonPaths;
    use crate::state::{RunOptions, atomic_write_json, create_run};

    struct Fixture {
        _temp: TempDir,
        paths: DeadreckonPaths,
        state: crate::PipelineState,
        authority: JobAuthority,
        marker: crate::AcceptanceMarker,
        judgment: SemanticJudgment,
    }

    fn fixture() -> Fixture {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "ship verified change".to_string(),
                cwd: source,
                sandbox: "sandbox-exec".to_string(),
                provider: Some("judge".to_string()),
                skill_name: "deadreckon".to_string(),
                max_spend_usd: Some(2.0),
                max_wall_seconds: Some(60.0),
                run_id: Some("job-1".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        fs::create_dir_all(&state.working_dir).expect("working");
        fs::write(state.working_dir.join("result.txt"), "verified\n").expect("result");
        let contract_path = crate::acceptance_spec_path_for_run_root(&state.run_root);
        fs::write(
            &contract_path,
            "name: result\nchecks:\n  - file_exists: result.txt\n",
        )
        .expect("contract");
        fs::create_dir_all(paths.job_dir("job-1")).expect("job dir");
        let launch_path = paths.job_launch_plan("job-1");
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
        };
        let authority = JobAuthority {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId("job-1".to_string()),
            run_id: RunId("job-1".to_string()),
            approved_at: Utc::now(),
            accepted_by: AuthorityAcceptedBy::Operator,
            goal_sha256: sha256_text("ship verified change"),
            contract_sha256: sha256_file(&contract_path).expect("contract digest"),
            effective_policy_sha256: sha256_text(
                &serde_json::to_string(&policy).expect("policy json"),
            ),
            launch_plan_sha256: sha256_file(&launch_path).expect("launch digest"),
            source_tree_sha256: build_working_file_index(&state.working_dir)
                .expect("source index")
                .tree_hash(),
            source_revision: None,
            sandbox_requested: "sandbox-exec".to_string(),
            semantic_judge_mode: SemanticJudgeMode::Required,
        };
        atomic_write_json(&paths.job_authority("job-1"), &authority).expect("authority");
        let job = Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId("job-1".to_string()),
            scope: state.scope.clone(),
            goal: state.goal.clone(),
            shape: JobShape::Single,
            created_at: Utc::now(),
            source_cwd: state.cwd.clone(),
            launch_plan_sha256: authority.launch_plan_sha256.clone(),
            authority_sha256: sha256_file(&paths.job_authority("job-1")).expect("authority digest"),
            policy,
        };
        write_job(&paths, &job).expect("job");
        let key = read_gate_key(&paths, "job-1").expect("key");
        let marker = write_native_acceptance_marker_with_results_and_key(
            &state.run_root,
            "job-1".to_string(),
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
            job_id: JobId("job-1".to_string()),
            run_id: RunId("job-1".to_string()),
            judged_at: Utc::now(),
            provider: "judge".to_string(),
            model: "judge-model".to_string(),
            decision: SemanticDecision::Achieved,
            summary: "the result satisfies the approved goal".to_string(),
            goal_coverage: vec![GoalCoverage {
                claim: "ship verified change".to_string(),
                status: GoalCoverageStatus::Met,
                evidence: vec!["deterministic-gate".to_string()],
            }],
            missing: Vec::new(),
            input_sha256: sha256_text("evidence"),
            spend_usd: 0.01,
        };
        atomic_write_json(
            &state.run_root.join(super::SEMANTIC_JUDGMENT_JSON),
            &judgment,
        )
        .expect("judgment");
        Fixture {
            _temp: temp,
            paths,
            state,
            authority,
            marker,
            judgment,
        }
    }

    #[test]
    fn achieved_plus_gate_pass_seals_two_key_receipt() {
        let fixture = fixture();
        let receipt = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        assert_eq!(receipt.signature.len(), 64);
        assert!(receipt.contained);
        assert_eq!(
            validate_completion_receipt(&fixture.paths, &fixture.state).expect("validate"),
            receipt
        );
    }

    #[test]
    fn achieved_without_evidence_backed_coverage_cannot_be_sealed() {
        let fixture = fixture();
        let mut unsupported = fixture.judgment.clone();
        unsupported.goal_coverage.clear();
        atomic_write_json(
            &fixture.state.run_root.join(super::SEMANTIC_JUDGMENT_JSON),
            &unsupported,
        )
        .expect("unsupported judgment");

        assert!(
            seal_completion_receipt(
                &fixture.paths,
                &fixture.state,
                &fixture.authority,
                &fixture.marker,
                &unsupported,
            )
            .expect_err("semantic parser invariant is repeated at receipt sealing")
            .to_string()
            .contains("lacks complete evidence-backed goal coverage")
        );
        assert!(!fixture.paths.job_receipt("job-1").exists());
    }

    #[test]
    fn strict_job_cannot_seal_an_uncontained_gate() {
        let fixture = fixture();
        let key = read_gate_key(&fixture.paths, &fixture.state.run_id).expect("key");
        let marker = write_native_acceptance_marker_with_results_and_key(
            &fixture.state.run_root,
            fixture.state.run_id.clone(),
            fixture.state.working_dir.clone(),
            fixture.marker.checks.clone(),
            &key,
            AcceptanceContainment::uncontained("none"),
        )
        .expect("uncontained marker");
        assert!(
            seal_completion_receipt(
                &fixture.paths,
                &fixture.state,
                &fixture.authority,
                &marker,
                &fixture.judgment,
            )
            .expect_err("strict jobs fail closed")
            .to_string()
            .contains("requires a contained deterministic gate")
        );
    }

    #[test]
    fn receipt_stays_valid_after_library_promotion_adds_its_manifest() {
        let mut fixture = fixture();
        seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        crate::promote_completed_run(&fixture.paths, &mut fixture.state).expect("promote");
        validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect("receipt survives owned manifest");
    }

    #[test]
    fn semantic_judgment_mutation_invalidates_receipt() {
        let fixture = fixture();
        seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        let mut changed = fixture.judgment.clone();
        changed.summary = "worker edited the proof".to_string();
        atomic_write_json(
            &fixture.state.run_root.join(super::SEMANTIC_JUDGMENT_JSON),
            &changed,
        )
        .expect("mutate");
        assert!(
            validate_completion_receipt(&fixture.paths, &fixture.state)
                .expect_err("mutation refused")
                .to_string()
                .contains("semantic judgment digest changed")
        );
    }

    #[test]
    fn backdated_result_mutation_invalidates_receipt() {
        let fixture = fixture();
        seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        fs::write(
            fixture.state.working_dir.join("result.txt"),
            "changed after sealing\n",
        )
        .expect("mutate result");
        assert!(
            validate_completion_receipt(&fixture.paths, &fixture.state)
                .expect_err("tree digest refused")
                .to_string()
                .contains("result tree digest changed")
        );
    }
}
