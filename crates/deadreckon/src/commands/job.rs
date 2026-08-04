//! Durable job creation and detached supervisor launch.
//!
//! `start` resolves and approves the mutable inputs. This module freezes those
//! inputs before the first agent turn, writes the initial append-only control
//! facts, and only then starts a supervisor.

use super::super::*;

use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::Read;
use std::process::{Command, Stdio};

use chrono::{DateTime, Utc};
use deadreckon_protocol::{
    AppliedGitDeliveryReceipt, AuthorityAcceptedBy, DockerGateIdentity,
    GATE_EVALUATOR_IDENTITY_SCHEMA_VERSION, GATE_EVALUATOR_PROTOCOL_MARKER,
    GATE_EVALUATOR_PROTOCOL_VERSION, GateBinaryIdentity, GateEvaluatorIdentity, Job, JobAuthority,
    JobEvent, JobEventKind, JobEventSequence, JobExecutionPolicy, JobId, JobPolicy,
    JobSchemaVersion, JobShape, RunId, SemanticJudgeMode, StopReason,
};
use sha2::Sha256;

const JOB_ACCEPTANCE_FILE: &str = "acceptance.yaml";
const JOB_ACCEPTANCE_DOC: &str = "acceptance.md";
const JOB_ACCEPTANCE_HELPERS: &str = "acceptance";
const SUPERVISOR_LAUNCH_STDOUT: &str = "supervisor.out";
const SUPERVISOR_LAUNCH_STDERR: &str = "supervisor.err";
pub(crate) const DURABLE_SCOPE_ROOT_SIGNAL: &str = "watchkeeper_scope_root";
pub(crate) const DURABLE_CONTRACT_BUNDLE_SIGNAL: &str = "watchkeeper_contract_bundle";
const CONTRACT_BUNDLE_MAX_FILES: usize = 64;
const CONTRACT_BUNDLE_MAX_FILE_BYTES: u64 = 1_048_576;
const CONTRACT_BUNDLE_MAX_TOTAL_BYTES: u64 = 4_194_304;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrozenContractFile {
    pub(crate) path: String,
    pub(crate) sha256: String,
    pub(crate) bytes: u64,
    pub(crate) executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrozenContractBundle {
    pub(crate) schema_version: u32,
    pub(crate) files: Vec<FrozenContractFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DurableSourceMode {
    Worktree,
    Copy,
    Fresh,
    InitGit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct DurableSource {
    pub(crate) mode: DurableSourceMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) from: Option<PathBuf>,
    #[serde(default)]
    pub(crate) allow_dirty: bool,
}

pub(crate) struct CreateJob<'a> {
    pub(crate) paths: &'a DeadreckonPaths,
    pub(crate) source_cwd: &'a Path,
    pub(crate) scope: String,
    pub(crate) launch_plan: commands::course::LaunchPlan,
    pub(crate) shape: JobShape,
    pub(crate) driver: Option<commands::graph_job::DriverSpec>,
    pub(crate) contract_source: Option<&'a Path>,
    pub(crate) source: DurableSource,
    pub(crate) max_spend_usd: f64,
    pub(crate) max_wall_seconds: u64,
    pub(crate) max_attempts: u32,
    pub(crate) deadline: Option<DateTime<Utc>>,
    pub(crate) sandbox_requested: String,
    pub(crate) accepted_by: AuthorityAcceptedBy,
}

/// Convert the CLI/config wall-cap representation without silently widening
/// an invalid or sub-second approval into a different Job policy.
pub(crate) fn checked_job_wall_seconds(value: f64) -> Result<u64> {
    if !value.is_finite() || value <= 0.0 || value.fract() != 0.0 || value >= u64::MAX as f64 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable Job wall-clock cap must be a positive, finite, whole number of seconds that is representable as u64"
                .to_string(),
        )));
    }
    Ok(value as u64)
}

struct PendingJobDirectory {
    path: PathBuf,
    armed: bool,
}

impl PendingJobDirectory {
    fn create(path: &Path) -> Result<Self> {
        let parent = path.parent().ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "Job directory has no parent: {}",
                path.display()
            )))
        })?;
        fs::create_dir_all(parent)?;
        fs::create_dir(path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(Self {
            path: path.to_path_buf(),
            armed: true,
        })
    }

    fn commit(mut self) {
        self.armed = false;
    }
}

impl Drop for PendingJobDirectory {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let _ = fs::remove_dir_all(&self.path);
        #[cfg(unix)]
        if let Some(parent) = self.path.parent() {
            let _ = fs::File::open(parent).and_then(|directory| directory.sync_all());
        }
    }
}

pub(crate) fn create_job(mut request: CreateJob<'_>) -> Result<Job> {
    if !request.max_spend_usd.is_finite() || request.max_spend_usd <= 0.0 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable Job spend cap must be finite and greater than zero".to_string(),
        )));
    }
    if request.max_wall_seconds == 0 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable Job wall-clock cap must be greater than zero".to_string(),
        )));
    }
    if request.max_attempts == 0 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable Job attempt cap must be greater than zero".to_string(),
        )));
    }
    ensure_admission_deadline_future(request.deadline, Utc::now())?;
    if request
        .launch_plan
        .budget
        .ceiling_usd
        .is_some_and(|cap| !cap.is_finite() || cap <= 0.0)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable Job launch-plan spend cap must be finite and greater than zero".to_string(),
        )));
    }
    if request.launch_plan.budget.wall_seconds == Some(0) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable Job launch-plan wall-clock cap must be greater than zero".to_string(),
        )));
    }
    if request.sandbox_requested.trim() == "none" {
        return Err(CliError::Core(deadreckon_core::user_error(
            "durable Jobs require containment; sandbox `none` cannot be frozen as trusted execution policy",
            "use sandbox auto or an available sandbox-exec, bwrap, or docker backend",
        )));
    }
    request.launch_plan.budget.deadline = request.deadline;
    let expected_shape = match request.launch_plan.shape {
        commands::course::CourseShape::Single => JobShape::Single,
        commands::course::CourseShape::Plan => JobShape::Graph,
        commands::course::CourseShape::Campaign => JobShape::LegacyCampaign,
        commands::course::CourseShape::ChainExtend => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "legacy chain jobs remain process-bound; direct chain execution must compile to a durable linear graph".to_string(),
            )));
        }
    };
    if request.shape != expected_shape {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "durable job shape does not match its launch plan".to_string(),
        )));
    }
    if let Some(driver) = request.driver.as_ref() {
        commands::graph_job::embed_driver_spec(&mut request.launch_plan, driver)?;
    } else if request.shape != JobShape::Single {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "advanced durable jobs require an immutable driver specification".to_string(),
        )));
    }
    let job_id = JobId(Uuid::new_v4().simple().to_string());
    let job_dir = request.paths.job_dir(job_id.as_ref());
    let pending_job_directory = PendingJobDirectory::create(&job_dir)?;
    if matches!(request.source.mode, DurableSourceMode::Copy) {
        let requested = request.source.from.as_deref().unwrap_or(request.source_cwd);
        let requested = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            request.source_cwd.join(requested)
        };
        request.source.from = Some(requested.canonicalize().map_err(|error| {
            CliError::Core(deadreckon_core::user_error(
                &format!(
                    "copy source cannot be resolved: {} ({error})",
                    requested.display()
                ),
                "deadreckon start \"<goal>\" --from <existing-dir>",
            ))
        })?);
    }
    let authority_source_cwd = authority_source_cwd(&request.source, request.source_cwd, &job_dir)?;
    let scope_root = effective_scope_root(request.source_cwd)?;

    let contract_path = job_dir.join(JOB_ACCEPTANCE_FILE);
    let contract_bundle = freeze_contract_bundle(
        request.contract_source,
        &authority_source_cwd,
        &contract_path,
    )?;

    let mut signals = request
        .launch_plan
        .signals
        .as_object()
        .cloned()
        .unwrap_or_default();
    signals.insert(
        "watchkeeper_source".to_string(),
        serde_json::to_value(&request.source)?,
    );
    signals.insert(
        DURABLE_SCOPE_ROOT_SIGNAL.to_string(),
        serde_json::to_value(&scope_root)?,
    );
    signals.insert(
        DURABLE_CONTRACT_BUNDLE_SIGNAL.to_string(),
        serde_json::to_value(&contract_bundle)?,
    );
    request.launch_plan.signals = serde_json::Value::Object(signals);

    let launch_path = request.paths.job_launch_plan(job_id.as_ref());
    commands::course::save_launch_plan(&launch_path, &request.launch_plan)?;
    sync_file(&launch_path)?;
    let launch_plan_sha256 = deadreckon_core::flight::sha256_file(&launch_path)?;

    deadreckon_core::validate_strict_contract(&contract_path, job_id.as_ref()).map_err(|error| {
        CliError::Core(deadreckon_core::user_error(
            &format!("durable Job cannot start: {error}"),
            "review or create a behavioral contract with: deadreckon def-done \"what should count as done\"",
        ))
    })?;
    let contract_sha256 = deadreckon_core::flight::sha256_file(&contract_path)?;

    let gate_evaluator =
        freeze_gate_evaluator_identity(request.paths, job_id.as_ref(), &request.sandbox_requested)?;
    let gate_evaluator_sha256 = gate_evaluator_identity_sha256(&gate_evaluator)?;
    let mut execution = JobExecutionPolicy::workspace_only(request.sandbox_requested.clone());
    execution.gate_evaluator = Some(gate_evaluator);
    let policy = JobPolicy {
        max_spend_usd: request.max_spend_usd,
        max_wall_seconds: request.max_wall_seconds,
        max_attempts: request.max_attempts,
        deadline: request.deadline,
        semantic_judge: SemanticJudgeMode::Required,
        execution: Some(execution),
    };
    let effective_policy_sha256 = deadreckon_core::flight::sha256_text(
        &serde_json::to_string(&policy).map_err(|source| DeadreckonError::Json {
            path: job_dir.join("policy"),
            source,
        })?,
    );
    let source_tree_sha256 =
        deadreckon_core::flight::build_deliverable_file_index(&authority_source_cwd)?.tree_hash();
    let authority = JobAuthority {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: job_id.clone(),
        run_id: RunId(job_id.as_ref().to_string()),
        approved_at: Utc::now(),
        accepted_by: request.accepted_by,
        goal_sha256: deadreckon_core::flight::sha256_text(&request.launch_plan.goal),
        contract_sha256,
        effective_policy_sha256,
        launch_plan_sha256: launch_plan_sha256.clone(),
        source_tree_sha256,
        source_revision: if matches!(request.source.mode, DurableSourceMode::Fresh) {
            None
        } else {
            git_revision(&authority_source_cwd)
        },
        sandbox_requested: request.sandbox_requested,
        semantic_judge_mode: SemanticJudgeMode::Required,
        gate_evaluator_sha256: Some(gate_evaluator_sha256.clone()),
    };
    // Absolute deadlines continue advancing during contract authoring,
    // service repair and input freezing. Recheck at the last admission
    // boundary so start never queues a Job that is already terminal.
    ensure_admission_deadline_future(request.deadline, Utc::now())?;
    let authority_path = request.paths.job_authority(job_id.as_ref());
    write_json_synced(&authority_path, &authority)?;
    let authority_sha256 = deadreckon_core::flight::sha256_file(&authority_path)?;

    let job = Job {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: job_id.clone(),
        scope: request.scope,
        goal: request.launch_plan.goal,
        shape: request.shape,
        created_at: Utc::now(),
        source_cwd: authority_source_cwd,
        launch_plan_sha256,
        authority_sha256,
        policy,
    };
    deadreckon_core::write_job(request.paths, &job)?;
    for (sequence, kind, detail) in [
        (
            1,
            JobEventKind::Created,
            json!({ "shape": request.shape, "root_id": job_id.as_ref() }),
        ),
        (
            2,
            JobEventKind::ContractApproved,
            json!({
                "contract_sha256": authority.contract_sha256,
                "accepted_by": authority.accepted_by,
                "gate_evaluator_sha256": gate_evaluator_sha256,
            }),
        ),
        (
            3,
            JobEventKind::Queued,
            json!({ "reason": "approved inputs frozen before first agent turn" }),
        ),
    ] {
        let sequence = JobEventSequence::new(sequence).ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "initial job event sequence must be non-zero".to_string(),
            ))
        })?;
        deadreckon_core::append_job_event(
            request.paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: job_id.clone(),
                sequence,
                event_id: Uuid::new_v4().to_string(),
                causation_id: format!("start:{}", job_id.as_ref()),
                timestamp: Utc::now(),
                lease_epoch: 0,
                kind,
                detail,
            },
        )?;
    }
    pending_job_directory.commit();
    Ok(job)
}

fn ensure_admission_deadline_future(
    deadline: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<()> {
    if deadline.is_some_and(|deadline| deadline <= now) {
        return Err(CliError::Core(deadreckon_core::user_error(
            "the approved absolute deadline elapsed before the durable Job could be queued",
            "choose a later --deadline and rerun deadreckon start",
        )));
    }
    Ok(())
}

pub(crate) fn launch_detached_supervisor(paths: &DeadreckonPaths, job_id: &JobId) -> Result<()> {
    let executable = std::env::current_exe()?;
    let job_dir = paths.job_dir(job_id.as_ref());
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(job_dir.join(SUPERVISOR_LAUNCH_STDOUT))?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(job_dir.join(SUPERVISOR_LAUNCH_STDERR))?;
    let mut command = Command::new(executable);
    command
        .arg("supervisor")
        .arg("serve")
        .arg("--once")
        .arg(job_id.as_ref())
        .env("DEADRECKON_HOME", paths.home())
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    let (child, _terminator) = deadreckon_core::spawn_grouped(command)?;
    drop(child);
    Ok(())
}

pub(crate) fn job_acceptance_path(paths: &DeadreckonPaths, job_id: &str) -> PathBuf {
    paths.job_dir(job_id).join(JOB_ACCEPTANCE_FILE)
}

pub(crate) fn list_jobs(
    paths: &DeadreckonPaths,
    scope: Option<&str>,
) -> Result<Vec<deadreckon_core::JobView>> {
    let entries = match fs::read_dir(paths.jobs_dir()) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(source.into()),
    };
    let mut jobs = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(ToString::to_string))
        .filter_map(|id| deadreckon_core::JobView::load(paths, &id).ok())
        .filter(|view| scope.is_none_or(|scope| view.job.scope == scope))
        .collect::<Vec<_>>();
    jobs.sort_by_key(|view| view.projection.updated_at.unwrap_or(view.job.created_at));
    Ok(jobs)
}

pub(crate) fn job_status_label(view: &deadreckon_core::JobView) -> String {
    if view.verified_receipt_error.is_some() {
        return "verified_proof_invalid".to_string();
    }
    view.projection
        .outcome
        .map(serialized_label)
        .unwrap_or_else(|| serialized_label(view.projection.phase))
}

pub(crate) fn print_job_status(view: &deadreckon_core::JobView, json_output: bool) -> Result<()> {
    print_job_status_with_open_action(view, json_output, "attach")
}

pub(crate) fn print_job_status_after_attach(
    view: &deadreckon_core::JobView,
    json_output: bool,
) -> Result<()> {
    print_job_status_with_open_action(view, json_output, "status")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JobDeliveryKind {
    Exported,
}

/// Mirror one already-authenticated applied-delivery receipt into the Job event
/// history. This event is a lifecycle fact, never delivery or undo authority:
/// consumers must validate the protected signed receipt and require this fact
/// to match it exactly.
pub(crate) fn record_signed_applied_job_delivery(
    paths: &DeadreckonPaths,
    job_id: &str,
    receipt: &AppliedGitDeliveryReceipt,
    operation_lock: &deadreckon_core::JobOperationLock,
) -> Result<()> {
    if operation_lock.job_id() != job_id {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "cannot record delivery for Job {job_id} while holding the operation lock for Job {}",
            operation_lock.job_id()
        ))));
    }
    let validated = deadreckon_core::validate_applied_git_delivery_receipt_snapshot(paths, job_id)?;
    if &validated.receipt != receipt {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "signed applied delivery receipt changed before recording Job {job_id}"
        ))));
    }
    let detail = signed_applied_job_delivery_detail(receipt, &validated.sha256);
    let history = deadreckon_core::read_job_history(&paths.job_events(job_id))?;
    let existing = history
        .events()
        .iter()
        .filter(|event| event.kind == JobEventKind::ResultApplied)
        .collect::<Vec<_>>();
    if existing.len() > 1 || existing.first().is_some_and(|event| event.detail != detail) {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "Job {job_id} has conflicting unsigned applied-delivery history; the signed receipt was not redirected"
            ),
            &format!("deadreckon show {}", run_prefix(job_id)),
        )));
    }
    record_job_delivery_detail(paths, job_id, JobEventKind::ResultApplied, &detail)?;
    let history = deadreckon_core::read_job_history(&paths.job_events(job_id))?;
    let applied = history
        .events()
        .iter()
        .filter(|event| event.kind == JobEventKind::ResultApplied)
        .collect::<Vec<_>>();
    if applied.len() != 1 || applied[0].detail != detail {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "Job {job_id} has conflicting applied-delivery history; the signed receipt remains the only delivery authority"
            ),
            &format!("deadreckon show {}", run_prefix(job_id)),
        )));
    }
    Ok(())
}

pub(crate) fn signed_applied_job_delivery_detail(
    receipt: &AppliedGitDeliveryReceipt,
    delivery_receipt_sha256: &str,
) -> Value {
    json!({
        "delivery_receipt_sha256": delivery_receipt_sha256,
        "completion_receipt_sha256": receipt.completion_receipt_sha256,
        "delivery_intent_sha256": receipt.delivery_intent_sha256,
        "destination": receipt.repository.worktree_root,
        "git_common_dir": receipt.repository.git_common_dir,
        "target_ref": receipt.target_ref,
        "destination_revision_before": receipt.pre_revision,
        "resulting_revision": receipt.applied_revision,
        "source_revision": receipt.signed_source_revision,
        "result_revision": receipt.signed_result_revision,
        "effective_policy_sha256": receipt.effective_policy_sha256,
        "strategy": receipt.strategy,
    })
}

/// Record a successful operator export transition after it happens.
///
/// The event is idempotent for the same kind, destination and resulting
/// revision. It intentionally does not authorize delivery: `finish` validates
/// verified Git apply is deliberately excluded: it must pass through the
/// signed applied-receipt path above.
pub(crate) fn record_job_delivery(
    paths: &DeadreckonPaths,
    job_id: &str,
    kind: JobDeliveryKind,
    destination: &Path,
    resulting_revision: Option<&str>,
) -> Result<()> {
    let event_kind = match kind {
        JobDeliveryKind::Exported => JobEventKind::ResultExported,
    };
    let destination = destination.to_path_buf();
    let detail = json!({
        "destination": destination,
        "resulting_revision": resulting_revision,
    });
    record_job_delivery_detail(paths, job_id, event_kind, &detail)
}

fn record_job_delivery_detail(
    paths: &DeadreckonPaths,
    job_id: &str,
    event_kind: JobEventKind,
    detail: &Value,
) -> Result<()> {
    let event_fingerprint = deadreckon_core::flight::sha256_text(&format!(
        "{}:{}",
        serialized_label(event_kind),
        serde_json::to_string(&detail)?
    ));
    let event_id = format!("finish-delivery:{event_fingerprint}");
    let mut last_error = None;
    // A duplicate finish may race after both callers successfully delivered
    // the same result. The append API deliberately rejects stale sequences, so
    // reload and retry a small bounded number of times. If the peer appended
    // the same factual event, this call is already complete.
    for _ in 0..4 {
        let view = deadreckon_core::JobView::load(paths, job_id)?;
        if view.projection.outcome != Some(deadreckon_protocol::JobOutcome::Verified) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot record delivery for unverified job {job_id}"
            ))));
        }
        let history = deadreckon_core::read_job_history(&paths.job_events(job_id))?;
        if history
            .events()
            .iter()
            .any(|event| event.kind == event_kind && &event.detail == detail)
        {
            return Ok(());
        }
        let sequence =
            JobEventSequence::new(view.projection.last_sequence + 1).ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "job {job_id} event sequence exhausted"
                )))
            })?;
        match deadreckon_core::append_job_event(
            paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: view.job.job_id,
                sequence,
                event_id: event_id.clone(),
                causation_id: format!("finish:{job_id}"),
                timestamp: Utc::now(),
                lease_epoch: view.projection.current_lease_epoch,
                kind: event_kind,
                detail: detail.clone(),
            },
        ) {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(CliError::Core(last_error.unwrap_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "could not record delivery for job {job_id} after bounded retries"
        ))
    })))
}

/// Append one factual Job undo transition under the same per-Job control lock
/// used by every lifecycle event. `operation_id` is stable across crash
/// recovery, so repeated completion writes are byte-idempotent.
pub(crate) fn record_job_undo_event(
    paths: &DeadreckonPaths,
    job_id: &str,
    kind: JobEventKind,
    operation_id: &str,
    detail: &Value,
) -> Result<()> {
    if !matches!(
        kind,
        JobEventKind::UndoStarted | JobEventKind::UndoCompleted | JobEventKind::UndoFailed
    ) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "job undo recorder received a non-undo event".to_string(),
        )));
    }
    let suffix = serialized_label(kind);
    let event_id = format!("undo:{operation_id}:{suffix}");
    let mut last_error = None;
    for _ in 0..4 {
        let view = deadreckon_core::JobView::load(paths, job_id)?;
        if view.projection.outcome != Some(deadreckon_protocol::JobOutcome::Verified) {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot record undo evidence for unverified job {job_id}"
            ))));
        }
        let history = deadreckon_core::read_job_history(&paths.job_events(job_id))?;
        if let Some(existing) = history
            .events()
            .iter()
            .find(|event| event.event_id == event_id)
        {
            return if existing.kind == kind && &existing.detail == detail {
                Ok(())
            } else {
                Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "job {job_id} undo event {event_id} already has different evidence"
                ))))
            };
        }
        let sequence =
            JobEventSequence::new(view.projection.last_sequence + 1).ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "job {job_id} event sequence exhausted"
                )))
            })?;
        match deadreckon_core::append_job_event(
            paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: view.job.job_id,
                sequence,
                event_id: event_id.clone(),
                causation_id: format!("undo:{operation_id}"),
                timestamp: Utc::now(),
                lease_epoch: view.projection.current_lease_epoch,
                kind,
                detail: detail.clone(),
            },
        ) {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(CliError::Core(last_error.unwrap_or_else(|| {
        DeadreckonError::InvalidInput(format!(
            "could not record undo evidence for job {job_id} after bounded retries"
        ))
    })))
}

fn print_job_status_with_open_action(
    view: &deadreckon_core::JobView,
    json_output: bool,
    open_action: &str,
) -> Result<()> {
    let id = view.job.job_id.as_ref();
    let status = job_status_label(view);
    let next_action = format!(
        "deadreckon {} {}",
        job_primary_action(view, open_action),
        run_prefix(id)
    );
    let (process_durability, machine_restart_durability) =
        super::supervisor_service::guided_durability_labels();
    let delivery = view
        .projection
        .delivery
        .as_ref()
        .map(|delivery| serialized_label(delivery.kind))
        .unwrap_or_else(|| "-".to_string());
    let delivered_to = view
        .projection
        .delivery
        .as_ref()
        .map(|delivery| delivery.destination.display().to_string())
        .unwrap_or_else(|| "-".to_string());
    let proof_status = match (
        view.projection.outcome,
        view.verified_receipt_error.as_deref(),
    ) {
        (Some(deadreckon_protocol::JobOutcome::Verified), None) => "valid",
        (Some(deadreckon_protocol::JobOutcome::Verified), Some(_)) => "invalid",
        _ => "not-applicable",
    };
    if json_output {
        let paths = DeadreckonPaths::discover();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "job_status",
                "id": id,
                "status": status,
                "verified_proof": {
                    "status": proof_status,
                    "error": view.verified_receipt_error.as_deref(),
                },
                "next_actions": [&next_action],
                "try_lines": Vec::<String>::new(),
                "paths": job_status_paths(&paths, view),
                "durability": {
                    "process": process_durability,
                    "machine_restart": machine_restart_durability,
                },
                "job": view,
            }))?
        );
        return Ok(());
    }
    println!("{}", ui_heading("job"));
    print_kv_block(&[
        ("id", id),
        ("status", &status),
        ("goal", &view.job.goal),
        ("scope", &view.job.scope),
        ("attempts", &view.projection.attempt_count.to_string()),
        (
            "stop reason",
            &view
                .projection
                .stop_reason
                .map(serialized_label)
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("process durability", process_durability),
        ("machine restart", &machine_restart_durability),
        ("delivery", &delivery),
        ("delivered to", &delivered_to),
        ("verified proof", proof_status),
    ]);
    if let Some(error) = view.verified_receipt_error.as_deref() {
        println!();
        println!("  {} {}", ui_muted("proof error:"), error);
    }
    println!();
    println!("  {} {}", ui_muted("next:"), ui_command(next_action));
    Ok(())
}

fn job_status_paths(paths: &DeadreckonPaths, view: &deadreckon_core::JobView) -> Value {
    json!({
        "job": paths.job_dir(view.job.job_id.as_ref()),
        "source": &view.job.source_cwd,
    })
}

pub(crate) fn job_primary_action<'a>(
    view: &deadreckon_core::JobView,
    open_action: &'a str,
) -> &'a str {
    if !view.projection.is_terminal() {
        return open_action;
    }
    if view.verified_receipt_error.is_some() {
        return "status";
    }
    match view.projection.outcome {
        Some(deadreckon_protocol::JobOutcome::Verified) if view.projection.delivery.is_some() => {
            "report"
        }
        Some(deadreckon_protocol::JobOutcome::Verified) => "finish",
        _ => open_action,
    }
}

fn serialized_label<T: Serialize>(value: T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(ToString::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

pub(crate) fn cancel_job(
    paths: &DeadreckonPaths,
    view: &deadreckon_core::JobView,
    force: bool,
) -> Result<()> {
    if view.projection.is_terminal() {
        return print_job_status(view, false);
    }
    let sequence = JobEventSequence::new(view.projection.last_sequence + 1).ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "job {} event sequence exhausted",
            view.job.job_id
        )))
    })?;
    deadreckon_core::append_job_event(
        paths,
        &JobEvent {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: view.job.job_id.clone(),
            sequence,
            event_id: Uuid::new_v4().to_string(),
            causation_id: format!("operator-cancel:{}", view.job.job_id),
            timestamp: Utc::now(),
            lease_epoch: view.projection.current_lease_epoch,
            kind: JobEventKind::CancelRequested,
            detail: json!({ "stop_reason": StopReason::CancelRequested }),
        },
    )?;
    let mut supervised = Vec::new();
    let state = load_run(paths, view.job.job_id.as_ref()).ok();
    if let Some(state) = state.as_ref() {
        write_cancel_marker(state, "operator cancelled durable job")?;
    }
    // Cancellation freezes new Job-owned writes. Validate the complete nested
    // Campaign process inventory before the first signal so corrupt authority
    // cannot leave an outer driver stopped while an untrusted child survives.
    let campaign_inventory = commands::graph_job::validate_campaign_sub_process_inventory_for_job(
        paths,
        view.job.job_id.as_ref(),
    )?;
    let merge_repair_inventory =
        commands::graph_job::validate_merge_repair_process_inventory_for_job(
            paths,
            view.job.job_id.as_ref(),
        )?;
    let metadata_path = paths
        .job_dir(view.job.job_id.as_ref())
        .join("supervised-child.json");
    if let Ok(process) = deadreckon_core::read_supervised_process(&metadata_path) {
        supervised.push(process);
    }
    supervised.sort_by_key(|process| (process.pid, process.pgid));
    supervised.dedup();
    let grace = if force {
        Duration::ZERO
    } else {
        Duration::from_secs(2)
    };
    for process in supervised {
        let outcome = terminate_supervised_process(process, grace);
        if let deadreckon_core::TerminationOutcome::Failed(reason) = outcome {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "job {} cancellation was recorded but supervised process {} could not be stopped: {reason}",
                view.job.job_id, process.pid,
            ))));
        }
    }
    commands::graph_job::terminate_validated_campaign_sub_processes(
        paths,
        campaign_inventory,
        grace,
    )?;
    commands::graph_job::terminate_validated_merge_repair_processes(
        paths,
        merge_repair_inventory,
        grace,
    )?;
    if let Some(state) = state.as_ref() {
        reconcile_run_supervised_processes(state, grace, false)?;
    }
    reconcile_job_docker_executions(paths, &view.job)?;
    let updated = deadreckon_core::JobView::load(paths, view.job.job_id.as_ref())?;
    print_job_status(&updated, false)
}

pub(crate) fn reconcile_run_supervised_processes(
    state: &deadreckon_core::PipelineState,
    grace: Duration,
    boot_changed: bool,
) -> Result<()> {
    let directory = state.run_root.join("child-pids");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut paths = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    for path in paths {
        if is_docker_cid_sidecar(&path) {
            continue;
        }
        reconcile_run_supervised_process_path(&path, grace, boot_changed)?;
    }
    Ok(())
}

fn reconcile_run_supervised_process_path(
    path: &Path,
    grace: Duration,
    boot_changed: bool,
) -> Result<()> {
    match deadreckon_core::read_supervised_process_record(path) {
        Ok(record) => reconcile_guarded_process_record(path, &record, grace),
        Err(record_error) => {
            let process = match read_legacy_nested_supervised_process(path) {
                Ok(process) => process,
                Err(_error) if path_is_confirmed_absent(path)? => return Ok(()),
                Err(error) => {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                        "cannot reconcile supervised process record {}: {record_error}; legacy parse also failed: {error}",
                        path.display()
                    ))));
                }
            };
            if boot_changed {
                remove_supervised_file(path)?;
                return Ok(());
            }
            if deadreckon_core::pid_is_alive(process.pid) {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "cannot prove legacy supervised process {} from {} is dead because it has no boot and process-start identity",
                    process.pid,
                    path.display()
                ))));
            }
            remove_supervised_file(path)
        }
    }
}

fn is_docker_cid_sidecar(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(std::ffi::OsStr::to_str) else {
        return false;
    };
    path.extension() == Some(std::ffi::OsStr::new("cid"))
        && (name.starts_with("docker-gate-probe-") || name.starts_with("docker-gate-evaluate-"))
}

pub(crate) fn reconcile_job_docker_executions(paths: &DeadreckonPaths, job: &Job) -> Result<()> {
    let directory = paths.job_dir(job.job_id.as_ref()).join("docker-executions");
    let metadata = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(source.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "Docker execution records for job {} are not a trusted directory",
            job.job_id
        ))));
    }
    let mut records = fs::read_dir(&directory)?.collect::<std::result::Result<Vec<_>, _>>()?;
    records.sort_by_key(fs::DirEntry::file_name);
    for entry in records {
        let path = entry.path();
        if path.extension() != Some(OsStr::new("json")) {
            continue;
        }
        reconcile_job_docker_execution_path(&path, job)?;
    }
    Ok(())
}

fn reconcile_job_docker_execution_path(path: &Path, job: &Job) -> Result<()> {
    deadreckon_sandbox::reconcile_docker_execution_record_for_job(path, job.job_id.as_ref())
        .map_err(|error| CliError::Core(DeadreckonError::InvalidInput(error.to_string())))
}

fn path_is_confirmed_absent(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

fn read_legacy_nested_supervised_process(
    path: &Path,
) -> std::io::Result<deadreckon_core::SupervisedProcess> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct LegacySupervisedProcess {
        pid: u32,
        #[serde(default)]
        pgid: Option<u32>,
    }

    let raw = fs::read(path)?;
    let process = if raw.iter().copied().find(|byte| !byte.is_ascii_whitespace()) == Some(b'{') {
        let legacy: LegacySupervisedProcess = serde_json::from_slice(&raw)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        deadreckon_core::SupervisedProcess {
            pid: legacy.pid,
            pgid: legacy.pgid,
        }
    } else {
        deadreckon_core::read_supervised_process(path)?
    };
    if process.pid == 0
        || process
            .pgid
            .is_some_and(|pgid| pgid == 0 || pgid != process.pid)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid legacy supervised process identity",
        ));
    }
    Ok(process)
}

fn reconcile_guarded_process_record(
    path: &Path,
    record: &deadreckon_core::SupervisedProcessRecord,
    grace: Duration,
) -> Result<()> {
    use deadreckon_core::SupervisedProcessIdentity;

    match record.identity() {
        SupervisedProcessIdentity::DifferentBoot => {
            // A machine restart makes the old process identity impossible.
            // Remove stale control state without signalling a potentially
            // reused numeric PID.
        }
        SupervisedProcessIdentity::Current => {
            terminate_guarded_process(record, grace)?;
        }
        SupervisedProcessIdentity::Exited =>
        {
            #[cfg(unix)]
            if let Some(pgid) = record.process.pgid {
                use deadreckon_core::ChildTerminator as _;

                let pgid = i32::try_from(pgid).map_err(|_| {
                    CliError::Core(DeadreckonError::InvalidInput(format!(
                        "invalid process group in {}",
                        path.display()
                    )))
                })?;
                if let deadreckon_core::TerminationOutcome::Failed(reason) =
                    deadreckon_core::ProcessGroupTerminator::new(pgid).terminate(grace)
                {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                        "could not reconcile residual evaluator group {pgid} from {}: {reason}",
                        path.display()
                    ))));
                }
            }
        }
        SupervisedProcessIdentity::Reused => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "refused to signal reused pid {} from stale evaluator record {}",
                record.process.pid,
                path.display()
            ))));
        }
        SupervisedProcessIdentity::Unverifiable => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot verify evaluator identity in {}",
                path.display()
            ))));
        }
    }
    let removed = deadreckon_core::remove_supervised_process_record_if_same(path, record)?;
    if !removed && path.exists() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "evaluator identity changed while reconciling {}",
            path.display()
        ))));
    }
    Ok(())
}

fn terminate_guarded_process(
    record: &deadreckon_core::SupervisedProcessRecord,
    grace: Duration,
) -> Result<()> {
    #[cfg(unix)]
    {
        use deadreckon_core::ChildTerminator as _;

        let pgid =
            i32::try_from(record.process.pgid.unwrap_or(record.process.pid)).map_err(|_| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "invalid guarded process group {}",
                    record.process.pid
                )))
            })?;
        // The prepared -> running transition changes the target from a raw
        // child to a process-group leader. Sweep group, raw PID, then group
        // again so cancellation cannot lose that transition race.
        for outcome in [
            deadreckon_core::ProcessGroupTerminator::new(pgid).terminate(Duration::ZERO),
            deadreckon_core::RawPidTerminator::new(record.process.pid).terminate(grace),
            deadreckon_core::ProcessGroupTerminator::new(pgid).terminate(grace),
        ] {
            if let deadreckon_core::TerminationOutcome::Failed(reason) = outcome {
                return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "could not stop guarded evaluator {}: {reason}",
                    record.process.pid
                ))));
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        if let deadreckon_core::TerminationOutcome::Failed(reason) =
            terminate_supervised_process(record.process, grace)
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "could not stop guarded evaluator {}: {reason}",
                record.process.pid
            ))));
        }
        Ok(())
    }
}

fn remove_supervised_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn terminate_supervised_process(
    process: deadreckon_core::SupervisedProcess,
    grace: Duration,
) -> deadreckon_core::TerminationOutcome {
    use deadreckon_core::ChildTerminator as _;

    #[cfg(unix)]
    {
        process
            .pgid
            .and_then(|pgid| i32::try_from(pgid).ok())
            .map_or_else(
                || deadreckon_core::RawPidTerminator::new(process.pid).terminate(grace),
                |pgid| deadreckon_core::ProcessGroupTerminator::new(pgid).terminate(grace),
            )
    }
    #[cfg(not(unix))]
    {
        deadreckon_core::RawPidTerminator::new(process.pid).terminate(grace)
    }
}

fn authority_source_cwd(
    source: &DurableSource,
    requested_cwd: &Path,
    job_dir: &Path,
) -> Result<PathBuf> {
    match source.mode {
        DurableSourceMode::Fresh => {
            let approved_source = job_dir.join("approved-source");
            fs::create_dir_all(&approved_source)?;
            Ok(approved_source)
        }
        DurableSourceMode::Copy => {
            let source = source.from.as_deref().unwrap_or(requested_cwd);
            let source = if source.is_absolute() {
                source.to_path_buf()
            } else {
                requested_cwd.join(source)
            };
            if !source.is_dir() {
                return Err(CliError::Core(deadreckon_core::user_error(
                    &format!("copy source is not a directory: {}", source.display()),
                    "deadreckon start \"<goal>\" --from <existing-dir>",
                )));
            }
            let expected =
                deadreckon_core::flight::build_deliverable_file_index(&source)?.tree_hash();
            let approved_source = job_dir.join("approved-source");
            let staging = job_dir.join("approved-source-preparing");
            deadreckon_core::copy_deliverable_tree(&source, &staging)?;
            let copied =
                deadreckon_core::flight::build_deliverable_file_index(&staging)?.tree_hash();
            let source_after =
                deadreckon_core::flight::build_deliverable_file_index(&source)?.tree_hash();
            if copied != expected || source_after != expected {
                return Err(CliError::Core(deadreckon_core::user_error(
                    "approved source copy changed while it was being frozen",
                    "rerun the same deadreckon start command after source writes stop",
                )));
            }
            fs::rename(&staging, &approved_source)?;
            #[cfg(unix)]
            fs::File::open(job_dir)?.sync_all()?;
            Ok(approved_source)
        }
        DurableSourceMode::Worktree | DurableSourceMode::InitGit => Ok(requested_cwd.to_path_buf()),
    }
}

fn effective_scope_root(requested_cwd: &Path) -> Result<PathBuf> {
    let root = if let Some(root) = std::env::var_os("DEADRECKON_SCOPE_ROOT") {
        PathBuf::from(root)
    } else {
        deadreckon_core::find_git_root(requested_cwd)?
            .unwrap_or_else(|| requested_cwd.to_path_buf())
    };
    root.canonicalize().map_err(CliError::from)
}

fn freeze_contract_bundle(
    source: Option<&Path>,
    source_cwd: &Path,
    target: &Path,
) -> Result<FrozenContractBundle> {
    if let Some(source) = source {
        freeze_contract_file(source, target)?;
        let source_dir = source.parent().ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "done contract has no parent directory: {}",
                source.display()
            )))
        })?;
        let target_dir = target.parent().ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "frozen done contract has no parent directory: {}",
                target.display()
            )))
        })?;
        let source_doc = source_dir.join(JOB_ACCEPTANCE_DOC);
        if path_entry_exists(&source_doc)? {
            freeze_contract_file(&source_doc, &target_dir.join(JOB_ACCEPTANCE_DOC))?;
        }
        let source_helpers = source_dir.join(JOB_ACCEPTANCE_HELPERS);
        if path_entry_exists(&source_helpers)? {
            freeze_contract_helper_tree(&source_helpers, &target_dir.join(JOB_ACCEPTANCE_HELPERS))?;
        }
        let source_bundle = source_contract_bundle_inventory(source)?;
        let frozen_bundle = contract_bundle_inventory(target_dir)?;
        if source_bundle != frozen_bundle {
            return Err(CliError::Core(deadreckon_core::user_error(
                "done-contract bundle changed while it was being frozen",
                "rerun deadreckon start after contract writes stop",
            )));
        }
        return Ok(frozen_bundle);
    }
    let kind = deadreckon_core::acceptance_defaults::detect_project_kind(source_cwd);
    let checks = deadreckon_core::acceptance_defaults::default_checks_for(&kind, source_cwd)
        .into_iter()
        .map(portable_default_check)
        .collect::<Vec<_>>();
    #[derive(Serialize)]
    struct FrozenSpec {
        name: String,
        checks: Vec<deadreckon_core::AcceptanceCheck>,
    }
    let body = serde_yaml::to_string(&FrozenSpec {
        name: format!(
            "deadreckon detected {}",
            deadreckon_core::acceptance_defaults::kind_label(&kind)
        ),
        checks,
    })
    .map_err(|source| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "failed to freeze detected done contract: {source}"
        )))
    })?;
    fs::write(target, body)?;
    sync_file(target)?;
    contract_bundle_inventory(target.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "frozen done contract has no parent directory: {}",
            target.display()
        )))
    })?)
}

fn freeze_contract_file(source: &Path, target: &Path) -> Result<()> {
    let (bytes, metadata) = read_stable_contract_member(source)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut target_file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(target)?;
    target_file.write_all(&bytes)?;
    target_file.set_permissions(metadata.permissions())?;
    target_file.sync_all()?;
    drop(target_file);
    sync_file(target)?;
    let expected = contract_sha256(&bytes);
    if deadreckon_core::flight::sha256_file(target)? != expected {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "frozen done-contract bundle did not match its captured source: {}",
                target.display()
            ),
            "rerun deadreckon start after storage is healthy",
        )));
    }
    Ok(())
}

fn path_entry_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn freeze_contract_helper_tree(source: &Path, target: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "done-contract helper root is not a regular directory: {}",
                source.display()
            ),
            "replace symlinks and special files under .deadreckon/acceptance",
        )));
    }
    fs::create_dir_all(target)?;
    let mut entries = fs::read_dir(source)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let source_path = entry.path();
        let target_path = target.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)?;
        if metadata.file_type().is_symlink() {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "done-contract helper is a symlink: {}",
                    source_path.display()
                ),
                "replace acceptance helper symlinks with regular files",
            )));
        }
        if metadata.file_type().is_dir() {
            freeze_contract_helper_tree(&source_path, &target_path)?;
        } else if metadata.file_type().is_file() {
            freeze_contract_file(&source_path, &target_path)?;
        } else {
            return Err(CliError::Core(deadreckon_core::user_error(
                &format!(
                    "done-contract helper is not a regular file: {}",
                    source_path.display()
                ),
                "remove special files from .deadreckon/acceptance",
            )));
        }
    }
    Ok(())
}

fn contract_bundle_inventory(root: &Path) -> Result<FrozenContractBundle> {
    let mut files = Vec::new();
    inventory_contract_file(root, Path::new(JOB_ACCEPTANCE_FILE), &mut files)?;
    let doc = root.join(JOB_ACCEPTANCE_DOC);
    if path_entry_exists(&doc)? {
        inventory_contract_file(root, Path::new(JOB_ACCEPTANCE_DOC), &mut files)?;
    }
    let helpers = root.join(JOB_ACCEPTANCE_HELPERS);
    if path_entry_exists(&helpers)? {
        inventory_contract_helper_tree(root, &helpers, &mut files)?;
    }
    finalize_contract_bundle(files)
}

pub(crate) fn source_contract_bundle_inventory(source: &Path) -> Result<FrozenContractBundle> {
    let root = source.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "done contract has no parent directory: {}",
            source.display()
        )))
    })?;
    let root_metadata = fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.file_type().is_dir() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "done-contract bundle root is not a regular directory: {}",
                root.display()
            ),
            "replace the .deadreckon symlink or special file with a regular directory",
        )));
    }
    let mut files = Vec::new();
    inventory_contract_absolute(source, Path::new(JOB_ACCEPTANCE_FILE), &mut files)?;
    let doc = root.join(JOB_ACCEPTANCE_DOC);
    if path_entry_exists(&doc)? {
        inventory_contract_absolute(&doc, Path::new(JOB_ACCEPTANCE_DOC), &mut files)?;
    }
    let helpers = root.join(JOB_ACCEPTANCE_HELPERS);
    if path_entry_exists(&helpers)? {
        inventory_contract_helper_tree(root, &helpers, &mut files)?;
    }
    finalize_contract_bundle(files)
}

fn finalize_contract_bundle(mut files: Vec<FrozenContractFile>) -> Result<FrozenContractBundle> {
    files.sort_by(|left, right| left.path.cmp(&right.path));
    if files.len() > CONTRACT_BUNDLE_MAX_FILES {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "done-contract bundle contains {} files; the limit is {}",
                files.len(),
                CONTRACT_BUNDLE_MAX_FILES
            ),
            "combine or remove generated acceptance helpers before starting the Job",
        )));
    }
    let total = files.iter().try_fold(0u64, |total, file| {
        total.checked_add(file.bytes).ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "done-contract bundle byte count overflowed".to_string(),
            ))
        })
    })?;
    if total > CONTRACT_BUNDLE_MAX_TOTAL_BYTES {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "done-contract bundle contains {total} bytes; the limit is {CONTRACT_BUNDLE_MAX_TOTAL_BYTES}"
            ),
            "reduce generated acceptance helpers before starting the Job",
        )));
    }
    Ok(FrozenContractBundle {
        schema_version: 1,
        files,
    })
}

fn inventory_contract_helper_tree(
    root: &Path,
    directory: &Path,
    files: &mut Vec<FrozenContractFile>,
) -> Result<()> {
    let metadata = fs::symlink_metadata(directory)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "frozen done-contract helper directory is invalid: {}",
            directory.display()
        ))));
    }
    let mut entries = fs::read_dir(directory)?.collect::<std::result::Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "frozen done-contract helper is a symlink: {}",
                path.display()
            ))));
        }
        if metadata.file_type().is_dir() {
            inventory_contract_helper_tree(root, &path, files)?;
        } else if metadata.file_type().is_file() {
            let relative = path.strip_prefix(root).map_err(|_| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "frozen done-contract helper escaped its bundle".to_string(),
                ))
            })?;
            inventory_contract_file(root, relative, files)?;
        } else {
            return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "frozen done-contract helper is not a regular file: {}",
                path.display()
            ))));
        }
    }
    Ok(())
}

fn inventory_contract_file(
    root: &Path,
    relative: &Path,
    files: &mut Vec<FrozenContractFile>,
) -> Result<()> {
    let path = root.join(relative);
    inventory_contract_absolute(&path, relative, files)
}

fn inventory_contract_absolute(
    path: &Path,
    logical_relative: &Path,
    files: &mut Vec<FrozenContractFile>,
) -> Result<()> {
    let (bytes, metadata) = read_stable_contract_member(path)?;
    files.push(FrozenContractFile {
        path: logical_relative.to_string_lossy().replace('\\', "/"),
        sha256: contract_sha256(&bytes),
        bytes: u64::try_from(bytes.len()).map_err(|_| {
            CliError::Core(DeadreckonError::InvalidInput(
                "done-contract bundle member byte count overflowed".to_string(),
            ))
        })?,
        executable: contract_file_executable(&metadata),
    });
    Ok(())
}

pub(crate) fn contract_file_matches_inventory(
    path: &Path,
    expected: &FrozenContractFile,
) -> Result<bool> {
    let (bytes, metadata) = read_stable_contract_member(path)?;
    Ok(u64::try_from(bytes.len()).ok() == Some(expected.bytes)
        && contract_sha256(&bytes) == expected.sha256
        && contract_file_executable(&metadata) == expected.executable)
}

/// Capture a contract member through one no-follow descriptor, with a hard
/// byte ceiling and identity checks before and after the read. This prevents a
/// helper from becoming a symlink or growing without bound between inventory,
/// admission and Job freeze.
fn read_stable_contract_member(path: &Path) -> Result<(Vec<u8>, fs::Metadata)> {
    let before = fs::symlink_metadata(path)?;
    if before.file_type().is_symlink() || !before.file_type().is_file() {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "done-contract bundle member is not a regular file: {}",
                path.display()
            ),
            "replace symlinks and special files under .deadreckon with regular files",
        )));
    }
    if before.len() > CONTRACT_BUNDLE_MAX_FILE_BYTES {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "done-contract bundle member exceeds {} bytes: {}",
                CONTRACT_BUNDLE_MAX_FILE_BYTES,
                path.display()
            ),
            "reduce generated acceptance helpers before starting the Job",
        )));
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    let opened = file.metadata()?;
    if !stable_contract_metadata(&before, &opened) {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "done-contract bundle member changed identity while it was opened: {}",
                path.display()
            ),
            "rerun deadreckon start after contract writes stop",
        )));
    }

    let limit = CONTRACT_BUNDLE_MAX_FILE_BYTES.saturating_add(1);
    let mut bytes = Vec::with_capacity(usize::try_from(opened.len()).unwrap_or(0));
    (&mut file).take(limit).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > CONTRACT_BUNDLE_MAX_FILE_BYTES {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "done-contract bundle member exceeded {} bytes while it was read: {}",
                CONTRACT_BUNDLE_MAX_FILE_BYTES,
                path.display()
            ),
            "reduce generated acceptance helpers before starting the Job",
        )));
    }
    let after = file.metadata()?;
    let post_path = fs::symlink_metadata(path)?;
    if !stable_contract_metadata(&opened, &after)
        || !stable_contract_metadata(&after, &post_path)
        || u64::try_from(bytes.len()).ok() != Some(after.len())
    {
        return Err(CliError::Core(deadreckon_core::user_error(
            &format!(
                "done-contract bundle member changed while its bytes were captured: {}",
                path.display()
            ),
            "rerun deadreckon start after contract writes stop",
        )));
    }
    Ok((bytes, after))
}

fn contract_sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", <Sha256 as sha2::Digest>::digest(bytes))
}

#[cfg(unix)]
fn stable_contract_metadata(left: &fs::Metadata, right: &fs::Metadata) -> bool {
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
fn stable_contract_metadata(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.file_type().is_file()
        && right.file_type().is_file()
        && left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
}

#[cfg(unix)]
fn contract_file_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn contract_file_executable(_metadata: &fs::Metadata) -> bool {
    false
}

pub(crate) fn contract_bundle_from_plan(
    plan: &commands::course::LaunchPlan,
) -> Result<Option<FrozenContractBundle>> {
    plan.signals
        .get(DURABLE_CONTRACT_BUNDLE_SIGNAL)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(CliError::from)
}

pub(crate) fn validate_frozen_contract_bundle(
    paths: &DeadreckonPaths,
    job_id: &str,
    plan: &commands::course::LaunchPlan,
) -> Result<()> {
    let Some(expected) = contract_bundle_from_plan(plan)? else {
        return Ok(());
    };
    if expected.schema_version != 1 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "unsupported frozen done-contract bundle schema {} for job {job_id}",
            expected.schema_version
        ))));
    }
    let actual = contract_bundle_inventory(&paths.job_dir(job_id))?;
    if actual != expected {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "done-contract bundle changed after job {job_id} was approved"
        ))));
    }
    Ok(())
}

fn freeze_gate_evaluator_identity(
    paths: &DeadreckonPaths,
    job_id: &str,
    sandbox_requested: &str,
) -> Result<GateEvaluatorIdentity> {
    let controller_os = host_gate_os()?;
    let controller_arch = host_gate_arch()?;
    let (_, controller_bytes) =
        read_compatible_installed_gate("dr-gate", controller_os, controller_arch, false)?;
    let controller_target = paths.job_frozen_controller_gate(job_id);
    let controller_sha256 = freeze_executable(&controller_target, &controller_bytes)?;
    let controller = GateBinaryIdentity {
        sha256: controller_sha256,
        os: controller_os.to_string(),
        arch: controller_arch.to_string(),
    };

    let (evaluator, docker) = if sandbox_requested == "docker" {
        let image = deadreckon_sandbox::inspect_docker_image(OsStr::new("rust:1"))
            .map_err(|error| {
                CliError::Core(DeadreckonError::InvalidInput(format!(
                    "Docker containment requires the cached rust:1 image before approval ({error}); run `docker pull rust:1` explicitly, then retry"
                )))
            })?;
        let (arch, sidecar_name) = match image.platform() {
            deadreckon_sandbox::DockerPlatform::LinuxArm64 => {
                ("aarch64", "dr-gate-evaluator-aarch64-unknown-linux-musl")
            }
            deadreckon_sandbox::DockerPlatform::LinuxAmd64 => {
                ("x86_64", "dr-gate-evaluator-x86_64-unknown-linux-musl")
            }
        };
        let (_, evaluator_bytes) =
            read_compatible_installed_gate(sidecar_name, "linux", arch, true)?;
        let evaluator_target = paths.job_frozen_evaluator_gate(job_id);
        let evaluator_sha256 = freeze_executable(&evaluator_target, &evaluator_bytes)?;
        (
            GateBinaryIdentity {
                sha256: evaluator_sha256,
                os: "linux".to_string(),
                arch: arch.to_string(),
            },
            Some(DockerGateIdentity {
                image_id: image.id().to_string(),
                platform: image.platform().as_str().to_string(),
                guest_path: PathBuf::from(deadreckon_protocol::DOCKER_GATE_GUEST_PATH),
            }),
        )
    } else {
        let evaluator_target = paths.job_frozen_evaluator_gate(job_id);
        let evaluator_sha256 = freeze_executable(&evaluator_target, &controller_bytes)?;
        (
            GateBinaryIdentity {
                sha256: evaluator_sha256,
                os: controller_os.to_string(),
                arch: controller_arch.to_string(),
            },
            None,
        )
    };

    Ok(GateEvaluatorIdentity {
        schema_version: GATE_EVALUATOR_IDENTITY_SCHEMA_VERSION,
        protocol_version: GATE_EVALUATOR_PROTOCOL_VERSION,
        controller,
        evaluator,
        docker,
    })
}

fn gate_evaluator_identity_sha256(identity: &GateEvaluatorIdentity) -> Result<String> {
    let raw = serde_json::to_string(identity).map_err(|source| {
        CliError::Core(DeadreckonError::Json {
            path: PathBuf::from("gate-evaluator-identity"),
            source,
        })
    })?;
    Ok(deadreckon_core::flight::sha256_text(&raw))
}

fn installed_gate_candidates(name: &str) -> Result<Vec<PathBuf>> {
    let current = std::env::current_exe().map_err(|source| {
        CliError::Core(DeadreckonError::Io {
            path: PathBuf::from("current-exe"),
            source,
        })
    })?;
    let mut roots = Vec::new();
    for executable in [current.clone(), current.canonicalize().unwrap_or(current)] {
        let Some(parent) = executable.parent() else {
            continue;
        };
        roots.push(parent.to_path_buf());
        if parent.file_name() == Some(OsStr::new("deps"))
            && let Some(target_dir) = parent.parent()
        {
            roots.push(target_dir.to_path_buf());
        }
        if let Some(prefix) = parent.parent() {
            roots.push(prefix.join("libexec"));
            roots.push(prefix.join("libexec").join("deadreckon"));
        }
    }
    roots.sort();
    roots.dedup();

    let native_name = if name == "dr-gate" {
        format!("{name}{}", std::env::consts::EXE_SUFFIX)
    } else {
        name.to_string()
    };
    Ok(roots
        .into_iter()
        .map(|root| root.join(&native_name))
        .filter(|candidate| candidate.exists())
        .collect())
}

fn read_compatible_installed_gate(
    name: &str,
    os: &str,
    arch: &str,
    require_static_elf: bool,
) -> Result<(PathBuf, Vec<u8>)> {
    let candidates = installed_gate_candidates(name)?;
    if candidates.is_empty() {
        return Err(CliError::Core(DeadreckonError::NotFound(format!(
            "trusted release helper {name} next to the DeadReckon installation; reinstall the matching release, or for a source build run `cargo build --release --workspace --locked`"
        ))));
    }

    let mut rejected = Vec::new();
    for candidate in candidates {
        let attempt = (|| {
            let bytes = read_stable_executable(&candidate)?;
            validate_gate_binary(&bytes, os, arch, require_static_elf, &candidate)?;
            validate_gate_protocol(&bytes, &candidate)?;
            Ok::<_, CliError>(bytes)
        })();
        match attempt {
            Ok(bytes) => return Ok((candidate, bytes)),
            Err(error) => rejected.push(format!("{} ({error})", candidate.display())),
        }
    }

    Err(CliError::Core(DeadreckonError::InvalidInput(format!(
        "no compatible {name} helper was found for gate protocol {GATE_EVALUATOR_PROTOCOL_VERSION}; rejected {}; reinstall the matching release, or for a source build run `cargo build --release --workspace --locked`",
        rejected.join(", ")
    ))))
}

pub(crate) fn inspect_compatible_host_gate() -> Result<PathBuf> {
    let (path, _) =
        read_compatible_installed_gate("dr-gate", host_gate_os()?, host_gate_arch()?, false)?;
    Ok(path)
}

fn read_stable_executable(path: &Path) -> Result<Vec<u8>> {
    let before = fs::symlink_metadata(path)?;
    if !before.file_type().is_file() || before.file_type().is_symlink() {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "gate executable must be a regular non-symlink file: {}",
            path.display()
        ))));
    }
    const MAX_GATE_BYTES: u64 = 256 * 1024 * 1024;
    if before.len() == 0 || before.len() > MAX_GATE_BYTES {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "gate executable has an invalid bounded size: {}",
            path.display()
        ))));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    let opened = file.metadata()?;
    let mut bytes = Vec::with_capacity(usize::try_from(opened.len()).unwrap_or(0));
    file.read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    let post_path = fs::symlink_metadata(path)?;
    if !stable_gate_metadata(&before, &opened)
        || !stable_gate_metadata(&opened, &after)
        || !stable_gate_metadata(&after, &post_path)
        || u64::try_from(bytes.len()).ok() != Some(after.len())
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "gate executable changed while its trusted bytes were read: {}",
            path.display()
        ))));
    }
    Ok(bytes)
}

#[cfg(unix)]
fn stable_gate_metadata(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.file_type().is_file()
        && right.file_type().is_file()
}

#[cfg(not(unix))]
fn stable_gate_metadata(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len()
        && left.modified().ok() == right.modified().ok()
        && left.file_type().is_file()
        && right.file_type().is_file()
}

fn freeze_executable(path: &Path, bytes: &[u8]) -> Result<String> {
    let parent = path.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "frozen gate path has no parent: {}",
            path.display()
        )))
    })?;
    fs::create_dir_all(parent)?;
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o500))?;
        fs::File::open(parent)?.sync_all()?;
    }
    let expected = format!("sha256:{:x}", <Sha256 as sha2::Digest>::digest(bytes));
    let actual = deadreckon_core::flight::sha256_file(path)?;
    if actual != expected {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "frozen gate digest mismatch at {}",
            path.display()
        ))));
    }
    Ok(actual)
}

fn host_gate_os() -> Result<&'static str> {
    match std::env::consts::OS {
        "macos" => Ok("macos"),
        "linux" => Ok("linux"),
        "windows" => Ok("windows"),
        other => Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "unsupported gate controller operating system {other}"
        )))),
    }
}

fn host_gate_arch() -> Result<&'static str> {
    match std::env::consts::ARCH {
        "aarch64" => Ok("aarch64"),
        "x86_64" => Ok("x86_64"),
        other => Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "unsupported gate controller architecture {other}"
        )))),
    }
}

fn validate_gate_binary(
    bytes: &[u8],
    os: &str,
    arch: &str,
    require_static_elf: bool,
    path: &Path,
) -> Result<()> {
    let valid = match os {
        "macos" => {
            bytes.get(0..4) == Some(&[0xcf, 0xfa, 0xed, 0xfe])
                && read_u32_le(bytes, 4)
                    == Some(match arch {
                        "aarch64" => 0x0100_000c,
                        "x86_64" => 0x0100_0007,
                        _ => 0,
                    })
        }
        "linux" => {
            let expected_machine = match arch {
                "aarch64" => 183,
                "x86_64" => 62,
                _ => 0,
            };
            bytes.get(0..6) == Some(&[0x7f, b'E', b'L', b'F', 2, 1])
                && read_u16_le(bytes, 18) == Some(expected_machine)
                && (!require_static_elf || !elf_has_interpreter(bytes))
        }
        "windows" => {
            pe_machine(bytes)
                == Some(match arch {
                    "x86_64" => 0x8664,
                    _ => 0,
                })
        }
        _ => false,
    };
    if !valid {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "gate executable {} is not a compatible {} {}{} binary",
            path.display(),
            os,
            arch,
            if require_static_elf { " static" } else { "" }
        ))));
    }
    Ok(())
}

fn validate_gate_protocol(bytes: &[u8], path: &Path) -> Result<()> {
    let marker = GATE_EVALUATOR_PROTOCOL_MARKER.as_bytes();
    if !bytes.windows(marker.len()).any(|window| window == marker) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "gate executable {} does not implement required protocol {}",
            path.display(),
            GATE_EVALUATOR_PROTOCOL_VERSION
        ))));
    }
    let build_id = env!("DEADRECKON_BUNDLE_BUILD_ID").as_bytes();
    if !bytes
        .windows(build_id.len())
        .any(|window| window == build_id)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "gate executable {} belongs to a different DeadReckon build bundle",
            path.display()
        ))));
    }
    Ok(())
}

fn elf_has_interpreter(bytes: &[u8]) -> bool {
    if bytes.get(0..6) != Some(&[0x7f, b'E', b'L', b'F', 2, 1]) {
        return false;
    }
    let Some(offset) = read_u64_le(bytes, 32).and_then(|value| usize::try_from(value).ok()) else {
        return true;
    };
    let Some(entry_size) = read_u16_le(bytes, 54).map(usize::from) else {
        return true;
    };
    let Some(count) = read_u16_le(bytes, 56).map(usize::from) else {
        return true;
    };
    if entry_size < 4 {
        return true;
    }
    (0..count).any(|index| {
        offset
            .checked_add(index.saturating_mul(entry_size))
            .and_then(|start| read_u32_le(bytes, start))
            == Some(3)
    })
}

fn pe_machine(bytes: &[u8]) -> Option<u16> {
    if bytes.get(0..2) != Some(b"MZ") {
        return None;
    }
    let offset = read_u32_le(bytes, 0x3c).and_then(|value| usize::try_from(value).ok())?;
    if bytes.get(offset..offset.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    read_u16_le(bytes, offset.checked_add(4)?)
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn read_u64_le(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset.checked_add(8)?)?.try_into().ok()?,
    ))
}

fn portable_default_check(
    check: deadreckon_core::AcceptanceCheck,
) -> deadreckon_core::AcceptanceCheck {
    match check {
        deadreckon_core::AcceptanceCheck::Shell {
            command, must_pass, ..
        } => deadreckon_core::AcceptanceCheck::Shell {
            command,
            cwd: Some("{working_dir}".to_string()),
            must_pass,
        },
        other => other,
    }
}

pub(crate) fn write_json_synced<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "job artifact path has no parent: {}",
            path.display()
        )))
    })?;
    fs::create_dir_all(parent)?;
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    serde_json::to_writer_pretty(&mut file, value).map_err(|source| {
        CliError::Core(DeadreckonError::Json {
            path: path.to_path_buf(),
            source,
        })
    })?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

/// Durably replace a mutable controller-owned JSON projection.
///
/// Immutable authority files use `write_json_synced` and refuse replacement.
/// A small number of active recovery pointers must advance atomically while
/// their prior value remains in an immutable archive.
pub(crate) fn replace_json_synced<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "job artifact path has no parent: {}",
            path.display()
        )))
    })?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temp, value).map_err(|source| {
        CliError::Core(DeadreckonError::Json {
            path: path.to_path_buf(),
            source,
        })
    })?;
    temp.write_all(b"\n")?;
    temp.as_file_mut().sync_all()?;
    temp.persist(path).map_err(|error| {
        CliError::Core(DeadreckonError::Io {
            path: path.to_path_buf(),
            source: error.error,
        })
    })?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn sync_file(path: &Path) -> Result<()> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

fn git_revision(cwd: &Path) -> Option<String> {
    let output = deadreckon_core::git::run_git(cwd, &["rev-parse", "HEAD"]).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|revision| !revision.is_empty())
}

#[cfg(test)]
mod tests {
    use deadreckon_protocol::{AuthorityAcceptedBy, JobEventKind, SemanticJudgeMode};
    use tempfile::TempDir;

    use super::*;

    #[test]
    fn gate_helper_requires_protocol_and_exact_bundle_build_identity() {
        let compatible = format!(
            "prefix{}middle{}suffix",
            GATE_EVALUATOR_PROTOCOL_MARKER,
            env!("DEADRECKON_BUNDLE_BUILD_ID")
        );
        validate_gate_protocol(compatible.as_bytes(), Path::new("dr-gate"))
            .expect("matching gate helper");

        let stale_same_protocol = format!(
            "prefix{}middledeadreckon-bundle-build-id-sha256:{}suffix",
            GATE_EVALUATOR_PROTOCOL_MARKER,
            "0".repeat(64)
        );
        let error = validate_gate_protocol(stale_same_protocol.as_bytes(), Path::new("dr-gate"))
            .expect_err("same-version stale bundle must be rejected");
        assert!(
            error
                .to_string()
                .contains("different DeadReckon build bundle")
        );

        let error = validate_gate_protocol(
            env!("DEADRECKON_BUNDLE_BUILD_ID").as_bytes(),
            Path::new("dr-gate"),
        )
        .expect_err("missing protocol marker must be rejected");
        assert!(error.to_string().contains("required protocol"));
    }

    fn request<'a>(
        paths: &'a DeadreckonPaths,
        source: &'a Path,
        contract: Option<&'a Path>,
    ) -> CreateJob<'a> {
        if contract.is_none() {
            fs::create_dir_all(source).expect("fixture source");
            fs::write(source.join("fixture-proof.txt"), "approved fixture\n")
                .expect("fixture proof");
            fs::write(
                source.join("Makefile"),
                "test:\n\t@test -f fixture-proof.txt\n",
            )
            .expect("fixture test contract");
        }
        let mut plan = commands::course::trivial_operator_plan(
            "finish the durable fixture",
            commands::course::CourseShape::Single,
            "test",
        );
        plan.accepted_by = Some("operator".to_string());
        CreateJob {
            paths,
            source_cwd: source,
            scope: deadreckon_core::paths::workspace_scope(source).expect("fixture scope"),
            launch_plan: plan,
            shape: JobShape::Single,
            driver: None,
            contract_source: contract,
            source: DurableSource {
                mode: DurableSourceMode::Copy,
                from: Some(source.to_path_buf()),
                allow_dirty: false,
            },
            max_spend_usd: 4.0,
            max_wall_seconds: 90,
            max_attempts: 2,
            deadline: None,
            sandbox_requested: "auto".to_string(),
            accepted_by: AuthorityAcceptedBy::Operator,
        }
    }

    fn copy_job_fixture() -> (TempDir, DeadreckonPaths, PathBuf, Job) {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let launch_source = temp.path().join("launch");
        let copy_source = launch_source.join("fixtures/source");
        fs::create_dir_all(&copy_source).expect("copy source");
        fs::write(copy_source.join("README.md"), "copy").expect("copy file");
        fs::write(copy_source.join("fixture-proof.txt"), "approved fixture\n").expect("copy proof");
        fs::write(
            copy_source.join("Makefile"),
            "test:\n\t@test -f fixture-proof.txt\n",
        )
        .expect("copy test contract");
        let mut request = request(&paths, &launch_source, None);
        request.launch_plan.shape = commands::course::CourseShape::Plan;
        request.shape = JobShape::Graph;
        request.driver = Some(commands::graph_job::DriverSpec {
            kind: commands::graph_job::DriverKind::FullPlan,
            child_count: Some(2),
            apply: deadreckon_core::plan::ApplyWhen::AtEnd,
            planner_provider: Some("smoke".to_string()),
            child_provider: Some("smoke".to_string()),
            child_provider_overrides: Vec::new(),
            coder_provider: None,
            reviewer_provider: None,
            planner_model: None,
            child_model: None,
            child_model_overrides: Vec::new(),
            coder_model: None,
            reviewer_model: None,
            model: None,
            source_init_git: true,
        });
        request.source = DurableSource {
            mode: DurableSourceMode::Copy,
            from: Some(PathBuf::from("fixtures/source")),
            allow_dirty: false,
        };
        let job = create_job(request).expect("create copy job");
        let copy_source = copy_source.canonicalize().expect("canonical copy source");
        (temp, paths, copy_source, job)
    }

    #[test]
    fn wall_cap_conversion_never_rewrites_an_invalid_or_fractional_approval() {
        assert_eq!(checked_job_wall_seconds(90.0).expect("whole cap"), 90);
        for invalid in [
            0.0,
            -1.0,
            0.5,
            1.5,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
            u64::MAX as f64,
        ] {
            let error = checked_job_wall_seconds(invalid)
                .expect_err("invalid wall approval must fail closed");
            assert!(error.to_string().contains("wall-clock cap"), "{error}");
        }
    }

    #[test]
    fn central_job_creation_rejects_invalid_approved_limits_before_persistence() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");

        for invalid_spend in [0.0, -1.0, f64::INFINITY, f64::NAN] {
            let mut invalid = request(&paths, &source, None);
            invalid.max_spend_usd = invalid_spend;
            let error = create_job(invalid).expect_err("invalid spend cap must fail closed");
            assert!(error.to_string().contains("spend cap"), "{error}");
        }

        let mut zero_wall = request(&paths, &source, None);
        zero_wall.max_wall_seconds = 0;
        let error = create_job(zero_wall).expect_err("zero wall cap must fail closed");
        assert!(error.to_string().contains("wall-clock cap"), "{error}");

        let mut zero_attempts = request(&paths, &source, None);
        zero_attempts.max_attempts = 0;
        let error = create_job(zero_attempts).expect_err("zero attempt cap must fail closed");
        assert!(error.to_string().contains("attempt cap"), "{error}");

        let mut invalid_plan_spend = request(&paths, &source, None);
        invalid_plan_spend.launch_plan.budget.ceiling_usd = Some(f64::NAN);
        let error = create_job(invalid_plan_spend)
            .expect_err("invalid embedded spend cap must fail closed");
        assert!(
            error.to_string().contains("launch-plan spend cap"),
            "{error}"
        );

        let mut zero_plan_wall = request(&paths, &source, None);
        zero_plan_wall.launch_plan.budget.wall_seconds = Some(0);
        let error =
            create_job(zero_plan_wall).expect_err("zero embedded wall cap must fail closed");
        assert!(
            error.to_string().contains("launch-plan wall-clock cap"),
            "{error}"
        );

        assert!(
            !paths.jobs_dir().exists(),
            "invalid approved policy must fail before creating durable state"
        );
    }

    #[test]
    fn elapsed_admission_deadline_creates_no_job() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let mut expired = request(&paths, &source, None);
        expired.deadline = Some(Utc::now() - chrono::TimeDelta::seconds(1));

        let error = create_job(expired).expect_err("elapsed deadline must refuse admission");
        assert!(error.to_string().contains("elapsed before"), "{error}");
        assert!(
            !paths.jobs_dir().exists(),
            "an already elapsed deadline must fail before durable identity"
        );
    }

    #[test]
    fn final_deadline_boundary_refuses_crossing_without_extending_it() {
        let approved = Utc::now() + chrono::TimeDelta::seconds(30);
        ensure_admission_deadline_future(
            Some(approved),
            approved - chrono::TimeDelta::nanoseconds(1),
        )
        .expect("still-future deadline");
        let error = ensure_admission_deadline_future(Some(approved), approved)
            .expect_err("deadline crossing must refuse at the final boundary");
        assert!(error.to_string().contains("choose a later --deadline"));
    }

    #[test]
    fn approved_inputs_exist_before_job_is_queued() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("README.md"), "durable").expect("source file");
        let contract = source.join("acceptance.yaml");
        fs::write(
            &contract,
            "name: durable fixture\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("contract");

        let deadline = Utc::now() + chrono::TimeDelta::hours(1);
        let mut approved = request(&paths, &source, Some(&contract));
        approved.deadline = Some(deadline);
        let job = create_job(approved).expect("create job");
        let authority_path = paths.job_authority(job.job_id.as_ref());
        let authority: JobAuthority =
            serde_json::from_slice(&fs::read(&authority_path).expect("authority bytes"))
                .expect("authority");
        assert_eq!(
            authority.contract_sha256,
            deadreckon_core::flight::sha256_file(&job_acceptance_path(&paths, job.job_id.as_ref()))
                .expect("contract digest")
        );
        assert_eq!(authority.semantic_judge_mode, SemanticJudgeMode::Required);
        assert_eq!(job.policy.deadline, Some(deadline));
        let launch =
            commands::course::load_launch_plan(&paths.job_launch_plan(job.job_id.as_ref()))
                .expect("launch plan");
        assert_eq!(launch.budget.deadline, Some(deadline));
        let execution = job
            .policy
            .execution
            .as_ref()
            .expect("immutable execution policy");
        assert!(execution.require_containment);
        assert_eq!(execution.sandbox_requested, "auto");
        assert!(execution.tools.contains_key("bash"));
        assert!(execution.tools.contains_key("write_file"));
        assert!(execution.tools.values().all(|tool| {
            tool.workspace_read && tool.workspace_write && tool.network_allowlist.is_empty()
        }));
        let gate_evaluator = execution
            .gate_evaluator
            .as_ref()
            .expect("immutable gate evaluator identity");
        assert!(
            gate_evaluator.docker.is_none(),
            "an auto-contained Job freezes the host-native evaluator"
        );
        let gate_evaluator_sha256 = deadreckon_core::gate_evaluator_identity_sha256(gate_evaluator)
            .expect("gate evaluator identity digest");
        assert_eq!(
            authority.gate_evaluator_sha256.as_deref(),
            Some(gate_evaluator_sha256.as_str())
        );
        let controller_path = paths.job_frozen_controller_gate(job.job_id.as_ref());
        let evaluator_path = paths.job_frozen_evaluator_gate(job.job_id.as_ref());
        assert_eq!(
            deadreckon_core::flight::sha256_file(&controller_path)
                .expect("frozen controller digest"),
            gate_evaluator.controller.sha256
        );
        assert_eq!(
            deadreckon_core::flight::sha256_file(&evaluator_path).expect("frozen evaluator digest"),
            gate_evaluator.evaluator.sha256
        );
        assert_eq!(
            fs::read(&controller_path).expect("frozen controller"),
            fs::read(&evaluator_path).expect("frozen evaluator"),
            "non-Docker Jobs use two independently frozen copies of the approved native helper"
        );
        #[cfg(unix)]
        for path in [&controller_path, &evaluator_path] {
            use std::os::unix::fs::PermissionsExt as _;

            assert_eq!(
                fs::metadata(path)
                    .expect("gate metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o500,
                "frozen gate helpers are executable but not writable"
            );
        }
        assert_eq!(
            authority.effective_policy_sha256,
            deadreckon_core::flight::sha256_text(
                &serde_json::to_string(&job.policy).expect("policy json")
            )
        );
        assert_eq!(
            job.authority_sha256,
            deadreckon_core::flight::sha256_file(&authority_path).expect("authority digest")
        );
        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        assert_eq!(
            history
                .events()
                .iter()
                .map(|event| event.kind)
                .collect::<Vec<_>>(),
            vec![
                JobEventKind::Created,
                JobEventKind::ContractApproved,
                JobEventKind::Queued,
            ]
        );
        assert!(history.events().iter().all(|event| event.lease_epoch == 0));
    }

    #[test]
    fn durable_job_freezes_and_binds_the_complete_contract_bundle() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        let contract_dir = source.join(".deadreckon");
        let helper_dir = contract_dir.join("acceptance");
        fs::create_dir_all(&helper_dir).expect("helper directory");
        fs::write(source.join("README.md"), "durable\n").expect("source file");
        let contract = contract_dir.join("acceptance.yaml");
        fs::write(
            &contract,
            concat!(
                "name: helper-backed fixture\n",
                "checks:\n",
                "  - kind: shell\n",
                "    command: sh .deadreckon/acceptance/check.sh\n",
                "    cwd: \"{working_dir}\"\n",
            ),
        )
        .expect("contract");
        fs::write(contract_dir.join("acceptance.md"), "# Done\n").expect("notes");
        fs::write(helper_dir.join("check.sh"), "#!/bin/sh\nexit 0\n").expect("helper");

        let job = create_job(request(&paths, &source, Some(&contract))).expect("create job");
        let plan = commands::course::load_launch_plan(&paths.job_launch_plan(job.job_id.as_ref()))
            .expect("launch plan");
        let bundle = contract_bundle_from_plan(&plan)
            .expect("bundle signal")
            .expect("bound bundle");
        assert_eq!(
            bundle
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec!["acceptance.md", "acceptance.yaml", "acceptance/check.sh"]
        );
        validate_frozen_contract_bundle(&paths, job.job_id.as_ref(), &plan)
            .expect("frozen bundle validates");
        assert_eq!(
            fs::read_to_string(
                paths
                    .job_dir(job.job_id.as_ref())
                    .join("acceptance/check.sh")
            )
            .expect("frozen helper"),
            "#!/bin/sh\nexit 0\n"
        );

        fs::write(
            paths
                .job_dir(job.job_id.as_ref())
                .join("acceptance/check.sh"),
            "#!/bin/sh\nexit 1\n",
        )
        .expect("tamper frozen helper");
        let error = super::super::supervisor::validate_launch_inputs_for_test(&paths, &job)
            .expect_err("tampered helper must fail before launch");
        assert!(error.to_string().contains("bundle changed"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn durable_job_rejects_contract_helper_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        let contract_dir = source.join(".deadreckon");
        let helper_dir = contract_dir.join("acceptance");
        fs::create_dir_all(&helper_dir).expect("helper directory");
        fs::write(source.join("README.md"), "durable\n").expect("source file");
        let contract = contract_dir.join("acceptance.yaml");
        fs::write(
            &contract,
            "name: fixture\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("contract");
        fs::write(source.join("outside.sh"), "exit 0\n").expect("outside helper");
        symlink(source.join("outside.sh"), helper_dir.join("check.sh")).expect("helper symlink");

        let error = create_job(request(&paths, &source, Some(&contract)))
            .expect_err("helper symlink must fail closed");
        assert!(error.to_string().contains("symlink"), "{error}");
        assert!(
            !paths.jobs_dir().exists()
                || fs::read_dir(paths.jobs_dir())
                    .expect("jobs")
                    .next()
                    .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn durable_job_rejects_a_symlinked_contract_root() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        let external_contract_dir = temp.path().join("external-contract");
        fs::create_dir_all(&source).expect("source");
        fs::create_dir_all(&external_contract_dir).expect("external contract directory");
        fs::write(source.join("README.md"), "durable\n").expect("source file");
        let external_contract = external_contract_dir.join("acceptance.yaml");
        fs::write(
            &external_contract,
            "name: fixture\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("external contract");
        symlink(&external_contract_dir, source.join(".deadreckon")).expect("symlink contract root");

        let error = create_job(request(
            &paths,
            &source,
            Some(&source.join(".deadreckon/acceptance.yaml")),
        ))
        .expect_err("symlinked authority root must fail closed");
        assert!(
            error.to_string().contains("bundle root")
                && error.to_string().contains("regular directory"),
            "{error}"
        );
        assert!(
            !paths.jobs_dir().exists()
                || fs::read_dir(paths.jobs_dir())
                    .expect("jobs")
                    .next()
                    .is_none(),
            "failed admission must roll back its partial Job"
        );
    }

    #[test]
    fn source_contract_inventory_enforces_file_and_count_budgets() {
        let temp = TempDir::new().expect("tempdir");
        let root = temp.path().join(".deadreckon");
        fs::create_dir_all(root.join("acceptance")).expect("helper root");
        let contract = root.join("acceptance.yaml");
        fs::write(
            &contract,
            "name: fixture\nchecks:\n  - kind: file_exists\n    path: '{working_dir}/README.md'\n",
        )
        .expect("contract");
        let oversized = root.join("acceptance/oversized.bin");
        let file = fs::File::create(&oversized).expect("oversized helper");
        file.set_len(CONTRACT_BUNDLE_MAX_FILE_BYTES + 1)
            .expect("grow helper");
        let error = source_contract_bundle_inventory(&contract)
            .expect_err("oversized member must fail closed");
        assert!(error.to_string().contains("exceeds"), "{error}");

        fs::remove_file(oversized).expect("remove oversized helper");
        for index in 0..CONTRACT_BUNDLE_MAX_FILES {
            fs::write(root.join(format!("acceptance/helper-{index:02}")), b"x")
                .expect("counted helper");
        }
        let error = source_contract_bundle_inventory(&contract)
            .expect_err("the contract plus too many helpers must fail closed");
        assert!(
            error.to_string().contains("files") && error.to_string().contains("limit"),
            "{error}"
        );
    }

    #[test]
    fn frozen_gate_tamper_is_rejected_before_unattended_launch() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("README.md"), "durable").expect("source file");

        let job = create_job(request(&paths, &source, None)).expect("create job");
        let evaluator_path = paths.job_frozen_evaluator_gate(job.job_id.as_ref());
        let mut tampered = fs::read(&evaluator_path).expect("approved evaluator");
        tampered.push(0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            fs::set_permissions(&evaluator_path, fs::Permissions::from_mode(0o700))
                .expect("make fixture writable");
        }
        fs::write(&evaluator_path, tampered).expect("tamper evaluator");

        let error = super::super::supervisor::validate_launch_inputs_for_test(&paths, &job)
            .expect_err("tampered evaluator must be rejected before launch");
        assert!(
            error.to_string().contains("gate evaluator changed"),
            "{error}"
        );
        assert!(
            !paths.job_lease(job.job_id.as_ref()).exists(),
            "input validation must fail before an unattended lease is written"
        );
    }

    #[test]
    fn detected_contract_is_frozen_without_source_absolute_path() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(
            source.join("package.json"),
            r#"{"scripts":{"test":"node test.js"}}"#,
        )
        .expect("package");

        let job = create_job(request(&paths, &source, None)).expect("create job");
        let frozen =
            fs::read_to_string(job_acceptance_path(&paths, job.job_id.as_ref())).expect("frozen");
        assert!(frozen.contains("{working_dir}"));
        assert!(!frozen.contains(&source.display().to_string()));
    }

    #[test]
    fn durable_job_refuses_uncontained_policy_before_writing_identity() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("README.md"), "durable").expect("source file");
        let contract = source.join("acceptance.yaml");
        fs::write(
            &contract,
            "name: durable fixture\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/README.md\"\n",
        )
        .expect("contract");
        let mut request = request(&paths, &source, Some(&contract));
        request.sandbox_requested = "none".to_string();

        let error = create_job(request).expect_err("uncontained Job must be refused");

        assert!(error.to_string().contains("require containment"), "{error}");
        assert!(
            !paths.jobs_dir().exists()
                || fs::read_dir(paths.jobs_dir())
                    .expect("jobs")
                    .next()
                    .is_none(),
            "refusal must happen before a durable identity is written"
        );
    }

    #[test]
    fn pending_job_directory_is_removed_unless_creation_commits() {
        let temp = TempDir::new().expect("tempdir");
        let abandoned = temp.path().join("jobs").join("abandoned");
        {
            let _pending = PendingJobDirectory::create(&abandoned).expect("pending directory");
            fs::write(abandoned.join("partial.json"), "{}").expect("partial artifact");
        }
        assert!(
            !abandoned.exists(),
            "failed creation must not leave a partial Job identity"
        );

        let committed = temp.path().join("jobs").join("committed");
        let pending = PendingJobDirectory::create(&committed).expect("pending directory");
        fs::write(committed.join("job.json"), "{}").expect("Job artifact");
        pending.commit();
        assert!(
            committed.join("job.json").is_file(),
            "committed Job state must survive the creation guard"
        );
    }

    #[test]
    fn fresh_job_authority_is_bound_to_its_empty_job_local_source() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let launch_source = temp.path().join("launch-source");
        fs::create_dir_all(&launch_source).expect("launch source");
        fs::write(launch_source.join("unrelated.txt"), "before").expect("launch file");
        let contract = launch_source.join("acceptance.yaml");
        let contract_body = "name: fresh\nchecks:\n  - kind: file_exists\n    path: '{working_dir}/result.txt'\n    must_pass: true\n";
        fs::write(&contract, contract_body).expect("contract");
        let mut request = request(&paths, &launch_source, Some(&contract));
        request.source = DurableSource {
            mode: DurableSourceMode::Fresh,
            from: None,
            allow_dirty: false,
        };

        let job = create_job(request).expect("create fresh job");
        let expected_source = paths.job_dir(job.job_id.as_ref()).join("approved-source");
        assert_eq!(job.source_cwd, expected_source);
        assert!(job.source_cwd.is_dir());
        assert!(
            fs::read_dir(&job.source_cwd)
                .expect("approved source entries")
                .next()
                .is_none(),
            "fresh authority source must start empty"
        );

        let authority: JobAuthority = serde_json::from_slice(
            &fs::read(paths.job_authority(job.job_id.as_ref())).expect("authority bytes"),
        )
        .expect("authority");
        let plan = commands::course::load_launch_plan(&paths.job_launch_plan(job.job_id.as_ref()))
            .expect("launch plan");
        assert_eq!(
            serde_json::from_value::<PathBuf>(
                plan.signals
                    .get(DURABLE_SCOPE_ROOT_SIGNAL)
                    .cloned()
                    .expect("scope root signal")
            )
            .expect("scope root"),
            launch_source
                .canonicalize()
                .expect("canonical launch source")
        );
        assert_eq!(
            authority.source_tree_sha256,
            deadreckon_core::flight::build_deliverable_file_index(&job.source_cwd)
                .expect("approved source index")
                .tree_hash()
        );
        assert_eq!(authority.source_revision, None);
        assert_eq!(
            fs::read_to_string(job_acceptance_path(&paths, job.job_id.as_ref()))
                .expect("frozen contract"),
            contract_body
        );
        let run = deadreckon_core::create_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: job.goal.clone(),
                cwd: launch_source.clone(),
                sandbox: "sandbox-exec".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "test".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(30.0),
                run_id: Some(job.job_id.as_ref().to_string()),
                codebase: Some(deadreckon_core::CodebaseRecord::fresh()),
            },
        )
        .expect("same-scope worker run");
        assert_eq!(run.scope, job.scope);
        deadreckon_core::append_job_event(
            &paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: job.job_id.clone(),
                sequence: JobEventSequence::new(4).expect("sequence"),
                event_id: "fresh-worker-linked".to_string(),
                causation_id: "fresh-worker".to_string(),
                timestamp: Utc::now(),
                lease_epoch: 0,
                kind: JobEventKind::ChildLinked,
                detail: json!({ "run_id": job.job_id.as_ref() }),
            },
        )
        .expect("link worker");
        let view = deadreckon_core::JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(view.attempts.len(), 1);
        assert!(view.missing_attempts.is_empty());

        fs::write(launch_source.join("unrelated.txt"), "after").expect("mutate launch source");
        super::super::supervisor::validate_launch_inputs_for_test(&paths, &job)
            .expect("unused launch mutation must not invalidate fresh authority");
        assert_eq!(
            authority.source_tree_sha256,
            deadreckon_core::flight::build_deliverable_file_index(&job.source_cwd)
                .expect("approved source index after launch mutation")
                .tree_hash(),
            "mutating the unused launch checkout must not invalidate a fresh job"
        );
        fs::create_dir_all(job.source_cwd.join(".specstory/history"))
            .expect("provider evidence directory");
        fs::write(
            job.source_cwd.join(".specstory/history/session.md"),
            "provider-private evidence",
        )
        .expect("provider evidence");
        super::super::supervisor::validate_launch_inputs_for_test(&paths, &job)
            .expect("provider-private source evidence must not invalidate authority");
        fs::write(job.source_cwd.join("unexpected.txt"), "mutation")
            .expect("mutate approved source");
        let error = super::super::supervisor::validate_launch_inputs_for_test(&paths, &job)
            .expect_err("approved source mutation must invalidate authority");
        assert!(error.to_string().contains("source tree changed"), "{error}");
    }

    #[test]
    fn fresh_default_contract_is_rejected_before_job_admission() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let launch_source = temp.path().join("node-project");
        fs::create_dir_all(&launch_source).expect("launch source");
        fs::write(
            launch_source.join("package.json"),
            r#"{"scripts":{"test":"npm run real-tests"}}"#,
        )
        .expect("package");
        let mut request = request(&paths, &launch_source, None);
        request.source = DurableSource {
            mode: DurableSourceMode::Fresh,
            from: None,
            allow_dirty: false,
        };

        let error = create_job(request).expect_err("empty fresh source has no durable contract");
        assert!(
            error
                .to_string()
                .contains("only proves that its pre-created working directory exists"),
            "{error}"
        );
        assert!(
            !paths.jobs_dir().exists()
                || fs::read_dir(paths.jobs_dir())
                    .expect("jobs directory")
                    .next()
                    .is_none(),
            "failed admission must not leave a pending Job behind"
        );
    }

    #[test]
    fn explicit_directory_only_contract_is_rejected_before_job_admission() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let contract = source.join("acceptance.yaml");
        fs::write(
            &contract,
            "name: noop\nchecks:\n  - kind: file_exists\n    path: '{working_dir}'\n    must_pass: true\n",
        )
        .expect("contract");

        let error = create_job(request(&paths, &source, Some(&contract)))
            .expect_err("explicit no-op contract must fail admission");

        assert!(
            error
                .to_string()
                .contains("only proves that its pre-created working directory exists"),
            "{error}"
        );
        assert!(
            !paths.jobs_dir().exists()
                || fs::read_dir(paths.jobs_dir())
                    .expect("jobs directory")
                    .next()
                    .is_none(),
            "failed admission must be atomic"
        );
    }

    #[test]
    fn copy_source_is_normalized_once_before_the_launch_plan_is_frozen() {
        let (_temp, paths, copy_source, job) = copy_job_fixture();
        let approved_source = paths.job_dir(job.job_id.as_ref()).join("approved-source");
        assert_eq!(job.source_cwd, approved_source);
        assert_ne!(job.source_cwd, copy_source);
        assert_eq!(
            deadreckon_core::flight::build_deliverable_file_index(&job.source_cwd)
                .expect("approved source index")
                .tree_hash(),
            deadreckon_core::flight::build_deliverable_file_index(&copy_source)
                .expect("operator source index")
                .tree_hash()
        );
        let plan = commands::course::load_launch_plan(&paths.job_launch_plan(job.job_id.as_ref()))
            .expect("launch plan");
        let frozen_source: DurableSource = serde_json::from_value(
            plan.signals
                .get("watchkeeper_source")
                .cloned()
                .expect("source signal"),
        )
        .expect("source");
        assert_eq!(frozen_source.from.as_deref(), Some(copy_source.as_path()));
    }

    #[test]
    fn full_plan_from_creates_graph_job_with_copy_source() {
        let (_temp, paths, operator_source, job) = copy_job_fixture();
        let plan = commands::course::load_launch_plan(&paths.job_launch_plan(job.job_id.as_ref()))
            .expect("launch plan");
        let source: DurableSource = serde_json::from_value(
            plan.signals
                .get("watchkeeper_source")
                .cloned()
                .expect("source signal"),
        )
        .expect("source projection");

        assert_eq!(job.shape, JobShape::Graph);
        assert_eq!(source.mode, DurableSourceMode::Copy);
        assert_eq!(source.from.as_deref(), Some(operator_source.as_path()));
        assert_ne!(job.source_cwd, operator_source);
        assert!(
            job.source_cwd
                .starts_with(paths.job_dir(job.job_id.as_ref())),
            "approved source must be controller-owned: {}",
            job.source_cwd.display()
        );
        assert!(job.source_cwd.join("README.md").is_file());
    }

    #[test]
    fn graph_copy_freezes_untracked_deliverables_before_queue() {
        let (_temp, paths, operator_source, job) = copy_job_fixture();

        // The fixture is deliberately not a Git repository: every source file
        // is therefore an untracked deliverable, and all must be frozen before
        // the queued Job becomes visible to a supervisor.
        for relative in ["README.md", "Makefile", "fixture-proof.txt"] {
            assert!(operator_source.join(relative).is_file());
            assert!(
                job.source_cwd.join(relative).is_file(),
                "missing {relative}"
            );
        }
        let events =
            fs::read_to_string(paths.job_events(job.job_id.as_ref())).expect("job event history");
        assert!(events.contains("\"queued\""), "{events}");
        assert!(
            job.source_cwd
                .starts_with(paths.job_dir(job.job_id.as_ref()))
        );
    }

    #[test]
    fn graph_copy_never_modifies_the_operator_source() {
        let temp = TempDir::new().expect("tempdir");
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("fixture-proof.txt"), "approved fixture\n").expect("fixture proof");
        fs::write(
            source.join("Makefile"),
            "test:\n\t@test -f fixture-proof.txt\n",
        )
        .expect("makefile");
        let before = deadreckon_core::flight::build_deliverable_file_index(&source)
            .expect("source before")
            .tree_hash();
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let _job = create_job(request(&paths, &source, None)).expect("create copy job");
        let after = deadreckon_core::flight::build_deliverable_file_index(&source)
            .expect("source after")
            .tree_hash();

        assert_eq!(before, after);
    }

    #[test]
    fn graph_execution_survives_original_source_mutation_or_removal() {
        let (_temp, paths, source, job) = copy_job_fixture();
        fs::write(source.join("README.md"), "mutated after approval")
            .expect("mutate original source");
        fs::remove_dir_all(&source).expect("remove original source");

        assert!(job.source_cwd.is_dir());
        assert_eq!(
            fs::read_to_string(job.source_cwd.join("README.md")).expect("approved readme"),
            "copy"
        );
        super::super::supervisor::validate_launch_inputs_for_test(&paths, &job)
            .expect("approved source must not depend on the original path");
    }

    #[test]
    fn graph_authority_and_preview_bind_the_same_source_digest() {
        let (_temp, paths, source, job) = copy_job_fixture();
        let authority: JobAuthority = serde_json::from_slice(
            &fs::read(paths.job_authority(job.job_id.as_ref())).expect("authority"),
        )
        .expect("authority json");
        let source_digest = deadreckon_core::flight::build_deliverable_file_index(&source)
            .expect("operator source")
            .tree_hash();
        let plan = commands::course::load_launch_plan(&paths.job_launch_plan(job.job_id.as_ref()))
            .expect("launch plan");
        let projected: DurableSource = serde_json::from_value(
            plan.signals
                .get("watchkeeper_source")
                .cloned()
                .expect("source projection"),
        )
        .expect("source projection json");

        assert_eq!(authority.source_tree_sha256, source_digest);
        assert_eq!(projected.mode, DurableSourceMode::Copy);
        assert_eq!(projected.from.as_deref(), Some(source.as_path()));
    }

    #[test]
    fn accepted_contract_is_frozen_with_the_resolved_source_job() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let launch = temp.path().join("launch");
        let source = temp.path().join("cloudwing");
        fs::create_dir_all(launch.join(".deadreckon")).expect("acceptance directory");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("README.md"), "Cloudwing\n").expect("source readme");
        let contract = launch.join(".deadreckon/acceptance.yaml");
        let contract_body = concat!(
            "name: Cloudwing continuation\n",
            "checks:\n",
            "  - kind: file_exists\n",
            "    path: \"{working_dir}/README.md\"\n",
        );
        fs::write(&contract, contract_body).expect("accepted contract");
        let mut request = request(&paths, &launch, Some(&contract));
        request.source = DurableSource {
            mode: DurableSourceMode::Copy,
            from: Some(source.clone()),
            allow_dirty: false,
        };

        let job = create_job(request).expect("create resolved-source job");
        let authority: JobAuthority = serde_json::from_slice(
            &fs::read(paths.job_authority(job.job_id.as_ref())).expect("authority"),
        )
        .expect("authority json");

        assert_eq!(
            fs::read_to_string(job_acceptance_path(&paths, job.job_id.as_ref()))
                .expect("frozen contract"),
            contract_body
        );
        assert_eq!(
            authority.source_tree_sha256,
            deadreckon_core::flight::build_deliverable_file_index(&source)
                .expect("resolved source")
                .tree_hash()
        );
        assert!(
            job.source_cwd
                .starts_with(paths.job_dir(job.job_id.as_ref()))
        );
    }

    #[test]
    fn fresh_source_revision_stays_empty_inside_an_enclosing_git_repository() {
        let temp = TempDir::new().expect("tempdir");
        let repository = temp.path().join("repository");
        fs::create_dir_all(&repository).expect("repository");
        for args in [
            vec!["init"],
            vec!["config", "user.email", "watchkeeper@example.invalid"],
            vec!["config", "user.name", "Watchkeeper Test"],
        ] {
            let output = deadreckon_core::git::run_git(&repository, &args).expect("git setup");
            assert!(output.status.success(), "{args:?}");
        }
        fs::write(repository.join("README.md"), "repository").expect("readme");
        for args in [vec!["add", "README.md"], vec!["commit", "-m", "fixture"]] {
            let output = deadreckon_core::git::run_git(&repository, &args).expect("git commit");
            assert!(output.status.success(), "{args:?}");
        }

        let paths = DeadreckonPaths::from_home(repository.join("state"));
        let launch_source = repository.join("launch");
        fs::create_dir_all(&launch_source).expect("launch source");
        let contract = launch_source.join("acceptance.yaml");
        fs::write(
            &contract,
            "name: fresh\nchecks:\n  - kind: file_exists\n    path: '{working_dir}/result.txt'\n    must_pass: true\n",
        )
        .expect("contract");
        let mut request = request(&paths, &launch_source, Some(&contract));
        request.source = DurableSource {
            mode: DurableSourceMode::Fresh,
            from: None,
            allow_dirty: false,
        };

        let job = create_job(request).expect("create fresh job");
        assert!(
            git_revision(&job.source_cwd).is_some(),
            "the fixture must prove the approved source is nested under git"
        );
        let authority: JobAuthority = serde_json::from_slice(
            &fs::read(paths.job_authority(job.job_id.as_ref())).expect("authority bytes"),
        )
        .expect("authority");
        assert_eq!(
            authority.source_revision, None,
            "Fresh authority never inherits an enclosing repository revision"
        );
    }

    #[test]
    fn durable_graph_job_freezes_shape_and_preserves_isolated_per_node_delivery() {
        use commands::course::{CoursePiece, CourseSubplan};
        use deadreckon_core::plan::ApplyWhen;

        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("README.md"), "graph").expect("source file");
        let mut request = request(&paths, &source, None);
        request.launch_plan.shape = commands::course::CourseShape::Plan;
        request.launch_plan.n = Some(3);
        request.launch_plan.pieces = vec![
            CoursePiece {
                id: "foundation".to_string(),
                goal: "build foundation".to_string(),
                done_hint: None,
                role: None,
                provider: None,
                model: None,
                budget_usd: None,
                depends_on: Vec::new(),
                subplan: None,
            },
            CoursePiece {
                id: "parallel".to_string(),
                goal: "build independent slice".to_string(),
                done_hint: None,
                role: None,
                provider: None,
                model: None,
                budget_usd: None,
                depends_on: Vec::new(),
                subplan: None,
            },
            CoursePiece {
                id: "nested".to_string(),
                goal: "integrate nested slice".to_string(),
                done_hint: None,
                role: None,
                provider: None,
                model: None,
                budget_usd: None,
                depends_on: vec!["foundation".to_string()],
                subplan: Some(CourseSubplan {
                    apply: ApplyWhen::PerNode,
                    pieces: vec![CoursePiece {
                        id: "nested-check".to_string(),
                        goal: "verify nested result".to_string(),
                        done_hint: None,
                        role: None,
                        provider: None,
                        model: None,
                        budget_usd: None,
                        depends_on: Vec::new(),
                        subplan: None,
                    }],
                }),
            },
        ];
        request.shape = JobShape::Graph;
        request.driver = Some(commands::graph_job::DriverSpec {
            kind: commands::graph_job::DriverKind::FullPlan,
            child_count: Some(3),
            apply: ApplyWhen::PerNode,
            planner_provider: Some("planner".to_string()),
            child_provider: Some("worker".to_string()),
            child_provider_overrides: Vec::new(),
            coder_provider: None,
            reviewer_provider: None,
            planner_model: None,
            child_model: None,
            child_model_overrides: Vec::new(),
            coder_model: None,
            reviewer_model: None,
            model: None,
            source_init_git: false,
        });

        let job = create_job(request).expect("create graph job");
        assert_eq!(job.shape, JobShape::Graph);
        let frozen =
            commands::course::load_launch_plan(&paths.job_launch_plan(job.job_id.as_ref()))
                .expect("frozen launch");
        assert_eq!(frozen.pieces.len(), 3);
        assert!(frozen.pieces[0].depends_on.is_empty());
        assert!(frozen.pieces[1].depends_on.is_empty());
        assert_eq!(frozen.pieces[2].depends_on, ["foundation"]);
        assert_eq!(
            frozen.pieces[2]
                .subplan
                .as_ref()
                .expect("nested graph")
                .apply,
            ApplyWhen::PerNode
        );
        let frozen_driver = commands::graph_job::driver_spec(&frozen).expect("driver");
        assert_eq!(
            frozen_driver.kind,
            commands::graph_job::DriverKind::FullPlan
        );
        assert_eq!(frozen_driver.apply, ApplyWhen::PerNode);
    }

    #[test]
    fn public_job_labels_use_snake_case() {
        assert_eq!(
            serialized_label(deadreckon_protocol::JobOutcome::NeedsReview),
            "needs_review"
        );
        assert_eq!(
            serialized_label(deadreckon_protocol::StopReason::SemanticUnavailable),
            "semantic_unavailable"
        );
    }

    #[test]
    fn status_distinguishes_phase_from_stop_reason() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let job = create_job(request(&paths, &source, None)).expect("job");
        let mut view =
            deadreckon_core::JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        view.projection.phase = deadreckon_protocol::JobPhase::Waiting;
        view.projection.stop_reason = Some(deadreckon_protocol::StopReason::LostContainment);

        assert_eq!(job_status_label(&view), "waiting");
        assert_eq!(
            serialized_label(view.projection.stop_reason.expect("stop reason")),
            "lost_containment"
        );
    }

    #[test]
    fn status_never_presents_a_tampered_terminal_proof_as_verified() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let job = create_job(request(&paths, &source, None)).expect("job");
        let mut view =
            deadreckon_core::JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        view.projection.phase = deadreckon_protocol::JobPhase::Terminal;
        view.projection.outcome = Some(deadreckon_protocol::JobOutcome::Verified);
        view.projection.stop_reason = Some(deadreckon_protocol::StopReason::Verified);
        view.verified_receipt_error = Some("receipt signature is invalid".to_string());

        assert_eq!(job_status_label(&view), "verified_proof_invalid");
        assert_eq!(job_primary_action(&view, "attach"), "status");
    }

    #[test]
    fn status_json_distinguishes_job_state_from_approved_source_path() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let job = create_job(request(&paths, &source, None)).expect("job");
        let view = deadreckon_core::JobView::load(&paths, job.job_id.as_ref()).expect("job view");

        let status_paths = job_status_paths(&paths, &view);

        assert_eq!(
            status_paths["job"],
            json!(paths.job_dir(job.job_id.as_ref()))
        );
        assert_eq!(status_paths["source"], json!(job.source_cwd));
        assert_eq!(
            status_paths["source"],
            json!(paths.job_dir(job.job_id.as_ref()).join("approved-source"))
        );
        assert_ne!(status_paths["source"], json!(source));
        assert_ne!(status_paths["job"], status_paths["source"]);
    }

    #[test]
    fn campaign_start_also_enters_the_durable_job_queue() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("README.md"), "campaign").expect("source file");
        let mut request = request(&paths, &source, None);
        request.launch_plan.shape = commands::course::CourseShape::Campaign;
        request.shape = JobShape::LegacyCampaign;
        request.driver = Some(commands::graph_job::DriverSpec {
            kind: commands::graph_job::DriverKind::Campaign,
            child_count: Some(3),
            apply: deadreckon_core::plan::ApplyWhen::AtEnd,
            planner_provider: Some("planner".to_string()),
            child_provider: Some("worker".to_string()),
            child_provider_overrides: Vec::new(),
            coder_provider: None,
            reviewer_provider: None,
            planner_model: None,
            child_model: None,
            child_model_overrides: Vec::new(),
            coder_model: None,
            reviewer_model: None,
            model: None,
            source_init_git: false,
        });

        let job = create_job(request).expect("create campaign job");
        assert_eq!(job.shape, JobShape::LegacyCampaign);
        let view = deadreckon_core::JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(view.projection.phase, deadreckon_protocol::JobPhase::Queued);
        let frozen =
            commands::course::load_launch_plan(&paths.job_launch_plan(job.job_id.as_ref()))
                .expect("launch");
        assert_eq!(
            commands::graph_job::driver_spec(&frozen)
                .expect("driver")
                .kind,
            commands::graph_job::DriverKind::Campaign
        );
    }

    #[test]
    fn legacy_chain_is_rejected_before_durable_job_state_is_written() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("README.md"), "legacy chain").expect("source file");
        let mut request = request(&paths, &source, None);
        request.launch_plan.shape = commands::course::CourseShape::ChainExtend;
        request.shape = JobShape::LegacyChain;

        let error = create_job(request).expect_err("legacy chain must stay outside job scheduler");

        assert!(
            error
                .to_string()
                .contains("legacy chain jobs remain process-bound"),
            "{error}"
        );
        assert!(
            !paths.jobs_dir().exists()
                || fs::read_dir(paths.jobs_dir())
                    .expect("jobs dir")
                    .next()
                    .is_none(),
            "refusal must happen before a partial job is persisted"
        );
    }

    #[test]
    fn successful_job_delivery_is_recorded_once_with_factual_after_state() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let job = create_job(request(&paths, &source, None)).expect("job");
        deadreckon_core::append_job_event(
            &paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: job.job_id.clone(),
                sequence: JobEventSequence::new(4).expect("sequence"),
                event_id: "verified".to_string(),
                causation_id: "test-verification".to_string(),
                timestamp: Utc::now(),
                lease_epoch: 0,
                kind: JobEventKind::Verified,
                detail: json!({ "stop_reason": StopReason::Verified }),
            },
        )
        .expect("verified");
        let destination = temp.path().join("delivered");

        record_job_delivery(
            &paths,
            job.job_id.as_ref(),
            JobDeliveryKind::Exported,
            &destination,
            Some("result-revision"),
        )
        .expect("first delivery");
        record_job_delivery(
            &paths,
            job.job_id.as_ref(),
            JobDeliveryKind::Exported,
            &destination,
            Some("result-revision"),
        )
        .expect("idempotent delivery");

        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        let delivered = history
            .events()
            .iter()
            .filter(|event| event.kind == JobEventKind::ResultExported)
            .collect::<Vec<_>>();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].detail["destination"], json!(destination));
        assert_eq!(delivered[0].detail["resulting_revision"], "result-revision");
        let view = deadreckon_core::JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::Verified)
        );
        let delivery = view
            .projection
            .delivery
            .as_ref()
            .expect("projected delivery");
        assert_eq!(delivery.kind, deadreckon_core::JobDeliveryKind::Exported);
        assert_eq!(delivery.destination, destination);
        assert_eq!(
            delivery.resulting_revision.as_deref(),
            Some("result-revision")
        );
        assert!(
            view.verified_receipt_error.is_some(),
            "the synthetic terminal event has no authenticated receipt binding"
        );
        assert_eq!(job_primary_action(&view, "attach"), "status");
    }

    #[test]
    fn concurrent_duplicate_finish_delivery_is_one_idempotent_fact() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let job = create_job(request(&paths, &source, None)).expect("job");
        deadreckon_core::append_job_event(
            &paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: job.job_id.clone(),
                sequence: JobEventSequence::new(4).expect("sequence"),
                event_id: "verified-concurrent".to_string(),
                causation_id: "test-verification".to_string(),
                timestamp: Utc::now(),
                lease_epoch: 0,
                kind: JobEventKind::Verified,
                detail: json!({ "stop_reason": StopReason::Verified }),
            },
        )
        .expect("verified");
        let destination = temp.path().join("delivered");
        let barrier = std::sync::Barrier::new(4);

        std::thread::scope(|scope| {
            let handles = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        record_job_delivery(
                            &paths,
                            job.job_id.as_ref(),
                            JobDeliveryKind::Exported,
                            &destination,
                            Some("revision-after-apply"),
                        )
                    })
                })
                .collect::<Vec<_>>();
            for handle in handles {
                handle.join().expect("delivery thread").expect("delivery");
            }
        });

        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| event.kind == JobEventKind::ResultExported)
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn malformed_campaign_inventory_signals_neither_outer_nor_nested_process() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let job = create_job(request(&paths, &source, None)).expect("job");
        let view = deadreckon_core::JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        let mut outer = Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("outer process");
        deadreckon_core::write_supervised_process(
            &paths
                .job_dir(job.job_id.as_ref())
                .join("supervised-child.json"),
            deadreckon_core::SupervisedProcess {
                pid: outer.id(),
                pgid: None,
            },
        )
        .expect("outer authority");
        let nested_dir = paths
            .job_dir(job.job_id.as_ref())
            .join("campaign-sub-launches");
        fs::create_dir_all(&nested_dir).expect("nested authority directory");
        fs::write(nested_dir.join("malformed.json"), b"{}").expect("malformed authority");

        cancel_job(&paths, &view, true)
            .expect_err("malformed nested inventory must fail before signalling");
        assert!(
            outer.try_wait().expect("poll outer").is_none(),
            "outer process must remain alive when nested authority is malformed"
        );

        let _ = outer.kill();
        let _ = outer.wait();
    }

    #[cfg(unix)]
    #[test]
    fn malformed_merge_repair_inventory_does_not_signal_the_outer_job_process() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let job = create_job(request(&paths, &source, None)).expect("job");
        let view = deadreckon_core::JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        let mut outer = Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("outer process");
        deadreckon_core::write_supervised_process(
            &paths
                .job_dir(job.job_id.as_ref())
                .join("supervised-child.json"),
            deadreckon_core::SupervisedProcess {
                pid: outer.id(),
                pgid: None,
            },
        )
        .expect("outer authority");
        let repair_dir = paths
            .job_dir(job.job_id.as_ref())
            .join("merge-repair-authorities");
        fs::create_dir_all(&repair_dir).expect("merge-repair authority directory");
        fs::write(repair_dir.join("malformed.json"), b"{}").expect("malformed authority");

        cancel_job(&paths, &view, true)
            .expect_err("malformed merge-repair inventory must fail before signalling");
        assert!(
            outer.try_wait().expect("poll outer").is_none(),
            "outer process must remain alive when merge-repair authority is malformed"
        );

        let _ = outer.kill();
        let _ = outer.wait();
    }

    fn supervised_state(temp: &TempDir) -> deadreckon_core::PipelineState {
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = temp.path().join("workspace");
        fs::create_dir_all(&cwd).expect("workspace");
        deadreckon_core::create_run(
            &paths,
            deadreckon_core::RunOptions {
                goal: "reconcile guarded process".to_string(),
                cwd,
                sandbox: "auto".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(30.0),
                run_id: Some("guarded-process-test".to_string()),
                codebase: None,
            },
        )
        .expect("run state")
    }

    #[test]
    fn supervised_process_record_removed_after_directory_snapshot_is_idempotent() {
        let temp = TempDir::new().expect("tempdir");
        let state = supervised_state(&temp);
        let directory = state.run_root.join("child-pids");
        fs::create_dir_all(&directory).expect("child-pids");
        let original = directory.join("disappearing-process.json");
        fs::write(&original, b"123\n").expect("legacy process record");
        let snapshot = fs::read_dir(&directory)
            .expect("directory snapshot")
            .next()
            .expect("snapshot entry")
            .expect("snapshot path")
            .path();
        fs::remove_file(&original).expect("concurrent trusted reconciliation");

        reconcile_run_supervised_process_path(&snapshot, Duration::ZERO, false)
            .expect("an already reconciled snapshot path is success");
    }

    #[test]
    fn docker_record_removed_after_directory_snapshot_is_idempotent() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let job = create_job(request(&paths, &source, None)).expect("job");
        let directory = paths.job_dir(job.job_id.as_ref()).join("docker-executions");
        fs::create_dir_all(&directory).expect("Docker execution directory");
        let original = directory.join("disappearing-execution.json");
        fs::write(&original, b"{}\n").expect("execution placeholder");
        let snapshot = fs::read_dir(&directory)
            .expect("directory snapshot")
            .next()
            .expect("snapshot entry")
            .expect("snapshot path")
            .path();
        fs::remove_file(&original).expect("concurrent trusted reconciliation");

        reconcile_job_docker_execution_path(&snapshot, &job)
            .expect("an already reconciled Docker snapshot path is success");
    }

    #[cfg(unix)]
    #[test]
    fn current_guarded_process_is_stopped_before_its_record_is_removed() {
        let temp = TempDir::new().expect("tempdir");
        let state = supervised_state(&temp);
        let mut child = Command::new("sleep").arg("60").spawn().expect("sleep");
        let path = state
            .run_root
            .join("child-pids")
            .join("dr-gate-evaluate-attempt-launch.json");
        let record = deadreckon_core::SupervisedProcessRecord::prepared(
            deadreckon_core::SupervisedProcess {
                pid: child.id(),
                pgid: None,
            },
            "evaluator-launch".to_string(),
            1,
            Some("job-launch".to_string()),
            "release-digest".to_string(),
        )
        .expect("process identity");
        deadreckon_core::write_supervised_process_record(&path, &record).expect("record");

        reconcile_run_supervised_processes(&state, Duration::ZERO, false)
            .expect("reconcile evaluator");

        let status = child.wait().expect("reap evaluator");
        assert!(!status.success());
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn current_provider_record_stops_the_owned_process_group_before_removal() {
        let temp = TempDir::new().expect("tempdir");
        let state = supervised_state(&temp);
        let grandchild_path = temp.path().join("provider-grandchild.pid");
        let mut command = Command::new("sh");
        command.args([
            "-c",
            &format!(
                "sleep 60 & child=$!; echo $child > '{}'; wait",
                grandchild_path.display()
            ),
        ]);
        let (mut child, _terminator) =
            deadreckon_core::spawn_grouped(command).expect("grouped provider fixture");
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !grandchild_path.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let grandchild = fs::read_to_string(&grandchild_path)
            .expect("grandchild pid")
            .trim()
            .parse::<u32>()
            .expect("numeric grandchild pid");
        let pid = child.id();
        let path = state
            .run_root
            .join("child-pids")
            .join("provider-turn-1.pid");
        let record =
            deadreckon_core::SupervisedProcessRecord::running(deadreckon_core::SupervisedProcess {
                pid,
                pgid: Some(pid),
            })
            .expect("provider process identity");
        deadreckon_core::write_supervised_process_record(&path, &record).expect("record");

        reconcile_run_supervised_processes(&state, Duration::ZERO, false)
            .expect("reconcile provider group");

        let status = child.wait().expect("reap provider");
        assert!(!status.success());
        for _ in 0..40 {
            if !deadreckon_core::pid_is_alive(grandchild) {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            !deadreckon_core::pid_is_alive(grandchild),
            "provider grandchild survived identity-bound cancellation"
        );
        assert!(!path.exists());
    }

    #[test]
    fn reboot_stale_guarded_record_is_removed_without_signalling_reused_pid() {
        let temp = TempDir::new().expect("tempdir");
        let state = supervised_state(&temp);
        let path = state
            .run_root
            .join("child-pids")
            .join("dr-gate-evaluate-stale.json");
        let mut record = deadreckon_core::SupervisedProcessRecord::prepared(
            deadreckon_core::SupervisedProcess {
                pid: std::process::id(),
                pgid: None,
            },
            "stale-launch".to_string(),
            1,
            Some("old-job-launch".to_string()),
            "release-digest".to_string(),
        )
        .expect("current identity");
        record.boot_id = "different-boot".to_string();
        deadreckon_core::write_supervised_process_record(&path, &record).expect("record");

        reconcile_run_supervised_processes(&state, Duration::ZERO, true)
            .expect("discard stale record");

        assert!(deadreckon_core::pid_is_alive(std::process::id()));
        assert!(!path.exists());
    }

    #[test]
    fn corrupt_nested_process_record_fails_closed_and_remains_for_recovery() {
        let temp = TempDir::new().expect("tempdir");
        let state = supervised_state(&temp);
        let path = state
            .run_root
            .join("child-pids")
            .join("dr-gate-evaluate-corrupt.json");
        fs::create_dir_all(path.parent().expect("parent")).expect("child-pids");
        fs::write(&path, b"{\"pid\":").expect("partial record");

        let error = reconcile_run_supervised_processes(&state, Duration::ZERO, false)
            .expect_err("corrupt identity must block");

        assert!(error.to_string().contains("cannot reconcile"), "{error}");
        assert!(path.exists(), "corrupt evidence must remain inspectable");
    }

    #[test]
    fn guarded_process_reconciliation_leaves_trusted_docker_cid_sidecars_to_docker_cleanup() {
        let temp = TempDir::new().expect("tempdir");
        let state = supervised_state(&temp);
        let path = state
            .run_root
            .join("child-pids")
            .join("docker-gate-evaluate-1-launch.cid");
        fs::create_dir_all(path.parent().expect("parent")).expect("child-pids");
        fs::write(&path, "a".repeat(64)).expect("Docker cidfile");

        reconcile_run_supervised_processes(&state, Duration::ZERO, false)
            .expect("Docker cid sidecar is not a process record");

        assert!(
            path.exists(),
            "the Docker lifecycle owns validation and removal of its cidfile"
        );
    }

    #[cfg(unix)]
    #[test]
    fn malformed_new_record_never_falls_back_to_legacy_cleanup() {
        let temp = TempDir::new().expect("tempdir");
        let state = supervised_state(&temp);
        let mut child = Command::new("true").spawn().expect("short-lived child");
        let pid = child.id();
        child.wait().expect("reap short-lived child");
        assert!(!deadreckon_core::pid_is_alive(pid));

        for boot_changed in [false, true] {
            let path = state
                .run_root
                .join("child-pids")
                .join(format!("dr-gate-evaluate-corrupt-{boot_changed}.json"));
            fs::create_dir_all(path.parent().expect("parent")).expect("child-pids");
            fs::write(
                &path,
                format!(
                    "{{\"schema_version\":99,\"pid\":{pid},\"launch_id\":\"corrupt-launch\",\"attempt\":1,\"release_token_sha256\":\"digest\",\"boot_id\":\"boot\",\"process_start_identity\":\"start\",\"phase\":\"prepared\"}}\n"
                ),
            )
            .expect("malformed new record");

            let error = reconcile_run_supervised_processes(&state, Duration::ZERO, boot_changed)
                .expect_err("new-format corruption must never become legacy cleanup");

            assert!(error.to_string().contains("cannot reconcile"), "{error}");
            assert!(path.exists(), "corrupt evidence must remain inspectable");
            fs::remove_file(&path).expect("remove fixture for next iteration");
        }
    }
}
