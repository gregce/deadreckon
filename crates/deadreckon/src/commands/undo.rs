//! Verified reversal of one durable Job delivery.
//!
//! The legacy chain undo is the behavioral sibling: confirm, revert, and leave
//! factual evidence. The intentional difference is authority. A Job undo may
//! only use a controller-signed applied-delivery receipt, re-prove its exact
//! delta against the still-valid two-key completion receipt, and require the
//! unsigned Job event to be an exact factual mirror. Mutable events never
//! decide what repository, ref, or revision to change.

use super::super::*;
use deadreckon_protocol::JobEventKind;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AppliedDeliveryAuthority {
    destination: PathBuf,
    git_common_dir: PathBuf,
    target_ref: String,
    destination_revision_before: String,
    resulting_revision: String,
    delivery_receipt_sha256: String,
    completion_receipt_sha256: String,
    delivery_intent_sha256: String,
    source_revision: String,
    result_revision: String,
    effective_policy_sha256: String,
    strategy: deadreckon_protocol::GitDeliveryStrategy,
}

#[derive(Debug)]
struct UndoHistoryState {
    completed: Option<(String, String)>,
    pending: Option<String>,
    next_attempt: usize,
}

pub(crate) fn job_undo_command(
    paths: &DeadreckonPaths,
    job: &deadreckon_core::JobView,
    no_confirm: bool,
) -> Result<()> {
    let state = super::lifecycle::finish_job_state(paths, job)?;
    let receipt = deadreckon_core::validate_completion_receipt(paths, &state)?;
    job_undo_with_verified_receipt_inner(paths, job, &receipt, no_confirm, true)
}

#[cfg(test)]
fn job_undo_with_verified_receipt(
    paths: &DeadreckonPaths,
    job: &deadreckon_core::JobView,
    receipt: &deadreckon_protocol::CompletionReceipt,
    no_confirm: bool,
) -> Result<()> {
    // Tests use a deliberately synthetic receipt that cannot pass the native
    // completion validator. Production enters through job_undo_command and
    // always revalidates the current receipt after acquiring the lock.
    job_undo_with_verified_receipt_inner(paths, job, receipt, no_confirm, false)
}

fn job_undo_with_verified_receipt_inner(
    paths: &DeadreckonPaths,
    job: &deadreckon_core::JobView,
    receipt: &deadreckon_protocol::CompletionReceipt,
    no_confirm: bool,
    revalidate_completion_after_lock: bool,
) -> Result<()> {
    let job_id = job.job.job_id.as_ref();
    let authority = applied_delivery_authority(paths, job, receipt)?;
    if !no_confirm && io::stdin().is_terminal() {
        if !prompt::confirm(
            &format!(
                "revert verified Job {} from {}?",
                run_prefix(job_id),
                authority.destination.display()
            ),
            false,
        )? {
            println!("{}", ui_status("cancelled"));
            return Ok(());
        }
    } else if !no_confirm {
        return Err(CliError::Core(deadreckon_core::user_error(
            "non-interactive Job undo requires --no-confirm",
            &format!("deadreckon undo {} --no-confirm", run_prefix(job_id)),
        )));
    }

    let authority_sha256 = authority_sha256(&authority);
    let _operation_lock = deadreckon_core::acquire_job_operation_lock(paths, job_id)?;

    // Re-read both immutable lifecycle truth and receipt bytes after acquiring
    // the operation lock. A stale pre-confirmation view never authorizes Git.
    let current_job = deadreckon_core::JobView::load(paths, job_id)?;
    let current_receipt = if revalidate_completion_after_lock {
        let current_state = super::lifecycle::finish_job_state(paths, &current_job)?;
        deadreckon_core::validate_completion_receipt(paths, &current_state)?
    } else {
        receipt.clone()
    };
    let current_authority = applied_delivery_authority(paths, &current_job, &current_receipt)?;
    if current_authority != authority {
        return Err(job_undo_refusal(
            job_id,
            "the verified applied-delivery authority changed before undo began",
        ));
    }
    let history = deadreckon_core::read_job_history(&paths.job_events(job_id))?;
    let undo_state = undo_history_state(&history, &authority_sha256)?;

    if let Some((_operation_id, undo_revision)) = undo_state.completed {
        require_clean_exact_destination(&authority)?;
        let head = git_stdout_at(&authority.destination, &["rev-parse", "HEAD"])?;
        if head != undo_revision
            || !is_exact_revert_commit(&authority.destination, &authority, &head)?
        {
            return Err(job_undo_refusal(
                job_id,
                "the Job was already undone, but its destination no longer has the recorded undo revision",
            ));
        }
        print_undo_result(job_id, &authority, &undo_revision, true);
        return Ok(());
    }

    let pending = undo_state.pending;
    let operation_id = pending.clone().unwrap_or_else(|| {
        let digest = authority_sha256
            .strip_prefix("sha256:")
            .unwrap_or(&authority_sha256);
        format!(
            "{}-{}",
            &digest[..digest.len().min(20)],
            undo_state.next_attempt
        )
    });
    if pending.is_none() {
        super::job::record_job_undo_event(
            paths,
            job_id,
            JobEventKind::UndoStarted,
            &operation_id,
            &undo_event_detail(&authority, &authority_sha256, &operation_id, None, None),
        )?;
    }

    let operation = perform_or_recover_undo(paths, job_id, &authority);
    match operation {
        Ok(undo_revision) => {
            super::job::record_job_undo_event(
                paths,
                job_id,
                JobEventKind::UndoCompleted,
                &operation_id,
                &undo_event_detail(
                    &authority,
                    &authority_sha256,
                    &operation_id,
                    Some(&undo_revision),
                    None,
                ),
            )?;
            print_undo_result(job_id, &authority, &undo_revision, false);
            Ok(())
        }
        Err(error) => {
            let reason = one_line(&error.to_string(), 500);
            let evidence = undo_event_detail(
                &authority,
                &authority_sha256,
                &operation_id,
                None,
                Some(&reason),
            );
            if let Err(record_error) = super::job::record_job_undo_event(
                paths,
                job_id,
                JobEventKind::UndoFailed,
                &operation_id,
                &evidence,
            ) {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Job undo failed ({error}) and its factual failure evidence could not be recorded ({record_error})"
                ))));
            }
            Err(error)
        }
    }
}

fn applied_delivery_authority(
    paths: &DeadreckonPaths,
    job: &deadreckon_core::JobView,
    receipt: &deadreckon_protocol::CompletionReceipt,
) -> Result<AppliedDeliveryAuthority> {
    let job_id = job.job.job_id.as_ref();
    if job.projection.outcome != Some(deadreckon_protocol::JobOutcome::Verified)
        || job.projection.stop_reason != Some(deadreckon_protocol::StopReason::Verified)
        || receipt.job_id.as_ref() != job_id
        || receipt.run_id.as_ref() != job_id
        || receipt.outcome != deadreckon_protocol::JobOutcome::Verified
        || receipt.stop_reason != deadreckon_protocol::StopReason::Verified
        || receipt.proof_kind != deadreckon_protocol::CompletionProofKind::TwoKeyCompletion
        || receipt.issuer != deadreckon_protocol::CompletionReceiptIssuer::DeadreckonSupervisor
    {
        return Err(job_undo_refusal(
            job_id,
            "the Job does not have a supervisor-issued two-key verified result",
        ));
    }
    let applied_snapshot =
        deadreckon_core::validate_applied_git_delivery_receipt_snapshot(paths, job_id)?;
    let applied_receipt = &applied_snapshot.receipt;
    let completion_receipt_sha256 =
        deadreckon_core::flight::sha256_file(&paths.job_receipt(job_id))?;
    if applied_receipt.job_id != receipt.job_id
        || applied_receipt.run_id != receipt.run_id
        || applied_receipt.completion_receipt_sha256 != completion_receipt_sha256
        || receipt.source_revision.as_deref()
            != Some(applied_receipt.signed_source_revision.as_str())
        || receipt.result_revision.as_deref()
            != Some(applied_receipt.signed_result_revision.as_str())
        || receipt.effective_policy_sha256 != applied_receipt.effective_policy_sha256
    {
        return Err(job_undo_refusal(
            job_id,
            "the signed applied delivery does not match the current signed completion receipt",
        ));
    }
    if applied_receipt.pre_revision == applied_receipt.applied_revision {
        return Err(job_undo_refusal(
            job_id,
            "the signed applied delivery did not create a distinct Git revision",
        ));
    }

    let target =
        deadreckon_core::GitDeliveryTarget::inspect(&applied_receipt.repository.worktree_root)?;
    if target.repository != applied_receipt.repository
        || target.target_ref != applied_receipt.target_ref
    {
        return Err(job_undo_refusal(
            job_id,
            "the destination is not the same worktree and attached target ref named by the signed delivery",
        ));
    }
    super::lifecycle::verify_signed_delivery_delta(
        job_id,
        receipt,
        &applied_receipt.repository.worktree_root,
        &applied_receipt.pre_revision,
        &applied_receipt.applied_revision,
    )?;
    super::lifecycle::validate_applied_delivery_topology(
        &applied_receipt.repository.worktree_root,
        applied_receipt.strategy,
        &applied_receipt.pre_revision,
        &applied_receipt.applied_revision,
        &applied_receipt.signed_source_revision,
        &applied_receipt.signed_result_revision,
    )?;

    let history = deadreckon_core::read_job_history(&paths.job_events(job_id))?;
    if !history.caveats.is_empty() {
        return Err(job_undo_refusal(
            job_id,
            "the Job history has a torn final event",
        ));
    }
    let applied = history
        .events()
        .iter()
        .filter(|event| event.kind == JobEventKind::ResultApplied)
        .collect::<Vec<_>>();
    if applied.len() != 1 {
        return Err(job_undo_refusal(
            job_id,
            if applied.is_empty() {
                "there is no verified applied delivery to undo"
            } else {
                "more than one applied delivery is recorded, so the undo target is ambiguous"
            },
        ));
    }
    let expected_event =
        super::job::signed_applied_job_delivery_detail(applied_receipt, &applied_snapshot.sha256);
    if applied[0].detail != expected_event {
        return Err(job_undo_refusal(
            job_id,
            "the unsigned applied-delivery event was changed or redirected and no longer mirrors the signed receipt",
        ));
    }
    Ok(AppliedDeliveryAuthority {
        destination: applied_receipt.repository.worktree_root.clone(),
        git_common_dir: applied_receipt.repository.git_common_dir.clone(),
        target_ref: applied_receipt.target_ref.clone(),
        destination_revision_before: applied_receipt.pre_revision.clone(),
        resulting_revision: applied_receipt.applied_revision.clone(),
        delivery_receipt_sha256: applied_snapshot.sha256,
        completion_receipt_sha256,
        delivery_intent_sha256: applied_receipt.delivery_intent_sha256.clone(),
        source_revision: applied_receipt.signed_source_revision.clone(),
        result_revision: applied_receipt.signed_result_revision.clone(),
        effective_policy_sha256: applied_receipt.effective_policy_sha256.clone(),
        strategy: applied_receipt.strategy,
    })
}

fn authority_sha256(authority: &AppliedDeliveryAuthority) -> String {
    authority.delivery_receipt_sha256.clone()
}

fn undo_history_state(
    history: &deadreckon_core::JobHistory,
    authority_sha256: &str,
) -> Result<UndoHistoryState> {
    let mut starts = BTreeSet::new();
    let mut terminal = BTreeSet::new();
    let mut completed = Vec::new();
    for event in history.events() {
        if !matches!(
            event.kind,
            JobEventKind::UndoStarted | JobEventKind::UndoCompleted | JobEventKind::UndoFailed
        ) || event.detail.get("authority_sha256").and_then(Value::as_str)
            != Some(authority_sha256)
        {
            continue;
        }
        let operation_id = event
            .detail
            .get("operation_id")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "Job undo history omitted operation_id".to_string(),
                ))
            })?
            .to_string();
        match event.kind {
            JobEventKind::UndoStarted => {
                starts.insert(operation_id);
            }
            JobEventKind::UndoCompleted => {
                let revision = event
                    .detail
                    .get("undo_revision")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        CliError::Core(DeadreckonError::InvalidInput(
                            "Job undo completion omitted undo_revision".to_string(),
                        ))
                    })?
                    .to_string();
                terminal.insert(operation_id.clone());
                completed.push((operation_id, revision));
            }
            JobEventKind::UndoFailed => {
                terminal.insert(operation_id);
            }
            _ => unreachable!(),
        }
    }
    if completed.len() > 1 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Job history records more than one completed undo for the same applied delivery"
                .to_string(),
        )));
    }
    let pending = starts.difference(&terminal).cloned().collect::<Vec<_>>();
    if pending.len() > 1 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "Job history records concurrent incomplete undo operations".to_string(),
        )));
    }
    Ok(UndoHistoryState {
        completed: completed.pop(),
        pending: pending.into_iter().next(),
        next_attempt: starts.len() + 1,
    })
}

fn perform_or_recover_undo(
    paths: &DeadreckonPaths,
    job_id: &str,
    authority: &AppliedDeliveryAuthority,
) -> Result<String> {
    require_clean_exact_destination(authority)?;
    let head = git_stdout_at(&authority.destination, &["rev-parse", "HEAD"])?;
    if head != authority.resulting_revision {
        if is_exact_revert_commit(&authority.destination, authority, &head)? {
            return Ok(head);
        }
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "verified delivery destination is at {head}, not applied revision {}",
                authority.resulting_revision
            ),
            "inspect the destination and restore the exact delivered revision before retrying undo",
        )));
    }

    let parents = commit_parents(&authority.destination, &authority.resulting_revision)?;
    let parent_positions = parents
        .iter()
        .enumerate()
        .filter(|(_, parent)| *parent == &authority.destination_revision_before)
        .map(|(index, _)| index + 1)
        .collect::<Vec<_>>();
    if parent_positions.len() != 1 {
        return Err(CliError::Core(deadreckon_core::user_error(
            "the recorded pre-delivery revision is not one unambiguous parent of the applied revision",
            "inspect the Job delivery history; do not choose a Git mainline by guesswork",
        )));
    }
    let mut args = vec![
        "-c".to_string(),
        "revert.reference=false".to_string(),
        "revert".to_string(),
        "--no-edit".to_string(),
    ];
    if parents.len() > 1 {
        args.push("-m".to_string());
        args.push(parent_positions[0].to_string());
    }
    args.push(authority.resulting_revision.clone());
    let borrowed = args.iter().map(String::as_str).collect::<Vec<_>>();
    let disabled_hooks = super::lifecycle::disabled_delivery_hooks(paths, job_id)?;
    super::lifecycle::hardened_delivery_git_status(
        &authority.destination,
        Some(&disabled_hooks),
        &borrowed,
    )?;
    super::lifecycle::require_clean_git_state_after_delivery(&authority.destination)?;
    let undo_revision = git_stdout_at(&authority.destination, &["rev-parse", "HEAD"])?;
    if !is_exact_revert_commit(&authority.destination, authority, &undo_revision)? {
        return Err(CliError::Core(deadreckon_core::user_error(
            "Git reported success, but the resulting commit is not the exact inverse of the recorded delivery",
            "stop and inspect the destination before making another Git change",
        )));
    }
    Ok(undo_revision)
}

fn require_clean_exact_destination(authority: &AppliedDeliveryAuthority) -> Result<()> {
    let destination = &authority.destination;
    let canonical = fs::canonicalize(destination)?;
    if canonical != *destination {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "verified delivery destination {} no longer has its recorded path identity",
                destination.display()
            ),
            "inspect the recorded destination; aliases and substitutions cannot be used for Job undo",
        )));
    }
    let root = PathBuf::from(git_stdout_at(
        destination,
        &["rev-parse", "--show-toplevel"],
    )?);
    if fs::canonicalize(&root)? != canonical {
        return Err(CliError::Core(deadreckon_core::user_error(
            "verified delivery destination is no longer the exact Git worktree root",
            "run undo from the originally delivered repository after restoring its path identity",
        )));
    }
    super::lifecycle::refuse_in_progress_git_operation(destination)?;
    let target = deadreckon_core::GitDeliveryTarget::inspect(destination)?;
    if target.repository.worktree_root != authority.destination
        || target.repository.git_common_dir != authority.git_common_dir
        || target.target_ref != authority.target_ref
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            "verified delivery destination is not attached to the signed target ref and repository",
            "check out the originally delivered branch in the original worktree before retrying undo",
        )));
    }
    let status = deadreckon_core::git::run_git(
        destination,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    if !status.status.success() || !status.stdout.is_empty() {
        return Err(CliError::Core(deadreckon_core::user_error(
            "verified delivery destination is not clean",
            "commit or remove destination changes, then retry the same Job undo",
        )));
    }
    Ok(())
}

fn is_exact_revert_commit(
    destination: &Path,
    authority: &AppliedDeliveryAuthority,
    revision: &str,
) -> Result<bool> {
    let parents = commit_parents(destination, revision)?;
    if parents.as_slice() != [authority.resulting_revision.as_str()] {
        return Ok(false);
    }
    let undo_tree = git_stdout_at(destination, &["rev-parse", &format!("{revision}^{{tree}}")])?;
    let before_tree = git_stdout_at(
        destination,
        &[
            "rev-parse",
            &format!("{}^{{tree}}", authority.destination_revision_before),
        ],
    )?;
    if undo_tree != before_tree {
        return Ok(false);
    }
    let message = git_stdout_at(destination, &["show", "-s", "--format=%B", revision])?;
    Ok(message.contains(&format!(
        "This reverts commit {}.",
        authority.resulting_revision
    )))
}

fn commit_parents(destination: &Path, revision: &str) -> Result<Vec<String>> {
    let line = git_stdout_at(destination, &["rev-list", "--parents", "-n", "1", revision])?;
    let mut fields = line.split_whitespace();
    let actual = fields.next().unwrap_or_default();
    if actual != revision {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Git resolved {revision} as unexpected commit {actual}"
        ))));
    }
    Ok(fields.map(ToOwned::to_owned).collect())
}

fn git_stdout_at(destination: &Path, args: &[&str]) -> Result<String> {
    let output = deadreckon_core::git::run_git(destination, args)?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            if detail.is_empty() {
                format!("git {args:?} failed in {}", destination.display())
            } else {
                detail
            },
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
fn git_status_at(destination: &Path, args: &[&str]) -> Result<()> {
    git_stdout_at(destination, args).map(|_| ())
}

fn undo_event_detail(
    authority: &AppliedDeliveryAuthority,
    authority_sha256: &str,
    operation_id: &str,
    undo_revision: Option<&str>,
    reason: Option<&str>,
) -> Value {
    json!({
        "operation_id": operation_id,
        "authority_sha256": authority_sha256,
        "destination": authority.destination,
        "git_common_dir": authority.git_common_dir,
        "target_ref": authority.target_ref,
        "applied_revision": authority.resulting_revision,
        "destination_revision_before": authority.destination_revision_before,
        "delivery_receipt_sha256": authority.delivery_receipt_sha256,
        "completion_receipt_sha256": authority.completion_receipt_sha256,
        "delivery_intent_sha256": authority.delivery_intent_sha256,
        "source_revision": authority.source_revision,
        "result_revision": authority.result_revision,
        "effective_policy_sha256": authority.effective_policy_sha256,
        "strategy": authority.strategy,
        "undo_revision": undo_revision,
        "reason": reason,
    })
}

fn job_undo_refusal(job_id: &str, reason: &str) -> CliError {
    CliError::Core(deadreckon_core::user_error(
        &format!(
            "DeadReckon did not undo Job {} because {reason}",
            run_prefix(job_id)
        ),
        &format!("deadreckon show {}", run_prefix(job_id)),
    ))
}

fn print_undo_result(
    job_id: &str,
    authority: &AppliedDeliveryAuthority,
    undo_revision: &str,
    already_complete: bool,
) {
    let status = if already_complete {
        "already undone"
    } else {
        "undone"
    };
    println!("{} {}", ui_ok(status), run_prefix(job_id));
    println!("  destination: {}", authority.destination.display());
    println!("  reverted: {}", authority.resulting_revision);
    println!("  undo revision: {undo_revision}");
}

#[cfg(test)]
mod tests {
    use deadreckon_protocol::{
        AppliedGitDeliveryReceipt, CompletionProofKind, CompletionReceipt, CompletionReceiptIssuer,
        GitDeliveryIntent, GitDeliveryStrategy, Job, JobEvent, JobEventSequence, JobId, JobOutcome,
        JobPolicy, JobSchemaVersion, JobShape, RunId, SemanticJudgeMode, StopReason,
    };
    use tempfile::TempDir;

    use super::*;

    struct Fixture {
        _temp: TempDir,
        paths: DeadreckonPaths,
        job_id: String,
        destination: PathBuf,
        before: String,
        applied: String,
        receipt: CompletionReceipt,
    }

    impl Fixture {
        fn new(bound_delivery: bool) -> Self {
            let temp = TempDir::new().expect("tempdir");
            let paths = DeadreckonPaths::from_home(temp.path().join("home"));
            let destination = temp.path().join("destination");
            fs::create_dir_all(&destination).expect("destination");
            git_status_at(&destination, &["init"]).expect("git init");
            git_status_at(
                &destination,
                &["config", "user.email", "deadreckon@example.invalid"],
            )
            .expect("git email");
            git_status_at(&destination, &["config", "user.name", "DeadReckon Test"])
                .expect("git name");
            fs::write(destination.join("result.txt"), "before\n").expect("before file");
            git_status_at(&destination, &["add", "result.txt"]).expect("add before");
            git_status_at(&destination, &["commit", "-m", "before delivery"])
                .expect("commit before");
            let before = git_stdout_at(&destination, &["rev-parse", "HEAD"]).expect("before");
            fs::write(destination.join("result.txt"), "verified result\n").expect("result file");
            git_status_at(&destination, &["add", "result.txt"]).expect("add result");
            git_status_at(&destination, &["commit", "-m", "verified delivery"])
                .expect("commit delivery");
            let applied = git_stdout_at(&destination, &["rev-parse", "HEAD"]).expect("applied");
            let job_id = "ababababababababababababababab1234".to_string();
            let policy = JobPolicy {
                max_spend_usd: 2.0,
                max_wall_seconds: 60,
                max_attempts: 2,
                deadline: None,
                semantic_judge: SemanticJudgeMode::Required,
                execution: None,
            };
            deadreckon_core::write_job(
                &paths,
                &Job {
                    schema_version: JobSchemaVersion::CURRENT,
                    job_id: JobId(job_id.clone()),
                    scope: "undo-test".to_string(),
                    goal: "deliver then undo safely".to_string(),
                    shape: JobShape::Single,
                    created_at: Utc::now(),
                    source_cwd: destination.clone(),
                    launch_plan_sha256: "launch".to_string(),
                    authority_sha256: "authority".to_string(),
                    policy,
                },
            )
            .expect("Job");
            append_event(&paths, &job_id, 1, JobEventKind::Created, json!({}));
            append_event(&paths, &job_id, 2, JobEventKind::Verified, json!({}));
            let policy_sha256 = deadreckon_core::flight::sha256_text("policy");
            let receipt = CompletionReceipt {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: JobId(job_id.clone()),
                run_id: RunId(job_id.clone()),
                attempt: 1,
                outer_launch_id: "00000000-0000-4000-8000-000000000001".to_string(),
                issued_at: Utc::now(),
                issuer: CompletionReceiptIssuer::DeadreckonSupervisor,
                proof_kind: CompletionProofKind::TwoKeyCompletion,
                outcome: JobOutcome::Verified,
                stop_reason: StopReason::Verified,
                authority_sha256: "authority".to_string(),
                goal_sha256: "goal".to_string(),
                contract_sha256: "contract".to_string(),
                effective_policy_sha256: policy_sha256.clone(),
                launch_plan_sha256: "launch".to_string(),
                source_tree_sha256: "source-tree".to_string(),
                source_revision: Some(before.clone()),
                result_tree_sha256: "result-tree".to_string(),
                result_revision: Some(applied.clone()),
                deterministic_marker_sha256: "marker".to_string(),
                semantic_judgment_sha256: "semantic".to_string(),
                sandbox_boundary_observation_sha256: "boundary".to_string(),
                execution_evidence: None,
                contained: true,
                sandbox_backend: "sandbox-exec".to_string(),
                signature: "test-signature".to_string(),
            };
            let receipt_path = paths.job_receipt(&job_id);
            fs::write(
                &receipt_path,
                serde_json::to_vec_pretty(&receipt).expect("receipt json"),
            )
            .expect("receipt");
            let detail = if bound_delivery {
                deadreckon_core::write_gate_key(&paths, &job_id, &[23_u8; 32])
                    .expect("protected delivery key");
                let target = deadreckon_core::GitDeliveryTarget::inspect(&destination)
                    .expect("delivery target");
                let intent = deadreckon_core::seal_git_delivery_intent(
                    &paths,
                    &GitDeliveryIntent {
                        schema_version: JobSchemaVersion::CURRENT,
                        job_id: JobId(job_id.clone()),
                        run_id: RunId(job_id.clone()),
                        prepared_at: Utc::now(),
                        completion_receipt_sha256: deadreckon_core::flight::sha256_file(
                            &receipt_path,
                        )
                        .expect("receipt digest"),
                        repository: target.repository,
                        target_ref: target.target_ref,
                        pre_revision: before.clone(),
                        signed_source_revision: before.clone(),
                        signed_result_revision: applied.clone(),
                        effective_policy_sha256: policy_sha256,
                        strategy: GitDeliveryStrategy::Squash,
                        signature: String::new(),
                    },
                )
                .expect("signed delivery intent");
                let applied_receipt = deadreckon_core::seal_applied_git_delivery_receipt(
                    &paths,
                    &AppliedGitDeliveryReceipt {
                        schema_version: JobSchemaVersion::CURRENT,
                        job_id: intent.job_id.clone(),
                        run_id: intent.run_id.clone(),
                        issued_at: Utc::now(),
                        delivery_intent_sha256: deadreckon_core::flight::sha256_file(
                            &paths.job_delivery_intent(&job_id),
                        )
                        .expect("intent digest"),
                        completion_receipt_sha256: intent.completion_receipt_sha256.clone(),
                        repository: intent.repository.clone(),
                        target_ref: intent.target_ref.clone(),
                        pre_revision: intent.pre_revision.clone(),
                        applied_revision: applied.clone(),
                        signed_source_revision: intent.signed_source_revision.clone(),
                        signed_result_revision: intent.signed_result_revision.clone(),
                        effective_policy_sha256: intent.effective_policy_sha256.clone(),
                        strategy: intent.strategy,
                        signature: String::new(),
                    },
                )
                .expect("signed applied delivery");
                let applied_snapshot =
                    deadreckon_core::validate_applied_git_delivery_receipt_snapshot(
                        &paths, &job_id,
                    )
                    .expect("applied receipt snapshot");
                super::super::job::signed_applied_job_delivery_detail(
                    &applied_receipt,
                    &applied_snapshot.sha256,
                )
            } else {
                json!({
                    "destination": destination,
                    "resulting_revision": applied,
                })
            };
            append_event(&paths, &job_id, 3, JobEventKind::ResultApplied, detail);
            Self {
                _temp: temp,
                paths,
                job_id,
                destination,
                before,
                applied,
                receipt,
            }
        }

        fn view(&self) -> deadreckon_core::JobView {
            deadreckon_core::JobView::load(&self.paths, &self.job_id).expect("Job view")
        }
    }

    fn append_event(
        paths: &DeadreckonPaths,
        job_id: &str,
        sequence: u64,
        kind: JobEventKind,
        detail: Value,
    ) {
        deadreckon_core::append_job_event(
            paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: JobId(job_id.to_string()),
                sequence: JobEventSequence::new(sequence).expect("sequence"),
                event_id: format!("fixture-{sequence}"),
                causation_id: "undo-fixture".to_string(),
                timestamp: Utc::now(),
                lease_epoch: 0,
                kind,
                detail,
            },
        )
        .expect("append event");
    }

    fn rewrite_applied_event_detail(
        paths: &DeadreckonPaths,
        job_id: &str,
        mutate: impl FnOnce(&mut Value),
    ) {
        let path = paths.job_events(job_id);
        let raw = fs::read_to_string(&path).expect("job history");
        let mut events = raw
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str::<JobEvent>(line).expect("Job event"))
            .collect::<Vec<_>>();
        let applied = events
            .iter_mut()
            .find(|event| event.kind == JobEventKind::ResultApplied)
            .expect("applied event");
        mutate(&mut applied.detail);
        let rewritten = events
            .iter()
            .map(|event| serde_json::to_string(event).expect("event json"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(path, rewritten).expect("tampered Job history");
    }

    #[test]
    fn verified_job_undo_reverts_only_recorded_delivery_and_is_idempotent() {
        let fixture = Fixture::new(true);

        job_undo_with_verified_receipt(&fixture.paths, &fixture.view(), &fixture.receipt, true)
            .expect("first undo");
        let undo_revision =
            git_stdout_at(&fixture.destination, &["rev-parse", "HEAD"]).expect("undo revision");
        assert_ne!(undo_revision, fixture.applied);
        assert_eq!(
            git_stdout_at(
                &fixture.destination,
                &["rev-parse", &format!("{undo_revision}^{{tree}}")],
            )
            .expect("undo tree"),
            git_stdout_at(
                &fixture.destination,
                &["rev-parse", &format!("{}^{{tree}}", fixture.before)],
            )
            .expect("before tree")
        );

        job_undo_with_verified_receipt(&fixture.paths, &fixture.view(), &fixture.receipt, true)
            .expect("idempotent retry");
        assert_eq!(
            git_stdout_at(&fixture.destination, &["rev-parse", "HEAD"]).expect("same head"),
            undo_revision
        );
        let history = deadreckon_core::read_job_history(&fixture.paths.job_events(&fixture.job_id))
            .expect("history");
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| event.kind == JobEventKind::UndoStarted)
                .count(),
            1
        );
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| event.kind == JobEventKind::UndoCompleted)
                .count(),
            1
        );
    }

    #[test]
    fn retry_recovers_revert_committed_after_started_event_without_reverting_twice() {
        let fixture = Fixture::new(true);
        let authority =
            applied_delivery_authority(&fixture.paths, &fixture.view(), &fixture.receipt)
                .expect("authority");
        let authority_digest = authority_sha256(&authority);
        let operation_id = "crash-window-1";
        super::super::job::record_job_undo_event(
            &fixture.paths,
            &fixture.job_id,
            JobEventKind::UndoStarted,
            operation_id,
            &undo_event_detail(&authority, &authority_digest, operation_id, None, None),
        )
        .expect("started evidence");
        let committed = perform_or_recover_undo(&fixture.paths, &fixture.job_id, &authority)
            .expect("Git revert before crash");

        job_undo_with_verified_receipt(&fixture.paths, &fixture.view(), &fixture.receipt, true)
            .expect("retry completes pending operation");

        assert_eq!(
            git_stdout_at(&fixture.destination, &["rev-parse", "HEAD"]).expect("head"),
            committed
        );
        let history = deadreckon_core::read_job_history(&fixture.paths.job_events(&fixture.job_id))
            .expect("history");
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| event.kind == JobEventKind::UndoStarted)
                .count(),
            1
        );
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| event.kind == JobEventKind::UndoCompleted)
                .count(),
            1
        );
    }

    #[test]
    fn job_undo_refuses_legacy_unbound_applied_delivery_without_mutating_git() {
        let fixture = Fixture::new(false);
        let error =
            job_undo_with_verified_receipt(&fixture.paths, &fixture.view(), &fixture.receipt, true)
                .expect_err("legacy delivery must fail closed");

        assert!(
            error.to_string().contains("applied-receipt.json"),
            "{error}"
        );
        assert_eq!(
            git_stdout_at(&fixture.destination, &["rev-parse", "HEAD"]).expect("head"),
            fixture.applied
        );
    }

    #[test]
    fn dirty_destination_records_failed_attempt_and_never_runs_revert() {
        let fixture = Fixture::new(true);
        fs::write(fixture.destination.join("operator.txt"), "preserve\n").expect("operator dirt");

        let error =
            job_undo_with_verified_receipt(&fixture.paths, &fixture.view(), &fixture.receipt, true)
                .expect_err("dirty destination must fail closed");

        assert!(error.to_string().contains("not clean"), "{error}");
        assert_eq!(
            git_stdout_at(&fixture.destination, &["rev-parse", "HEAD"]).expect("head"),
            fixture.applied
        );
        assert_eq!(
            fs::read_to_string(fixture.destination.join("operator.txt")).expect("operator file"),
            "preserve\n"
        );
        let history = deadreckon_core::read_job_history(&fixture.paths.job_events(&fixture.job_id))
            .expect("history");
        assert!(
            history
                .events()
                .iter()
                .any(|event| event.kind == JobEventKind::UndoStarted)
        );
        assert!(
            history
                .events()
                .iter()
                .any(|event| event.kind == JobEventKind::UndoFailed)
        );
    }

    #[test]
    fn receipt_binding_mismatch_is_refused_before_started_evidence_or_git_change() {
        let mut fixture = Fixture::new(true);
        fixture.receipt.effective_policy_sha256 = "different-policy".to_string();

        let error =
            job_undo_with_verified_receipt(&fixture.paths, &fixture.view(), &fixture.receipt, true)
                .expect_err("mismatched receipt must fail closed");

        assert!(error.to_string().contains("does not match"), "{error}");
        assert_eq!(
            git_stdout_at(&fixture.destination, &["rev-parse", "HEAD"]).expect("head"),
            fixture.applied
        );
        let history = deadreckon_core::read_job_history(&fixture.paths.job_events(&fixture.job_id))
            .expect("history");
        assert!(history.events().iter().all(|event| !matches!(
            event.kind,
            JobEventKind::UndoStarted | JobEventKind::UndoCompleted | JobEventKind::UndoFailed
        )));
    }

    #[test]
    fn tampered_unsigned_event_cannot_redirect_signed_job_undo() {
        let fixture = Fixture::new(true);
        let view = fixture.view();
        let redirected = fixture._temp.path().join("redirected");
        fs::create_dir_all(&redirected).expect("redirect target");
        rewrite_applied_event_detail(&fixture.paths, &fixture.job_id, |detail| {
            detail["destination"] = json!(redirected);
        });

        let error = job_undo_with_verified_receipt(&fixture.paths, &view, &fixture.receipt, true)
            .expect_err("tampered event must fail closed");

        assert!(
            error.to_string().contains("changed or redirected"),
            "{error}"
        );
        assert_eq!(
            git_stdout_at(&fixture.destination, &["rev-parse", "HEAD"]).expect("head"),
            fixture.applied
        );
    }

    #[test]
    fn switched_target_ref_is_refused_before_job_undo_mutates_git() {
        let fixture = Fixture::new(true);
        git_status_at(&fixture.destination, &["switch", "-c", "other-target"])
            .expect("switch branch");

        let error =
            job_undo_with_verified_receipt(&fixture.paths, &fixture.view(), &fixture.receipt, true)
                .expect_err("switched ref must fail closed");

        assert!(error.to_string().contains("attached target ref"), "{error}");
        assert_eq!(
            git_stdout_at(&fixture.destination, &["rev-parse", "HEAD"]).expect("head"),
            fixture.applied
        );
    }

    #[test]
    fn detached_target_is_refused_before_job_undo_mutates_git() {
        let fixture = Fixture::new(true);
        git_status_at(
            &fixture.destination,
            &["switch", "--detach", &fixture.applied],
        )
        .expect("detach HEAD");

        let error =
            job_undo_with_verified_receipt(&fixture.paths, &fixture.view(), &fixture.receipt, true)
                .expect_err("detached target must fail closed");

        assert!(error.to_string().contains("detached HEAD"), "{error}");
        assert_eq!(
            git_stdout_at(&fixture.destination, &["rev-parse", "HEAD"]).expect("head"),
            fixture.applied
        );
    }
}
