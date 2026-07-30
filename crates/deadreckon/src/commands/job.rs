//! Durable job creation and detached supervisor launch.
//!
//! `start` resolves and approves the mutable inputs. This module freezes those
//! inputs before the first agent turn, writes the initial append-only control
//! facts, and only then starts a supervisor.

use super::super::*;

use std::fs::OpenOptions;
use std::process::{Command, Stdio};

use chrono::Utc;
use deadreckon_protocol::{
    AuthorityAcceptedBy, Job, JobAuthority, JobEvent, JobEventKind, JobEventSequence,
    JobExecutionPolicy, JobId, JobPolicy, JobSchemaVersion, JobShape, RunId, SemanticJudgeMode,
    StopReason,
};

const JOB_ACCEPTANCE_FILE: &str = "acceptance.yaml";
const SUPERVISOR_LAUNCH_STDOUT: &str = "supervisor.out";
const SUPERVISOR_LAUNCH_STDERR: &str = "supervisor.err";
pub(crate) const DURABLE_SCOPE_ROOT_SIGNAL: &str = "watchkeeper_scope_root";

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub(crate) sandbox_requested: String,
    pub(crate) accepted_by: AuthorityAcceptedBy,
}

pub(crate) fn create_job(mut request: CreateJob<'_>) -> Result<Job> {
    if request.sandbox_requested.trim() == "none" {
        return Err(CliError::Core(deadreckon_core::user_error(
            "durable Jobs require containment; sandbox `none` cannot be frozen as trusted execution policy",
            "use sandbox auto or an available sandbox-exec, bwrap, or docker backend",
        )));
    }
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
    fs::create_dir_all(&job_dir)?;
    let authority_source_cwd = authority_source_cwd(&request.source, request.source_cwd, &job_dir)?;
    let scope_root = effective_scope_root(request.source_cwd)?;
    if matches!(request.source.mode, DurableSourceMode::Copy) {
        request.source.from = Some(authority_source_cwd.clone());
    }

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
    request.launch_plan.signals = serde_json::Value::Object(signals);

    let launch_path = request.paths.job_launch_plan(job_id.as_ref());
    commands::course::save_launch_plan(&launch_path, &request.launch_plan)?;
    sync_file(&launch_path)?;
    let launch_plan_sha256 = deadreckon_core::flight::sha256_file(&launch_path)?;

    let contract_path = job_dir.join(JOB_ACCEPTANCE_FILE);
    freeze_contract(
        request.contract_source,
        &authority_source_cwd,
        &contract_path,
    )?;
    let contract_sha256 = deadreckon_core::flight::sha256_file(&contract_path)?;

    let policy = JobPolicy {
        max_spend_usd: request.max_spend_usd,
        max_wall_seconds: request.max_wall_seconds,
        max_attempts: request.max_attempts.max(1),
        deadline: None,
        semantic_judge: SemanticJudgeMode::Required,
        execution: Some(JobExecutionPolicy::workspace_only(
            request.sandbox_requested.clone(),
        )),
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
    };
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
    Ok(job)
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
    Applied,
    Exported,
}

/// Record the successful operator delivery transition after it happens.
///
/// The event is idempotent for the same kind, destination and resulting
/// revision. It intentionally does not authorize delivery: `finish` validates
/// the sealed receipt first and calls this only after apply/export succeeds.
pub(crate) fn record_job_delivery(
    paths: &DeadreckonPaths,
    job_id: &str,
    kind: JobDeliveryKind,
    destination: &Path,
    resulting_revision: Option<&str>,
) -> Result<()> {
    let event_kind = match kind {
        JobDeliveryKind::Applied => JobEventKind::ResultApplied,
        JobDeliveryKind::Exported => JobEventKind::ResultExported,
    };
    let destination = destination.to_path_buf();
    let detail = json!({
        "destination": destination,
        "resulting_revision": resulting_revision,
    });
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
            .any(|event| event.kind == event_kind && event.detail == detail)
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
    if json_output {
        let paths = DeadreckonPaths::discover();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": "job_status",
                "id": id,
                "status": status,
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
    ]);
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
    if let Some(state) = state.as_ref() {
        reconcile_run_supervised_processes(state, grace, false)?;
    }
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
        match deadreckon_core::read_supervised_process_record(&path) {
            Ok(record) => reconcile_guarded_process_record(&path, &record, grace)?,
            Err(record_error) => {
                let process = read_legacy_nested_supervised_process(&path).map_err(|error| {
                    CliError::Core(DeadreckonError::InvalidInput(format!(
                        "cannot reconcile supervised process record {}: {record_error}; legacy parse also failed: {error}",
                        path.display()
                    )))
                })?;
                if boot_changed {
                    remove_supervised_file(&path)?;
                    continue;
                }
                if deadreckon_core::pid_is_alive(process.pid) {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                        "cannot prove legacy supervised process {} from {} is dead because it has no boot and process-start identity",
                        process.pid,
                        path.display()
                    ))));
                }
                remove_supervised_file(&path)?;
            }
        }
    }
    Ok(())
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
    let removed = deadreckon_core::remove_supervised_process_record_if_matches(
        path,
        &record.launch_id,
        record.process.pid,
    )?;
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
            Ok(if source.is_absolute() {
                source.to_path_buf()
            } else {
                requested_cwd.join(source)
            })
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

fn freeze_contract(source: Option<&Path>, source_cwd: &Path, target: &Path) -> Result<()> {
    if let Some(source) = source {
        fs::copy(source, target)?;
        sync_file(target)?;
        return Ok(());
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
    Ok(())
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

    fn request<'a>(
        paths: &'a DeadreckonPaths,
        source: &'a Path,
        contract: Option<&'a Path>,
    ) -> CreateJob<'a> {
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
            sandbox_requested: "auto".to_string(),
            accepted_by: AuthorityAcceptedBy::Operator,
        }
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

        let job = create_job(request(&paths, &source, Some(&contract))).expect("create job");
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
    fn fresh_job_authority_is_bound_to_its_empty_job_local_source() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let launch_source = temp.path().join("launch-source");
        fs::create_dir_all(&launch_source).expect("launch source");
        fs::write(launch_source.join("unrelated.txt"), "before").expect("launch file");
        let contract = launch_source.join("acceptance.yaml");
        let contract_body = "name: fresh\nchecks: []\n";
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
    fn fresh_default_contract_is_detected_from_empty_approved_source() {
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

        let job = create_job(request).expect("create fresh job");
        let frozen =
            fs::read_to_string(job_acceptance_path(&paths, job.job_id.as_ref())).expect("frozen");
        assert!(frozen.contains("deadreckon detected unknown"), "{frozen}");
        assert!(!frozen.contains("npm run real-tests"), "{frozen}");
    }

    #[test]
    fn copy_source_is_normalized_once_before_the_launch_plan_is_frozen() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let launch_source = temp.path().join("launch");
        let copy_source = launch_source.join("fixtures/source");
        fs::create_dir_all(&copy_source).expect("copy source");
        fs::write(copy_source.join("README.md"), "copy").expect("copy file");
        let mut request = request(&paths, &launch_source, None);
        request.source = DurableSource {
            mode: DurableSourceMode::Copy,
            from: Some(PathBuf::from("fixtures/source")),
            allow_dirty: false,
        };

        let job = create_job(request).expect("create copy job");
        assert_eq!(job.source_cwd, copy_source);
        let plan = commands::course::load_launch_plan(&paths.job_launch_plan(job.job_id.as_ref()))
            .expect("launch plan");
        let frozen_source: DurableSource = serde_json::from_value(
            plan.signals
                .get("watchkeeper_source")
                .cloned()
                .expect("source signal"),
        )
        .expect("source");
        assert_eq!(
            frozen_source.from.as_deref(),
            Some(job.source_cwd.as_path())
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
        fs::write(&contract, "name: fresh\nchecks: []\n").expect("contract");
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
    fn durable_graph_job_freezes_shape_and_normalizes_parent_delivery_to_at_end() {
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
        assert_eq!(frozen_driver.apply, ApplyWhen::AtEnd);
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
    fn status_json_distinguishes_job_state_from_source_paths() {
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
        assert_eq!(status_paths["source"], json!(source));
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
        assert_eq!(job_primary_action(&view, "attach"), "report");
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
                            JobDeliveryKind::Applied,
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
                .filter(|event| event.kind == JobEventKind::ResultApplied)
                .count(),
            1
        );
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
