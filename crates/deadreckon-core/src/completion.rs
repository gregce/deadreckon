//! Cryptographically bound two-key completion receipts.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use deadreckon_protocol::{
    CompletionProofKind, CompletionReceipt, CompletionReceiptIssuer, GoalCoverageStatus,
    JobAuthority, JobOutcome, JobSchemaVersion, SemanticDecision, SemanticJudgment, StopReason,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::flight::{build_deliverable_file_index, sha256_file, sha256_text};
use crate::gate::{
    AcceptanceCheck, AcceptanceMarker, acceptance_checks_from_yaml, read_gate_key,
    validate_acceptance_marker,
};
use crate::job::load_job;
use crate::paths::DeadreckonPaths;
use crate::state::{PipelineState, atomic_write_json};
use crate::{
    CodebaseMode, WorkspacePathClass, classify_workspace_path, read_trusted_codebase_record,
};

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
    let result_revision = validate_worktree_result_boundary(state, authority, None)?;
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
        result_revision: result_revision.or_else(|| current_git_revision(&state.working_dir)),
        deterministic_marker_sha256: sha256_file(&marker_path)?,
        semantic_judgment_sha256: sha256_file(&semantic_path)?,
        contained: marker.contained,
        sandbox_backend: marker.sandbox_backend.clone(),
        signature: String::new(),
    };
    if let Some(revision) = receipt.result_revision.as_deref() {
        retain_signed_result_revision(state, authority.job_id.as_ref(), revision)?;
    }
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
    validate_worktree_result_boundary(state, &authority, Some(&receipt))?;
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
    let execution = job.policy.execution.as_ref().ok_or_else(|| {
        completion_error(
            job_id,
            "job predates immutable execution policy; strict completion is unavailable",
        )
    })?;
    if !execution.require_containment
        || execution.sandbox_requested != authority.sandbox_requested
        || !execution.tools.contains_key("bash")
        || !execution.tools.contains_key("write_file")
    {
        return Err(completion_error(
            job_id,
            "approved execution policy does not match containment authority",
        ));
    }
    let contract_path = crate::acceptance_spec_path_for_run_root(&state.run_root);
    require_digest(
        &authority.contract_sha256,
        &sha256_file(&contract_path)?,
        "approved contract",
        job_id,
    )?;
    validate_strict_contract(&contract_path, job_id)
}

fn validate_strict_contract(contract_path: &Path, job_id: &str) -> Result<()> {
    let raw = fs::read_to_string(contract_path).with_path(contract_path)?;
    let checks = acceptance_checks_from_yaml(&raw)?;
    if checks.is_empty() {
        return Err(completion_error(
            job_id,
            "the approved deterministic contract contains no checks",
        ));
    }
    let required = checks
        .iter()
        .filter(|check| check_is_required(check))
        .collect::<Vec<_>>();
    if required.is_empty() {
        return Err(completion_error(
            job_id,
            "the approved deterministic contract contains no required checks",
        ));
    }
    if required.iter().all(|check| {
        matches!(
            check,
            AcceptanceCheck::FileExists { path, .. } if path.trim() == "{working_dir}"
        )
    }) {
        return Err(completion_error(
            job_id,
            "the approved deterministic contract only proves that its pre-created working directory exists",
        ));
    }
    Ok(())
}

fn check_is_required(check: &AcceptanceCheck) -> bool {
    match check {
        AcceptanceCheck::CargoTest { must_pass, .. }
        | AcceptanceCheck::FileExists { must_pass, .. }
        | AcceptanceCheck::ContentMatch { must_pass, .. }
        | AcceptanceCheck::BuildSuccess { must_pass, .. }
        | AcceptanceCheck::Shell { must_pass, .. } => *must_pass,
    }
}

fn validate_worktree_result_boundary(
    state: &PipelineState,
    authority: &JobAuthority,
    receipt: Option<&CompletionReceipt>,
) -> Result<Option<String>> {
    let job_id = authority.job_id.as_ref();
    let record = read_trusted_codebase_record(&state.run_root).map_err(|error| {
        completion_error(
            job_id,
            &format!("result lifecycle record is unavailable or invalid: {error}"),
        )
    })?;
    if record.mode != CodebaseMode::Worktree {
        return Ok(None);
    }

    let git_root = record.source_git_root.as_deref().ok_or_else(|| {
        completion_error(
            job_id,
            "worktree result is missing its source Git repository",
        )
    })?;
    let branch = record
        .branch_name
        .as_deref()
        .ok_or_else(|| completion_error(job_id, "worktree result is missing its result branch"))?;
    let base = record
        .base_sha
        .as_deref()
        .ok_or_else(|| completion_error(job_id, "worktree result is missing its approved base"))?;
    let approved_base = authority.source_revision.as_deref().ok_or_else(|| {
        completion_error(
            job_id,
            "worktree result authority is missing its approved source revision",
        )
    })?;
    if base != approved_base {
        return Err(completion_error(
            job_id,
            "worktree result base does not match the approved source revision",
        ));
    }

    let result_revision = if let Some(revision) = git_revision(git_root, branch)? {
        revision
    } else {
        let sealed = receipt
            .and_then(|receipt| receipt.result_revision.as_deref())
            .filter(|_| state.promoted_library_dir.is_some())
            .ok_or_else(|| completion_error(job_id, "worktree result branch is unavailable"))?;
        git_revision(git_root, sealed)?.ok_or_else(|| {
            completion_error(
                job_id,
                "signed result revision is no longer available after worktree cleanup",
            )
        })?
    };

    let gitlinks = gitlink_paths(git_root, &result_revision)?;
    if !gitlinks.is_empty() {
        return Err(completion_error(
            job_id,
            &format!(
                "strict result contains unsupported Git submodule entries: {}; stop for review instead of signing an incomplete filesystem projection",
                display_paths(&gitlinks)
            ),
        ));
    }

    let non_deliverable = non_deliverable_git_history_paths(git_root, base, &result_revision)?;
    if !non_deliverable.is_empty() {
        return Err(completion_error(
            job_id,
            &format!(
                "result history contains non-deliverable paths relative to the approved base: {}",
                display_paths(&non_deliverable)
            ),
        ));
    }

    if let Some(worktree) = record.worktree_path.as_deref().filter(|path| path.is_dir()) {
        let dirty_deliverables = uncommitted_deliverable_paths(worktree)?;
        if !dirty_deliverables.is_empty() {
            return Err(completion_error(
                job_id,
                &format!(
                    "worktree has uncommitted deliverable changes: {}",
                    display_paths(&dirty_deliverables)
                ),
            ));
        }
        let worktree_revision = git_revision(worktree, "HEAD")?.ok_or_else(|| {
            completion_error(job_id, "worktree result has no committed HEAD revision")
        })?;
        if worktree_revision != result_revision {
            return Err(completion_error(
                job_id,
                "worktree HEAD does not match its result branch",
            ));
        }
    } else if state.promoted_library_dir.is_none() {
        return Err(completion_error(
            job_id,
            "worktree result cannot be sealed or validated without its isolated checkout",
        ));
    }

    let artifact_root = record
        .worktree_path
        .as_deref()
        .filter(|path| path.is_dir())
        .unwrap_or(&state.working_dir);
    require_filesystem_matches_git_result(
        state,
        git_root,
        approved_base,
        &result_revision,
        artifact_root,
    )?;

    if let Some(receipt) = receipt {
        if receipt.result_revision.as_deref() != Some(result_revision.as_str()) {
            return Err(completion_error(
                job_id,
                "result branch revision does not match the sealed receipt",
            ));
        }
    } else if current_git_revision(&state.working_dir).as_deref() != Some(result_revision.as_str())
    {
        return Err(completion_error(
            job_id,
            "the filesystem being sealed is not the committed result branch",
        ));
    }
    Ok(Some(result_revision))
}

/// Return non-deliverable paths touched anywhere in `base..result`.
///
/// Finish uses this independently of receipt issuance so legacy result
/// branches cannot smuggle provider-private, lifecycle, or runtime blobs
/// through commit history, even when a later commit deletes the path and leaves
/// the final tree clean.
pub fn non_deliverable_git_history_paths(
    git_root: &Path,
    base: &str,
    result: &str,
) -> Result<Vec<PathBuf>> {
    Ok(git_history_paths(git_root, base, result)?
        .into_iter()
        .filter(|path| classify_workspace_path(path) != WorkspacePathClass::Deliverable)
        .collect())
}

/// Return every path changed by the delivered first-parent history.
///
/// A merge commit is compared only with its first parent: paths already
/// present on the operator's target branch are not delivery side effects.
/// Walking every first-parent commit still exposes an unexpected path that a
/// later delivery commit deletes before the final tree is inspected.
pub fn git_delivery_history_paths(
    git_root: &Path,
    before: &str,
    delivered: &str,
) -> Result<Vec<PathBuf>> {
    require_git_ancestor(git_root, before, delivered)?;
    let range = format!("{before}..{delivered}");
    let output = crate::git::run_git(
        git_root,
        &["rev-list", "--first-parent", "--reverse", &range],
    )?;
    require_git_success(git_root, &output, "enumerate delivery commits")?;
    let mut paths = BTreeSet::new();
    for revision in String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let parent = format!("{revision}^1");
        let output = crate::git::run_git(
            git_root,
            &[
                "diff",
                "--name-only",
                "--no-renames",
                "-z",
                &parent,
                revision,
                "--",
            ],
        )?;
        require_git_success(git_root, &output, "inspect delivery commit paths")?;
        paths.extend(nul_paths(&output.stdout)?);
    }
    Ok(paths.into_iter().collect())
}

/// Return deliverable paths changed by `result` relative to `base`.
///
/// Rename detection is disabled deliberately: both the removed and added path
/// must be proven at delivery time.
pub fn deliverable_git_delta_paths(
    git_root: &Path,
    base: &str,
    result: &str,
) -> Result<Vec<PathBuf>> {
    require_git_ancestor(git_root, base, result)?;
    Ok(git_delta_paths(git_root, base, result)?
        .into_iter()
        .filter(|path| classify_workspace_path(path) == WorkspacePathClass::Deliverable)
        .collect())
}

fn require_git_ancestor(git_root: &Path, base: &str, result: &str) -> Result<()> {
    let output = crate::git::run_git(git_root, &["merge-base", "--is-ancestor", base, result])?;
    if output.status.success() {
        Ok(())
    } else {
        Err(DeadreckonError::InvalidInput(format!(
            "approved base {base} is not an ancestor of result {result}"
        )))
    }
}

fn git_revisions(git_root: &Path, base: &str, result: &str) -> Result<Vec<String>> {
    let range = format!("{base}..{result}");
    let output = crate::git::run_git(git_root, &["rev-list", "--reverse", &range])?;
    require_git_success(git_root, &output, "enumerate result commits")?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn git_history_paths(git_root: &Path, base: &str, result: &str) -> Result<Vec<PathBuf>> {
    require_git_ancestor(git_root, base, result)?;
    let revisions = git_revisions(git_root, base, result)?;
    let mut paths = BTreeSet::new();
    for revision in revisions {
        let output = crate::git::run_git(
            git_root,
            &[
                "diff-tree",
                "--root",
                "--no-commit-id",
                "--name-only",
                "--no-renames",
                "-r",
                "-m",
                "-z",
                &revision,
                "--",
            ],
        )?;
        require_git_success(git_root, &output, "inspect result commit paths")?;
        paths.extend(nul_paths(&output.stdout)?);
    }
    Ok(paths.into_iter().collect())
}

fn git_delta_paths(git_root: &Path, base: &str, result: &str) -> Result<Vec<PathBuf>> {
    let range = format!("{base}..{result}");
    let output = crate::git::run_git(
        git_root,
        &["diff", "--name-only", "--no-renames", "-z", &range, "--"],
    )?;
    require_git_success(git_root, &output, "inspect result branch paths")?;
    nul_paths(&output.stdout)
}

fn gitlink_paths(git_root: &Path, revision: &str) -> Result<Vec<PathBuf>> {
    let output = crate::git::run_git(
        git_root,
        &["ls-tree", "-r", "-z", "--full-tree", revision, "--"],
    )?;
    require_git_success(git_root, &output, "inspect result Git entry kinds")?;
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let tab = entry.iter().position(|byte| *byte == b'\t')?;
            entry[..tab]
                .starts_with(b"160000 ")
                .then_some(&entry[tab + 1..])
        })
        .map(path_from_git_bytes)
        .collect()
}

fn uncommitted_deliverable_paths(worktree: &Path) -> Result<Vec<PathBuf>> {
    let commands: [&[&str]; 4] = [
        &["diff", "--name-only", "--no-renames", "-z", "--"],
        &[
            "diff",
            "--cached",
            "--name-only",
            "--no-renames",
            "-z",
            "--",
        ],
        &["ls-files", "--others", "--exclude-standard", "-z", "--"],
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
            "--",
        ],
    ];
    let mut paths = BTreeSet::new();
    for args in commands {
        let output = crate::git::run_git(worktree, args)?;
        require_git_success(worktree, &output, "inspect uncommitted result paths")?;
        paths.extend(
            nul_paths(&output.stdout)?
                .into_iter()
                .filter(|path| classify_workspace_path(path) == WorkspacePathClass::Deliverable),
        );
    }
    paths.extend(masked_git_index_paths(worktree)?);
    Ok(paths.into_iter().collect())
}

fn masked_git_index_paths(worktree: &Path) -> Result<Vec<PathBuf>> {
    let output = crate::git::run_git(worktree, &["ls-files", "-v", "-z", "--"])?;
    require_git_success(worktree, &output, "inspect result index flags")?;
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|entry| entry.len() > 2 && entry[1] == b' ')
        .filter(|entry| entry[0] == b'S' || entry[0].is_ascii_lowercase())
        .map(|entry| path_from_git_bytes(&entry[2..]))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .filter(|path| classify_workspace_path(path) == WorkspacePathClass::Deliverable)
        .collect())
}

fn require_filesystem_matches_git_result(
    state: &PipelineState,
    git_root: &Path,
    base_revision: &str,
    revision: &str,
    artifact_root: &Path,
) -> Result<()> {
    let materialized = tempfile::TempDir::new().map_err(|source| DeadreckonError::Io {
        path: std::env::temp_dir(),
        source,
    })?;
    let index_path = materialized.path().join("index");
    let tree_path = materialized.path().join("tree");
    fs::create_dir_all(&tree_path).with_path(&tree_path)?;

    let mut read_tree = crate::git::git_command(git_root, &["read-tree", revision]);
    read_tree.env("GIT_INDEX_FILE", &index_path);
    let output = read_tree.output().map_err(|source| DeadreckonError::Io {
        path: git_root.to_path_buf(),
        source,
    })?;
    require_git_success(git_root, &output, "materialize signed result index")?;
    refuse_filtered_result_entries(git_root, &index_path)?;
    if base_revision != revision {
        let base_index_path = materialized.path().join("base-index");
        let mut read_base = crate::git::git_command(git_root, &["read-tree", base_revision]);
        read_base.env("GIT_INDEX_FILE", &base_index_path);
        let output = read_base.output().map_err(|source| DeadreckonError::Io {
            path: git_root.to_path_buf(),
            source,
        })?;
        require_git_success(git_root, &output, "materialize approved base index")?;
        refuse_filtered_result_entries(git_root, &base_index_path)?;
    }

    let mut prefix = OsString::from("--prefix=");
    prefix.push(&tree_path);
    prefix.push(std::path::MAIN_SEPARATOR.to_string());
    let mut checkout = crate::git::git_command(git_root, &["checkout-index", "--all", "--force"]);
    checkout.env("GIT_INDEX_FILE", &index_path).arg(prefix);
    let output = checkout.output().map_err(|source| DeadreckonError::Io {
        path: git_root.to_path_buf(),
        source,
    })?;
    require_git_success(git_root, &output, "materialize signed result tree")?;

    let committed = build_deliverable_file_index(&tree_path)?;
    let actual = result_deliverable_index(state, artifact_root)?;
    if committed == actual {
        return Ok(());
    }
    let paths = differing_index_paths(&committed, &actual);
    Err(completion_error(
        &state.run_id,
        &format!(
            "result filesystem does not exactly match signed Git revision {revision}: {}",
            display_paths(&paths)
        ),
    ))
}

fn refuse_filtered_result_entries(git_root: &Path, index_path: &Path) -> Result<()> {
    let mut list = crate::git::git_command(git_root, &["ls-files", "-z", "--"]);
    list.env("GIT_INDEX_FILE", index_path);
    let output = list.output().map_err(|source| DeadreckonError::Io {
        path: git_root.to_path_buf(),
        source,
    })?;
    require_git_success(git_root, &output, "enumerate signed result paths")?;
    if output.stdout.is_empty() {
        return Ok(());
    }

    let mut check = crate::git::git_command(
        git_root,
        &["check-attr", "--cached", "-z", "--stdin", "filter"],
    );
    check
        .env("GIT_INDEX_FILE", index_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = check.spawn().map_err(|source| DeadreckonError::Io {
        path: git_root.to_path_buf(),
        source,
    })?;
    child
        .stdin
        .take()
        .ok_or_else(|| {
            DeadreckonError::InvalidInput(
                "Git filter-attribute inventory has no input pipe".to_string(),
            )
        })?
        .write_all(&output.stdout)
        .map_err(|source| DeadreckonError::Io {
            path: git_root.to_path_buf(),
            source,
        })?;
    let output = child
        .wait_with_output()
        .map_err(|source| DeadreckonError::Io {
            path: git_root.to_path_buf(),
            source,
        })?;
    require_git_success(git_root, &output, "inspect signed result filter attributes")?;

    let fields = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty())
        .collect::<Vec<_>>();
    if fields.len() % 3 != 0 {
        return Err(DeadreckonError::InvalidInput(
            "Git filter-attribute inventory returned a malformed record".to_string(),
        ));
    }
    let filtered = fields
        .chunks_exact(3)
        .filter(|record| record[2] != b"unspecified")
        .map(|record| path_from_git_bytes(record[0]))
        .collect::<Result<Vec<_>>>()?;
    if filtered.is_empty() {
        return Ok(());
    }
    Err(DeadreckonError::InvalidInput(format!(
        "strict result applies external Git filter attributes to signed paths: {}; refusing to execute mutable smudge commands during receipt verification",
        display_paths(&filtered)
    )))
}

fn differing_index_paths(
    expected: &crate::flight::ArtifactFileIndex,
    actual: &crate::flight::ArtifactFileIndex,
) -> Vec<PathBuf> {
    expected
        .files
        .keys()
        .chain(actual.files.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| expected.files.get(*path) != actual.files.get(*path))
        .cloned()
        .collect()
}

fn git_revision(git_root: &Path, revision: &str) -> Result<Option<String>> {
    let output = crate::git::run_git(git_root, &["rev-parse", "--verify", revision])?;
    if !output.status.success() {
        return Ok(None);
    }
    let revision = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok((!revision.is_empty()).then_some(revision))
}

fn require_git_success(
    git_root: &Path,
    output: &std::process::Output,
    operation: &str,
) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(DeadreckonError::InvalidInput(if stderr.is_empty() {
        format!("could not {operation} in {}", git_root.display())
    } else {
        format!("could not {operation} in {}: {stderr}", git_root.display())
    }))
}

fn nul_paths(raw: &[u8]) -> Result<Vec<PathBuf>> {
    raw.split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(path_from_git_bytes)
        .collect()
}

#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)] // The non-Unix implementation can reject unrepresentable paths.
fn path_from_git_bytes(raw: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    Ok(PathBuf::from(OsString::from_vec(raw.to_vec())))
}

#[cfg(not(unix))]
fn path_from_git_bytes(raw: &[u8]) -> Result<PathBuf> {
    String::from_utf8(raw.to_vec())
        .map(PathBuf::from)
        .map_err(|_| {
            DeadreckonError::InvalidInput(
                "Git returned a result path that cannot be represented on this platform"
                    .to_string(),
            )
        })
}

fn display_paths(paths: &[PathBuf]) -> String {
    let mut display = paths
        .iter()
        .take(8)
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > 8 {
        display.push_str(&format!(", and {} more", paths.len() - 8));
    }
    display
}

fn result_tree_hash(state: &PipelineState) -> Result<String> {
    Ok(result_deliverable_index(state, &state.working_dir)?.tree_hash())
}

fn result_deliverable_index(
    state: &PipelineState,
    root: &Path,
) -> Result<crate::flight::ArtifactFileIndex> {
    let mut index = build_deliverable_file_index(root)?;
    // Promotion adds DeadReckon's own library manifest after the result was
    // sealed, and finish appends its delivery ledger. Both are lifecycle
    // metadata, not agent output, so the same receipt remains valid before
    // and after those operations.
    if state.promoted_library_dir.as_deref() == Some(state.working_dir.as_path()) {
        index.files.remove(Path::new("manifest.json"));
        index.files.remove(Path::new(".materialized-to"));
    }
    Ok(index)
}

fn retain_signed_result_revision(
    state: &PipelineState,
    job_id: &str,
    revision: &str,
) -> Result<()> {
    let record = read_trusted_codebase_record(&state.run_root)?;
    if record.mode != CodebaseMode::Worktree {
        return Ok(());
    }
    let git_root = record.source_git_root.as_deref().ok_or_else(|| {
        completion_error(
            job_id,
            "worktree result is missing its source Git repository",
        )
    })?;
    let ref_digest = sha256_text(job_id);
    let ref_digest = ref_digest.strip_prefix("sha256:").unwrap_or(&ref_digest);
    let retention_ref = format!("refs/deadreckon/results/{ref_digest}");
    if let Some(existing) = git_revision(git_root, &retention_ref)? {
        if existing == revision {
            return Ok(());
        }
        return Err(completion_error(
            job_id,
            "signed result retention ref already names a different revision",
        ));
    }
    let object_format = crate::git::run_git(git_root, &["rev-parse", "--show-object-format"])?;
    require_git_success(git_root, &object_format, "inspect Git object format")?;
    let zero_oid = match String::from_utf8_lossy(&object_format.stdout).trim() {
        "sha256" => "0".repeat(64),
        _ => "0".repeat(40),
    };
    let output = crate::git::run_git(
        git_root,
        &["update-ref", &retention_ref, revision, &zero_oid],
    )?;
    if output.status.success() {
        return Ok(());
    }
    require_git_success(git_root, &output, "retain signed result revision")
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
    use std::path::PathBuf;

    use chrono::Utc;
    use deadreckon_protocol::{
        AuthorityAcceptedBy, GoalCoverage, GoalCoverageStatus, Job, JobAuthority, JobId, JobPolicy,
        JobSchemaVersion, JobShape, RunId, SemanticDecision, SemanticJudgeMode, SemanticJudgment,
    };
    use tempfile::TempDir;

    use super::{seal_completion_receipt, validate_completion_receipt};
    use crate::codebase::{
        CodebaseMode, CodebaseRecord, write_codebase_record, write_trusted_codebase_record,
    };
    use crate::flight::{build_deliverable_file_index, sha256_file, sha256_text};
    use crate::gate::{
        AcceptanceCheckResult, AcceptanceContainment, read_gate_key,
        write_native_acceptance_marker_with_results_and_key,
    };
    use crate::job::{load_job, write_job};
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

    fn fixture_with_contract(contract: &str) -> Fixture {
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
        fs::write(&contract_path, contract).expect("contract");
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
            execution: Some(deadreckon_protocol::JobExecutionPolicy::workspace_only(
                "sandbox-exec",
            )),
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
            source_tree_sha256: build_deliverable_file_index(&state.working_dir)
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

    fn fixture() -> Fixture {
        fixture_with_contract("name: result\nchecks:\n  - file_exists: result.txt\n")
    }

    fn worktree_fixture() -> Fixture {
        let mut fixture = fixture();
        git_ok(&fixture.state.working_dir, &["init"]);
        git_ok(
            &fixture.state.working_dir,
            &["config", "user.email", "deadreckon@example.invalid"],
        );
        git_ok(
            &fixture.state.working_dir,
            &["config", "user.name", "DeadReckon Test"],
        );
        fs::write(
            fixture.state.working_dir.join(".git/info/exclude"),
            ".deadreckon/\n",
        )
        .expect("git exclude");
        git_ok(&fixture.state.working_dir, &["add", "-A"]);
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "approved base"],
        );
        let base = git_stdout(&fixture.state.working_dir, &["rev-parse", "HEAD"]);
        fs::write(
            fixture.state.working_dir.join("result.txt"),
            "verified result\n",
        )
        .expect("result change");
        git_ok(&fixture.state.working_dir, &["add", "result.txt"]);
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "deliver result"],
        );
        let branch = git_stdout(
            &fixture.state.working_dir,
            &["symbolic-ref", "--short", "HEAD"],
        );
        let record = CodebaseRecord {
            schema_version: crate::codebase::CODEBASE_RECORD_VERSION,
            mode: CodebaseMode::Worktree,
            source_path: Some(fixture.state.working_dir.clone()),
            source_git_root: Some(fixture.state.working_dir.clone()),
            branch_name: Some(branch),
            base_ref: Some(base.clone()),
            base_sha: Some(base.clone()),
            parent_branch: None,
            worktree_path: Some(fixture.state.working_dir.clone()),
            dirty_files_seeded: false,
            head_was_detached: false,
            created_at: Utc::now(),
            deadreckon_version: env!("CARGO_PKG_VERSION").to_string(),
            doc_polish_hash: None,
        };
        write_codebase_record(&fixture.state.working_dir, &record).expect("worktree record");
        write_trusted_codebase_record(&fixture.state.run_root, &record)
            .expect("trusted worktree record");
        fixture.authority.source_revision = Some(base);
        atomic_write_json(&fixture.paths.job_authority("job-1"), &fixture.authority)
            .expect("worktree authority");
        let mut job = load_job(&fixture.paths, "job-1").expect("job");
        job.authority_sha256 =
            sha256_file(&fixture.paths.job_authority("job-1")).expect("authority digest");
        atomic_write_json(&fixture.paths.job_json("job-1"), &job).expect("updated job");
        fixture
    }

    fn git_ok(cwd: &std::path::Path, args: &[&str]) {
        let output = crate::git::run_git(cwd, args).expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(cwd: &std::path::Path, args: &[&str]) -> String {
        let output = crate::git::run_git(cwd, args).expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
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
    fn receipt_binds_deliverable_tree_not_specstory_evidence() {
        let fixture = fixture();
        let private = fixture
            .state
            .working_dir
            .join(".specstory/history/session.md");
        fs::create_dir_all(private.parent().expect("private parent")).expect("private dir");
        fs::write(&private, "before sealing\n").expect("private evidence");
        let receipt = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");

        fs::write(&private, "changed after sealing\n").expect("mutate private evidence");

        assert_eq!(
            validate_completion_receipt(&fixture.paths, &fixture.state).expect("validate"),
            receipt
        );
    }

    #[test]
    fn worktree_receipt_refuses_uncommitted_deliverable_changes() {
        let fixture = worktree_fixture();
        fs::write(
            fixture.state.working_dir.join("uncommitted.txt"),
            "not in the result revision\n",
        )
        .expect("dirty deliverable");

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("uncommitted deliverable must not be sealed");

        assert!(
            error
                .to_string()
                .contains("uncommitted deliverable changes"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_refuses_ignored_uncommitted_deliverables() {
        let fixture = worktree_fixture();
        fs::write(
            fixture.state.working_dir.join(".gitignore"),
            "ignored.txt\n",
        )
        .expect("ignore rule");
        git_ok(&fixture.state.working_dir, &["add", ".gitignore"]);
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "ignore fixture output"],
        );
        fs::write(
            fixture.state.working_dir.join("ignored.txt"),
            "signed filesystem content absent from result revision\n",
        )
        .expect("ignored deliverable");

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("ignored uncommitted deliverable must not be sealed");

        assert!(
            error
                .to_string()
                .contains("uncommitted deliverable changes"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_refuses_assume_unchanged_deliverables() {
        let fixture = worktree_fixture();
        git_ok(
            &fixture.state.working_dir,
            &["update-index", "--assume-unchanged", "result.txt"],
        );
        fs::write(
            fixture.state.working_dir.join("result.txt"),
            "hidden from ordinary Git status\n",
        )
        .expect("hidden deliverable change");

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("assume-unchanged deliverable must not be sealed");

        assert!(
            error
                .to_string()
                .contains("uncommitted deliverable changes"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_refuses_skip_worktree_deliverables() {
        let fixture = worktree_fixture();
        git_ok(
            &fixture.state.working_dir,
            &["update-index", "--skip-worktree", "result.txt"],
        );

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("skip-worktree deliverable must not be sealed");

        assert!(
            error
                .to_string()
                .contains("uncommitted deliverable changes"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn worktree_receipt_compares_executable_mode_with_signed_revision() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = worktree_fixture();
        git_ok(
            &fixture.state.working_dir,
            &["config", "core.filemode", "false"],
        );
        let result = fixture.state.working_dir.join("result.txt");
        let mut permissions = fs::metadata(&result)
            .expect("result metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&result, permissions).expect("make result executable");

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("filesystem mode absent from signed revision must be refused");

        assert!(
            error
                .to_string()
                .contains("does not exactly match signed Git revision"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_refuses_gitlinks_instead_of_omitting_them() {
        let fixture = worktree_fixture();
        let nested = fixture.state.working_dir.join("vendor/nested-repository");
        fs::create_dir_all(&nested).expect("nested repository");
        git_ok(&nested, &["init", "-q", "-b", "main"]);
        git_ok(
            &nested,
            &["config", "user.email", "fixture@example.invalid"],
        );
        git_ok(&nested, &["config", "user.name", "fixture"]);
        fs::write(nested.join("README.md"), "nested result\n").expect("nested file");
        git_ok(&nested, &["add", "README.md"]);
        git_ok(&nested, &["commit", "-m", "nested base"]);
        git_ok(
            &fixture.state.working_dir,
            &["add", "vendor/nested-repository"],
        );
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "provider-created gitlink"],
        );

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("gitlink cannot be silently omitted from strict receipt");

        assert!(
            error.to_string().contains("unsupported Git submodule"),
            "{error}"
        );
        assert!(
            error.to_string().contains("vendor/nested-repository"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_refuses_filters_before_materializing_the_signed_tree() {
        let fixture = worktree_fixture();
        let sentinel = fixture.paths.home().join("smudge-filter-ran");
        fs::write(
            fixture.state.working_dir.join(".gitattributes"),
            "result.txt filter=evil\n",
        )
        .expect("filter attributes");
        git_ok(
            &fixture.state.working_dir,
            &[
                "config",
                "filter.evil.smudge",
                &format!("sh -c 'touch {}; cat'", sentinel.display()),
            ],
        );
        git_ok(&fixture.state.working_dir, &["add", ".gitattributes"]);
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "provider filter attribute"],
        );

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("external filter must be refused before checkout-index");

        assert!(error.to_string().contains("external Git filter"), "{error}");
        assert!(
            !sentinel.exists(),
            "receipt verification must not execute the configured smudge command"
        );
    }

    #[test]
    fn trusted_worktree_record_prevents_workspace_routing_tamper() {
        let fixture = worktree_fixture();
        write_codebase_record(&fixture.state.working_dir, &CodebaseRecord::fresh())
            .expect("forge workspace record");
        fs::write(
            fixture.state.working_dir.join("uncommitted.txt"),
            "not in the result revision\n",
        )
        .expect("dirty deliverable");

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("workspace record must not redirect the receipt boundary");

        assert!(
            error
                .to_string()
                .contains("uncommitted deliverable changes"),
            "{error}"
        );
    }

    #[test]
    fn strict_receipt_does_not_fall_back_to_workspace_routing() {
        let fixture = worktree_fixture();
        fs::remove_file(
            fixture
                .state
                .run_root
                .join(crate::codebase::TRUSTED_CODEBASE_RECORD),
        )
        .expect("remove trusted routing record");

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("strict receipt must not trust the workspace fallback");

        assert!(
            error
                .to_string()
                .contains("result lifecycle record is unavailable"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_refuses_private_path_added_then_deleted_in_history() {
        let fixture = worktree_fixture();
        let private_paths = [
            ".specstory/history/session.md",
            ".deadreckon/forged.json",
            "target/debug/provider-output",
        ];
        for relative in private_paths {
            let private = fixture.state.working_dir.join(relative);
            fs::create_dir_all(private.parent().expect("private parent")).expect("private dir");
            fs::write(&private, "private provider transcript\n").expect("private evidence");
        }
        git_ok(
            &fixture.state.working_dir,
            &[
                "add",
                "-f",
                ".specstory/history/session.md",
                ".deadreckon/forged.json",
                "target/debug/provider-output",
            ],
        );
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "accidentally add private evidence"],
        );
        for relative in private_paths {
            fs::remove_file(fixture.state.working_dir.join(relative))
                .expect("remove private evidence");
        }
        git_ok(
            &fixture.state.working_dir,
            &[
                "add",
                "-u",
                ".specstory/history/session.md",
                ".deadreckon/forged.json",
                "target/debug/provider-output",
            ],
        );
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "remove private evidence"],
        );
        assert!(
            super::deliverable_git_delta_paths(
                &fixture.state.working_dir,
                fixture.authority.source_revision.as_deref().expect("base"),
                "HEAD",
            )
            .expect("final deliverable diff")
            .iter()
            .all(|path| !path.starts_with(".specstory"))
        );
        assert_eq!(
            super::non_deliverable_git_history_paths(
                &fixture.state.working_dir,
                fixture.authority.source_revision.as_deref().expect("base"),
                "HEAD",
            )
            .expect("history paths"),
            [
                ".deadreckon/forged.json",
                ".specstory/history/session.md",
                "target/debug/provider-output",
            ]
            .map(PathBuf::from)
            .to_vec()
        );

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("private evidence in history must not be sealed");

        assert!(
            error.to_string().contains("non-deliverable paths"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_refuses_private_paths_hidden_in_merged_history() {
        let fixture = worktree_fixture();
        let result_branch = git_stdout(
            &fixture.state.working_dir,
            &["symbolic-ref", "--short", "HEAD"],
        );
        git_ok(
            &fixture.state.working_dir,
            &["switch", "-c", "provider-private-side"],
        );
        let private = fixture
            .state
            .working_dir
            .join(".specstory/history/merged-session.md");
        fs::create_dir_all(private.parent().expect("private parent")).expect("private dir");
        fs::write(&private, "private side-branch evidence\n").expect("private evidence");
        git_ok(
            &fixture.state.working_dir,
            &["add", "-f", ".specstory/history/merged-session.md"],
        );
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "side branch adds private evidence"],
        );
        fs::remove_file(&private).expect("remove private evidence");
        git_ok(
            &fixture.state.working_dir,
            &["add", "-u", ".specstory/history/merged-session.md"],
        );
        git_ok(
            &fixture.state.working_dir,
            &["commit", "-m", "side branch removes private evidence"],
        );
        git_ok(&fixture.state.working_dir, &["switch", &result_branch]);
        git_ok(
            &fixture.state.working_dir,
            &[
                "merge",
                "--no-ff",
                "provider-private-side",
                "-m",
                "merge provider side branch",
            ],
        );

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("private evidence in merged history must not be sealed");
        assert!(
            error.to_string().contains("non-deliverable paths"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_validation_refuses_post_seal_uncommitted_deliverable() {
        let fixture = worktree_fixture();
        seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        fs::write(
            fixture.state.working_dir.join("post-seal.txt"),
            "not signed\n",
        )
        .expect("post-seal deliverable");

        let error = validate_completion_receipt(&fixture.paths, &fixture.state)
            .expect_err("dirty result must invalidate receipt");

        assert!(
            error
                .to_string()
                .contains("uncommitted deliverable changes"),
            "{error}"
        );
    }

    #[test]
    fn worktree_receipt_retains_the_signed_result_revision() {
        let fixture = worktree_fixture();
        let receipt = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect("seal");
        let digest = sha256_text("job-1");
        let retention_ref = format!(
            "refs/deadreckon/results/{}",
            digest.strip_prefix("sha256:").expect("digest prefix")
        );

        assert_eq!(
            git_stdout(&fixture.state.working_dir, &["rev-parse", &retention_ref],),
            receipt.result_revision.expect("result revision")
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
    fn strict_job_cannot_seal_an_empty_deterministic_contract() {
        let fixture = fixture_with_contract("name: empty\nchecks: []\n");

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("an empty contract is not a deterministic completion key");

        assert!(error.to_string().contains("contains no checks"), "{error}");
        assert!(!fixture.paths.job_receipt("job-1").exists());
    }

    #[test]
    fn strict_job_cannot_seal_the_unknown_project_directory_noop() {
        let fixture = fixture_with_contract(
            "name: deadreckon detected unknown\nchecks:\n  - kind: file_exists\n    path: '{working_dir}'\n    must_pass: true\n",
        );

        let error = seal_completion_receipt(
            &fixture.paths,
            &fixture.state,
            &fixture.authority,
            &fixture.marker,
            &fixture.judgment,
        )
        .expect_err("the pre-created working directory is not a definition of done");

        assert!(
            error
                .to_string()
                .contains("only proves that its pre-created working directory exists"),
            "{error}"
        );
        assert!(!fixture.paths.job_receipt("job-1").exists());
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
