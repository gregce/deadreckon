//! Durable local supervision for the first, single-leaf Watchkeeper slice.
//!
//! The append-only job history is control truth. Process exit is only a wakeup
//! to inspect persisted run evidence; it is never accepted as completion.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use chrono::Utc;
use deadreckon_core::{
    DeadreckonError, DeadreckonPaths, JobProjection, JobView, LeaseClaimDisposition, LeaseOwner,
    LeaseReclaimReason, LeaseToken, ProviderFailureDisposition, SupervisedProcess,
    append_fenced_job_event, claim_job_lease, heartbeat_job_lease, load_run, pid_is_alive,
    read_supervised_process, spawn_grouped, validate_acceptance_marker,
    validate_completion_receipt, write_supervised_process,
};
use deadreckon_protocol::{
    Job, JobAuthority, JobEvent, JobEventKind, JobEventSequence, JobSchemaVersion, JobShape, RunId,
    SemanticDecision, SemanticJudgment, StopReason,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::super::{CliError, Result};
use super::course::{CourseShape, LaunchPlan};
use super::run::{
    TRUSTED_SUPERVISOR_JOB_ID_ENV, TRUSTED_SUPERVISOR_LAUNCH_PLAN_ENV, durable_leaf_spec,
};

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
const LEASE_TTL: Duration = Duration::from_secs(15);
const CHILD_METADATA_FILE: &str = "supervised-child.json";
const SUPERVISOR_STDOUT_FILE: &str = "supervisor.out";
const SUPERVISOR_STDERR_FILE: &str = "supervisor.err";

#[derive(Debug, Clone)]
struct SupervisorInstance {
    owner: LeaseOwner,
    executable: PathBuf,
}

#[derive(Debug, Clone)]
struct LaunchInputs {
    plan: LaunchPlan,
    authority: JobAuthority,
}

#[derive(Debug)]
enum MonitoredChild {
    Owned(Child),
    Adopted(u32),
}

#[derive(Debug, Clone, Copy)]
struct ChildExit {
    status: Option<ExitStatus>,
    adopted: bool,
}

pub(crate) async fn supervisor_serve_command(
    once: bool,
    requested_job_id: Option<String>,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let instance = SupervisorInstance {
        owner: LeaseOwner {
            owner_id: Uuid::new_v4().to_string(),
            boot_id: boot_identity(),
            pid: std::process::id(),
            // Advisory only. Child process groups are persisted separately.
            process_group: std::process::id(),
        },
        executable: std::env::current_exe()?,
    };
    // A long-running scanner is one service per durable home. Detached
    // `--once --job-id` launchers deliberately coexist: the per-job fenced
    // lease, not this service singleton, arbitrates those lazy supervisors.
    let _service_guard = if super::supervisor_service::supervisor_requires_service_singleton(
        once,
        requested_job_id.as_deref(),
    ) {
        Some(super::supervisor_service::acquire_supervisor_service_guard(
            &paths,
            &instance.owner.owner_id,
            &instance.owner.boot_id,
            &instance.executable,
        )?)
    } else {
        None
    };

    loop {
        let job_ids = match requested_job_id.as_deref() {
            Some(job_id) => vec![job_id.to_string()],
            None => eligible_job_ids(&paths)?,
        };
        for job_id in job_ids {
            match supervise_one_job(&paths, &instance, &job_id).await {
                Ok(()) => {}
                Err(error) if requested_job_id.is_none() && live_lease_refusal(&error) => {}
                Err(error) => return Err(error),
            }
        }
        if once {
            return Ok(());
        }
        tokio::time::sleep(HEARTBEAT_INTERVAL).await;
    }
}

fn eligible_job_ids(paths: &DeadreckonPaths) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    let entries = match fs::read_dir(paths.jobs_dir()) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(ids),
        Err(source) => return Err(source.into()),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(job_id) = entry.file_name().to_str().map(ToString::to_string) else {
            continue;
        };
        let Ok(view) = JobView::load(paths, &job_id) else {
            continue;
        };
        if !view.projection.is_terminal()
            && matches!(
                view.job.shape,
                JobShape::Single | JobShape::Graph | JobShape::LegacyCampaign
            )
        {
            ids.push(job_id);
        }
    }
    ids.sort();
    Ok(ids)
}

async fn supervise_one_job(
    paths: &DeadreckonPaths,
    instance: &SupervisorInstance,
    job_id: &str,
) -> Result<()> {
    let initial = JobView::load(paths, job_id)?;
    if initial.projection.is_terminal() {
        return Ok(());
    }
    if !matches!(
        initial.job.shape,
        JobShape::Single | JobShape::Graph | JobShape::LegacyCampaign
    ) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "supervisor cannot execute unsupported {:?} job {job_id}",
            initial.job.shape
        ))));
    }

    let claim = claim_job_lease(
        paths,
        &initial.job.job_id,
        &instance.owner,
        Utc::now(),
        LEASE_TTL,
    )?;
    let token = claim.token();
    let reboot_reclaim = matches!(
        claim.disposition,
        LeaseClaimDisposition::Reclaimed(LeaseReclaimReason::BootIdentityChanged)
    );
    let claimed = JobView::load(paths, job_id)?;
    if claimed.projection.stop_reason == Some(StopReason::CancelRequested) {
        append_terminal_event(
            paths,
            &token,
            JobEventKind::Cancelled,
            StopReason::CancelRequested,
            json!({ "reason": "operator cancelled before a child was launched" }),
        )?;
        return Ok(());
    }
    if initial
        .job
        .policy
        .deadline
        .is_some_and(|deadline| deadline <= Utc::now())
    {
        append_terminal_event(
            paths,
            &token,
            JobEventKind::DeadlineReached,
            StopReason::Deadline,
            json!({ "reason": "approved job deadline elapsed before the next attempt" }),
        )?;
        return Ok(());
    }

    let max_attempts = initial.job.policy.max_attempts.max(1);
    let mut resuming_advanced = false;
    if let Some(child) = child_metadata(paths, job_id)? {
        // A PID observed after a reboot may belong to an unrelated process.
        // Boot identity is stronger evidence than PID reuse, so never adopt it.
        if !reboot_reclaim && pid_is_alive(child.pid) {
            append_control_event(
                paths,
                &token,
                JobEventKind::ChildLinked,
                format!("adopt-child:{}:{}", token.epoch, child.pid),
                child_link_detail(&initial.job, child, true, None),
            )?;
            let exit = monitor_child(paths, &token, MonitoredChild::Adopted(child.pid)).await?;
            let attempt = initial.projection.attempt_count.max(1);
            let _ = fs::remove_file(child_metadata_path(paths, job_id));
            if initial.job.shape == JobShape::Single
                && maybe_schedule_leaf_retry(
                    paths,
                    &initial.job,
                    &token,
                    &exit,
                    attempt,
                    max_attempts,
                )?
            {
                resuming_advanced = true;
            } else {
                return classify_job_attempt(
                    paths,
                    &initial.job,
                    &token,
                    exit,
                    attempt >= max_attempts,
                )
                .await;
            }
        }
        if !resuming_advanced
            && advanced_artifact_recoverable(paths, &initial.job)
            && initial.projection.attempt_count < max_attempts
        {
            schedule_advanced_recovery(paths, &token, initial.projection.attempt_count)?;
            let _ = fs::remove_file(child_metadata_path(paths, job_id));
            resuming_advanced = true;
        } else if !resuming_advanced
            && initial.job.shape == JobShape::Single
            && maybe_schedule_leaf_retry(
                paths,
                &initial.job,
                &token,
                &ChildExit {
                    status: None,
                    adopted: true,
                },
                initial.projection.attempt_count.max(1),
                max_attempts,
            )?
        {
            let _ = fs::remove_file(child_metadata_path(paths, job_id));
            resuming_advanced = true;
        } else if !resuming_advanced {
            return classify_job_attempt(
                paths,
                &initial.job,
                &token,
                ChildExit {
                    status: None,
                    adopted: true,
                },
                initial.projection.attempt_count >= max_attempts,
            )
            .await;
        }
    }

    // Plan merge is durable before the conductor exits. If the machine dies
    // after that write but before the parent receipt is sealed, finish the
    // parent verification directly instead of declaring an orphaned attempt.
    if (initial.job.shape == JobShape::Graph
        && merged_graph_waits_for_parent_completion(paths, &initial.job))
        || (initial.job.shape == JobShape::LegacyCampaign
            && merged_campaign_waits_for_parent_completion(paths, &initial.job))
    {
        return classify_advanced_attempt(
            paths,
            &initial.job,
            &token,
            ChildExit {
                status: None,
                adopted: true,
            },
        )
        .await;
    }

    // An attempt was durably started but no child identity survived. P6 cannot
    // prove the old process group is dead, so fail closed instead of duplicating
    // mutating work. P7 closes the smaller spawn-before-sidecar window.
    if initial.projection.attempt_count > 0
        && !resuming_advanced
        && reboot_reclaim
        && initial.job.shape == JobShape::Single
        && maybe_schedule_leaf_retry(
            paths,
            &initial.job,
            &token,
            &ChildExit {
                status: None,
                adopted: true,
            },
            initial.projection.attempt_count,
            max_attempts,
        )?
    {
        resuming_advanced = true;
    }
    if initial.projection.attempt_count > 0
        && !resuming_advanced
        && advanced_artifact_recoverable(paths, &initial.job)
        && initial.projection.attempt_count < max_attempts
    {
        schedule_advanced_recovery(paths, &token, initial.projection.attempt_count)?;
        resuming_advanced = true;
    }
    if initial.projection.attempt_count > 0 && !resuming_advanced {
        append_attempt_stopped(
            paths,
            &token,
            StopReason::LostContainment,
            json!({ "reason": "attempt exists without adoptable child metadata" }),
        )?;
        append_terminal_event(
            paths,
            &token,
            JobEventKind::Blocked,
            StopReason::LostContainment,
            json!({ "reason": "cannot prove the prior process group is dead" }),
        )?;
        return Ok(());
    }

    let launch = match load_launch_inputs(paths, &initial.job) {
        Ok(launch) => launch,
        Err(error) => {
            append_terminal_event(
                paths,
                &token,
                JobEventKind::Failed,
                StopReason::CorruptHistory,
                json!({ "error": error.to_string() }),
            )?;
            return Ok(());
        }
    };
    let first_attempt = initial.projection.attempt_count.saturating_add(1);
    for attempt in first_attempt..=max_attempts {
        append_control_event(
            paths,
            &token,
            JobEventKind::AttemptStarted,
            format!("attempt-started:{}:{attempt}", token.epoch),
            attempt_detail(&initial.job, attempt),
        )?;

        match spawn_job_driver(paths, &initial.job, &launch, &instance.executable, attempt) {
            Ok((child, metadata)) => {
                append_control_event(
                    paths,
                    &token,
                    JobEventKind::ChildLinked,
                    format!("child-linked:{}:{attempt}", token.epoch),
                    child_link_detail(&initial.job, metadata, false, Some(attempt)),
                )?;
                let exit = monitor_child(paths, &token, MonitoredChild::Owned(child)).await?;
                let _ = fs::remove_file(child_metadata_path(paths, job_id));
                if initial.job.shape == JobShape::Single
                    && maybe_schedule_leaf_retry(
                        paths,
                        &initial.job,
                        &token,
                        &exit,
                        attempt,
                        max_attempts,
                    )?
                {
                    heartbeat_job_lease(paths, &token, Utc::now(), LEASE_TTL)?;
                    continue;
                }
                if initial.job.shape != JobShape::Single
                    && advanced_artifact_recoverable(paths, &initial.job)
                    && attempt < max_attempts
                {
                    schedule_advanced_recovery(paths, &token, attempt)?;
                    continue;
                }
                return classify_job_attempt(
                    paths,
                    &initial.job,
                    &token,
                    exit,
                    attempt >= max_attempts,
                )
                .await;
            }
            Err(error) => {
                append_attempt_stopped(
                    paths,
                    &token,
                    StopReason::FatalProvider,
                    json!({ "attempt": attempt, "spawn_error": error.to_string() }),
                )?;
                if attempt < max_attempts {
                    append_control_event(
                        paths,
                        &token,
                        JobEventKind::RetryScheduled,
                        format!("retry-scheduled:{}:{attempt}", token.epoch),
                        json!({ "after_attempt": attempt, "reason": "spawn_failed" }),
                    )?;
                    heartbeat_job_lease(paths, &token, Utc::now(), LEASE_TTL)?;
                    continue;
                }
                append_terminal_event(
                    paths,
                    &token,
                    JobEventKind::Failed,
                    StopReason::AttemptLimit,
                    json!({
                        "attempts": max_attempts,
                        "last_error": error.to_string()
                    }),
                )?;
                return Ok(());
            }
        }
    }
    Ok(())
}

fn advanced_artifact_recoverable(paths: &DeadreckonPaths, job: &Job) -> bool {
    let Ok(driver) = super::graph_job::load_driver_state(paths, job.job_id.as_ref()) else {
        return false;
    };
    if driver.job_id != job.job_id || driver.artifact_id != job.job_id.as_ref() {
        return false;
    }
    match job.shape {
        JobShape::Graph if driver.artifact_kind == "plan" => {
            let Ok(plan) = deadreckon_core::plan::load_plan(paths, job.job_id.as_ref()) else {
                return false;
            };
            matches!(
                plan.status,
                deadreckon_core::plan::PlanStatus::Pending
                    | deadreckon_core::plan::PlanStatus::Forked
            )
        }
        JobShape::LegacyCampaign if driver.artifact_kind == "campaign" => {
            let campaign_dir = paths.plan_dir(job.job_id.as_ref());
            let Ok(campaign) = deadreckon_core::campaign::read_campaign(&campaign_dir) else {
                return false;
            };
            matches!(
                campaign.status,
                deadreckon_core::campaign::CampaignStatus::Pending
                    | deadreckon_core::campaign::CampaignStatus::Forked
            )
        }
        _ => false,
    }
}

fn merged_graph_waits_for_parent_completion(paths: &DeadreckonPaths, job: &Job) -> bool {
    if job.shape != JobShape::Graph {
        return false;
    }
    let Ok(driver) = super::graph_job::load_driver_state(paths, job.job_id.as_ref()) else {
        return false;
    };
    if driver.job_id != job.job_id
        || driver.artifact_kind != "plan"
        || driver.artifact_id != job.job_id.as_ref()
    {
        return false;
    }
    let Ok(plan) = deadreckon_core::plan::load_plan(paths, job.job_id.as_ref()) else {
        return false;
    };
    plan.status == deadreckon_core::plan::PlanStatus::Merged
}

fn merged_campaign_waits_for_parent_completion(paths: &DeadreckonPaths, job: &Job) -> bool {
    if job.shape != JobShape::LegacyCampaign {
        return false;
    }
    let Ok(driver) = super::graph_job::load_driver_state(paths, job.job_id.as_ref()) else {
        return false;
    };
    if driver.job_id != job.job_id
        || driver.artifact_kind != "campaign"
        || driver.artifact_id != job.job_id.as_ref()
    {
        return false;
    }
    let campaign_dir = paths.plan_dir(job.job_id.as_ref());
    let Ok(campaign) = deadreckon_core::campaign::read_campaign(&campaign_dir) else {
        return false;
    };
    campaign.status == deadreckon_core::campaign::CampaignStatus::Merged
}

fn schedule_advanced_recovery(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    after_attempt: u32,
) -> Result<()> {
    append_attempt_stopped(
        paths,
        token,
        StopReason::LostContainment,
        json!({
            "attempt": after_attempt,
            "reason": "advanced conductor died; persisted artifact is resumable"
        }),
    )?;
    append_control_event(
        paths,
        token,
        JobEventKind::RetryScheduled,
        format!("advanced-recovery:{}:{after_attempt}", token.epoch),
        json!({
            "after_attempt": after_attempt,
            "reason": "resume_persisted_advanced_artifact"
        }),
    )?;
    heartbeat_job_lease(paths, token, Utc::now(), LEASE_TTL)?;
    Ok(())
}

fn load_launch_inputs(paths: &DeadreckonPaths, job: &Job) -> Result<LaunchInputs> {
    let plan_path = paths.job_launch_plan(job.job_id.as_ref());
    let plan_digest = deadreckon_core::flight::sha256_file(&plan_path)?;
    if plan_digest != job.launch_plan_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "launch plan digest mismatch for job {}",
            job.job_id
        ))));
    }
    let plan = super::course::load_launch_plan(&plan_path)?;
    let expected_shape = match job.shape {
        JobShape::Single => CourseShape::Single,
        JobShape::Graph => CourseShape::Plan,
        JobShape::LegacyCampaign => CourseShape::Campaign,
        JobShape::LegacyChain => CourseShape::ChainExtend,
    };
    if plan.shape != expected_shape || plan.goal != job.goal {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "launch plan identity does not match job {}",
            job.job_id
        ))));
    }
    if job.shape != JobShape::Single {
        super::graph_job::driver_spec(&plan)?;
    }

    let authority_path = paths.job_authority(job.job_id.as_ref());
    let authority_digest = deadreckon_core::flight::sha256_file(&authority_path)?;
    if authority_digest != job.authority_sha256 {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "authority digest mismatch for job {}",
            job.job_id
        ))));
    }
    let authority: JobAuthority = serde_json::from_slice(&fs::read(&authority_path)?)?;
    if authority.job_id != job.job_id
        || authority.run_id != RunId(job.job_id.as_ref().to_string())
        || authority.launch_plan_sha256 != job.launch_plan_sha256
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "authority identity does not match job {}",
            job.job_id
        ))));
    }
    require_authority_digest(
        job,
        "goal",
        &authority.goal_sha256,
        &deadreckon_core::flight::sha256_text(&job.goal),
    )?;
    let contract_path = super::job::job_acceptance_path(paths, job.job_id.as_ref());
    require_authority_digest(
        job,
        "done contract",
        &authority.contract_sha256,
        &deadreckon_core::flight::sha256_file(&contract_path)?,
    )?;
    let policy_sha256 = deadreckon_core::flight::sha256_text(
        &serde_json::to_string(&job.policy).map_err(|source| DeadreckonError::Json {
            path: paths.job_json(job.job_id.as_ref()),
            source,
        })?,
    );
    require_authority_digest(
        job,
        "effective policy",
        &authority.effective_policy_sha256,
        &policy_sha256,
    )?;
    require_authority_digest(
        job,
        "source tree",
        &authority.source_tree_sha256,
        &deadreckon_core::flight::build_working_file_index(&job.source_cwd)?.tree_hash(),
    )?;
    if authority.semantic_judge_mode != job.policy.semantic_judge {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "semantic judge policy mismatch for job {}",
            job.job_id
        ))));
    }
    if let Some(expected) = authority.source_revision.as_deref()
        && current_source_revision(&job.source_cwd).as_deref() != Some(expected)
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "source revision changed after job {} was approved",
            job.job_id
        ))));
    }
    Ok(LaunchInputs { plan, authority })
}

fn require_authority_digest(job: &Job, label: &str, expected: &str, actual: &str) -> Result<()> {
    if expected == actual {
        Ok(())
    } else {
        Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "{label} changed after job {} was approved",
            job.job_id
        ))))
    }
}

fn current_source_revision(cwd: &Path) -> Option<String> {
    let output = deadreckon_core::git::run_git(cwd, &["rev-parse", "HEAD"]).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|revision| !revision.is_empty())
}

fn spawn_job_driver(
    paths: &DeadreckonPaths,
    job: &Job,
    launch: &LaunchInputs,
    executable: &Path,
    attempt: u32,
) -> Result<(Child, SupervisedProcess)> {
    let job_dir = paths.job_dir(job.job_id.as_ref());
    fs::create_dir_all(&job_dir)?;
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(job_dir.join(SUPERVISOR_STDOUT_FILE))?;
    let stderr = OpenOptions::new()
        .create(true)
        .append(true)
        .open(job_dir.join(SUPERVISOR_STDERR_FILE))?;
    let mut command = match job.shape {
        JobShape::Single if attempt == 1 => build_leaf_command(paths, job, launch, executable),
        JobShape::Single => build_leaf_resume_command(paths, job, executable),
        JobShape::Graph | JobShape::LegacyCampaign => {
            build_advanced_command(paths, job, executable)
        }
        JobShape::LegacyChain => {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "legacy chain jobs remain process-bound".to_string(),
            )));
        }
    };
    command
        .current_dir(&job.source_cwd)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    let (child, terminator) = spawn_grouped(command)?;
    let metadata = SupervisedProcess {
        pid: child.id(),
        #[cfg(unix)]
        pgid: Some(child.id()),
        #[cfg(not(unix))]
        pgid: None,
    };
    let metadata_path = child_metadata_path(paths, job.job_id.as_ref());
    if let Err(error) = write_supervised_process(&metadata_path, metadata) {
        let _ = terminator.terminate(Duration::from_secs(2));
        return Err(error.into());
    }
    Ok((child, metadata))
}

fn build_leaf_resume_command(paths: &DeadreckonPaths, job: &Job, executable: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("supervisor")
        .arg("resume")
        .arg(job.job_id.as_ref())
        .env("DEADRECKON_HOME", paths.home())
        .env(TRUSTED_SUPERVISOR_JOB_ID_ENV, job.job_id.as_ref());
    command
}

fn build_advanced_command(paths: &DeadreckonPaths, job: &Job, executable: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("supervisor")
        .arg("drive")
        .arg(job.job_id.as_ref())
        .env("DEADRECKON_HOME", paths.home())
        .env(TRUSTED_SUPERVISOR_JOB_ID_ENV, job.job_id.as_ref())
        .env(
            TRUSTED_SUPERVISOR_LAUNCH_PLAN_ENV,
            paths.job_launch_plan(job.job_id.as_ref()),
        );
    command
}

fn attempt_detail(job: &Job, attempt: u32) -> Value {
    if job.shape == JobShape::Single {
        json!({ "attempt": attempt, "run_id": job.job_id.as_ref() })
    } else {
        json!({
            "attempt": attempt,
            "root_id": job.job_id.as_ref(),
            "shape": job.shape,
            "driver": "advanced"
        })
    }
}

fn child_link_detail(
    job: &Job,
    metadata: SupervisedProcess,
    adopted: bool,
    attempt: Option<u32>,
) -> Value {
    let mut detail = serde_json::Map::new();
    detail.insert("adopted".to_string(), Value::Bool(adopted));
    detail.insert("pid".to_string(), json!(metadata.pid));
    detail.insert("process_group".to_string(), json!(metadata.pgid));
    if let Some(attempt) = attempt {
        detail.insert("attempt".to_string(), json!(attempt));
    }
    if job.shape == JobShape::Single {
        detail.insert("run_id".to_string(), json!(job.job_id.as_ref()));
    } else {
        detail.insert("root_id".to_string(), json!(job.job_id.as_ref()));
        detail.insert("shape".to_string(), json!(job.shape));
        detail.insert("driver".to_string(), json!("advanced"));
    }
    Value::Object(detail)
}

fn build_leaf_command(
    paths: &DeadreckonPaths,
    job: &Job,
    launch: &LaunchInputs,
    executable: &Path,
) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("run")
        .arg(&job.goal)
        .arg("--run-id")
        .arg(job.job_id.as_ref())
        .arg("--yes")
        .arg("--quiet")
        .arg("--plain")
        .arg("--no-confirm")
        .arg("--no-hints")
        .arg("--i-know-its-a-lot")
        .arg("--prevent-sleep")
        .arg("off")
        .arg("--sandbox")
        .arg(&launch.authority.sandbox_requested)
        .arg("--max-spend")
        .arg(effective_spend(job, &launch.plan).to_string())
        .arg("--max-wall-seconds")
        .arg(effective_wall_seconds(job, &launch.plan).to_string())
        .env("DEADRECKON_HOME", paths.home())
        .env(TRUSTED_SUPERVISOR_JOB_ID_ENV, job.job_id.as_ref())
        .env(
            TRUSTED_SUPERVISOR_LAUNCH_PLAN_ENV,
            paths.job_launch_plan(job.job_id.as_ref()),
        );
    let contract = super::job::job_acceptance_path(paths, job.job_id.as_ref());
    if contract.is_file() {
        command.arg("--acceptance").arg(contract);
    }
    if let Some(source) = launch
        .plan
        .signals
        .get("watchkeeper_source")
        .and_then(|value| serde_json::from_value::<super::job::DurableSource>(value.clone()).ok())
    {
        match source.mode {
            super::job::DurableSourceMode::Worktree => {
                command.arg("--worktree");
            }
            super::job::DurableSourceMode::Copy => {
                command
                    .arg("--from")
                    .arg(source.from.as_deref().unwrap_or(&job.source_cwd));
            }
            super::job::DurableSourceMode::Fresh => {
                command.arg("--fresh");
            }
            super::job::DurableSourceMode::InitGit => {
                command.arg("--init-git").arg("--worktree");
            }
        }
        if source.allow_dirty {
            command.arg("--allow-dirty");
        }
    }

    let piece = launch.plan.pieces.first();
    if let Some(provider) =
        piece
            .and_then(|piece| piece.provider.as_deref())
            .or(launch.plan.providers.coder.as_deref())
    {
        command.arg("--provider").arg(provider);
    }
    if let Some(model) = piece.and_then(|piece| piece.model.as_deref()) {
        command.arg("--model").arg(model);
    }
    if let Ok(Some(leaf)) = durable_leaf_spec(&launch.plan) {
        if let Some(base) = leaf.base {
            command.arg("--base").arg(base);
        }
        if let Some(branch) = leaf.branch {
            command.arg("--branch").arg(branch);
        }
        if leaf.no_seams {
            command.arg("--no-seams");
        }
        if let Some(provider) = leaf.doc_provider {
            command.arg("--doc-provider").arg(provider);
        }
        command.arg("--skill").arg(leaf.skill);
        if leaf.no_docs {
            command.arg("--no-docs");
        }
        if let Some(skill) = leaf.doc_skill {
            command.arg("--doc-skill").arg(skill);
        }
        if leaf.narrate {
            command.arg("--narrate");
        }
        if leaf.no_narrate {
            command.arg("--no-narrate");
        }
        if let Some(model) = leaf.narrator_model {
            command.arg("--narrator-model").arg(model);
        }
    }
    command
}

fn effective_spend(job: &Job, plan: &LaunchPlan) -> f64 {
    plan.budget
        .ceiling_usd
        .map_or(job.policy.max_spend_usd, |plan_cap| {
            plan_cap.min(job.policy.max_spend_usd)
        })
}

fn effective_wall_seconds(job: &Job, plan: &LaunchPlan) -> u64 {
    plan.budget
        .wall_seconds
        .map_or(job.policy.max_wall_seconds, |plan_cap| {
            plan_cap.min(job.policy.max_wall_seconds)
        })
}

fn child_metadata_path(paths: &DeadreckonPaths, job_id: &str) -> PathBuf {
    paths.job_dir(job_id).join(CHILD_METADATA_FILE)
}

fn child_metadata(paths: &DeadreckonPaths, job_id: &str) -> Result<Option<SupervisedProcess>> {
    let path = child_metadata_path(paths, job_id);
    let metadata = match read_supervised_process(&path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(source.into()),
    };
    Ok(Some(metadata))
}

async fn monitor_child(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    mut child: MonitoredChild,
) -> Result<ChildExit> {
    loop {
        let status = match &mut child {
            MonitoredChild::Owned(child) => child.try_wait()?,
            MonitoredChild::Adopted(pid) if !pid_is_alive(*pid) => {
                return Ok(ChildExit {
                    status: None,
                    adopted: true,
                });
            }
            MonitoredChild::Adopted(_) => None,
        };
        if let Some(status) = status {
            return Ok(ChildExit {
                status: Some(status),
                adopted: false,
            });
        }
        tokio::time::sleep(HEARTBEAT_INTERVAL).await;
        heartbeat_job_lease(paths, token, Utc::now(), LEASE_TTL)?;
    }
}

async fn classify_job_attempt(
    paths: &DeadreckonPaths,
    job: &Job,
    token: &LeaseToken,
    exit: ChildExit,
    attempts_exhausted: bool,
) -> Result<()> {
    match job.shape {
        JobShape::Single => classify_persisted_attempt(paths, job, token, exit, attempts_exhausted),
        JobShape::Graph | JobShape::LegacyCampaign => {
            classify_advanced_attempt(paths, job, token, exit).await
        }
        JobShape::LegacyChain => fail_advanced_attempt(
            paths,
            token,
            exit,
            StopReason::FatalProvider,
            "legacy chain execution is not available through the durable supervisor",
        ),
    }
}

fn maybe_schedule_leaf_retry(
    paths: &DeadreckonPaths,
    job: &Job,
    token: &LeaseToken,
    exit: &ChildExit,
    attempt: u32,
    max_attempts: u32,
) -> Result<bool> {
    if attempt >= max_attempts {
        return Ok(false);
    }
    let Ok(state) = load_run(paths, job.job_id.as_ref()) else {
        // Without persisted run state there is no isolated checkpoint to
        // continue safely. The caller classifies this as a terminal failure.
        return Ok(false);
    };
    let terminal_by_policy = state
        .max_spend_usd
        .is_some_and(|cap| state.total_spend_usd >= cap)
        || state
            .max_wall_seconds
            .is_some_and(|cap| state.total_wall_seconds >= cap);
    if terminal_by_policy {
        return Ok(false);
    }
    let retry_stop_reason = match state.status {
        deadreckon_core::RunStatus::Pending
        | deadreckon_core::RunStatus::Planned
        | deadreckon_core::RunStatus::Executing => StopReason::LostContainment,
        deadreckon_core::RunStatus::Failed
            if state.provider_failure == Some(ProviderFailureDisposition::Retryable) =>
        {
            StopReason::TransientProvider
        }
        deadreckon_core::RunStatus::Completed
        | deadreckon_core::RunStatus::Killed
        | deadreckon_core::RunStatus::Failed => return Ok(false),
    };

    append_attempt_stopped(
        paths,
        token,
        retry_stop_reason,
        json!({
            "attempt": attempt,
            "exit": exit_detail(exit),
            "run_status": state.status,
            "failure_reason": state.failure_reason,
            "provider_failure": state.provider_failure,
            "reason": "worker stopped with a resumable isolated run"
        }),
    )?;
    append_control_event(
        paths,
        token,
        JobEventKind::RetryScheduled,
        format!("leaf-retry:{}:{attempt}", token.epoch),
        json!({
            "after_attempt": attempt,
            "reason": "resume_persisted_run"
        }),
    )?;
    Ok(true)
}

async fn classify_advanced_attempt(
    paths: &DeadreckonPaths,
    job: &Job,
    token: &LeaseToken,
    exit: ChildExit,
) -> Result<()> {
    let driver = match super::graph_job::load_driver_state(paths, job.job_id.as_ref()) {
        Ok(driver) => driver,
        Err(error) => {
            return fail_advanced_attempt(
                paths,
                token,
                exit,
                StopReason::FatalProvider,
                &format!("advanced driver exited without an artifact mapping: {error}"),
            );
        }
    };
    if driver.job_id != job.job_id || driver.artifact_id != job.job_id.as_ref() {
        return fail_advanced_attempt(
            paths,
            token,
            exit,
            StopReason::CorruptHistory,
            "advanced artifact mapping does not retain the parent job identity",
        );
    }

    match job.shape {
        JobShape::Graph => {
            if driver.artifact_kind != "plan"
                || !matches!(
                    driver.kind,
                    super::graph_job::DriverKind::Review | super::graph_job::DriverKind::FullPlan
                )
            {
                return fail_advanced_attempt(
                    paths,
                    token,
                    exit,
                    StopReason::CorruptHistory,
                    "graph job has a mismatched advanced artifact mapping",
                );
            }
            let plan = match deadreckon_core::plan::load_plan(paths, job.job_id.as_ref()) {
                Ok(plan) => plan,
                Err(error) => {
                    return fail_advanced_attempt(
                        paths,
                        token,
                        exit,
                        StopReason::FatalProvider,
                        &format!("graph driver exited without readable plan state: {error}"),
                    );
                }
            };
            match plan.status {
                deadreckon_core::plan::PlanStatus::Merged => {
                    let launch = match load_launch_inputs(paths, job) {
                        Ok(launch) => launch,
                        Err(error) => {
                            return fail_advanced_attempt(
                                paths,
                                token,
                                exit,
                                StopReason::CorruptHistory,
                                &format!(
                                    "graph parent authority changed before completion: {error}"
                                ),
                            );
                        }
                    };
                    match super::graph_job::complete_merged_plan_parent(
                        paths,
                        job,
                        &launch.authority,
                        &plan,
                    )
                    .await
                    {
                        Ok(super::graph_job::ParentCompletion::Verified(receipt)) => {
                            append_control_event(
                                paths,
                                token,
                                JobEventKind::DeterministicGatePassed,
                                format!("graph-gate-passed:{}", token.epoch),
                                json!({
                                    "marker": deadreckon_core::marker_path_for_run_root(
                                        &load_run(paths, job.job_id.as_ref())?.run_root
                                    ),
                                    "merged_run_id": plan.merged_run_id,
                                }),
                            )?;
                            append_control_event(
                                paths,
                                token,
                                JobEventKind::SemanticJudgeAchieved,
                                format!("graph-semantic-achieved:{}", token.epoch),
                                json!({
                                    "judgment": load_run(paths, job.job_id.as_ref())?
                                        .run_root
                                        .join(deadreckon_core::SEMANTIC_JUDGMENT_JSON),
                                    "merged_run_id": plan.merged_run_id,
                                }),
                            )?;
                            append_attempt_stopped(
                                paths,
                                token,
                                StopReason::Verified,
                                json!({
                                    "exit": exit_detail(&exit),
                                    "artifact": "plan",
                                    "result_run_id": plan.merged_run_id,
                                    "receipt_issued_at": receipt.issued_at,
                                }),
                            )?;
                            append_terminal_event(
                                paths,
                                token,
                                JobEventKind::Verified,
                                StopReason::Verified,
                                json!({
                                    "receipt": paths.job_receipt(job.job_id.as_ref()),
                                    "result_run_id": plan.merged_run_id,
                                }),
                            )?;
                            Ok(())
                        }
                        Ok(super::graph_job::ParentCompletion::NeedsReview {
                            reason,
                            decision,
                            stop_reason,
                        }) => {
                            append_control_event(
                                paths,
                                token,
                                JobEventKind::DeterministicGatePassed,
                                format!("graph-gate-passed:{}", token.epoch),
                                json!({
                                    "marker": deadreckon_core::marker_path_for_run_root(
                                        &load_run(paths, job.job_id.as_ref())?.run_root
                                    ),
                                    "merged_run_id": plan.merged_run_id,
                                }),
                            )?;
                            if let Some(decision) = decision {
                                let kind = match decision {
                                    SemanticDecision::Achieved => {
                                        JobEventKind::SemanticJudgeAchieved
                                    }
                                    SemanticDecision::Revise => JobEventKind::SemanticJudgeRevise,
                                    SemanticDecision::Uncertain => {
                                        JobEventKind::SemanticJudgeUncertain
                                    }
                                };
                                append_control_event(
                                    paths,
                                    token,
                                    kind,
                                    format!(
                                        "graph-semantic-{}:{}",
                                        serde_json::to_value(decision)?
                                            .as_str()
                                            .unwrap_or("unknown"),
                                        token.epoch
                                    ),
                                    json!({
                                        "judgment": load_run(paths, job.job_id.as_ref())?
                                            .run_root
                                            .join(deadreckon_core::SEMANTIC_JUDGMENT_JSON),
                                        "merged_run_id": plan.merged_run_id,
                                    }),
                                )?;
                            }
                            append_attempt_stopped(
                                paths,
                                token,
                                stop_reason,
                                json!({
                                    "exit": exit_detail(&exit),
                                    "artifact": "plan",
                                    "result_run_id": plan.merged_run_id,
                                    "reason": reason,
                                }),
                            )?;
                            append_terminal_event(
                                paths,
                                token,
                                JobEventKind::NeedsReview,
                                stop_reason,
                                json!({
                                    "reason": reason,
                                    "artifact": "plan",
                                    "artifact_id": token.job_id.as_ref(),
                                    "result_run_id": plan.merged_run_id,
                                }),
                            )?;
                            Ok(())
                        }
                        Ok(super::graph_job::ParentCompletion::GateFailed(reason)) => {
                            append_control_event(
                                paths,
                                token,
                                JobEventKind::DeterministicGateFailed,
                                format!("graph-gate-failed:{}", token.epoch),
                                json!({
                                    "reason": reason,
                                    "merged_run_id": plan.merged_run_id,
                                }),
                            )?;
                            fail_advanced_attempt(
                                paths,
                                token,
                                exit,
                                StopReason::FatalGate,
                                &format!("graph parent deterministic gate failed: {reason}"),
                            )
                        }
                        Err(error) => fail_advanced_attempt(
                            paths,
                            token,
                            exit,
                            StopReason::FatalProvider,
                            &format!("graph parent completion failed: {error}"),
                        ),
                    }
                }
                deadreckon_core::plan::PlanStatus::Failed => fail_advanced_attempt(
                    paths,
                    token,
                    exit,
                    StopReason::FatalProvider,
                    "graph conductor persisted a failed plan",
                ),
                status => fail_advanced_attempt(
                    paths,
                    token,
                    exit,
                    StopReason::FatalProvider,
                    &format!("graph driver exited while its plan was {status:?}"),
                ),
            }
        }
        JobShape::LegacyCampaign => {
            if driver.artifact_kind != "campaign"
                || driver.kind != super::graph_job::DriverKind::Campaign
            {
                return fail_advanced_attempt(
                    paths,
                    token,
                    exit,
                    StopReason::CorruptHistory,
                    "campaign job has a mismatched advanced artifact mapping",
                );
            }
            let campaign_dir = paths.plan_dir(job.job_id.as_ref());
            let campaign = match deadreckon_core::campaign::read_campaign(&campaign_dir) {
                Ok(campaign) => campaign,
                Err(error) => {
                    return fail_advanced_attempt(
                        paths,
                        token,
                        exit,
                        StopReason::FatalProvider,
                        &format!("campaign driver exited without readable campaign state: {error}"),
                    );
                }
            };
            match campaign.status {
                deadreckon_core::campaign::CampaignStatus::Merged => {
                    let launch = match load_launch_inputs(paths, job) {
                        Ok(launch) => launch,
                        Err(error) => {
                            return fail_advanced_attempt(
                                paths,
                                token,
                                exit,
                                StopReason::CorruptHistory,
                                &format!(
                                    "campaign parent authority changed before completion: {error}"
                                ),
                            );
                        }
                    };
                    match super::graph_job::complete_merged_campaign_parent(
                        paths,
                        job,
                        &launch.authority,
                        &campaign,
                    )
                    .await
                    {
                        Ok(super::graph_job::ParentCompletion::Verified(receipt)) => {
                            append_control_event(
                                paths,
                                token,
                                JobEventKind::DeterministicGatePassed,
                                format!("campaign-gate-passed:{}", token.epoch),
                                json!({
                                    "marker": deadreckon_core::marker_path_for_run_root(
                                        &load_run(paths, job.job_id.as_ref())?.run_root
                                    ),
                                    "merged_run_id": campaign.merged_run_id,
                                }),
                            )?;
                            append_control_event(
                                paths,
                                token,
                                JobEventKind::SemanticJudgeAchieved,
                                format!("campaign-semantic-achieved:{}", token.epoch),
                                json!({
                                    "judgment": load_run(paths, job.job_id.as_ref())?
                                        .run_root
                                        .join(deadreckon_core::SEMANTIC_JUDGMENT_JSON),
                                    "merged_run_id": campaign.merged_run_id,
                                }),
                            )?;
                            append_attempt_stopped(
                                paths,
                                token,
                                StopReason::Verified,
                                json!({
                                    "exit": exit_detail(&exit),
                                    "artifact": "campaign",
                                    "result_run_id": campaign.merged_run_id,
                                    "receipt_issued_at": receipt.issued_at,
                                }),
                            )?;
                            append_terminal_event(
                                paths,
                                token,
                                JobEventKind::Verified,
                                StopReason::Verified,
                                json!({
                                    "receipt": paths.job_receipt(job.job_id.as_ref()),
                                    "result_run_id": campaign.merged_run_id,
                                }),
                            )?;
                            Ok(())
                        }
                        Ok(super::graph_job::ParentCompletion::NeedsReview {
                            reason,
                            decision,
                            stop_reason,
                        }) => {
                            append_control_event(
                                paths,
                                token,
                                JobEventKind::DeterministicGatePassed,
                                format!("campaign-gate-passed:{}", token.epoch),
                                json!({
                                    "marker": deadreckon_core::marker_path_for_run_root(
                                        &load_run(paths, job.job_id.as_ref())?.run_root
                                    ),
                                    "merged_run_id": campaign.merged_run_id,
                                }),
                            )?;
                            if let Some(decision) = decision {
                                let kind = match decision {
                                    SemanticDecision::Achieved => {
                                        JobEventKind::SemanticJudgeAchieved
                                    }
                                    SemanticDecision::Revise => JobEventKind::SemanticJudgeRevise,
                                    SemanticDecision::Uncertain => {
                                        JobEventKind::SemanticJudgeUncertain
                                    }
                                };
                                append_control_event(
                                    paths,
                                    token,
                                    kind,
                                    format!(
                                        "campaign-semantic-{}:{}",
                                        serde_json::to_value(decision)?
                                            .as_str()
                                            .unwrap_or("unknown"),
                                        token.epoch
                                    ),
                                    json!({
                                        "judgment": load_run(paths, job.job_id.as_ref())?
                                            .run_root
                                            .join(deadreckon_core::SEMANTIC_JUDGMENT_JSON),
                                        "merged_run_id": campaign.merged_run_id,
                                    }),
                                )?;
                            }
                            append_attempt_stopped(
                                paths,
                                token,
                                stop_reason,
                                json!({
                                    "exit": exit_detail(&exit),
                                    "artifact": "campaign",
                                    "result_run_id": campaign.merged_run_id,
                                    "reason": reason,
                                }),
                            )?;
                            append_terminal_event(
                                paths,
                                token,
                                JobEventKind::NeedsReview,
                                stop_reason,
                                json!({
                                    "reason": reason,
                                    "artifact": "campaign",
                                    "artifact_id": token.job_id.as_ref(),
                                    "result_run_id": campaign.merged_run_id,
                                }),
                            )?;
                            Ok(())
                        }
                        Ok(super::graph_job::ParentCompletion::GateFailed(reason)) => {
                            append_control_event(
                                paths,
                                token,
                                JobEventKind::DeterministicGateFailed,
                                format!("campaign-gate-failed:{}", token.epoch),
                                json!({
                                    "reason": reason,
                                    "merged_run_id": campaign.merged_run_id,
                                }),
                            )?;
                            fail_advanced_attempt(
                                paths,
                                token,
                                exit,
                                StopReason::FatalGate,
                                &format!("campaign parent deterministic gate failed: {reason}"),
                            )
                        }
                        Err(error) => fail_advanced_attempt(
                            paths,
                            token,
                            exit,
                            StopReason::FatalProvider,
                            &format!("campaign parent completion failed: {error}"),
                        ),
                    }
                }
                deadreckon_core::campaign::CampaignStatus::Killed => {
                    append_attempt_stopped(
                        paths,
                        token,
                        StopReason::CancelRequested,
                        json!({ "exit": exit_detail(&exit), "artifact": "campaign" }),
                    )?;
                    append_terminal_event(
                        paths,
                        token,
                        JobEventKind::Cancelled,
                        StopReason::CancelRequested,
                        json!({ "reason": "campaign conductor persisted a killed campaign" }),
                    )?;
                    Ok(())
                }
                deadreckon_core::campaign::CampaignStatus::Failed
                    if deadreckon_core::campaign::read_campaign_events(&campaign_dir)?
                        .iter()
                        .any(|event| event.kind == "budget_exhausted") =>
                {
                    append_attempt_stopped(
                        paths,
                        token,
                        StopReason::SpendCap,
                        json!({
                            "exit": exit_detail(&exit),
                            "artifact": "campaign",
                            "reason": "campaign tree spend budget was exhausted",
                        }),
                    )?;
                    append_terminal_event(
                        paths,
                        token,
                        JobEventKind::BudgetExhausted,
                        StopReason::SpendCap,
                        json!({
                            "reason": "campaign tree spend budget was exhausted",
                            "artifact": "campaign",
                        }),
                    )?;
                    Ok(())
                }
                deadreckon_core::campaign::CampaignStatus::Failed => fail_advanced_attempt(
                    paths,
                    token,
                    exit,
                    StopReason::FatalProvider,
                    "campaign conductor persisted a failed campaign",
                ),
                status => fail_advanced_attempt(
                    paths,
                    token,
                    exit,
                    StopReason::FatalProvider,
                    &format!("campaign driver exited while its campaign was {status:?}"),
                ),
            }
        }
        JobShape::Single | JobShape::LegacyChain => unreachable!("handled by caller"),
    }
}

fn fail_advanced_attempt(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    exit: ChildExit,
    reason: StopReason,
    message: &str,
) -> Result<()> {
    append_attempt_stopped(
        paths,
        token,
        reason,
        json!({ "exit": exit_detail(&exit), "reason": message }),
    )?;
    append_terminal_event(
        paths,
        token,
        JobEventKind::Failed,
        reason,
        json!({ "reason": message }),
    )?;
    Ok(())
}

fn classify_persisted_attempt(
    paths: &DeadreckonPaths,
    job: &Job,
    token: &LeaseToken,
    exit: ChildExit,
    attempts_exhausted: bool,
) -> Result<()> {
    let exit_detail = exit_detail(&exit);
    let state = match load_run(paths, job.job_id.as_ref()) {
        Ok(state) => state,
        Err(error) => {
            append_attempt_stopped(
                paths,
                token,
                StopReason::FatalProvider,
                json!({ "exit": exit_detail, "state_error": error.to_string() }),
            )?;
            append_terminal_event(
                paths,
                token,
                JobEventKind::Failed,
                StopReason::FatalProvider,
                json!({ "reason": "child exited without persisted run state" }),
            )?;
            return Ok(());
        }
    };

    match state.status {
        deadreckon_core::RunStatus::Completed => {
            if let Ok(receipt) = validate_completion_receipt(paths, &state) {
                append_control_event(
                    paths,
                    token,
                    JobEventKind::DeterministicGatePassed,
                    format!("deterministic-gate-passed:{}:{}", token.epoch, state.turn),
                    json!({ "marker": deadreckon_core::marker_path_for_run_root(&state.run_root) }),
                )?;
                append_control_event(
                    paths,
                    token,
                    JobEventKind::SemanticJudgeAchieved,
                    format!("semantic-judge-achieved:{}:{}", token.epoch, state.turn),
                    json!({ "judgment": state.run_root.join(deadreckon_core::SEMANTIC_JUDGMENT_JSON) }),
                )?;
                append_attempt_stopped(
                    paths,
                    token,
                    StopReason::Verified,
                    json!({
                        "exit": exit_detail,
                        "run_status": "completed",
                        "receipt_issued_at": receipt.issued_at
                    }),
                )?;
                append_terminal_event(
                    paths,
                    token,
                    JobEventKind::Verified,
                    StopReason::Verified,
                    json!({ "receipt": paths.job_receipt(job.job_id.as_ref()) }),
                )?;
            } else if let Err(error) = validate_acceptance_marker(&state) {
                append_control_event(
                    paths,
                    token,
                    JobEventKind::DeterministicGateFailed,
                    format!("deterministic-gate-failed:{}:{}", token.epoch, state.turn),
                    json!({ "error": error.to_string() }),
                )?;
                append_attempt_stopped(
                    paths,
                    token,
                    StopReason::FatalGate,
                    json!({ "exit": exit_detail, "gate_error": error.to_string() }),
                )?;
                append_terminal_event(
                    paths,
                    token,
                    JobEventKind::Failed,
                    StopReason::FatalGate,
                    json!({ "reason": "persisted deterministic proof is invalid" }),
                )?;
            } else {
                append_control_event(
                    paths,
                    token,
                    JobEventKind::DeterministicGatePassed,
                    format!("deterministic-gate-passed:{}:{}", token.epoch, state.turn),
                    json!({ "marker": deadreckon_core::marker_path_for_run_root(&state.run_root) }),
                )?;
                let semantic = append_persisted_semantic_event(paths, token, &state)?;
                let stop_reason = if semantic == Some(SemanticDecision::Uncertain) {
                    StopReason::SemanticUncertain
                } else {
                    StopReason::SemanticUnavailable
                };
                append_attempt_stopped(
                    paths,
                    token,
                    stop_reason,
                    json!({ "exit": exit_detail, "run_status": "completed" }),
                )?;
                append_terminal_event(
                    paths,
                    token,
                    JobEventKind::NeedsReview,
                    stop_reason,
                    json!({
                        "reason": "completed run has no valid combined completion receipt",
                        "receipt_present": paths.job_receipt(job.job_id.as_ref()).exists()
                    }),
                )?;
            }
        }
        deadreckon_core::RunStatus::Failed
            if state
                .failure_reason
                .as_deref()
                .is_some_and(|reason| reason.starts_with("NEEDS_REVIEW:")) =>
        {
            if let Err(error) = validate_acceptance_marker(&state) {
                append_control_event(
                    paths,
                    token,
                    JobEventKind::DeterministicGateFailed,
                    format!("deterministic-gate-failed:{}:{}", token.epoch, state.turn),
                    json!({ "error": error.to_string() }),
                )?;
                append_attempt_stopped(
                    paths,
                    token,
                    StopReason::FatalGate,
                    json!({ "exit": exit_detail, "gate_error": error.to_string() }),
                )?;
                append_terminal_event(
                    paths,
                    token,
                    JobEventKind::Failed,
                    StopReason::FatalGate,
                    json!({ "reason": "needs-review state has no valid deterministic proof" }),
                )?;
            } else {
                append_control_event(
                    paths,
                    token,
                    JobEventKind::DeterministicGatePassed,
                    format!("deterministic-gate-passed:{}:{}", token.epoch, state.turn),
                    json!({ "marker": deadreckon_core::marker_path_for_run_root(&state.run_root) }),
                )?;
                let semantic = append_persisted_semantic_event(paths, token, &state)?;
                let stop_reason = if semantic == Some(SemanticDecision::Uncertain) {
                    StopReason::SemanticUncertain
                } else {
                    StopReason::SemanticUnavailable
                };
                append_attempt_stopped(
                    paths,
                    token,
                    stop_reason,
                    json!({
                        "exit": exit_detail,
                        "run_status": "failed",
                        "failure_reason": state.failure_reason,
                    }),
                )?;
                append_terminal_event(
                    paths,
                    token,
                    JobEventKind::NeedsReview,
                    stop_reason,
                    json!({ "reason": "strict semantic completion was not available" }),
                )?;
            }
        }
        deadreckon_core::RunStatus::Killed => {
            append_attempt_stopped(
                paths,
                token,
                StopReason::CancelRequested,
                json!({ "exit": exit_detail, "run_status": "killed" }),
            )?;
            append_terminal_event(
                paths,
                token,
                JobEventKind::Cancelled,
                StopReason::CancelRequested,
                Value::Null,
            )?;
        }
        deadreckon_core::RunStatus::Failed
            if state
                .max_spend_usd
                .is_some_and(|cap| state.total_spend_usd >= cap) =>
        {
            append_attempt_stopped(
                paths,
                token,
                StopReason::SpendCap,
                json!({ "exit": exit_detail, "spent_usd": state.total_spend_usd }),
            )?;
            append_terminal_event(
                paths,
                token,
                JobEventKind::BudgetExhausted,
                StopReason::SpendCap,
                Value::Null,
            )?;
        }
        deadreckon_core::RunStatus::Failed
            if state
                .max_wall_seconds
                .is_some_and(|cap| state.total_wall_seconds >= cap) =>
        {
            append_attempt_stopped(
                paths,
                token,
                StopReason::WallCap,
                json!({ "exit": exit_detail, "wall_seconds": state.total_wall_seconds }),
            )?;
            append_terminal_event(
                paths,
                token,
                JobEventKind::BudgetExhausted,
                StopReason::WallCap,
                Value::Null,
            )?;
        }
        status => {
            let stop_reason = if attempts_exhausted {
                StopReason::AttemptLimit
            } else {
                StopReason::FatalProvider
            };
            append_attempt_stopped(
                paths,
                token,
                stop_reason,
                json!({
                    "exit": exit_detail,
                    "run_status": serde_json::to_value(status)?,
                    "failure_reason": state.failure_reason
                }),
            )?;
            append_terminal_event(
                paths,
                token,
                JobEventKind::Failed,
                stop_reason,
                json!({
                    "reason": if attempts_exhausted {
                        "child remained incomplete when the approved attempt limit was reached"
                    } else {
                        "child exit was not backed by completed persisted evidence"
                    }
                }),
            )?;
        }
    }
    Ok(())
}

fn append_persisted_semantic_event(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    state: &deadreckon_core::PipelineState,
) -> Result<Option<SemanticDecision>> {
    let path = state.run_root.join(deadreckon_core::SEMANTIC_JUDGMENT_JSON);
    let Ok(raw) = fs::read(&path) else {
        return Ok(None);
    };
    let Ok(judgment) = serde_json::from_slice::<SemanticJudgment>(&raw) else {
        return Ok(None);
    };
    let kind = match judgment.decision {
        SemanticDecision::Achieved => JobEventKind::SemanticJudgeAchieved,
        SemanticDecision::Revise => JobEventKind::SemanticJudgeRevise,
        SemanticDecision::Uncertain => JobEventKind::SemanticJudgeUncertain,
    };
    append_control_event(
        paths,
        token,
        kind,
        format!("semantic-judge-observed:{}:{}", token.epoch, state.turn),
        json!({
            "decision": judgment.decision,
            "provider": judgment.provider,
            "model": judgment.model,
            "judgment": path,
        }),
    )?;
    Ok(Some(judgment.decision))
}

fn exit_detail(exit: &ChildExit) -> Value {
    json!({
        "adopted": exit.adopted,
        "code": exit.status.as_ref().and_then(ExitStatus::code),
        "success": exit.status.as_ref().map(ExitStatus::success)
    })
}

fn append_attempt_stopped(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    reason: StopReason,
    extra: Value,
) -> Result<JobProjection> {
    append_control_event(
        paths,
        token,
        JobEventKind::AttemptStopped,
        format!("attempt-stopped:{}:{}", token.epoch, Uuid::new_v4()),
        merge_stop_reason(reason, extra),
    )
}

fn append_terminal_event(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    kind: JobEventKind,
    reason: StopReason,
    extra: Value,
) -> Result<JobProjection> {
    append_control_event(
        paths,
        token,
        kind,
        format!("terminal:{}:{}", token.epoch, Uuid::new_v4()),
        merge_stop_reason(reason, extra),
    )
}

fn merge_stop_reason(reason: StopReason, extra: Value) -> Value {
    let mut detail = match extra {
        Value::Object(detail) => detail,
        Value::Null => serde_json::Map::new(),
        value => {
            let mut detail = serde_json::Map::new();
            detail.insert("detail".to_string(), value);
            detail
        }
    };
    detail.insert(
        "stop_reason".to_string(),
        serde_json::to_value(reason).unwrap_or(Value::String("corrupt_history".to_string())),
    );
    Value::Object(detail)
}

fn append_control_event(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    kind: JobEventKind,
    causation_id: String,
    detail: Value,
) -> Result<JobProjection> {
    let now = Utc::now();
    let projection = JobView::load(paths, token.job_id.as_ref())?.projection;
    let sequence = projection
        .last_sequence
        .checked_add(1)
        .and_then(JobEventSequence::new)
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(format!(
                "job {} event sequence exhausted",
                token.job_id
            )))
        })?;
    let event = JobEvent {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: token.job_id.clone(),
        sequence,
        event_id: Uuid::new_v4().to_string(),
        causation_id,
        timestamp: now,
        lease_epoch: token.epoch,
        kind,
        detail,
    };
    Ok(append_fenced_job_event(paths, token, now, &event)?)
}

fn live_lease_refusal(error: &CliError) -> bool {
    error.to_string().contains("live lease held by owner")
}

fn boot_identity() -> String {
    if let Some(value) = std::env::var_os("DEADRECKON_BOOT_ID")
        && !value.is_empty()
    {
        return value.to_string_lossy().into_owned();
    }
    #[cfg(target_os = "linux")]
    if let Ok(value) = fs::read_to_string("/proc/sys/kernel/random/boot_id") {
        let value = value.trim();
        if !value.is_empty() {
            return value.to_string();
        }
    }
    #[cfg(target_os = "macos")]
    if let Ok(output) = Command::new("sysctl")
        .args(["-n", "kern.boottime"])
        .output()
        && output.status.success()
    {
        let value = String::from_utf8_lossy(&output.stdout);
        let value = value.trim();
        if !value.is_empty() {
            return format!("macos:{value}");
        }
    }
    // Unknown is deliberately stable. A random per-process value would look
    // like a reboot and could reclaim a live lease; stable unknown instead
    // waits for normal expiry and fails safely.
    "unknown-boot".to_string()
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::thread;
    use std::time::{Duration, Instant};

    use chrono::Utc;
    use deadreckon_core::{
        RunOptions, RunStatus, append_job_event, create_run, save_state, write_job,
        write_supervised_process,
    };
    use deadreckon_protocol::{
        AuthorityAcceptedBy, JobEvent, JobEventKind, JobEventSequence, JobId, JobPolicy,
        SemanticJudgeMode,
    };
    use tempfile::TempDir;

    use super::*;
    use crate::commands::course::{
        CourseBudget, CourseContract, CourseEscape, CourseProviders, CourseResolution,
        ResolutionSource,
    };

    fn fixture(temp: &TempDir, max_attempts: u32) -> (DeadreckonPaths, Job) {
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        let job_id = JobId("1234567890abcdef1234567890abcdef".to_string());
        let plan = LaunchPlan {
            schema: super::super::course::LAUNCH_PLAN_SCHEMA,
            created_at: Utc::now().to_rfc3339(),
            goal: "finish the fixture".to_string(),
            shape: CourseShape::Single,
            pieces: Vec::new(),
            n: None,
            providers: CourseProviders::default(),
            budget: CourseBudget {
                ceiling_usd: Some(2.0),
                split: Vec::new(),
                wall_seconds: Some(30),
            },
            contract: CourseContract::default(),
            signals: Value::Null,
            resolution: CourseResolution {
                source: ResolutionSource::Operator,
                confidence: 1.0,
                rationale: "test fixture".to_string(),
                clamps_applied: Vec::new(),
            },
            escape: CourseEscape::default(),
            accepted_by: Some("operator".to_string()),
            parent: None,
        };
        let plan_path = paths.job_launch_plan(job_id.as_ref());
        super::super::course::save_launch_plan(&plan_path, &plan).expect("plan");
        let launch_plan_sha256 =
            deadreckon_core::flight::sha256_file(&plan_path).expect("plan digest");
        let contract_path = super::super::job::job_acceptance_path(&paths, job_id.as_ref());
        fs::write(
            &contract_path,
            "name: fixture\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}\"\n",
        )
        .expect("contract");
        let policy = JobPolicy {
            max_spend_usd: 3.0,
            max_wall_seconds: 60,
            max_attempts,
            deadline: None,
            semantic_judge: SemanticJudgeMode::Required,
        };

        let authority = JobAuthority {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job_id.clone(),
            run_id: RunId(job_id.as_ref().to_string()),
            approved_at: Utc::now(),
            accepted_by: AuthorityAcceptedBy::Operator,
            goal_sha256: deadreckon_core::flight::sha256_text(&plan.goal),
            contract_sha256: deadreckon_core::flight::sha256_file(&contract_path)
                .expect("contract digest"),
            effective_policy_sha256: deadreckon_core::flight::sha256_text(
                &serde_json::to_string(&policy).expect("policy json"),
            ),
            launch_plan_sha256: launch_plan_sha256.clone(),
            source_tree_sha256: deadreckon_core::flight::build_working_file_index(&source)
                .expect("source index")
                .tree_hash(),
            source_revision: None,
            sandbox_requested: "none".to_string(),
            semantic_judge_mode: SemanticJudgeMode::Required,
        };
        let authority_path = paths.job_authority(job_id.as_ref());
        fs::create_dir_all(authority_path.parent().expect("parent")).expect("job dir");
        fs::write(
            &authority_path,
            serde_json::to_vec_pretty(&authority).expect("authority json"),
        )
        .expect("authority");
        let authority_sha256 =
            deadreckon_core::flight::sha256_file(&authority_path).expect("authority digest");
        let job = Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job_id.clone(),
            scope: "test-scope".to_string(),
            goal: plan.goal.clone(),
            shape: JobShape::Single,
            created_at: Utc::now(),
            source_cwd: source,
            launch_plan_sha256,
            authority_sha256,
            policy,
        };
        write_job(&paths, &job).expect("job");
        for (index, kind) in [JobEventKind::Created, JobEventKind::Queued]
            .into_iter()
            .enumerate()
        {
            append_job_event(
                &paths,
                &JobEvent {
                    schema_version: JobSchemaVersion::CURRENT,
                    job_id: job_id.clone(),
                    sequence: JobEventSequence::new(index as u64 + 1).expect("sequence"),
                    event_id: format!("fixture-{index}"),
                    causation_id: format!("fixture-{index}"),
                    timestamp: Utc::now(),
                    lease_epoch: 0,
                    kind,
                    detail: Value::Null,
                },
            )
            .expect("event");
        }
        (paths, job)
    }

    fn instance(executable: PathBuf) -> SupervisorInstance {
        SupervisorInstance {
            owner: LeaseOwner {
                owner_id: "test-supervisor".to_string(),
                boot_id: "test-boot".to_string(),
                pid: std::process::id(),
                process_group: std::process::id(),
            },
            executable,
        }
    }

    fn executing_attempt(paths: &DeadreckonPaths, job: &Job) {
        let mut state = create_run(
            paths,
            RunOptions {
                goal: job.goal.clone(),
                cwd: job.source_cwd.clone(),
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "deadreckon".to_string(),
                max_spend_usd: Some(job.policy.max_spend_usd),
                max_wall_seconds: Some(job.policy.max_wall_seconds as f64),
                run_id: Some(job.job_id.as_ref().to_string()),
                codebase: None,
            },
        )
        .expect("attempt state");
        state.status = RunStatus::Executing;
        save_state(&state).expect("save executing state");
    }

    fn failed_provider_attempt(
        paths: &DeadreckonPaths,
        job: &Job,
        disposition: ProviderFailureDisposition,
    ) {
        executing_attempt(paths, job);
        let mut state = load_run(paths, job.job_id.as_ref()).expect("attempt state");
        state.status = RunStatus::Failed;
        state.failure_reason = Some("opaque provider failure".to_string());
        state.provider_failure = Some(disposition);
        save_state(&state).expect("save provider failure");
    }

    fn claim_started_attempt(paths: &DeadreckonPaths, job: &Job, attempt: u32) -> LeaseToken {
        let owner = instance(PathBuf::from("/opt/deadreckon")).owner;
        let claim = claim_job_lease(paths, &job.job_id, &owner, Utc::now(), LEASE_TTL)
            .expect("claim attempt");
        let token = claim.token().clone();
        append_control_event(
            paths,
            &token,
            JobEventKind::AttemptStarted,
            format!("test-attempt-{attempt}"),
            attempt_detail(job, attempt),
        )
        .expect("attempt");
        token
    }

    fn graph_fixture(
        temp: &TempDir,
        status: deadreckon_core::plan::PlanStatus,
    ) -> (DeadreckonPaths, Job) {
        use deadreckon_core::plan::{Plan, PlanMode, PlanProviders, PlanRole, PlanTask};

        let (paths, mut job) = fixture(temp, 1);
        let launch_path = paths.job_launch_plan(job.job_id.as_ref());
        let mut launch = super::super::course::load_launch_plan(&launch_path).expect("launch plan");
        launch.shape = CourseShape::Plan;
        super::super::graph_job::embed_driver_spec(
            &mut launch,
            &super::super::graph_job::DriverSpec {
                kind: super::super::graph_job::DriverKind::FullPlan,
                child_count: Some(2),
                apply: deadreckon_core::plan::ApplyWhen::AtEnd,
                planner_provider: Some("planner".to_string()),
                child_provider: Some("worker".to_string()),
                child_provider_overrides: Vec::new(),
                coder_provider: None,
                reviewer_provider: None,
                model: None,
                source_init_git: false,
            },
        )
        .expect("driver");
        super::super::course::save_launch_plan(&launch_path, &launch).expect("save launch");
        job.shape = JobShape::Graph;
        job.scope =
            deadreckon_core::paths::workspace_scope(&job.source_cwd).expect("graph fixture scope");
        job.launch_plan_sha256 =
            deadreckon_core::flight::sha256_file(&launch_path).expect("launch digest");

        let authority_path = paths.job_authority(job.job_id.as_ref());
        let mut authority: JobAuthority =
            serde_json::from_slice(&fs::read(&authority_path).expect("authority bytes"))
                .expect("authority");
        authority.launch_plan_sha256 = job.launch_plan_sha256.clone();
        fs::write(
            &authority_path,
            serde_json::to_vec_pretty(&authority).expect("authority json"),
        )
        .expect("authority");
        job.authority_sha256 =
            deadreckon_core::flight::sha256_file(&authority_path).expect("authority digest");
        fs::write(
            paths.job_json(job.job_id.as_ref()),
            serde_json::to_vec_pretty(&job).expect("job json"),
        )
        .expect("graph job");

        let mut plan = Plan::new(
            job.goal.clone(),
            PlanMode::FullPlan,
            vec![
                PlanTask::new(0, "parallel-a", "do a", PlanRole::Child, None),
                PlanTask::new(1, "parallel-b", "do b", PlanRole::Child, None),
                PlanTask::new(2, "ordered", "integrate", PlanRole::Child, None),
            ],
            PlanProviders::default(),
            Some(job.scope.clone()),
            "test",
        )
        .expect("plan");
        plan.plan_id = job.job_id.as_ref().to_string();
        plan.tasks[2].depends_on =
            vec![plan.tasks[0].task_id.clone(), plan.tasks[1].task_id.clone()];
        plan.status = status;
        if status == deadreckon_core::plan::PlanStatus::Merged {
            plan.merged_run_id = Some("result-run".to_string());
        }
        deadreckon_core::plan::save_plan(&paths, &plan).expect("plan state");
        super::super::job::write_json_synced(
            &super::super::graph_job::driver_state_path(&paths, job.job_id.as_ref()),
            &super::super::graph_job::DriverState {
                schema_version: 1,
                job_id: job.job_id.clone(),
                kind: super::super::graph_job::DriverKind::FullPlan,
                artifact_kind: "plan".to_string(),
                artifact_id: job.job_id.as_ref().to_string(),
                recorded_at: Utc::now(),
            },
        )
        .expect("driver state");
        (paths, job)
    }

    fn campaign_fixture(
        temp: &TempDir,
        status: deadreckon_core::campaign::CampaignStatus,
    ) -> (DeadreckonPaths, Job, deadreckon_core::campaign::Campaign) {
        use deadreckon_core::campaign::{Campaign, SubGoalStatus, build_sub_goals};
        use deadreckon_core::plan::PlanProviders;

        let (paths, mut job) = fixture(temp, 2);
        let launch_path = paths.job_launch_plan(job.job_id.as_ref());
        let mut launch = super::super::course::load_launch_plan(&launch_path).expect("launch plan");
        launch.shape = CourseShape::Campaign;
        super::super::graph_job::embed_driver_spec(
            &mut launch,
            &super::super::graph_job::DriverSpec {
                kind: super::super::graph_job::DriverKind::Campaign,
                child_count: Some(2),
                apply: deadreckon_core::plan::ApplyWhen::AtEnd,
                planner_provider: Some("smoke".to_string()),
                child_provider: Some("smoke".to_string()),
                child_provider_overrides: Vec::new(),
                coder_provider: None,
                reviewer_provider: None,
                model: None,
                source_init_git: false,
            },
        )
        .expect("campaign driver");
        super::super::course::save_launch_plan(&launch_path, &launch).expect("save launch");
        job.shape = JobShape::LegacyCampaign;
        job.scope =
            deadreckon_core::paths::workspace_scope(&job.source_cwd).expect("campaign scope");
        job.launch_plan_sha256 =
            deadreckon_core::flight::sha256_file(&launch_path).expect("launch digest");

        let authority_path = paths.job_authority(job.job_id.as_ref());
        let mut authority: JobAuthority =
            serde_json::from_slice(&fs::read(&authority_path).expect("authority bytes"))
                .expect("authority");
        authority.launch_plan_sha256 = job.launch_plan_sha256.clone();
        fs::write(
            &authority_path,
            serde_json::to_vec_pretty(&authority).expect("authority json"),
        )
        .expect("authority");
        job.authority_sha256 =
            deadreckon_core::flight::sha256_file(&authority_path).expect("authority digest");
        fs::write(
            paths.job_json(job.job_id.as_ref()),
            serde_json::to_vec_pretty(&job).expect("job json"),
        )
        .expect("campaign job");

        let mut providers = PlanProviders::default();
        providers.planner = Some("smoke".to_string());
        providers.default_child = Some("smoke".to_string());
        let subs = build_sub_goals(vec!["campaign-a".to_string(), "campaign-b".to_string()], 2)
            .expect("sub goals");
        let mut campaign = Campaign::new(
            job.goal.clone(),
            subs,
            providers,
            0,
            Some(job.policy.max_spend_usd),
            Some(job.policy.max_wall_seconds as f64),
            "test",
        )
        .expect("campaign");
        campaign.campaign_id = job.job_id.as_ref().to_string();
        campaign.status = status;
        if status == deadreckon_core::campaign::CampaignStatus::Merged {
            campaign
                .sub_goals
                .iter_mut()
                .for_each(|sub| sub.status = SubGoalStatus::Merged);
            campaign.merged_run_id = Some("campaign-result".to_string());
        }
        let campaign_dir = paths.plan_dir(job.job_id.as_ref());
        deadreckon_core::campaign::write_campaign(&campaign_dir, &campaign).expect("campaign");
        super::super::job::write_json_synced(
            &super::super::graph_job::driver_state_path(&paths, job.job_id.as_ref()),
            &super::super::graph_job::DriverState {
                schema_version: 1,
                job_id: job.job_id.clone(),
                kind: super::super::graph_job::DriverKind::Campaign,
                artifact_kind: "campaign".to_string(),
                artifact_id: job.job_id.as_ref().to_string(),
                recorded_at: Utc::now(),
            },
        )
        .expect("driver state");
        (paths, job, campaign)
    }

    #[tokio::test]
    async fn failed_spawn_leaves_a_visible_typed_job() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = fixture(&temp, 1);
        supervise_one_job(
            &paths,
            &instance(temp.path().join("missing-deadreckon-binary")),
            job.job_id.as_ref(),
        )
        .await
        .expect("supervisor records spawn failure");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("visible job");
        assert!(view.projection.is_terminal());
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::RetryExhausted)
        );
        assert_eq!(view.projection.stop_reason, Some(StopReason::AttemptLimit));
        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        assert!(
            history
                .events()
                .iter()
                .any(|event| { event.kind == JobEventKind::AttemptStarted })
        );
        assert!(
            history
                .events()
                .iter()
                .any(|event| { event.kind == JobEventKind::AttemptStopped })
        );
    }

    #[tokio::test]
    async fn queued_operator_cancellation_never_launches_an_attempt() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = fixture(&temp, 1);
        append_job_event(
            &paths,
            &JobEvent {
                schema_version: JobSchemaVersion::CURRENT,
                job_id: job.job_id.clone(),
                sequence: JobEventSequence::new(3).expect("sequence"),
                event_id: "operator-cancel".to_string(),
                causation_id: "operator-cancel".to_string(),
                timestamp: Utc::now(),
                lease_epoch: 0,
                kind: JobEventKind::CancelRequested,
                detail: json!({ "stop_reason": StopReason::CancelRequested }),
            },
        )
        .expect("cancel intent");

        supervise_one_job(
            &paths,
            &instance(temp.path().join("must-not-launch")),
            job.job_id.as_ref(),
        )
        .await
        .expect("cancelled without launch");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(view.projection.attempt_count, 0);
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::Cancelled)
        );
        assert_eq!(
            view.projection.stop_reason,
            Some(StopReason::CancelRequested)
        );
    }

    #[tokio::test]
    async fn elapsed_deadline_stops_before_launch_with_distinct_outcome() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut job) = fixture(&temp, 1);
        job.policy.deadline = Some(Utc::now() - chrono::TimeDelta::seconds(1));
        fs::write(
            paths.job_json(job.job_id.as_ref()),
            serde_json::to_vec_pretty(&job).expect("job json"),
        )
        .expect("expired job fixture");

        supervise_one_job(
            &paths,
            &instance(temp.path().join("must-not-launch")),
            job.job_id.as_ref(),
        )
        .await
        .expect("deadline classification");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(view.projection.attempt_count, 0);
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::DeadlineReached)
        );
        assert_eq!(view.projection.stop_reason, Some(StopReason::Deadline));
    }

    #[test]
    fn graph_driver_command_keeps_the_job_as_the_only_root_identity() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Pending);
        let command = build_advanced_command(&paths, &job, Path::new("/opt/deadreckon"));
        assert_eq!(
            command.get_args().map(OsString::from).collect::<Vec<_>>(),
            vec![
                OsString::from("supervisor"),
                OsString::from("drive"),
                OsString::from(job.job_id.as_ref()),
            ]
        );
        assert_eq!(
            command
                .get_envs()
                .find(|(key, _)| *key == "DEADRECKON_HOME")
                .and_then(|(_, value)| value),
            Some(paths.home().as_os_str())
        );
    }

    #[test]
    fn interrupted_graph_is_recoverable_only_while_its_plan_can_resume() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Forked);
        assert!(advanced_artifact_recoverable(&paths, &job));

        let mut plan = deadreckon_core::plan::load_plan(&paths, job.job_id.as_ref()).expect("plan");
        plan.status = deadreckon_core::plan::PlanStatus::Merged;
        deadreckon_core::plan::save_plan(&paths, &plan).expect("merged plan");
        assert!(!advanced_artifact_recoverable(&paths, &job));
    }

    #[test]
    fn interrupted_campaign_is_recoverable_under_the_same_job_identity() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job, mut campaign) =
            campaign_fixture(&temp, deadreckon_core::campaign::CampaignStatus::Forked);
        campaign.sub_goals[0].status = deadreckon_core::campaign::SubGoalStatus::Merged;
        campaign.sub_goals[0].sub_plan_id = Some("existing-sub-plan".to_string());
        campaign.sub_goals[0].result_run_id = Some("existing-sub-result".to_string());
        deadreckon_core::campaign::write_campaign(&paths.plan_dir(job.job_id.as_ref()), &campaign)
            .expect("interrupted campaign");

        assert!(advanced_artifact_recoverable(&paths, &job));
        let persisted =
            deadreckon_core::campaign::read_campaign(&paths.plan_dir(job.job_id.as_ref()))
                .expect("same campaign");
        assert_eq!(persisted.campaign_id, job.job_id.as_ref());
        assert_eq!(
            persisted.sub_goals[0].sub_plan_id.as_deref(),
            Some("existing-sub-plan")
        );
    }

    #[tokio::test]
    async fn merged_campaign_without_an_evidence_run_fails_closed_before_semantic() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job, _) =
            campaign_fixture(&temp, deadreckon_core::campaign::CampaignStatus::Merged);
        let owner = instance(PathBuf::from("/opt/deadreckon")).owner;
        let claim =
            claim_job_lease(&paths, &job.job_id, &owner, Utc::now(), LEASE_TTL).expect("claim");
        let token = claim.token();
        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "campaign-attempt".to_string(),
            attempt_detail(&job, 1),
        )
        .expect("attempt");

        classify_advanced_attempt(
            &paths,
            &job,
            &token,
            ChildExit {
                status: None,
                adopted: true,
            },
        )
        .await
        .expect("classification");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::Failed)
        );
        assert_eq!(view.projection.stop_reason, Some(StopReason::FatalProvider));
        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        assert!(history.events().iter().all(|event| {
            !matches!(
                event.kind,
                JobEventKind::SemanticJudgeAchieved
                    | JobEventKind::SemanticJudgeRevise
                    | JobEventKind::SemanticJudgeUncertain
            )
        }));
    }

    #[tokio::test]
    async fn refused_campaign_rollup_never_invokes_the_semantic_judge() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job, campaign) =
            campaign_fixture(&temp, deadreckon_core::campaign::CampaignStatus::Merged);
        let rollup = deadreckon_core::campaign::build_rollup(&campaign, |_| {
            (
                "missing".to_string(),
                deadreckon_core::tamper::AcceptanceTamperVerdict::Refuse,
                Vec::new(),
            )
        });
        let campaign_dir = paths.plan_dir(job.job_id.as_ref());
        deadreckon_core::campaign::write_campaign_rollup(&campaign_dir, &rollup)
            .expect("persist refused rollup");
        let mut merged = create_run(
            &paths,
            RunOptions {
                goal: "campaign merged evidence".to_string(),
                cwd: job.source_cwd.clone(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("campaign-result".to_string()),
                codebase: None,
            },
        )
        .expect("merged run");
        fs::write(merged.working_dir.join("result.txt"), "campaign result\n").expect("result");
        deadreckon_core::campaign::write_campaign_rollup_at_run_root(&merged.run_root, &rollup)
            .expect("merged rollup");
        deadreckon_core::write_acceptance_marker(
            &merged.run_root,
            merged.run_id.clone(),
            merged.working_dir.clone(),
            1,
        )
        .expect("merged marker");
        merged
            .set_phase_status(
                deadreckon_core::PhaseId(60),
                deadreckon_core::PhaseStatus::Completed,
            )
            .expect("complete");
        save_state(&merged).expect("merged state");

        let owner = instance(PathBuf::from("/opt/deadreckon")).owner;
        let claim =
            claim_job_lease(&paths, &job.job_id, &owner, Utc::now(), LEASE_TTL).expect("claim");
        let token = claim.token();
        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "campaign-attempt".to_string(),
            attempt_detail(&job, 1),
        )
        .expect("attempt");
        classify_advanced_attempt(
            &paths,
            &job,
            &token,
            ChildExit {
                status: None,
                adopted: true,
            },
        )
        .await
        .expect("classification");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(view.projection.stop_reason, Some(StopReason::FatalGate));
        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        assert!(
            history
                .events()
                .iter()
                .any(|event| event.kind == JobEventKind::DeterministicGateFailed)
        );
        assert!(history.events().iter().all(|event| {
            !matches!(
                event.kind,
                JobEventKind::SemanticJudgeAchieved
                    | JobEventKind::SemanticJudgeRevise
                    | JobEventKind::SemanticJudgeUncertain
            )
        }));
    }

    #[tokio::test]
    async fn achieved_campaign_receipt_survives_crash_boundary_and_finish_validation() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job, mut campaign) =
            campaign_fixture(&temp, deadreckon_core::campaign::CampaignStatus::Merged);
        for (index, sub) in campaign.sub_goals.iter_mut().enumerate() {
            let run_id = format!("campaign-leaf-{index}");
            let mut leaf = create_run(
                &paths,
                RunOptions {
                    goal: sub.goal.clone(),
                    cwd: job.source_cwd.clone(),
                    sandbox: "none".to_string(),
                    provider: None,
                    skill_name: "default-coding".to_string(),
                    max_spend_usd: None,
                    max_wall_seconds: None,
                    run_id: Some(run_id.clone()),
                    codebase: None,
                },
            )
            .expect("leaf run");
            fs::write(leaf.working_dir.join(format!("leaf-{index}.txt")), "done\n")
                .expect("leaf output");
            deadreckon_core::write_acceptance_marker(
                &leaf.run_root,
                leaf.run_id.clone(),
                leaf.working_dir.clone(),
                1,
            )
            .expect("leaf marker");
            leaf.set_phase_status(
                deadreckon_core::PhaseId(60),
                deadreckon_core::PhaseStatus::Completed,
            )
            .expect("leaf complete");
            save_state(&leaf).expect("leaf state");
            sub.result_run_id = Some(run_id);
        }
        let rollup = deadreckon_core::campaign::build_rollup(&campaign, |run_id| {
            let state = load_run(&paths, run_id).expect("leaf");
            let gate = if deadreckon_core::validate_acceptance_marker(&state).is_ok() {
                "signed".to_string()
            } else {
                "refused".to_string()
            };
            (
                gate,
                deadreckon_core::tamper::AcceptanceTamperVerdict::Clean,
                Vec::new(),
            )
        });
        assert!(deadreckon_core::campaign::campaign_can_complete(
            &campaign, &rollup
        ));
        let campaign_dir = paths.plan_dir(job.job_id.as_ref());
        deadreckon_core::campaign::write_campaign(&campaign_dir, &campaign)
            .expect("campaign state");
        deadreckon_core::campaign::write_campaign_rollup(&campaign_dir, &rollup)
            .expect("campaign rollup");

        let mut merged = create_run(
            &paths,
            RunOptions {
                goal: "campaign merged evidence".to_string(),
                cwd: job.source_cwd.clone(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("campaign-result".to_string()),
                codebase: None,
            },
        )
        .expect("merged run");
        fs::write(merged.working_dir.join("campaign.txt"), "merged campaign\n")
            .expect("merged output");
        deadreckon_core::campaign::write_campaign_rollup_at_run_root(&merged.run_root, &rollup)
            .expect("merged rollup");
        deadreckon_core::write_acceptance_marker(
            &merged.run_root,
            merged.run_id.clone(),
            merged.working_dir.clone(),
            1,
        )
        .expect("merged marker");
        merged
            .set_phase_status(
                deadreckon_core::PhaseId(60),
                deadreckon_core::PhaseStatus::Completed,
            )
            .expect("merged complete");
        save_state(&merged).expect("merged state");

        let authority: JobAuthority = serde_json::from_slice(
            &fs::read(paths.job_authority(job.job_id.as_ref())).expect("authority"),
        )
        .expect("authority json");
        let parent =
            super::super::graph_job::prepare_parent_result_run(&paths, &job, &authority, &merged)
                .expect("parent");
        deadreckon_core::campaign::write_campaign_rollup_at_run_root(&parent.run_root, &rollup)
            .expect("parent rollup");
        let key = deadreckon_core::read_gate_key(&paths, job.job_id.as_ref()).expect("gate key");
        deadreckon_core::write_native_acceptance_marker_with_results_and_key(
            &parent.run_root,
            parent.run_id.clone(),
            parent.working_dir.clone(),
            vec![deadreckon_core::AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: true,
                must_pass: true,
                detail: "campaign merged result exists".to_string(),
                command: None,
                cwd: None,
                duration_ms: Some(1),
                stdout: None,
                stderr: None,
            }],
            &key,
            deadreckon_core::AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("native parent marker");
        let judgment = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job.job_id.clone(),
            run_id: RunId(job.job_id.as_ref().to_string()),
            judged_at: Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Achieved,
            summary: "the campaign result satisfies the approved goal".to_string(),
            goal_coverage: Vec::new(),
            missing: Vec::new(),
            input_sha256: "sha256:campaign-evidence".to_string(),
            spend_usd: 0.0,
        };
        fs::write(
            parent
                .run_root
                .join(deadreckon_core::SEMANTIC_JUDGMENT_JSON),
            serde_json::to_vec_pretty(&judgment).expect("judgment json"),
        )
        .expect("judgment");

        let owner = instance(PathBuf::from("/opt/deadreckon")).owner;
        let claim =
            claim_job_lease(&paths, &job.job_id, &owner, Utc::now(), LEASE_TTL).expect("claim");
        let token = claim.token();
        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "campaign-attempt".to_string(),
            attempt_detail(&job, 1),
        )
        .expect("attempt");
        classify_advanced_attempt(
            &paths,
            &job,
            &token,
            ChildExit {
                status: None,
                adopted: true,
            },
        )
        .await
        .expect("classification");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::Verified)
        );
        let parent = load_run(&paths, job.job_id.as_ref()).expect("parent");
        let first_receipt =
            deadreckon_core::validate_completion_receipt(&paths, &parent).expect("receipt");
        let finish = super::super::lifecycle::finish_job_state(&paths, &view)
            .expect("finish accepts campaign receipt");
        assert_eq!(finish.run_id, job.job_id.as_ref());
        assert!(finish.working_dir.join("campaign.txt").is_file());
        let replay = super::super::graph_job::complete_merged_campaign_parent(
            &paths, &job, &authority, &campaign,
        )
        .await
        .expect("idempotent campaign completion");
        let super::super::graph_job::ParentCompletion::Verified(replayed_receipt) = replay else {
            panic!("validated campaign receipt must reproject verified")
        };
        assert_eq!(first_receipt, *replayed_receipt);
    }

    #[tokio::test]
    async fn merged_graph_without_an_evidence_run_fails_closed() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Merged);
        let owner = instance(PathBuf::from("/opt/deadreckon")).owner;
        let claim =
            claim_job_lease(&paths, &job.job_id, &owner, Utc::now(), LEASE_TTL).expect("claim");
        let token = claim.token();
        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "graph-attempt".to_string(),
            attempt_detail(&job, 1),
        )
        .expect("attempt");
        classify_advanced_attempt(
            &paths,
            &job,
            &token,
            ChildExit {
                status: None,
                adopted: true,
            },
        )
        .await
        .expect("classification");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::Failed)
        );
        assert_eq!(view.projection.stop_reason, Some(StopReason::FatalProvider));
        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        assert!(
            history
                .events()
                .iter()
                .all(|event| event.kind != JobEventKind::Verified)
        );
        assert!(history.events().iter().all(|event| {
            event.kind != JobEventKind::DeterministicGatePassed
                && event.kind != JobEventKind::SemanticJudgeAchieved
        }));
    }

    #[tokio::test]
    async fn verified_graph_parent_is_promoted_and_finish_delivers_receipt_bound_output_after_crash_resume()
     {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Merged);
        let mut merged = create_run(
            &paths,
            RunOptions {
                goal: job.goal.clone(),
                cwd: job.source_cwd.clone(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("result-run".to_string()),
                codebase: None,
            },
        )
        .expect("merged evidence run");
        fs::write(merged.working_dir.join("result.txt"), "merged evidence\n")
            .expect("merged result");
        merged
            .set_phase_status(
                deadreckon_core::PhaseId(60),
                deadreckon_core::PhaseStatus::Completed,
            )
            .expect("merged complete");
        save_state(&merged).expect("merged state");

        let authority_path = paths.job_authority(job.job_id.as_ref());
        let authority: JobAuthority =
            serde_json::from_slice(&fs::read(&authority_path).expect("authority"))
                .expect("authority json");
        let parent =
            super::super::graph_job::prepare_parent_result_run(&paths, &job, &authority, &merged)
                .expect("parent result");
        let key = deadreckon_core::read_gate_key(&paths, job.job_id.as_ref()).expect("gate key");
        deadreckon_core::write_native_acceptance_marker_with_results_and_key(
            &parent.run_root,
            parent.run_id.clone(),
            parent.working_dir.clone(),
            vec![deadreckon_core::AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: true,
                must_pass: true,
                detail: "merged result exists".to_string(),
                command: None,
                cwd: None,
                duration_ms: Some(1),
                stdout: None,
                stderr: None,
            }],
            &key,
            deadreckon_core::AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("native marker");
        let judgment = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job.job_id.clone(),
            run_id: RunId(job.job_id.as_ref().to_string()),
            judged_at: Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Achieved,
            summary: "the merged result satisfies the parent goal".to_string(),
            goal_coverage: Vec::new(),
            missing: Vec::new(),
            input_sha256: "sha256:test-evidence".to_string(),
            spend_usd: 0.0,
        };
        let judgment_path = parent
            .run_root
            .join(deadreckon_core::SEMANTIC_JUDGMENT_JSON);
        fs::create_dir_all(judgment_path.parent().expect("proof dir")).expect("proof dir");
        fs::write(
            &judgment_path,
            serde_json::to_vec_pretty(&judgment).expect("judgment json"),
        )
        .expect("judgment");
        assert!(
            !paths.job_receipt(job.job_id.as_ref()).exists(),
            "fixture stops after persisted semantic proof but before receipt sealing"
        );

        let owner = instance(PathBuf::from("/opt/deadreckon")).owner;
        let claim =
            claim_job_lease(&paths, &job.job_id, &owner, Utc::now(), LEASE_TTL).expect("claim");
        let token = claim.token();
        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "graph-attempt".to_string(),
            attempt_detail(&job, 1),
        )
        .expect("attempt");
        classify_advanced_attempt(
            &paths,
            &job,
            &token,
            ChildExit {
                status: None,
                adopted: true,
            },
        )
        .await
        .expect("classification");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::Verified)
        );
        assert_eq!(view.projection.stop_reason, Some(StopReason::Verified));
        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        assert!(
            history
                .events()
                .iter()
                .any(|event| { event.kind == JobEventKind::DeterministicGatePassed })
        );
        assert!(
            history
                .events()
                .iter()
                .any(|event| { event.kind == JobEventKind::SemanticJudgeAchieved })
        );
        assert!(
            history
                .events()
                .iter()
                .any(|event| event.kind == JobEventKind::Verified)
        );
        let parent = load_run(&paths, job.job_id.as_ref()).expect("parent run");
        let library = paths.library_dir(&job.scope, job.job_id.as_ref());
        assert_eq!(
            parent.promoted_library_dir.as_deref(),
            Some(library.as_path())
        );
        assert_eq!(parent.working_dir, library);
        assert_eq!(
            fs::read_to_string(parent.working_dir.join("result.txt")).expect("promoted result"),
            "merged evidence\n"
        );
        let first_receipt = deadreckon_core::validate_completion_receipt(&paths, &parent)
            .expect("same validator finish uses");
        let finish_state = super::super::lifecycle::finish_job_state(&paths, &view)
            .expect("finish accepts verified graph parent");
        assert_eq!(finish_state.working_dir, parent.working_dir);
        let delivered = temp.path().join("delivered-parent");
        super::super::lifecycle::materialize_completed_run(
            &paths,
            &finish_state,
            Some(delivered.clone()),
            false,
            false,
        )
        .expect("finish materializes parent output");
        assert_eq!(
            fs::read_to_string(delivered.join("result.txt")).expect("delivered result"),
            "merged evidence\n"
        );
        deadreckon_core::validate_completion_receipt(&paths, &finish_state)
            .expect("delivery ledger does not invalidate receipt");
        let plan =
            deadreckon_core::plan::load_plan(&paths, job.job_id.as_ref()).expect("merged plan");
        let second =
            super::super::graph_job::complete_merged_plan_parent(&paths, &job, &authority, &plan)
                .await
                .expect("idempotent parent completion");
        let super::super::graph_job::ParentCompletion::Verified(second_receipt) = second else {
            panic!("existing receipt must reproject verified")
        };
        assert_eq!(first_receipt, *second_receipt);
    }

    #[tokio::test]
    async fn persisted_parent_needs_review_is_reprojected_without_rerunning_the_gate() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Merged);
        let mut merged = create_run(
            &paths,
            RunOptions {
                goal: job.goal.clone(),
                cwd: job.source_cwd.clone(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("result-run".to_string()),
                codebase: None,
            },
        )
        .expect("merged evidence run");
        fs::write(merged.working_dir.join("result.txt"), "merged evidence\n")
            .expect("merged result");
        merged
            .set_phase_status(
                deadreckon_core::PhaseId(60),
                deadreckon_core::PhaseStatus::Completed,
            )
            .expect("merged complete");
        save_state(&merged).expect("merged state");
        let authority_path = paths.job_authority(job.job_id.as_ref());
        let authority: JobAuthority =
            serde_json::from_slice(&fs::read(&authority_path).expect("authority"))
                .expect("authority json");
        let mut parent =
            super::super::graph_job::prepare_parent_result_run(&paths, &job, &authority, &merged)
                .expect("parent result");
        let judgment = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job.job_id.clone(),
            run_id: RunId(job.job_id.as_ref().to_string()),
            judged_at: Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Uncertain,
            summary: "evidence is incomplete".to_string(),
            goal_coverage: Vec::new(),
            missing: vec!["operator confirmation".to_string()],
            input_sha256: "sha256:test-evidence".to_string(),
            spend_usd: 0.0,
        };
        let judgment_path = parent
            .run_root
            .join(deadreckon_core::SEMANTIC_JUDGMENT_JSON);
        fs::create_dir_all(judgment_path.parent().expect("proof dir")).expect("proof dir");
        fs::write(
            &judgment_path,
            serde_json::to_vec_pretty(&judgment).expect("judgment json"),
        )
        .expect("judgment");
        parent.failure_reason =
            Some("NEEDS_REVIEW: independent semantic judge was uncertain".to_string());
        parent
            .set_phase_status(
                deadreckon_core::PhaseId(60),
                deadreckon_core::PhaseStatus::Failed,
            )
            .expect("needs review state");
        save_state(&parent).expect("parent state");
        let plan =
            deadreckon_core::plan::load_plan(&paths, job.job_id.as_ref()).expect("merged plan");

        for _ in 0..2 {
            let completion = super::super::graph_job::complete_merged_plan_parent(
                &paths, &job, &authority, &plan,
            )
            .await
            .expect("reproject needs review");
            assert!(matches!(
                completion,
                super::super::graph_job::ParentCompletion::NeedsReview {
                    decision: Some(SemanticDecision::Uncertain),
                    stop_reason: StopReason::SemanticUncertain,
                    ..
                }
            ));
        }
        assert!(!paths.job_receipt(job.job_id.as_ref()).exists());
    }

    #[test]
    fn root_run_uses_the_job_id() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = fixture(&temp, 1);
        let launch = load_launch_inputs(&paths, &job).expect("launch");
        let command = build_leaf_command(&paths, &job, &launch, Path::new("/opt/deadreckon"));
        let args = command.get_args().map(OsString::from).collect::<Vec<_>>();
        let run_id_position = args
            .iter()
            .position(|arg| arg == "--run-id")
            .expect("--run-id");
        assert_eq!(
            args.get(run_id_position + 1),
            Some(&OsString::from(job.job_id.as_ref()))
        );
        assert_eq!(
            command
                .get_envs()
                .find(|(key, _)| *key == TRUSTED_SUPERVISOR_JOB_ID_ENV)
                .and_then(|(_, value)| value),
            Some(std::ffi::OsStr::new(job.job_id.as_ref()))
        );
        let expected_launch_plan = paths.job_launch_plan(job.job_id.as_ref());
        assert_eq!(
            command
                .get_envs()
                .find(|(key, _)| *key == TRUSTED_SUPERVISOR_LAUNCH_PLAN_ENV)
                .and_then(|(_, value)| value),
            Some(expected_launch_plan.as_os_str())
        );
    }

    #[test]
    fn root_run_replays_frozen_direct_options_in_the_detached_child() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = fixture(&temp, 1);
        let mut launch = load_launch_inputs(&paths, &job).expect("launch");
        let leaf = super::super::run::DurableLeafSpec {
            base: Some("main".to_string()),
            branch: Some("deadreckon/direct".to_string()),
            no_seams: true,
            doc_provider: Some("docs".to_string()),
            skill: "coder".to_string(),
            no_docs: true,
            doc_skill: Some("documenter".to_string()),
            narrate: false,
            no_narrate: true,
            narrator_model: Some("narrator-model".to_string()),
        };
        let mut signals = launch.plan.signals.as_object().cloned().unwrap_or_default();
        signals.insert(
            "watchkeeper_leaf".to_string(),
            serde_json::to_value(leaf).expect("leaf json"),
        );
        launch.plan.signals = Value::Object(signals);

        let command = build_leaf_command(&paths, &job, &launch, Path::new("/opt/deadreckon"));
        let args = command.get_args().map(OsString::from).collect::<Vec<_>>();
        for expected in [
            "--base",
            "main",
            "--branch",
            "deadreckon/direct",
            "--no-seams",
            "--doc-provider",
            "docs",
            "--skill",
            "coder",
            "--no-docs",
            "--doc-skill",
            "documenter",
            "--no-narrate",
            "--narrator-model",
            "narrator-model",
        ] {
            assert!(
                args.contains(&OsString::from(expected)),
                "missing {expected:?} in {args:?}"
            );
        }
    }

    #[test]
    fn retry_resumes_the_exact_persisted_job_instead_of_creating_a_second_run() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = fixture(&temp, 2);
        let command = build_leaf_resume_command(&paths, &job, Path::new("/opt/deadreckon"));
        assert_eq!(
            command.get_args().map(OsString::from).collect::<Vec<_>>(),
            vec![
                OsString::from("supervisor"),
                OsString::from("resume"),
                OsString::from(job.job_id.as_ref()),
            ]
        );
        assert_eq!(
            command
                .get_envs()
                .find(|(key, _)| *key == TRUSTED_SUPERVISOR_JOB_ID_ENV)
                .and_then(|(_, value)| value),
            Some(std::ffi::OsStr::new(job.job_id.as_ref()))
        );
    }

    #[test]
    fn interrupted_leaf_with_a_dead_child_schedules_a_bounded_resume() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = fixture(&temp, 2);
        executing_attempt(&paths, &job);
        let owner = instance(PathBuf::from("/opt/deadreckon")).owner;
        let claim =
            claim_job_lease(&paths, &job.job_id, &owner, Utc::now(), LEASE_TTL).expect("claim");
        let token = claim.token();
        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "first-attempt".to_string(),
            attempt_detail(&job, 1),
        )
        .expect("attempt");

        assert!(
            maybe_schedule_leaf_retry(
                &paths,
                &job,
                &token,
                &ChildExit {
                    status: None,
                    adopted: true,
                },
                1,
                2,
            )
            .expect("schedule")
        );
        let view = JobView::load(&paths, job.job_id.as_ref()).expect("view");
        assert!(!view.projection.is_terminal());
        assert_eq!(view.projection.attempt_count, 1);
        assert_eq!(view.projection.stop_reason, None);
        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        assert!(
            history
                .events()
                .iter()
                .any(|event| event.kind == JobEventKind::RetryScheduled)
        );
    }

    #[test]
    fn fatal_provider_failure_stops_after_one_attempt_without_retry() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = fixture(&temp, 3);
        failed_provider_attempt(&paths, &job, ProviderFailureDisposition::Fatal);
        let token = claim_started_attempt(&paths, &job, 1);
        let exit = ChildExit {
            status: None,
            adopted: false,
        };

        assert!(
            !maybe_schedule_leaf_retry(&paths, &job, &token, &exit, 1, 3)
                .expect("fatal classification")
        );
        classify_persisted_attempt(&paths, &job, &token, exit, false)
            .expect("terminal fatal attempt");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("view");
        assert!(view.projection.is_terminal());
        assert_eq!(view.projection.attempt_count, 1);
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::Failed)
        );
        assert_eq!(view.projection.stop_reason, Some(StopReason::FatalProvider));
        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        assert!(
            history
                .events()
                .iter()
                .all(|event| event.kind != JobEventKind::RetryScheduled)
        );
    }

    #[test]
    fn transient_provider_failure_may_resume_within_attempt_cap() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = fixture(&temp, 3);
        failed_provider_attempt(&paths, &job, ProviderFailureDisposition::Retryable);
        let token = claim_started_attempt(&paths, &job, 1);

        assert!(
            maybe_schedule_leaf_retry(
                &paths,
                &job,
                &token,
                &ChildExit {
                    status: None,
                    adopted: false,
                },
                1,
                3,
            )
            .expect("transient classification")
        );
        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        let stopped = history
            .events()
            .iter()
            .find(|event| event.kind == JobEventKind::AttemptStopped)
            .expect("attempt stopped");
        assert_eq!(
            stopped.detail.get("stop_reason").and_then(Value::as_str),
            Some("transient_provider")
        );
        assert!(
            history
                .events()
                .iter()
                .any(|event| event.kind == JobEventKind::RetryScheduled)
        );
        let view = JobView::load(&paths, job.job_id.as_ref()).expect("view");
        assert!(!view.projection.is_terminal());
        assert_eq!(view.projection.attempt_count, 1);
    }

    #[tokio::test]
    async fn boot_identity_change_never_adopts_a_reused_pid() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = fixture(&temp, 2);
        executing_attempt(&paths, &job);
        let old_owner = LeaseOwner {
            owner_id: "old-supervisor".to_string(),
            boot_id: "old-boot".to_string(),
            pid: 41,
            process_group: 41,
        };
        let old_claim =
            claim_job_lease(&paths, &job.job_id, &old_owner, Utc::now(), LEASE_TTL).expect("claim");
        append_control_event(
            &paths,
            &old_claim.token(),
            JobEventKind::AttemptStarted,
            "old-attempt".to_string(),
            attempt_detail(&job, 1),
        )
        .expect("attempt");
        write_supervised_process(
            &child_metadata_path(&paths, job.job_id.as_ref()),
            SupervisedProcess {
                pid: std::process::id(),
                #[cfg(unix)]
                pgid: Some(std::process::id()),
                #[cfg(not(unix))]
                pgid: None,
            },
        )
        .expect("child metadata");
        let new_instance = SupervisorInstance {
            owner: LeaseOwner {
                owner_id: "new-supervisor".to_string(),
                boot_id: "new-boot".to_string(),
                pid: std::process::id(),
                process_group: std::process::id(),
            },
            executable: temp.path().join("missing-deadreckon"),
        };

        supervise_one_job(&paths, &new_instance, job.job_id.as_ref())
            .await
            .expect("reconciled reboot");

        let history = deadreckon_core::read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("history");
        assert!(
            history
                .events()
                .iter()
                .any(|event| event.kind == JobEventKind::LeaseReclaimed)
        );
        assert!(history.events().iter().all(|event| {
            event.kind != JobEventKind::ChildLinked
                || event.detail.get("adopted") != Some(&Value::Bool(true))
        }));
        let view = JobView::load(&paths, job.job_id.as_ref()).expect("view");
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::RetryExhausted)
        );
    }

    #[cfg(unix)]
    #[test]
    fn closing_start_parent_does_not_stop_job() {
        let temp = TempDir::new().expect("tempdir");
        let completed = temp.path().join("completed");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("sleep 0.05; printf survived > \"$1\"")
            .arg("deadreckon-supervisor-test")
            .arg(&completed)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let (child, _terminator) = spawn_grouped(command).expect("grouped child");
        drop(child);

        let deadline = Instant::now() + Duration::from_secs(2);
        while !fs::read_to_string(&completed)
            .map(|content| content == "survived")
            .unwrap_or(false)
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            fs::read_to_string(completed).expect("child survived dropped parent handle"),
            "survived"
        );
    }

    #[test]
    fn boot_identity_is_stable_across_supervisor_processes_on_the_same_boot() {
        assert_eq!(boot_identity(), boot_identity());
    }
}
