//! Durable local supervision for the first, single-leaf Watchkeeper slice.
//!
//! The append-only job history is control truth. Process exit is only a wakeup
//! to inspect persisted run evidence; it is never accepted as completion.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use chrono::Utc;
use deadreckon_core::{
    DeadreckonError, DeadreckonPaths, JobProjection, JobView, LeaseClaimDisposition, LeaseOwner,
    LeaseReclaimReason, LeaseToken, ProviderFailureDisposition, SupervisedProcess,
    append_fenced_job_event, claim_job_lease, heartbeat_job_lease, load_job_lease, load_run,
    pid_is_alive, read_job_history, read_supervised_process, spawn_grouped,
    validate_acceptance_marker, validate_completion_receipt,
};
use deadreckon_protocol::{
    Job, JobAuthority, JobEvent, JobEventKind, JobEventSequence, JobId, JobSchemaVersion, JobShape,
    RunId, SemanticDecision, SemanticJudgment, StopReason,
};
use serde::{Deserialize, Serialize};
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
const CHILD_RELEASE_ACK_PREFIX: &str = "supervised-release-";
const SUPERVISOR_STDOUT_FILE: &str = "supervisor.out";
const SUPERVISOR_STDERR_FILE: &str = "supervisor.err";
const GUARDED_LAUNCH_PROTOCOL: &str = "stdin_release_v1";
const MAX_RELEASE_TOKEN_BYTES: u64 = 512;
const GUARDED_CHILD_SETTLE_TIMEOUT: Duration = Duration::from_secs(2);
const SUPERVISOR_FAILPOINT_ENABLE_ENV: &str = "DEADRECKON_TEST_SUPERVISOR_FAILPOINTS";
const SUPERVISOR_FAILPOINT_ENV: &str = "DEADRECKON_TEST_SUPERVISOR_FAILPOINT";
const TRUSTED_DRIVER_ATTEMPT_ENV: &str = "DEADRECKON_SUPERVISOR_ATTEMPT";
const TRUSTED_DRIVER_LAUNCH_ID_ENV: &str = "DEADRECKON_SUPERVISOR_LAUNCH_ID";
const TRUSTED_DRIVER_RELEASE_DIGEST_ENV: &str = "DEADRECKON_SUPERVISOR_RELEASE_TOKEN_SHA256";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GuardedDriverAuthority {
    pub(crate) job_id: String,
    pub(crate) attempt: u32,
    pub(crate) launch_id: String,
    pub(crate) lease_epoch: u64,
    pub(crate) release_token_sha256: String,
}

/// Keeps fenced ownership alive for the entire claimed operation, including
/// synchronous source hashing and asynchronous parent verification. Child
/// monitoring also heartbeats defensively, but it starts too late to protect
/// pre-attempt authority validation.
struct LeaseHeartbeatGuard {
    stop: Option<std::sync::mpsc::Sender<()>>,
    handle:
        Option<std::thread::JoinHandle<std::result::Result<(), deadreckon_core::DeadreckonError>>>,
}

impl LeaseHeartbeatGuard {
    fn start(
        paths: DeadreckonPaths,
        token: LeaseToken,
        interval: Duration,
        ttl: Duration,
    ) -> Result<Self> {
        let (stop, stopped) = std::sync::mpsc::channel();
        let thread_name = format!(
            "dr-lease-{}",
            &token.job_id.as_ref()[..token.job_id.as_ref().len().min(8)]
        );
        let handle = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                loop {
                    match stopped.recv_timeout(interval) {
                        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            return Ok(());
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            heartbeat_job_lease(&paths, &token, Utc::now(), ttl)?;
                        }
                    }
                }
            })?;
        Ok(Self {
            stop: Some(stop),
            handle: Some(handle),
        })
    }

    fn stop_inner(&mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        match handle.join() {
            Ok(result) => result.map_err(CliError::Core),
            Err(_) => Err(CliError::Core(DeadreckonError::InvalidInput(
                "lease heartbeat thread panicked".to_string(),
            ))),
        }
    }

    #[cfg(test)]
    fn finish(mut self, operation: Result<()>) -> Result<()> {
        let heartbeat = self.stop_inner();
        operation.and(heartbeat)
    }
}

impl Drop for LeaseHeartbeatGuard {
    fn drop(&mut self) {
        let _ = self.stop_inner();
    }
}

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

#[derive(Debug, Clone)]
struct PreparedChildLaunch {
    attempt: u32,
    launch_id: String,
    release_token: String,
    release_token_sha256: String,
}

impl PreparedChildLaunch {
    fn new(attempt: u32) -> Self {
        let launch_id = Uuid::new_v4().to_string();
        let release_token = format!("{}:{}", launch_id, Uuid::new_v4());
        let release_token_sha256 = deadreckon_core::flight::sha256_text(&release_token);
        Self {
            attempt,
            launch_id,
            release_token,
            release_token_sha256,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SupervisorChildMetadata {
    #[serde(flatten)]
    process: SupervisedProcess,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    launch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attempt: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    release_token_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    boot_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process_start_identity: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SupervisorReleaseAck {
    launch_protocol: String,
    job_id: String,
    attempt: u32,
    launch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    release_token_sha256: Option<String>,
    /// Read-only migration support for acknowledgements written before the
    /// digest-only format. New acknowledgements never serialize this secret.
    #[serde(default, rename = "release_token", skip_serializing)]
    legacy_release_token: Option<String>,
    pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process_start_identity: Option<String>,
    acknowledged_at: chrono::DateTime<Utc>,
}

impl SupervisorChildMetadata {
    fn legacy(process: SupervisedProcess) -> Self {
        Self {
            process,
            launch_id: None,
            attempt: None,
            release_token_sha256: None,
            boot_id: None,
            process_start_identity: None,
        }
    }
}

#[derive(Debug)]
struct GuardedChild {
    child: Child,
    release: Option<ChildStdin>,
    metadata: SupervisorChildMetadata,
    release_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GuardedLaunchRecovery {
    attempt: u32,
    launch_id: String,
    release_token_sha256: String,
    attempt_started: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnlinkedLaunchDisposition {
    Relaunch,
    RecheckAcknowledgement,
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

pub(crate) fn supervisor_launch_command(
    job_id: &str,
    attempt: u32,
    launch_id: String,
    release_token_sha256: &str,
) -> Result<()> {
    let paths = DeadreckonPaths::discover();
    let job = deadreckon_core::load_job(&paths, job_id)?;
    if job.job_id.as_ref() != job_id
        || attempt == 0
        || attempt > job.policy.max_attempts.max(1)
        || Uuid::parse_str(&launch_id).is_err()
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "guarded launch does not match an approved job attempt".to_string(),
        )));
    }

    let mut release_bytes = Vec::new();
    std::io::stdin()
        .take(MAX_RELEASE_TOKEN_BYTES + 1)
        .read_to_end(&mut release_bytes)?;
    if release_bytes.len() as u64 > MAX_RELEASE_TOKEN_BYTES {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "guarded launch release token exceeded its bounded size".to_string(),
        )));
    }
    let release_token = std::str::from_utf8(&release_bytes)
        .map_err(|_| {
            CliError::Core(DeadreckonError::InvalidInput(
                "guarded launch release token was not UTF-8".to_string(),
            ))
        })?
        .trim();
    if release_token.is_empty()
        || deadreckon_core::flight::sha256_text(release_token) != release_token_sha256
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "guarded launch release token did not match the prepared launch".to_string(),
        )));
    }

    let process_start = process_start_identity(std::process::id());
    if !launcher_link_is_durable(
        &paths,
        job_id,
        attempt,
        &launch_id,
        release_token_sha256,
        std::process::id(),
        process_start.as_deref(),
    )? {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "guarded launch refused because its fenced ChildLinked event is not durable"
                .to_string(),
        )));
    }

    let release_ack = SupervisorReleaseAck {
        launch_protocol: GUARDED_LAUNCH_PROTOCOL.to_string(),
        job_id: job_id.to_string(),
        attempt,
        launch_id,
        release_token_sha256: Some(release_token_sha256.to_string()),
        legacy_release_token: None,
        pid: std::process::id(),
        process_start_identity: process_start,
        acknowledged_at: Utc::now(),
    };
    write_release_ack(&paths, &release_ack)?;
    supervisor_test_failpoint("after_release_ack");

    let launch = load_launch_inputs(&paths, &job)?;
    let executable = std::env::current_exe()?;
    let mut command = build_job_driver_command(&paths, &job, &launch, &executable, attempt)?;
    apply_durable_scope_root(&mut command, &launch.plan);
    apply_guarded_driver_metadata(&mut command, &release_ack, release_token_sha256);
    command
        .current_dir(&job.source_cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut driver = command.spawn()?;
    let mut driver_stdin = driver.stdin.take().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "guarded job driver did not expose its authentication pipe".to_string(),
        ))
    })?;
    driver_stdin.write_all(release_token.as_bytes())?;
    driver_stdin.write_all(b"\n")?;
    driver_stdin.flush()?;
    drop(driver_stdin);
    let status = driver.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "guarded job driver exited with status {status}"
        ))))
    }
}

pub(crate) fn require_guarded_driver_launch(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<GuardedDriverAuthority> {
    if std::env::var(TRUSTED_SUPERVISOR_JOB_ID_ENV).as_deref() != Ok(job_id) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "job driver identity does not match its durable supervisor".to_string(),
        )));
    }
    let attempt = std::env::var(TRUSTED_DRIVER_ATTEMPT_ENV)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "job driver is missing its guarded attempt identity".to_string(),
            ))
        })?;
    let launch_id = std::env::var(TRUSTED_DRIVER_LAUNCH_ID_ENV)
        .ok()
        .filter(|value| Uuid::parse_str(value).is_ok())
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "job driver is missing its guarded launch identity".to_string(),
            ))
        })?;
    let release_token_sha256 = std::env::var(TRUSTED_DRIVER_RELEASE_DIGEST_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CliError::Core(DeadreckonError::InvalidInput(
                "job driver is missing its guarded release digest".to_string(),
            ))
        })?;

    let mut release_bytes = Vec::new();
    std::io::stdin()
        .take(MAX_RELEASE_TOKEN_BYTES + 1)
        .read_to_end(&mut release_bytes)?;
    if release_bytes.len() as u64 > MAX_RELEASE_TOKEN_BYTES {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "job driver authentication token exceeded its bounded size".to_string(),
        )));
    }
    let release_token = std::str::from_utf8(&release_bytes)
        .map_err(|_| {
            CliError::Core(DeadreckonError::InvalidInput(
                "job driver authentication token was not UTF-8".to_string(),
            ))
        })?
        .trim();
    if release_token.is_empty()
        || deadreckon_core::flight::sha256_text(release_token) != release_token_sha256
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "job driver did not receive the private guarded-launch capability".to_string(),
        )));
    }

    let acknowledgement = release_ack(paths, job_id, &launch_id)?.ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(
            "job driver has no guarded-launch acknowledgement".to_string(),
        ))
    })?;
    let lease_epoch = load_job_lease(paths, &JobId(job_id.to_string()))?.epoch;
    let valid_ack = acknowledgement.launch_protocol == GUARDED_LAUNCH_PROTOCOL
        && acknowledgement.job_id == job_id
        && acknowledgement.attempt == attempt
        && acknowledgement.launch_id == launch_id
        && release_ack_token_sha256(&acknowledgement).as_deref()
            == Some(release_token_sha256.as_str())
        && launcher_link_is_durable(
            paths,
            job_id,
            attempt,
            &launch_id,
            &release_token_sha256,
            acknowledgement.pid,
            acknowledgement.process_start_identity.as_deref(),
        )?;
    if !valid_ack {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "job driver guarded-launch capability is not bound to the current fenced child"
                .to_string(),
        )));
    }
    Ok(GuardedDriverAuthority {
        job_id: job_id.to_string(),
        attempt,
        launch_id,
        lease_epoch,
        release_token_sha256,
    })
}

pub(crate) fn guarded_driver_authority_is_live(
    paths: &DeadreckonPaths,
    authority: &GuardedDriverAuthority,
) -> Result<bool> {
    let Some(acknowledgement) = release_ack(paths, &authority.job_id, &authority.launch_id)? else {
        return Ok(false);
    };
    if acknowledgement.launch_protocol != GUARDED_LAUNCH_PROTOCOL
        || acknowledgement.job_id != authority.job_id
        || acknowledgement.attempt != authority.attempt
        || acknowledgement.launch_id != authority.launch_id
        || release_ack_token_sha256(&acknowledgement).as_deref()
            != Some(authority.release_token_sha256.as_str())
    {
        return Ok(false);
    }
    let lease = load_job_lease(paths, &JobId(authority.job_id.clone()))?;
    if lease.epoch != authority.lease_epoch {
        return Ok(false);
    }
    launcher_link_is_durable(
        paths,
        &authority.job_id,
        authority.attempt,
        &authority.launch_id,
        &authority.release_token_sha256,
        acknowledgement.pid,
        acknowledgement.process_start_identity.as_deref(),
    )
}

fn apply_guarded_driver_metadata(
    command: &mut Command,
    acknowledgement: &SupervisorReleaseAck,
    release_token_sha256: &str,
) {
    command
        .env(
            TRUSTED_DRIVER_ATTEMPT_ENV,
            acknowledgement.attempt.to_string(),
        )
        .env(TRUSTED_DRIVER_LAUNCH_ID_ENV, &acknowledgement.launch_id)
        .env(TRUSTED_DRIVER_RELEASE_DIGEST_ENV, release_token_sha256);
}

pub(crate) fn remove_guarded_driver_metadata(command: &mut Command) {
    command
        .env_remove(TRUSTED_SUPERVISOR_JOB_ID_ENV)
        .env_remove(TRUSTED_SUPERVISOR_LAUNCH_PLAN_ENV)
        .env_remove(TRUSTED_DRIVER_ATTEMPT_ENV)
        .env_remove(TRUSTED_DRIVER_LAUNCH_ID_ENV)
        .env_remove(TRUSTED_DRIVER_RELEASE_DIGEST_ENV);
}

fn launcher_link_is_durable(
    paths: &DeadreckonPaths,
    job_id: &str,
    attempt: u32,
    launch_id: &str,
    release_token_sha256: &str,
    pid: u32,
    process_start_identity: Option<&str>,
) -> Result<bool> {
    let history = read_job_history(&paths.job_events(job_id))?;
    let lease = load_job_lease(paths, &JobId(job_id.to_string()))?;
    let view = JobView::load(paths, job_id)?;
    if lease.expires_at <= Utc::now() || !pid_is_alive(pid) || view.projection.is_terminal() {
        return Ok(false);
    }
    let current_lease_epoch = lease.epoch;
    let mut prepared = false;
    for event in history.events() {
        if event.kind == JobEventKind::ChildLaunchPrepared
            && launch_detail_matches(&event.detail, attempt, launch_id, release_token_sha256)
        {
            prepared = true;
            continue;
        }
        if prepared
            && event.kind == JobEventKind::ChildLinked
            && event.lease_epoch == current_lease_epoch
            && launch_detail_matches(&event.detail, attempt, launch_id, release_token_sha256)
            && event.detail.get("pid").and_then(Value::as_u64) == Some(u64::from(pid))
            && match process_start_identity {
                Some(identity) => {
                    event
                        .detail
                        .get("process_start_identity")
                        .and_then(Value::as_str)
                        == Some(identity)
                }
                None => event.detail.get("process_start_identity").is_none(),
            }
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn launch_detail_matches(
    detail: &Value,
    attempt: u32,
    launch_id: &str,
    release_token_sha256: &str,
) -> bool {
    detail.get("launch_protocol").and_then(Value::as_str) == Some(GUARDED_LAUNCH_PROTOCOL)
        && detail.get("attempt").and_then(Value::as_u64) == Some(u64::from(attempt))
        && detail.get("launch_id").and_then(Value::as_str) == Some(launch_id)
        && detail.get("release_token_sha256").and_then(Value::as_str) == Some(release_token_sha256)
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
    let _heartbeat =
        LeaseHeartbeatGuard::start(paths.clone(), token.clone(), HEARTBEAT_INTERVAL, LEASE_TTL)?;
    let reboot_reclaim = matches!(
        claim.disposition,
        LeaseClaimDisposition::Reclaimed(LeaseReclaimReason::BootIdentityChanged)
    );
    let claimed = JobView::load(paths, job_id)?;
    if claimed.projection.stop_reason == Some(StopReason::CancelRequested)
        && finish_cancel_requested(paths, &token, None)?
    {
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
    match super::graph_job::recover_pending_driver_state(paths, &initial.job) {
        Ok(super::graph_job::PendingDriverRecovery::BudgetExhausted {
            stop_reason,
            reason,
        }) => {
            let artifact = match initial.job.shape {
                JobShape::Graph => "plan",
                JobShape::LegacyCampaign => "campaign",
                JobShape::Single | JobShape::LegacyChain => "job",
            };
            finish_advanced_budget_attempt(
                paths,
                &token,
                ChildExit {
                    status: None,
                    adopted: true,
                },
                stop_reason,
                artifact,
                &reason,
            )?;
            return Ok(());
        }
        Ok(
            super::graph_job::PendingDriverRecovery::Unchanged
            | super::graph_job::PendingDriverRecovery::Recovered,
        ) => {}
        Err(error) => {
            append_terminal_event(
                paths,
                &token,
                JobEventKind::Blocked,
                StopReason::CorruptHistory,
                json!({
                    "reason": format!(
                        "could not safely recover a crash-partial advanced root artifact: {error}"
                    )
                }),
            )?;
            return Ok(());
        }
    }

    let max_attempts = initial.job.policy.max_attempts.max(1);
    let mut resuming_advanced = false;
    let mut guarded_recovery =
        match recoverable_unlinked_guarded_launch(paths, job_id, &initial.projection) {
            Ok(recovery) => recovery,
            Err(error) => {
                append_terminal_event(
                    paths,
                    &token,
                    JobEventKind::Blocked,
                    StopReason::CorruptHistory,
                    json!({ "reason": error.to_string() }),
                )?;
                return Ok(());
            }
        };
    let mut stored_child = match child_metadata(paths, job_id) {
        Ok(metadata) => metadata,
        Err(error) => {
            append_terminal_event(
                paths,
                &token,
                JobEventKind::Blocked,
                StopReason::CorruptHistory,
                json!({ "reason": error.to_string() }),
            )?;
            return Ok(());
        }
    };
    if let Some(recovery) = guarded_recovery.as_ref() {
        match prepare_unlinked_launch_recovery(paths, job_id, &mut stored_child, recovery) {
            Ok(UnlinkedLaunchDisposition::Relaunch) => {}
            Ok(UnlinkedLaunchDisposition::RecheckAcknowledgement) => {
                match recoverable_unlinked_guarded_launch(paths, job_id, &initial.projection) {
                    Ok(None) => guarded_recovery = None,
                    Ok(Some(_)) => {
                        append_terminal_event(
                            paths,
                            &token,
                            JobEventKind::Blocked,
                            StopReason::LostContainment,
                            json!({
                                "reason": "guarded child acknowledgement disappeared while recovery was reconciling it"
                            }),
                        )?;
                        return Ok(());
                    }
                    Err(error) => {
                        append_terminal_event(
                            paths,
                            &token,
                            JobEventKind::Blocked,
                            StopReason::CorruptHistory,
                            json!({ "reason": error.to_string() }),
                        )?;
                        return Ok(());
                    }
                }
            }
            Err(error) => {
                append_terminal_event(
                    paths,
                    &token,
                    JobEventKind::Blocked,
                    StopReason::LostContainment,
                    json!({ "reason": error.to_string() }),
                )?;
                return Ok(());
            }
        }
    }
    if let Some(child) = stored_child {
        // A PID observed after a reboot may belong to an unrelated process.
        // Boot identity is stronger evidence than PID reuse, so never adopt it.
        if !reboot_reclaim && child_identity_is_current(&child) {
            append_control_event(
                paths,
                &token,
                JobEventKind::ChildLinked,
                format!("adopt-child:{}:{}", token.epoch, child.process.pid),
                child_link_detail(&initial.job, &child, true, None),
            )?;
            let launch_id = child.launch_id.clone();
            let exit =
                monitor_child(paths, &token, MonitoredChild::Adopted(child.process.pid)).await?;
            if let Err(error) = reconcile_attempt_processes(
                paths,
                job_id,
                Some(&child),
                Duration::from_secs(2),
                false,
            ) {
                block_for_lost_containment(paths, &token, &error.to_string())?;
                return Ok(());
            }
            let attempt = initial.projection.attempt_count.max(1);
            remove_child_control_files(paths, job_id, launch_id.as_deref())?;
            match reconcile_child_exit(paths, &initial.job, &token, exit, attempt, max_attempts)
                .await?
            {
                ChildReconciliation::Retry => resuming_advanced = true,
                ChildReconciliation::Finished => return Ok(()),
            }
        }
        if !resuming_advanced {
            if let Err(error) = reconcile_attempt_processes(
                paths,
                job_id,
                (!reboot_reclaim).then_some(&child),
                Duration::from_secs(2),
                reboot_reclaim,
            ) {
                block_for_lost_containment(paths, &token, &error.to_string())?;
                return Ok(());
            }
            remove_child_control_files(paths, job_id, child.launch_id.as_deref())?;
            match reconcile_child_exit(
                paths,
                &initial.job,
                &token,
                ChildExit {
                    status: None,
                    adopted: true,
                },
                initial.projection.attempt_count.max(1),
                max_attempts,
            )
            .await?
            {
                ChildReconciliation::Retry => resuming_advanced = true,
                ChildReconciliation::Finished => return Ok(()),
            }
        }
    }
    if let Err(error) =
        reconcile_attempt_processes(paths, job_id, None, Duration::from_secs(2), reboot_reclaim)
    {
        block_for_lost_containment(paths, &token, &error.to_string())?;
        return Ok(());
    }

    // Advanced terminal evidence is durable before the conductor exits. If
    // the machine dies after the child sidecar is removed but before the Job
    // terminal is appended, classify that evidence instead of declaring a
    // resumable result lost.
    if advanced_artifact_waits_for_terminal_classification(paths, &initial.job) {
        classify_advanced_attempt(
            paths,
            &initial.job,
            &token,
            ChildExit {
                status: None,
                adopted: true,
            },
        )
        .await?;
        if latest_job_event_is_retry_scheduled(paths, job_id)? {
            resuming_advanced = true;
        } else {
            return Ok(());
        }
    }
    if matches!(
        initial.job.shape,
        JobShape::Graph | JobShape::LegacyCampaign
    ) && latest_job_event_is_retry_scheduled(paths, job_id)?
    {
        resuming_advanced = true;
    }

    if finish_cancel_requested(paths, &token, None)? {
        return Ok(());
    }

    // An attempt was durably started but no child identity survived. P6 cannot
    // prove the old process group is dead, so fail closed instead of duplicating
    // mutating work. P7 closes the smaller spawn-before-sidecar window.
    if initial.projection.attempt_count > 0
        && guarded_recovery.is_none()
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
        && guarded_recovery.is_none()
        && !resuming_advanced
        && maybe_schedule_advanced_recovery(
            paths,
            &initial.job,
            &token,
            initial.projection.attempt_count,
            max_attempts,
        )?
    {
        resuming_advanced = true;
    }
    if initial.projection.attempt_count > 0 && guarded_recovery.is_none() && !resuming_advanced {
        if finish_cancel_requested(paths, &token, None)? {
            return Ok(());
        }
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

    let _launch = match load_launch_inputs(paths, &initial.job) {
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
    let first_attempt = guarded_recovery.as_ref().map_or_else(
        || initial.projection.attempt_count.saturating_add(1),
        |recovery| recovery.attempt,
    );
    for attempt in first_attempt..=max_attempts {
        if finish_cancel_requested(paths, &token, None)? {
            return Ok(());
        }
        let attempt_already_started = guarded_recovery
            .as_ref()
            .is_some_and(|recovery| recovery.attempt == attempt && recovery.attempt_started);
        let prepared = PreparedChildLaunch::new(attempt);
        append_control_event(
            paths,
            &token,
            JobEventKind::ChildLaunchPrepared,
            format!(
                "child-launch-prepared:{}:{}",
                token.epoch, prepared.launch_id
            ),
            child_launch_prepared_detail(&initial.job, &prepared),
        )?;
        supervisor_test_failpoint("after_launch_prepared");
        if !attempt_already_started {
            append_control_event(
                paths,
                &token,
                JobEventKind::AttemptStarted,
                format!("attempt-started:{}:{attempt}", token.epoch),
                attempt_detail(&initial.job, attempt),
            )?;
            supervisor_test_failpoint("after_attempt_started");
        }
        guarded_recovery = None;

        if finish_cancel_requested(paths, &token, None)? {
            return Ok(());
        }
        match spawn_job_driver(paths, &initial.job, &instance.executable, &prepared) {
            Ok(mut guarded) => {
                append_control_event(
                    paths,
                    &token,
                    JobEventKind::ChildLinked,
                    format!("child-linked:{}:{attempt}", token.epoch),
                    child_link_detail(&initial.job, &guarded.metadata, false, Some(attempt)),
                )?;
                supervisor_test_failpoint("after_child_linked");
                let release_error = release_guarded_child(&mut guarded).err();
                if release_error.is_none() {
                    supervisor_test_failpoint("after_child_released");
                }
                let launch_id = guarded.metadata.launch_id.clone();
                let exit =
                    monitor_child(paths, &token, MonitoredChild::Owned(guarded.child)).await?;
                if let Err(error) = reconcile_attempt_processes(
                    paths,
                    job_id,
                    Some(&guarded.metadata),
                    Duration::from_secs(2),
                    false,
                ) {
                    block_for_lost_containment(paths, &token, &error.to_string())?;
                    return Ok(());
                }
                remove_child_control_files(paths, job_id, launch_id.as_deref())?;
                if finish_cancel_requested(paths, &token, Some(&exit))? {
                    return Ok(());
                }
                if let Some(error) = release_error {
                    append_attempt_stopped(
                        paths,
                        &token,
                        StopReason::LostContainment,
                        json!({
                            "attempt": attempt,
                            "reason": "guarded child release failed",
                            "error": error.to_string(),
                            "exit": exit_detail(&exit),
                        }),
                    )?;
                    append_terminal_event(
                        paths,
                        &token,
                        JobEventKind::Blocked,
                        StopReason::LostContainment,
                        json!({
                            "reason": "cannot prove whether the guarded child received its release token"
                        }),
                    )?;
                    return Ok(());
                }
                match reconcile_child_exit(paths, &initial.job, &token, exit, attempt, max_attempts)
                    .await?
                {
                    ChildReconciliation::Retry => {
                        heartbeat_job_lease(paths, &token, Utc::now(), LEASE_TTL)?;
                        continue;
                    }
                    ChildReconciliation::Finished => return Ok(()),
                }
            }
            Err(error) => {
                if finish_cancel_requested(paths, &token, None)? {
                    return Ok(());
                }
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
    if super::graph_job::parent_repair_is_pending(paths, job) {
        return true;
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

fn advanced_artifact_waits_for_terminal_classification(paths: &DeadreckonPaths, job: &Job) -> bool {
    let Ok(driver) = super::graph_job::load_driver_state(paths, job.job_id.as_ref()) else {
        return false;
    };
    if driver.job_id != job.job_id || driver.artifact_id != job.job_id.as_ref() {
        return false;
    }
    if super::graph_job::parent_repair_candidate_is_ready(paths, job)
        || super::graph_job::parent_repair_needs_projection(paths, job)
    {
        return true;
    }
    if super::graph_job::parent_repair_is_pending(paths, job) {
        return false;
    }
    match job.shape {
        JobShape::Graph
            if driver.artifact_kind == "plan"
                && matches!(
                    driver.kind,
                    super::graph_job::DriverKind::Review | super::graph_job::DriverKind::FullPlan
                ) =>
        {
            let Ok(plan) = deadreckon_core::plan::load_plan(paths, job.job_id.as_ref()) else {
                return false;
            };
            matches!(
                plan.status,
                deadreckon_core::plan::PlanStatus::Merged
                    | deadreckon_core::plan::PlanStatus::Failed
            ) || !matches!(plan_budget_exhaustion(paths, &plan.plan_id), Ok(None))
        }
        JobShape::LegacyCampaign
            if driver.artifact_kind == "campaign"
                && driver.kind == super::graph_job::DriverKind::Campaign =>
        {
            let campaign_dir = paths.plan_dir(job.job_id.as_ref());
            let Ok(campaign) = deadreckon_core::campaign::read_campaign(&campaign_dir) else {
                return false;
            };
            matches!(
                campaign.status,
                deadreckon_core::campaign::CampaignStatus::Merged
                    | deadreckon_core::campaign::CampaignStatus::Failed
                    | deadreckon_core::campaign::CampaignStatus::Killed
            ) || !matches!(campaign_budget_exhaustion(&campaign_dir), Ok(None))
        }
        JobShape::Single | JobShape::LegacyChain | JobShape::Graph | JobShape::LegacyCampaign => {
            false
        }
    }
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
        &deadreckon_core::flight::build_deliverable_file_index(&job.source_cwd)?.tree_hash(),
    )?;
    if authority.semantic_judge_mode != job.policy.semantic_judge {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "semantic judge policy mismatch for job {}",
            job.job_id
        ))));
    }
    let execution = job.policy.execution.as_ref().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "job {} predates immutable execution policy; refusing unattended execution",
            job.job_id
        )))
    })?;
    if !execution.require_containment
        || execution.sandbox_requested != authority.sandbox_requested
        || !execution.tools.contains_key("bash")
        || !execution.tools.contains_key("write_file")
    {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "effective execution policy does not match authority for job {}",
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

#[cfg(test)]
pub(super) fn validate_launch_inputs_for_test(paths: &DeadreckonPaths, job: &Job) -> Result<()> {
    load_launch_inputs(paths, job).map(drop)
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
    executable: &Path,
    prepared: &PreparedChildLaunch,
) -> Result<GuardedChild> {
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
    let mut command = Command::new(executable);
    command
        .arg("supervisor")
        .arg("launch")
        .arg(job.job_id.as_ref())
        .arg(prepared.attempt.to_string())
        .arg(&prepared.launch_id)
        .arg(&prepared.release_token_sha256)
        .env("DEADRECKON_HOME", paths.home())
        .current_dir(&job.source_cwd)
        .stdin(Stdio::piped())
        .stdout(stdout)
        .stderr(stderr);
    let (mut child, terminator) = spawn_grouped(command)?;
    supervisor_test_failpoint("after_guarded_spawn");
    let Some(release) = child.stdin.take() else {
        let _ = terminator.terminate(Duration::from_secs(2));
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "guarded launcher did not expose its release pipe".to_string(),
        )));
    };
    let process = SupervisedProcess {
        pid: child.id(),
        #[cfg(unix)]
        pgid: Some(child.id()),
        #[cfg(not(unix))]
        pgid: None,
    };
    let metadata = SupervisorChildMetadata {
        process,
        launch_id: Some(prepared.launch_id.clone()),
        attempt: Some(prepared.attempt),
        release_token_sha256: Some(prepared.release_token_sha256.clone()),
        boot_id: Some(boot_identity()),
        process_start_identity: process_start_identity(child.id()),
    };
    let metadata_path = child_metadata_path(paths, job.job_id.as_ref());
    if let Err(error) = write_child_metadata(&metadata_path, &metadata) {
        drop(release);
        let _ = terminator.terminate(Duration::from_secs(2));
        return Err(error);
    }
    supervisor_test_failpoint("after_child_metadata");
    Ok(GuardedChild {
        child,
        release: Some(release),
        metadata,
        release_token: prepared.release_token.clone(),
    })
}

fn build_job_driver_command(
    paths: &DeadreckonPaths,
    job: &Job,
    launch: &LaunchInputs,
    executable: &Path,
    attempt: u32,
) -> Result<Command> {
    let command = match job.shape {
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
    Ok(command)
}

fn release_guarded_child(child: &mut GuardedChild) -> std::io::Result<()> {
    let Some(mut release) = child.release.take() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "guarded child was already released",
        ));
    };
    release.write_all(child.release_token.as_bytes())?;
    release.write_all(b"\n")?;
    release.flush()
}

fn apply_durable_scope_root(command: &mut Command, plan: &LaunchPlan) {
    if let Some(scope_root) = plan
        .signals
        .get(super::job::DURABLE_SCOPE_ROOT_SIGNAL)
        .and_then(|value| serde_json::from_value::<PathBuf>(value.clone()).ok())
    {
        command.env("DEADRECKON_SCOPE_ROOT", scope_root);
    }
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

fn child_launch_prepared_detail(job: &Job, prepared: &PreparedChildLaunch) -> Value {
    let mut detail = attempt_detail(job, prepared.attempt)
        .as_object()
        .cloned()
        .unwrap_or_default();
    detail.insert(
        "launch_protocol".to_string(),
        json!(GUARDED_LAUNCH_PROTOCOL),
    );
    detail.insert("launch_id".to_string(), json!(prepared.launch_id));
    detail.insert(
        "release_token_sha256".to_string(),
        json!(prepared.release_token_sha256),
    );
    Value::Object(detail)
}

fn child_link_detail(
    job: &Job,
    metadata: &SupervisorChildMetadata,
    adopted: bool,
    attempt: Option<u32>,
) -> Value {
    let mut detail = serde_json::Map::new();
    detail.insert("adopted".to_string(), Value::Bool(adopted));
    detail.insert("pid".to_string(), json!(metadata.process.pid));
    detail.insert("process_group".to_string(), json!(metadata.process.pgid));
    if let Some(attempt) = attempt.or(metadata.attempt) {
        detail.insert("attempt".to_string(), json!(attempt));
    }
    if let Some(launch_id) = metadata.launch_id.as_deref() {
        detail.insert(
            "launch_protocol".to_string(),
            json!(GUARDED_LAUNCH_PROTOCOL),
        );
        detail.insert("launch_id".to_string(), json!(launch_id));
    }
    if let Some(digest) = metadata.release_token_sha256.as_deref() {
        detail.insert("release_token_sha256".to_string(), json!(digest));
    }
    if let Some(boot_id) = metadata.boot_id.as_deref() {
        detail.insert("boot_id".to_string(), json!(boot_id));
    }
    if let Some(identity) = metadata.process_start_identity.as_deref() {
        detail.insert("process_start_identity".to_string(), json!(identity));
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

fn write_child_metadata(path: &Path, metadata: &SupervisorChildMetadata) -> Result<()> {
    write_synced_json(path, metadata)
}

fn child_release_ack_path(paths: &DeadreckonPaths, job_id: &str, launch_id: &str) -> PathBuf {
    paths
        .job_dir(job_id)
        .join(format!("{CHILD_RELEASE_ACK_PREFIX}{launch_id}.json"))
}

fn remove_child_control_files(
    paths: &DeadreckonPaths,
    job_id: &str,
    launch_id: Option<&str>,
) -> Result<()> {
    remove_control_file_if_present(&child_metadata_path(paths, job_id))?;
    if let Some(launch_id) = launch_id {
        remove_control_file_if_present(&child_release_ack_path(paths, job_id, launch_id))?;
    }
    Ok(())
}

fn remove_control_file_if_present(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(source.into()),
    }
}

fn write_release_ack(paths: &DeadreckonPaths, ack: &SupervisorReleaseAck) -> Result<()> {
    write_synced_json(
        &child_release_ack_path(paths, &ack.job_id, &ack.launch_id),
        ack,
    )
}

fn release_ack_token_sha256(ack: &SupervisorReleaseAck) -> Option<String> {
    ack.release_token_sha256.clone().or_else(|| {
        ack.legacy_release_token
            .as_deref()
            .map(deadreckon_core::flight::sha256_text)
    })
}

fn release_ack(
    paths: &DeadreckonPaths,
    job_id: &str,
    launch_id: &str,
) -> Result<Option<SupervisorReleaseAck>> {
    let path = child_release_ack_path(paths, job_id, launch_id);
    let raw = match fs::read(&path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(source.into()),
    };
    serde_json::from_slice(&raw)
        .map(Some)
        .map_err(|source| CliError::Core(DeadreckonError::Json { path, source }))
}

fn write_synced_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or_else(|| {
        CliError::Core(DeadreckonError::InvalidInput(format!(
            "supervisor control path has no parent: {}",
            path.display()
        )))
    })?;
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("supervisor-control");
    let temp_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4()));
    let write_result = (|| -> Result<()> {
        let mut encoded = serde_json::to_vec(value).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: path.to_path_buf(),
                source,
            })
        })?;
        encoded.push(b'\n');
        let mut temp = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)?;
        temp.write_all(&encoded)?;
        temp.sync_all()?;
        fs::rename(&temp_path, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

fn child_metadata(
    paths: &DeadreckonPaths,
    job_id: &str,
) -> Result<Option<SupervisorChildMetadata>> {
    let path = child_metadata_path(paths, job_id);
    let raw = match fs::read(&path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(source.into()),
    };
    let trimmed = raw.strip_suffix(b"\n").unwrap_or(raw.as_slice());
    if trimmed.starts_with(b"{") {
        let metadata = serde_json::from_slice(trimmed).map_err(|source| {
            CliError::Core(DeadreckonError::Json {
                path: path.clone(),
                source,
            })
        })?;
        return Ok(Some(metadata));
    }
    let process = read_supervised_process(&path)?;
    Ok(Some(SupervisorChildMetadata::legacy(process)))
}

fn child_identity_is_current(metadata: &SupervisorChildMetadata) -> bool {
    if !pid_is_alive(metadata.process.pid)
        || metadata.boot_id.as_deref() != Some(boot_identity().as_str())
    {
        return false;
    }
    let Some(expected) = metadata.process_start_identity.as_deref() else {
        return false;
    };
    process_start_identity(metadata.process.pid).as_deref() == Some(expected)
}

fn child_process_may_still_be_live(metadata: &SupervisorChildMetadata) -> bool {
    if !pid_is_alive(metadata.process.pid)
        || metadata.boot_id.as_deref() != Some(boot_identity().as_str())
    {
        return false;
    }
    match metadata.process_start_identity.as_deref() {
        Some(expected) => process_start_identity(metadata.process.pid)
            .as_deref()
            .is_none_or(|observed| observed == expected),
        None => true,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SupervisorChildIdentity {
    Current,
    Exited,
    DifferentBoot,
    Reused,
    Unverifiable,
}

fn supervisor_child_identity(metadata: &SupervisorChildMetadata) -> SupervisorChildIdentity {
    if !pid_is_alive(metadata.process.pid) {
        return SupervisorChildIdentity::Exited;
    }
    let Some(stored_boot) = metadata.boot_id.as_deref() else {
        return SupervisorChildIdentity::Unverifiable;
    };
    if stored_boot != boot_identity() {
        return SupervisorChildIdentity::DifferentBoot;
    }
    let Some(expected) = metadata.process_start_identity.as_deref() else {
        return SupervisorChildIdentity::Unverifiable;
    };
    match process_start_identity(metadata.process.pid) {
        Some(observed) if observed == expected => SupervisorChildIdentity::Current,
        Some(_) => SupervisorChildIdentity::Reused,
        None => SupervisorChildIdentity::Unverifiable,
    }
}

fn reconcile_supervisor_child(metadata: &SupervisorChildMetadata, grace: Duration) -> Result<()> {
    match supervisor_child_identity(metadata) {
        SupervisorChildIdentity::DifferentBoot => Ok(()),
        SupervisorChildIdentity::Current => {
            let outcome = super::job::terminate_supervised_process(metadata.process, grace);
            if let deadreckon_core::TerminationOutcome::Failed(reason) = outcome {
                Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                    "could not stop supervised Job process {}: {reason}",
                    metadata.process.pid
                ))))
            } else {
                Ok(())
            }
        }
        SupervisorChildIdentity::Exited => {
            #[cfg(unix)]
            if let Some(pgid) = metadata.process.pgid {
                use deadreckon_core::ChildTerminator as _;

                let pgid = i32::try_from(pgid).map_err(|_| {
                    CliError::Core(DeadreckonError::InvalidInput(format!(
                        "invalid supervised Job process group {pgid}"
                    )))
                })?;
                if let deadreckon_core::TerminationOutcome::Failed(reason) =
                    deadreckon_core::ProcessGroupTerminator::new(pgid).terminate(grace)
                {
                    return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                        "could not reconcile residual Job process group {pgid}: {reason}"
                    ))));
                }
            }
            Ok(())
        }
        SupervisorChildIdentity::Reused => {
            Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "refused to signal reused supervised Job pid {}",
                metadata.process.pid
            ))))
        }
        SupervisorChildIdentity::Unverifiable => {
            Err(CliError::Core(DeadreckonError::InvalidInput(format!(
                "cannot verify supervised Job process identity {}",
                metadata.process.pid
            ))))
        }
    }
}

fn reconcile_attempt_processes(
    paths: &DeadreckonPaths,
    job_id: &str,
    outer: Option<&SupervisorChildMetadata>,
    grace: Duration,
    boot_changed: bool,
) -> Result<()> {
    if let Some(outer) = outer {
        reconcile_supervisor_child(outer, grace)?;
    }
    if let Ok(state) = load_run(paths, job_id) {
        super::job::reconcile_run_supervised_processes(&state, grace, boot_changed)?;
    }
    Ok(())
}

fn reconcile_attempt_processes_from_disk(
    paths: &DeadreckonPaths,
    job_id: &str,
    grace: Duration,
) -> Result<()> {
    let outer = child_metadata(paths, job_id)?;
    reconcile_attempt_processes(paths, job_id, outer.as_ref(), grace, false)?;
    if let Some(outer) = outer {
        remove_child_control_files(paths, job_id, outer.launch_id.as_deref())?;
    }
    Ok(())
}

fn recoverable_unlinked_guarded_launch(
    paths: &DeadreckonPaths,
    job_id: &str,
    projection: &JobProjection,
) -> Result<Option<GuardedLaunchRecovery>> {
    let history = read_job_history(&paths.job_events(job_id))?;
    for (index, prepared_event) in history.events().iter().enumerate().rev() {
        if prepared_event.kind != JobEventKind::ChildLaunchPrepared {
            continue;
        }
        let attempt = prepared_event
            .detail
            .get("attempt")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "ChildLaunchPrepared event is missing a valid attempt".to_string(),
                ))
            })?;
        let launch_id = prepared_event
            .detail
            .get("launch_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "ChildLaunchPrepared event is missing its launch id".to_string(),
                ))
            })?;
        let release_token_sha256 = prepared_event
            .detail
            .get("release_token_sha256")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CliError::Core(DeadreckonError::InvalidInput(
                    "ChildLaunchPrepared event is missing its release-token digest".to_string(),
                ))
            })?;
        if prepared_event
            .detail
            .get("launch_protocol")
            .and_then(Value::as_str)
            != Some(GUARDED_LAUNCH_PROTOCOL)
        {
            return Err(CliError::Core(DeadreckonError::InvalidInput(
                "ChildLaunchPrepared event names an unsupported launch protocol".to_string(),
            )));
        }

        let attempt_started = history.events().iter().any(|event| {
            event.kind == JobEventKind::AttemptStarted
                && event.detail.get("attempt").and_then(Value::as_u64) == Some(u64::from(attempt))
        });
        let expected_attempt = if attempt_started {
            projection.attempt_count
        } else {
            projection.attempt_count.saturating_add(1)
        };
        if attempt != expected_attempt {
            continue;
        }

        if let Some(ack) = release_ack(paths, job_id, launch_id)? {
            let linked = history.events()[index + 1..].iter().any(|event| {
                event.kind == JobEventKind::ChildLinked
                    && launch_detail_matches(
                        &event.detail,
                        attempt,
                        launch_id,
                        release_token_sha256,
                    )
                    && event.detail.get("pid").and_then(Value::as_u64) == Some(u64::from(ack.pid))
                    && match ack.process_start_identity.as_deref() {
                        Some(identity) => {
                            event
                                .detail
                                .get("process_start_identity")
                                .and_then(Value::as_str)
                                == Some(identity)
                        }
                        None => event.detail.get("process_start_identity").is_none(),
                    }
            });
            let valid_ack = ack.launch_protocol == GUARDED_LAUNCH_PROTOCOL
                && ack.job_id == job_id
                && ack.attempt == attempt
                && ack.launch_id == launch_id
                && release_ack_token_sha256(&ack).as_deref() == Some(release_token_sha256)
                && linked;
            if !valid_ack {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "guarded launch release acknowledgement failed validation".to_string(),
                )));
            }
            return Ok(None);
        }

        let invalidated = history.events()[index + 1..].iter().any(|event| {
            matches!(
                event.kind,
                JobEventKind::AttemptStopped
                    | JobEventKind::RetryScheduled
                    | JobEventKind::NeedsReview
                    | JobEventKind::Blocked
                    | JobEventKind::BudgetExhausted
                    | JobEventKind::DeadlineReached
                    | JobEventKind::Cancelled
                    | JobEventKind::Failed
                    | JobEventKind::Verified
            )
        });
        if invalidated {
            return Ok(None);
        }
        return Ok(Some(GuardedLaunchRecovery {
            attempt,
            launch_id: launch_id.to_string(),
            release_token_sha256: release_token_sha256.to_string(),
            attempt_started,
        }));
    }
    Ok(None)
}

fn prepare_unlinked_launch_recovery(
    paths: &DeadreckonPaths,
    job_id: &str,
    metadata: &mut Option<SupervisorChildMetadata>,
    recovery: &GuardedLaunchRecovery,
) -> Result<UnlinkedLaunchDisposition> {
    let Some(child) = metadata.as_ref() else {
        return Ok(UnlinkedLaunchDisposition::Relaunch);
    };
    let matches_prepared = child.launch_id.as_deref() == Some(recovery.launch_id.as_str())
        && child.attempt == Some(recovery.attempt)
        && child.release_token_sha256.as_deref() == Some(recovery.release_token_sha256.as_str());
    if !matches_prepared && child_process_may_still_be_live(child) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(
            "unlinked guarded launch conflicts with a different live supervised process"
                .to_string(),
        )));
    }
    if matches_prepared {
        let acknowledgement = child_release_ack_path(paths, job_id, recovery.launch_id.as_str());
        let deadline = Instant::now() + GUARDED_CHILD_SETTLE_TIMEOUT;
        loop {
            if acknowledgement.is_file() {
                return Ok(UnlinkedLaunchDisposition::RecheckAcknowledgement);
            }
            if !child_process_may_still_be_live(child) {
                break;
            }
            if Instant::now() >= deadline {
                return Err(CliError::Core(DeadreckonError::InvalidInput(
                    "linked guarded child remained alive without a durable release acknowledgement"
                        .to_string(),
                )));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if acknowledgement.is_file() {
            return Ok(UnlinkedLaunchDisposition::RecheckAcknowledgement);
        }
    }
    let path = child_metadata_path(paths, job_id);
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(source.into()),
    }
    *metadata = None;
    Ok(UnlinkedLaunchDisposition::Relaunch)
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
    if finish_cancel_requested(paths, token, Some(&exit))? {
        return Ok(());
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChildReconciliation {
    Retry,
    Finished,
}

async fn reconcile_child_exit(
    paths: &DeadreckonPaths,
    job: &Job,
    token: &LeaseToken,
    exit: ChildExit,
    attempt: u32,
    max_attempts: u32,
) -> Result<ChildReconciliation> {
    if finish_cancel_requested(paths, token, Some(&exit))? {
        return Ok(ChildReconciliation::Finished);
    }
    if matches!(job.shape, JobShape::Graph | JobShape::LegacyCampaign) {
        match super::graph_job::recover_pending_driver_state(paths, job) {
            Ok(super::graph_job::PendingDriverRecovery::BudgetExhausted {
                stop_reason,
                reason,
            }) => {
                let artifact = if job.shape == JobShape::Graph {
                    "plan"
                } else {
                    "campaign"
                };
                finish_advanced_budget_attempt(paths, token, exit, stop_reason, artifact, &reason)?;
                return Ok(ChildReconciliation::Finished);
            }
            Ok(
                super::graph_job::PendingDriverRecovery::Unchanged
                | super::graph_job::PendingDriverRecovery::Recovered,
            ) => {}
            Err(error) => {
                append_attempt_stopped(
                    paths,
                    token,
                    StopReason::CorruptHistory,
                    json!({
                        "exit": exit_detail(&exit),
                        "reason": format!(
                            "could not safely recover a crash-partial advanced root artifact: {error}"
                        ),
                    }),
                )?;
                append_terminal_event(
                    paths,
                    token,
                    JobEventKind::Blocked,
                    StopReason::CorruptHistory,
                    json!({
                        "reason": format!(
                            "could not safely recover a crash-partial advanced root artifact: {error}"
                        ),
                    }),
                )?;
                return Ok(ChildReconciliation::Finished);
            }
        }
    }
    let retry = match job.shape {
        JobShape::Single => {
            maybe_schedule_leaf_retry(paths, job, token, &exit, attempt, max_attempts)?
        }
        JobShape::Graph | JobShape::LegacyCampaign => {
            maybe_schedule_advanced_recovery(paths, job, token, attempt, max_attempts)?
        }
        JobShape::LegacyChain => false,
    };
    if retry {
        return Ok(ChildReconciliation::Retry);
    }
    classify_job_attempt(paths, job, token, exit, attempt >= max_attempts).await?;
    if matches!(job.shape, JobShape::Graph | JobShape::LegacyCampaign)
        && latest_job_event_is_retry_scheduled(paths, job.job_id.as_ref())?
    {
        return Ok(ChildReconciliation::Retry);
    }
    Ok(ChildReconciliation::Finished)
}

fn latest_job_event_is_retry_scheduled(paths: &DeadreckonPaths, job_id: &str) -> Result<bool> {
    let history = read_job_history(&paths.job_events(job_id))?;
    Ok(history
        .events()
        .last()
        .is_some_and(|event| event.kind == JobEventKind::RetryScheduled))
}

fn maybe_schedule_advanced_recovery(
    paths: &DeadreckonPaths,
    job: &Job,
    token: &LeaseToken,
    attempt: u32,
    max_attempts: u32,
) -> Result<bool> {
    if !matches!(job.shape, JobShape::Graph | JobShape::LegacyCampaign)
        || attempt >= max_attempts
        || !advanced_artifact_recoverable(paths, job)
    {
        return Ok(false);
    }
    if JobView::load(paths, job.job_id.as_ref())?
        .projection
        .stop_reason
        == Some(StopReason::CancelRequested)
    {
        return Ok(false);
    }
    schedule_advanced_recovery(paths, token, attempt)?;
    Ok(true)
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
    if finish_cancel_requested(paths, token, Some(&exit))? {
        return Ok(());
    }
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
            match plan_budget_exhaustion(paths, &plan.plan_id) {
                Ok(Some((stop_reason, reason))) => {
                    return finish_advanced_budget_attempt(
                        paths,
                        token,
                        exit,
                        stop_reason,
                        "plan",
                        &reason,
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    return fail_advanced_attempt(
                        paths,
                        token,
                        exit,
                        StopReason::CorruptHistory,
                        &format!("graph budget history is malformed: {error}"),
                    );
                }
            }
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
                        Ok(super::graph_job::ParentCompletion::ReviseRequested {
                            reason,
                            round,
                            intent_path,
                            intent_sha256,
                            judgment_path,
                            judgment_sha256,
                        }) => schedule_parent_semantic_repair(
                            paths,
                            job,
                            token,
                            &exit,
                            "plan",
                            plan.merged_run_id.as_deref(),
                            &reason,
                            round,
                            &intent_path,
                            &intent_sha256,
                            &judgment_path,
                            &judgment_sha256,
                        ),
                        Ok(super::graph_job::ParentCompletion::RepairPending {
                            reason,
                            round,
                            stop_reason,
                        }) => schedule_interrupted_parent_repair(
                            paths,
                            job,
                            token,
                            &exit,
                            "plan",
                            round,
                            stop_reason,
                            &reason,
                        ),
                        Ok(super::graph_job::ParentCompletion::RepairFailed {
                            reason,
                            stop_reason,
                        }) => fail_advanced_attempt(
                            paths,
                            token,
                            exit,
                            stop_reason,
                            &format!("graph parent repair failed: {reason}"),
                        ),
                        Ok(super::graph_job::ParentCompletion::Cancelled { reason }) => {
                            finish_cancelled_parent_completion(paths, token, &exit, "plan", &reason)
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
                        Ok(super::graph_job::ParentCompletion::BudgetExhausted {
                            reason,
                            stop_reason,
                        }) => {
                            append_parent_gate_passed_if_present(
                                paths,
                                job,
                                token,
                                "graph",
                                plan.merged_run_id.as_deref(),
                            )?;
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
                                JobEventKind::BudgetExhausted,
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
                            StopReason::CorruptHistory,
                            &format!("graph parent completion failed: {error}"),
                        ),
                    }
                }
                deadreckon_core::plan::PlanStatus::Failed => {
                    match plan_budget_exhaustion(paths, &plan.plan_id) {
                        Ok(Some((stop_reason, reason))) => finish_advanced_budget_attempt(
                            paths,
                            token,
                            exit,
                            stop_reason,
                            "plan",
                            &reason,
                        ),
                        Ok(None) => fail_advanced_attempt(
                            paths,
                            token,
                            exit,
                            StopReason::FatalProvider,
                            "graph conductor persisted a failed plan",
                        ),
                        Err(error) => fail_advanced_attempt(
                            paths,
                            token,
                            exit,
                            StopReason::CorruptHistory,
                            &format!("graph budget history is malformed: {error}"),
                        ),
                    }
                }
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
            match campaign_budget_exhaustion(&campaign_dir) {
                Ok(Some((stop_reason, reason))) => {
                    return finish_advanced_budget_attempt(
                        paths,
                        token,
                        exit,
                        stop_reason,
                        "campaign",
                        &reason,
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    return fail_advanced_attempt(
                        paths,
                        token,
                        exit,
                        StopReason::CorruptHistory,
                        &format!("campaign budget history is malformed: {error}"),
                    );
                }
            }
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
                        Ok(super::graph_job::ParentCompletion::ReviseRequested {
                            reason,
                            round,
                            intent_path,
                            intent_sha256,
                            judgment_path,
                            judgment_sha256,
                        }) => schedule_parent_semantic_repair(
                            paths,
                            job,
                            token,
                            &exit,
                            "campaign",
                            campaign.merged_run_id.as_deref(),
                            &reason,
                            round,
                            &intent_path,
                            &intent_sha256,
                            &judgment_path,
                            &judgment_sha256,
                        ),
                        Ok(super::graph_job::ParentCompletion::RepairPending {
                            reason,
                            round,
                            stop_reason,
                        }) => schedule_interrupted_parent_repair(
                            paths,
                            job,
                            token,
                            &exit,
                            "campaign",
                            round,
                            stop_reason,
                            &reason,
                        ),
                        Ok(super::graph_job::ParentCompletion::RepairFailed {
                            reason,
                            stop_reason,
                        }) => fail_advanced_attempt(
                            paths,
                            token,
                            exit,
                            stop_reason,
                            &format!("campaign parent repair failed: {reason}"),
                        ),
                        Ok(super::graph_job::ParentCompletion::Cancelled { reason }) => {
                            finish_cancelled_parent_completion(
                                paths, token, &exit, "campaign", &reason,
                            )
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
                        Ok(super::graph_job::ParentCompletion::BudgetExhausted {
                            reason,
                            stop_reason,
                        }) => {
                            append_parent_gate_passed_if_present(
                                paths,
                                job,
                                token,
                                "campaign",
                                campaign.merged_run_id.as_deref(),
                            )?;
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
                                JobEventKind::BudgetExhausted,
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
                            StopReason::CorruptHistory,
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
                deadreckon_core::campaign::CampaignStatus::Failed => {
                    match campaign_budget_exhaustion(&campaign_dir) {
                        Ok(Some((stop_reason, reason))) => finish_advanced_budget_attempt(
                            paths,
                            token,
                            exit,
                            stop_reason,
                            "campaign",
                            &reason,
                        ),
                        Ok(None) => fail_advanced_attempt(
                            paths,
                            token,
                            exit,
                            StopReason::FatalProvider,
                            "campaign conductor persisted a failed campaign",
                        ),
                        Err(error) => fail_advanced_attempt(
                            paths,
                            token,
                            exit,
                            StopReason::CorruptHistory,
                            &format!("campaign budget history is malformed: {error}"),
                        ),
                    }
                }
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

fn plan_budget_exhaustion(
    paths: &DeadreckonPaths,
    plan_id: &str,
) -> Result<Option<(StopReason, String)>> {
    let event = deadreckon_core::read_plan_events(paths, plan_id)?
        .into_iter()
        .rev()
        .find_map(|event| match event.event {
            deadreckon_core::PlanEventKind::TaskBudgetExhausted {
                dimension, reason, ..
            } => Some((dimension, reason)),
            deadreckon_core::PlanEventKind::RootBudgetExhausted { dimension, reason } => {
                Some((dimension, reason))
            }
            _ => None,
        });
    Ok(event.map(|(dimension, reason)| {
        let stop_reason = match dimension {
            deadreckon_core::plan::BudgetDimension::Spend => StopReason::SpendCap,
            deadreckon_core::plan::BudgetDimension::Wall => StopReason::WallCap,
        };
        (stop_reason, reason)
    }))
}

fn campaign_budget_exhaustion(campaign_dir: &Path) -> Result<Option<(StopReason, String)>> {
    let event = deadreckon_core::campaign::read_campaign_events(campaign_dir)?
        .into_iter()
        .rev()
        .find(|event| event.kind == "budget_exhausted");
    let Some(event) = event else {
        return Ok(None);
    };
    let stop_reason = match event.detail.get("stop_reason") {
        Some(value) => serde_json::from_value::<StopReason>(value.clone())?,
        None => StopReason::SpendCap,
    };
    if !matches!(stop_reason, StopReason::SpendCap | StopReason::WallCap) {
        return Err(CliError::Core(DeadreckonError::InvalidInput(format!(
            "campaign budget event names non-budget stop reason {stop_reason:?}"
        ))));
    }
    let reason = event
        .detail
        .get("reason")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| match stop_reason {
            StopReason::SpendCap => "campaign tree spend budget was exhausted".to_string(),
            StopReason::WallCap => "campaign tree wall-time budget was exhausted".to_string(),
            _ => unreachable!(),
        });
    Ok(Some((stop_reason, reason)))
}

fn append_parent_gate_passed_if_present(
    paths: &DeadreckonPaths,
    job: &Job,
    token: &LeaseToken,
    artifact: &str,
    merged_run_id: Option<&str>,
) -> Result<()> {
    let parent = load_run(paths, job.job_id.as_ref())?;
    let marker_path = deadreckon_core::marker_path_for_run_root(&parent.run_root);
    if !marker_path.is_file() {
        return Ok(());
    }
    validate_acceptance_marker(&parent).map_err(CliError::Core)?;
    append_control_event(
        paths,
        token,
        JobEventKind::DeterministicGatePassed,
        format!("{artifact}-gate-passed:{}", token.epoch),
        json!({
            "marker": marker_path,
            "merged_run_id": merged_run_id,
        }),
    )?;
    Ok(())
}

fn finish_cancelled_parent_completion(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    exit: &ChildExit,
    artifact: &str,
    reason: &str,
) -> Result<()> {
    if finish_cancel_requested(paths, token, Some(exit))? {
        return Ok(());
    }
    if attempt_is_active(paths, token.job_id.as_ref())? {
        append_attempt_stopped(
            paths,
            token,
            StopReason::CancelRequested,
            json!({
                "exit": exit_detail(exit),
                "artifact": artifact,
                "reason": reason,
            }),
        )?;
    }
    append_terminal_event(
        paths,
        token,
        JobEventKind::Cancelled,
        StopReason::CancelRequested,
        json!({
            "artifact": artifact,
            "reason": reason,
        }),
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn schedule_parent_semantic_repair(
    paths: &DeadreckonPaths,
    job: &Job,
    token: &LeaseToken,
    exit: &ChildExit,
    artifact: &str,
    merged_run_id: Option<&str>,
    reason: &str,
    round: u32,
    intent_path: &Path,
    intent_sha256: &str,
    judgment_path: &Path,
    judgment_sha256: &str,
) -> Result<()> {
    let parent = load_run(paths, job.job_id.as_ref())?;
    let marker_path = parent
        .run_root
        .join("proofs")
        .join("parent-repairs")
        .join(format!("round-{round}"))
        .join("pre-repair-marker.json");
    append_control_event(
        paths,
        token,
        JobEventKind::DeterministicGatePassed,
        format!("{artifact}-repair-gate-passed:{}:{round}", token.epoch),
        json!({
            "marker": marker_path,
            "merged_run_id": merged_run_id,
            "round": round,
        }),
    )?;
    append_control_event(
        paths,
        token,
        JobEventKind::SemanticJudgeRevise,
        format!("{artifact}-semantic-revise:{}:{round}", token.epoch),
        json!({
            "judgment": judgment_path,
            "judgment_sha256": judgment_sha256,
            "intent": intent_path,
            "intent_sha256": intent_sha256,
            "merged_run_id": merged_run_id,
            "round": round,
            "reason": reason,
        }),
    )?;
    append_attempt_stopped(
        paths,
        token,
        StopReason::SemanticRevise,
        json!({
            "exit": exit_detail(exit),
            "artifact": artifact,
            "result_run_id": merged_run_id,
            "round": round,
            "reason": reason,
        }),
    )?;
    if finish_cancel_requested(paths, token, Some(exit))? {
        return Ok(());
    }
    let attempt = JobView::load(paths, job.job_id.as_ref())?
        .projection
        .attempt_count;
    if attempt >= job.policy.max_attempts.max(1) {
        append_terminal_event(
            paths,
            token,
            JobEventKind::Failed,
            StopReason::AttemptLimit,
            json!({
                "reason": "semantic parent repair requires another attempt, but the approved attempt limit is exhausted",
                "semantic_reason": reason,
                "artifact": artifact,
                "round": round,
                "attempts": attempt,
            }),
        )?;
        return Ok(());
    }
    append_control_event(
        paths,
        token,
        JobEventKind::RetryScheduled,
        format!("parent-semantic-repair:{}:{round}", token.epoch),
        json!({
            "after_attempt": attempt,
            "reason": "parent_semantic_repair",
            "artifact": artifact,
            "round": round,
            "intent_sha256": intent_sha256,
        }),
    )?;
    let _ = finish_cancel_requested(paths, token, Some(exit))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn schedule_interrupted_parent_repair(
    paths: &DeadreckonPaths,
    job: &Job,
    token: &LeaseToken,
    exit: &ChildExit,
    artifact: &str,
    round: u32,
    stop_reason: StopReason,
    reason: &str,
) -> Result<()> {
    let attempt = JobView::load(paths, job.job_id.as_ref())?
        .projection
        .attempt_count;
    if attempt_is_active(paths, job.job_id.as_ref())? {
        append_attempt_stopped(
            paths,
            token,
            stop_reason,
            json!({
                "exit": exit_detail(exit),
                "artifact": artifact,
                "round": round,
                "reason": reason,
            }),
        )?;
    }
    if finish_cancel_requested(paths, token, Some(exit))? {
        return Ok(());
    }
    if attempt >= job.policy.max_attempts.max(1) {
        append_terminal_event(
            paths,
            token,
            JobEventKind::Failed,
            StopReason::AttemptLimit,
            json!({
                "reason": "interrupted semantic parent repair exhausted the approved attempt limit",
                "artifact": artifact,
                "round": round,
                "attempts": attempt,
            }),
        )?;
        return Ok(());
    }
    append_control_event(
        paths,
        token,
        JobEventKind::RetryScheduled,
        format!("parent-repair-recovery:{}:{round}:{attempt}", token.epoch),
        json!({
            "after_attempt": attempt,
            "reason": "resume_parent_semantic_repair",
            "artifact": artifact,
            "round": round,
        }),
    )?;
    let _ = finish_cancel_requested(paths, token, Some(exit))?;
    Ok(())
}

fn finish_advanced_budget_attempt(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    exit: ChildExit,
    stop_reason: StopReason,
    artifact: &str,
    reason: &str,
) -> Result<()> {
    if attempt_is_active(paths, token.job_id.as_ref())? {
        let projection = append_attempt_stopped(
            paths,
            token,
            stop_reason,
            json!({
                "exit": exit_detail(&exit),
                "artifact": artifact,
                "reason": reason,
            }),
        )?;
        if projection.stop_reason == Some(StopReason::CancelRequested) {
            finish_cancel_requested(paths, token, Some(&exit))?;
            return Ok(());
        }
    }
    append_terminal_event(
        paths,
        token,
        JobEventKind::BudgetExhausted,
        stop_reason,
        json!({
            "reason": reason,
            "artifact": artifact,
        }),
    )?;
    Ok(())
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
                let stop_reason = super::graph_job::semantic_decision_stop_reason(semantic)
                    .unwrap_or(StopReason::SemanticUnavailable);
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
                let stop_reason = super::graph_job::semantic_decision_stop_reason(semantic)
                    .unwrap_or(StopReason::SemanticUnavailable);
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
    let view = JobView::load(paths, token.job_id.as_ref())?;
    if view.projection.is_terminal() {
        return Ok(view.projection);
    }
    // Cancellation may only become terminal after every supervised process is
    // reconciled. If cleanup identity is lost, the honest terminal is blocked
    // containment—not a reassuring but unproved `Cancelled`.
    let containment_block = kind == JobEventKind::Blocked && reason == StopReason::LostContainment;
    let cancelled =
        view.projection.stop_reason == Some(StopReason::CancelRequested) && !containment_block;
    let suppressed_kind = serde_json::to_value(kind)?;
    let effective_kind = if cancelled {
        JobEventKind::Cancelled
    } else {
        kind
    };
    let effective_reason = if cancelled {
        StopReason::CancelRequested
    } else {
        reason
    };
    let effective_extra = if cancelled {
        json!({
            "reason": "operator cancellation won before terminal classification",
            "suppressed_terminal_kind": suppressed_kind,
        })
    } else {
        extra
    };
    let projection = append_control_event(
        paths,
        token,
        effective_kind,
        format!("terminal:{}:{}", token.epoch, Uuid::new_v4()),
        merge_stop_reason(effective_reason, effective_extra),
    )?;
    if !projection.is_terminal() && projection.stop_reason == Some(StopReason::CancelRequested) {
        return append_control_event(
            paths,
            token,
            JobEventKind::Cancelled,
            format!("terminal-cancelled:{}:{}", token.epoch, Uuid::new_v4()),
            merge_stop_reason(
                StopReason::CancelRequested,
                json!({
                    "reason": "operator cancellation raced terminal classification and won",
                    "suppressed_terminal_kind": suppressed_kind,
                }),
            ),
        );
    }
    Ok(projection)
}

fn finish_cancel_requested(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    exit: Option<&ChildExit>,
) -> Result<bool> {
    let view = JobView::load(paths, token.job_id.as_ref())?;
    if view.projection.is_terminal() {
        return Ok(true);
    }
    if view.projection.stop_reason != Some(StopReason::CancelRequested) {
        return Ok(false);
    }
    if let Err(error) =
        reconcile_attempt_processes_from_disk(paths, token.job_id.as_ref(), Duration::from_secs(2))
    {
        block_for_lost_containment(
            paths,
            token,
            &format!(
                "operator cancellation could not prove every supervised process stopped: {error}"
            ),
        )?;
        return Ok(true);
    }
    let attempt_active = attempt_is_active(paths, token.job_id.as_ref())?;
    if attempt_active {
        append_attempt_stopped(
            paths,
            token,
            StopReason::CancelRequested,
            json!({
                "reason": "operator cancelled the active durable attempt",
                "exit": exit.map(exit_detail),
            }),
        )?;
    }
    append_terminal_event(
        paths,
        token,
        JobEventKind::Cancelled,
        StopReason::CancelRequested,
        json!({
            "reason": if attempt_active {
                "operator cancelled the active durable attempt"
            } else {
                "operator cancelled before another attempt was launched"
            },
        }),
    )?;
    Ok(true)
}

fn block_for_lost_containment(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    reason: &str,
) -> Result<()> {
    if attempt_is_active(paths, token.job_id.as_ref())? {
        append_attempt_stopped(
            paths,
            token,
            StopReason::LostContainment,
            json!({ "reason": reason }),
        )?;
    }
    append_terminal_event(
        paths,
        token,
        JobEventKind::Blocked,
        StopReason::LostContainment,
        json!({ "reason": reason }),
    )?;
    Ok(())
}

fn attempt_is_active(paths: &DeadreckonPaths, job_id: &str) -> Result<bool> {
    let history = read_job_history(&paths.job_events(job_id))?;
    Ok(history
        .events()
        .iter()
        .rev()
        .find_map(|event| match event.kind {
            JobEventKind::AttemptStopped => Some(false),
            JobEventKind::AttemptStarted => Some(true),
            _ => None,
        })
        .unwrap_or(false))
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

fn supervisor_test_failpoint(name: &str) {
    if std::env::var(SUPERVISOR_FAILPOINT_ENABLE_ENV).as_deref() == Ok("1")
        && std::env::var(SUPERVISOR_FAILPOINT_ENV).as_deref() == Ok(name)
    {
        std::process::exit(86);
    }
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

fn process_start_identity(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let raw = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let command_end = raw.rfind(')')?;
        let fields = raw[command_end + 1..]
            .split_whitespace()
            .collect::<Vec<_>>();
        // `/proc/<pid>/stat` field 22 is process start ticks. The slice starts
        // at field 3 (`state`), so index 19 is the stable same-boot identity.
        return fields.get(19).map(|start| format!("linux:{start}"));
    }
    #[cfg(target_os = "macos")]
    {
        let output = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "lstart="])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let start = String::from_utf8_lossy(&output.stdout);
        let start = start.trim();
        return (!start.is_empty()).then(|| format!("macos:{start}"));
    }
    #[cfg(target_os = "windows")]
    {
        let script =
            format!("(Get-Process -Id {pid} -ErrorAction Stop).StartTime.ToUniversalTime().Ticks");
        let output = Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let start = String::from_utf8_lossy(&output.stdout);
        let start = start.trim();
        return (!start.is_empty()).then(|| format!("windows:{start}"));
    }
    #[allow(unreachable_code)]
    None
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

    #[test]
    fn release_ack_persists_only_the_capability_digest() {
        let ack = SupervisorReleaseAck {
            launch_protocol: GUARDED_LAUNCH_PROTOCOL.to_string(),
            job_id: "job".to_string(),
            attempt: 1,
            launch_id: "launch".to_string(),
            release_token_sha256: Some("sha256:approved".to_string()),
            legacy_release_token: Some("must-never-be-serialized".to_string()),
            pid: 1,
            process_start_identity: None,
            acknowledged_at: Utc::now(),
        };
        let value = serde_json::to_value(&ack).expect("ack JSON");
        assert_eq!(
            value.get("release_token_sha256").and_then(Value::as_str),
            Some("sha256:approved")
        );
        assert!(
            value.get("release_token").is_none(),
            "the guarded-launch secret leaked into its readable acknowledgement: {value}"
        );

        let legacy: SupervisorReleaseAck = serde_json::from_value(json!({
            "launch_protocol": GUARDED_LAUNCH_PROTOCOL,
            "job_id": "job",
            "attempt": 1,
            "launch_id": "launch",
            "release_token": "legacy-private-token",
            "pid": 1,
            "acknowledged_at": Utc::now(),
        }))
        .expect("legacy ack");
        assert_eq!(
            release_ack_token_sha256(&legacy).as_deref(),
            Some(deadreckon_core::flight::sha256_text("legacy-private-token").as_str())
        );
        let migrated = serde_json::to_value(&legacy).expect("migrated ack");
        assert!(
            migrated.get("release_token").is_none(),
            "legacy secrets must not be reserialized"
        );
    }

    fn fixture(temp: &TempDir, max_attempts: u32) -> (DeadreckonPaths, Job) {
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let source = temp.path().join("source");
        fs::create_dir_all(&source).expect("source");
        fs::write(source.join("fixture-proof.txt"), "approved fixture\n").expect("fixture proof");
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
            signals: json!({
                super::super::job::DURABLE_SCOPE_ROOT_SIGNAL: &source,
            }),
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
            "name: fixture\nchecks:\n  - kind: file_exists\n    path: \"{working_dir}/fixture-proof.txt\"\n",
        )
        .expect("contract");
        let policy = JobPolicy {
            max_spend_usd: 3.0,
            max_wall_seconds: 60,
            max_attempts,
            deadline: None,
            semantic_judge: SemanticJudgeMode::Required,
            execution: Some(deadreckon_protocol::JobExecutionPolicy::workspace_only(
                "none",
            )),
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
            source_tree_sha256: deadreckon_core::flight::build_deliverable_file_index(&source)
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
            scope: deadreckon_core::paths::workspace_scope(&source).expect("fixture scope"),
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

    #[test]
    fn lease_heartbeat_covers_blocking_pre_attempt_work() {
        let temp = TempDir::new().expect("temp");
        let (paths, job) = fixture(&temp, 1);
        let lease_ttl = Duration::from_millis(500);
        let owner = instance(PathBuf::from("/opt/deadreckon")).owner;
        let claim =
            claim_job_lease(&paths, &job.job_id, &owner, Utc::now(), lease_ttl).expect("claim");
        let token = claim.token();
        let heartbeat = LeaseHeartbeatGuard::start(
            paths.clone(),
            token.clone(),
            Duration::from_millis(50),
            lease_ttl,
        )
        .expect("heartbeat");

        // Model an authority/source-tree validation that takes longer than the
        // original lease. The fenced attempt event must still be accepted.
        thread::sleep(Duration::from_millis(1_200));
        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "attempt-after-slow-authority-validation".to_string(),
            attempt_detail(&job, 1),
        )
        .expect("attempt remains fenced by a live lease");
        heartbeat.finish(Ok(())).expect("stop heartbeat");

        let lease = deadreckon_core::load_job_lease(&paths, &job.job_id).expect("lease");
        assert!(lease.heartbeat_at > lease.acquired_at);
        assert!(lease.expires_at > Utc::now());
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

    fn parent_semantic_input_sha256(parent: &deadreckon_core::PipelineState, job: &Job) -> String {
        let marker = deadreckon_core::validate_acceptance_marker(parent).expect("parent marker");
        let evidence = deadreckon_runtime::build_semantic_evidence_against_source(
            parent,
            &marker,
            &job.source_cwd,
        )
        .expect("parent semantic evidence");
        deadreckon_core::flight::sha256_text(
            &serde_json::to_string(&evidence).expect("semantic evidence json"),
        )
    }

    fn update_job_attempt_limit(paths: &DeadreckonPaths, job: &mut Job, max_attempts: u32) {
        job.policy.max_attempts = max_attempts;
        let authority_path = paths.job_authority(job.job_id.as_ref());
        let mut authority: JobAuthority =
            serde_json::from_slice(&fs::read(&authority_path).expect("authority"))
                .expect("authority json");
        authority.effective_policy_sha256 = deadreckon_core::flight::sha256_text(
            &serde_json::to_string(&job.policy).expect("policy json"),
        );
        fs::write(
            &authority_path,
            serde_json::to_vec_pretty(&authority).expect("authority json"),
        )
        .expect("authority update");
        job.authority_sha256 =
            deadreckon_core::flight::sha256_file(&authority_path).expect("authority digest");
        fs::write(
            paths.job_json(job.job_id.as_ref()),
            serde_json::to_vec_pretty(job).expect("job json"),
        )
        .expect("job update");
    }

    fn append_test_child_link(
        paths: &DeadreckonPaths,
        token: &LeaseToken,
        attempt: u32,
        launch_id: &str,
    ) {
        append_control_event(
            paths,
            token,
            JobEventKind::ChildLinked,
            format!("test-child-linked:{attempt}:{launch_id}"),
            json!({
                "attempt": attempt,
                "launch_id": launch_id,
                "release_token_sha256": deadreckon_core::flight::sha256_text(launch_id),
                "pid": std::process::id(),
            }),
        )
        .expect("child link");
    }

    fn parent_result_tree_sha256(parent: &deadreckon_core::PipelineState) -> String {
        let mut index = deadreckon_core::flight::build_deliverable_file_index(&parent.working_dir)
            .expect("parent index");
        index.files.remove(Path::new("manifest.json"));
        index.tree_hash()
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
        plan.owner_job_id = Some(job.job_id.as_ref().to_string());
        plan.parent_cwd = Some(job.source_cwd.clone());
        plan.acceptance_path = Some(super::super::job::job_acceptance_path(
            &paths,
            job.job_id.as_ref(),
        ));
        plan.tasks[2].depends_on =
            vec![plan.tasks[0].task_id.clone(), plan.tasks[1].task_id.clone()];
        plan.status = status;
        if status == deadreckon_core::plan::PlanStatus::Merged {
            plan.merged_run_id = Some("result-run".to_string());
        }
        deadreckon_core::plan::save_plan(&paths, &plan).expect("plan state");
        super::super::graph_job::record_plan_planner_accounting(&paths, &plan.plan_id, None)
            .expect("planner accounting");
        super::super::graph_job::record_owned_plan_tree(&paths, &plan)
            .expect("owned plan definition");
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
        campaign.root_planner_accounting =
            Some(super::super::graph_job::root_planner_accounting(None));
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
        super::super::graph_job::write_owned_campaign_record(
            &paths,
            job.job_id.as_ref(),
            &campaign,
        )
        .expect("owned campaign definition");
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

    #[test]
    fn missing_advanced_mapping_is_repaired_without_replanning_or_zeroing_usage() {
        for shape in [JobShape::Graph, JobShape::LegacyCampaign] {
            let temp = TempDir::new().expect("tempdir");
            let (paths, job, accounting) = match shape {
                JobShape::Graph => {
                    let (paths, job) =
                        graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Pending);
                    let mut plan = deadreckon_core::plan::load_plan(&paths, job.job_id.as_ref())
                        .expect("plan");
                    plan.owner_job_id = Some(job.job_id.as_ref().to_string());
                    plan.parent_cwd = Some(job.source_cwd.clone());
                    plan.acceptance_path = Some(super::super::job::job_acceptance_path(
                        &paths,
                        job.job_id.as_ref(),
                    ));
                    let accounting = deadreckon_core::plan::RootPlannerAccounting {
                        schema_version: 1,
                        planner_invoked: true,
                        provider: Some("recovery-planner".to_string()),
                        model: Some("recovery-model".to_string()),
                        input_tokens: 101,
                        output_tokens: 23,
                        cost_usd: 0.42,
                        subscription: false,
                        wall_seconds: 1.5,
                        recorded_at: Utc::now(),
                    };
                    plan.root_planner_accounting = Some(accounting.clone());
                    deadreckon_core::plan::save_plan(&paths, &plan).expect("crash-partial plan");
                    fs::remove_file(
                        paths
                            .plan_dir(job.job_id.as_ref())
                            .join("root-planner-accounting.json"),
                    )
                    .expect("remove derived accounting");
                    (paths, job, accounting)
                }
                JobShape::LegacyCampaign => {
                    let (paths, job, mut campaign) =
                        campaign_fixture(&temp, deadreckon_core::campaign::CampaignStatus::Pending);
                    let accounting = deadreckon_core::plan::RootPlannerAccounting {
                        schema_version: 1,
                        planner_invoked: true,
                        provider: Some("recovery-planner".to_string()),
                        model: Some("recovery-model".to_string()),
                        input_tokens: 101,
                        output_tokens: 23,
                        cost_usd: 0.42,
                        subscription: false,
                        wall_seconds: 1.5,
                        recorded_at: Utc::now(),
                    };
                    campaign.root_planner_accounting = Some(accounting.clone());
                    deadreckon_core::campaign::write_campaign(
                        &paths.plan_dir(job.job_id.as_ref()),
                        &campaign,
                    )
                    .expect("crash-partial campaign");
                    (paths, job, accounting)
                }
                _ => unreachable!(),
            };
            let token = claim_started_attempt(&paths, &job, 1);
            drop(token);
            fs::remove_file(super::super::graph_job::driver_state_path(
                &paths,
                job.job_id.as_ref(),
            ))
            .expect("remove crash-partial mapping");

            assert_eq!(
                super::super::graph_job::recover_pending_driver_state(&paths, &job)
                    .expect("repair mapping"),
                super::super::graph_job::PendingDriverRecovery::Recovered
            );
            assert_eq!(
                super::super::graph_job::recover_pending_driver_state(&paths, &job)
                    .expect("idempotent mapping"),
                super::super::graph_job::PendingDriverRecovery::Unchanged
            );
            let mapping = super::super::graph_job::load_driver_state(&paths, job.job_id.as_ref())
                .expect("repaired mapping");
            assert_eq!(mapping.job_id, job.job_id);
            assert_eq!(mapping.artifact_id, job.job_id.as_ref());

            match shape {
                JobShape::Graph => {
                    let restored: deadreckon_core::plan::RootPlannerAccounting =
                        serde_json::from_slice(
                            &fs::read(
                                paths
                                    .plan_dir(job.job_id.as_ref())
                                    .join("root-planner-accounting.json"),
                            )
                            .expect("restored accounting"),
                        )
                        .expect("accounting JSON");
                    assert_eq!(restored, accounting);
                    assert!(
                        paths
                            .job_dir(job.job_id.as_ref())
                            .join("owned-plans")
                            .join(format!("{}.json", job.job_id))
                            .is_file()
                    );
                }
                JobShape::LegacyCampaign => {
                    let events = deadreckon_core::campaign::read_campaign_events(
                        &paths.plan_dir(job.job_id.as_ref()),
                    )
                    .expect("campaign events");
                    let event = events
                        .iter()
                        .rev()
                        .find(|event| event.kind == "root_planner_accounting")
                        .expect("restored accounting event");
                    assert_eq!(
                        event.detail.get("cost_usd").and_then(Value::as_f64),
                        Some(accounting.cost_usd)
                    );
                    assert_eq!(
                        event.detail.get("wall_seconds").and_then(Value::as_f64),
                        Some(accounting.wall_seconds)
                    );
                    let campaign = deadreckon_core::campaign::read_campaign(
                        &paths.plan_dir(job.job_id.as_ref()),
                    )
                    .expect("campaign");
                    super::super::graph_job::validate_owned_campaign(
                        &paths,
                        &campaign,
                        job.job_id.as_ref(),
                    )
                    .expect("repaired ownership");
                }
                _ => unreachable!(),
            }
            assert_eq!(
                fs::read_dir(paths.jobs_dir())
                    .expect("jobs")
                    .filter_map(std::result::Result::ok)
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .count(),
                1
            );
        }
    }

    #[tokio::test]
    async fn root_planner_budget_exhaustion_survives_mapping_creation_crash() {
        for shape in [JobShape::Graph, JobShape::LegacyCampaign] {
            for stop_reason in [StopReason::SpendCap, StopReason::WallCap] {
                let temp = TempDir::new().expect("tempdir");
                let (paths, job) = match shape {
                    JobShape::Graph => {
                        let (paths, job) =
                            graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Pending);
                        let mut plan =
                            deadreckon_core::plan::load_plan(&paths, job.job_id.as_ref())
                                .expect("plan");
                        plan.owner_job_id = Some(job.job_id.as_ref().to_string());
                        plan.parent_cwd = Some(job.source_cwd.clone());
                        plan.acceptance_path = Some(super::super::job::job_acceptance_path(
                            &paths,
                            job.job_id.as_ref(),
                        ));
                        plan.root_planner_accounting =
                            Some(deadreckon_core::plan::RootPlannerAccounting {
                                schema_version: 1,
                                planner_invoked: true,
                                provider: Some("bounded-planner".to_string()),
                                model: Some("bounded-model".to_string()),
                                input_tokens: 10,
                                output_tokens: 5,
                                cost_usd: if stop_reason == StopReason::SpendCap {
                                    job.policy.max_spend_usd
                                } else {
                                    0.25
                                },
                                subscription: false,
                                wall_seconds: if stop_reason == StopReason::WallCap {
                                    job.policy.max_wall_seconds as f64
                                } else {
                                    0.25
                                },
                                recorded_at: Utc::now(),
                            });
                        deadreckon_core::plan::save_plan(&paths, &plan)
                            .expect("crash-partial plan");
                        fs::remove_file(
                            paths
                                .plan_dir(job.job_id.as_ref())
                                .join("root-planner-accounting.json"),
                        )
                        .expect("remove derived accounting");
                        (paths, job)
                    }
                    JobShape::LegacyCampaign => {
                        let (paths, job, mut campaign) = campaign_fixture(
                            &temp,
                            deadreckon_core::campaign::CampaignStatus::Pending,
                        );
                        campaign.root_planner_accounting =
                            Some(deadreckon_core::plan::RootPlannerAccounting {
                                schema_version: 1,
                                planner_invoked: true,
                                provider: Some("bounded-planner".to_string()),
                                model: Some("bounded-model".to_string()),
                                input_tokens: 10,
                                output_tokens: 5,
                                cost_usd: if stop_reason == StopReason::SpendCap {
                                    job.policy.max_spend_usd
                                } else {
                                    0.25
                                },
                                subscription: false,
                                wall_seconds: if stop_reason == StopReason::WallCap {
                                    job.policy.max_wall_seconds as f64
                                } else {
                                    0.25
                                },
                                recorded_at: Utc::now(),
                            });
                        deadreckon_core::campaign::write_campaign(
                            &paths.plan_dir(job.job_id.as_ref()),
                            &campaign,
                        )
                        .expect("crash-partial campaign");
                        (paths, job)
                    }
                    _ => unreachable!(),
                };
                fs::remove_file(super::super::graph_job::driver_state_path(
                    &paths,
                    job.job_id.as_ref(),
                ))
                .expect("remove crash-partial mapping");
                let token = claim_started_attempt(&paths, &job, 1);

                assert_eq!(
                    reconcile_child_exit(
                        &paths,
                        &job,
                        &token,
                        ChildExit {
                            status: None,
                            adopted: true,
                        },
                        1,
                        job.policy.max_attempts,
                    )
                    .await
                    .expect("budget recovery"),
                    ChildReconciliation::Finished
                );

                let view = JobView::load(&paths, job.job_id.as_ref()).expect("job view");
                assert_eq!(
                    view.projection.outcome,
                    Some(deadreckon_protocol::JobOutcome::BudgetExhausted)
                );
                assert_eq!(view.projection.stop_reason, Some(stop_reason));
                let history =
                    read_job_history(&paths.job_events(job.job_id.as_ref())).expect("history");
                assert_eq!(
                    history
                        .events()
                        .iter()
                        .filter(|event| event.kind == JobEventKind::BudgetExhausted)
                        .count(),
                    1
                );
                assert_eq!(
                    history
                        .events()
                        .iter()
                        .filter(|event| is_terminal_kind(event.kind))
                        .count(),
                    1
                );
                assert!(history.events().iter().all(|event| {
                    !matches!(
                        event.kind,
                        JobEventKind::RetryScheduled | JobEventKind::Failed | JobEventKind::Blocked
                    )
                }));
                let mapping =
                    super::super::graph_job::load_driver_state(&paths, job.job_id.as_ref())
                        .expect("repaired mapping");
                assert_eq!(mapping.job_id, job.job_id);
                assert_eq!(mapping.artifact_id, job.job_id.as_ref());

                match shape {
                    JobShape::Graph => {
                        let plan = deadreckon_core::plan::load_plan(&paths, job.job_id.as_ref())
                            .expect("failed plan");
                        assert_eq!(plan.status, deadreckon_core::plan::PlanStatus::Failed);
                        assert!(plan.tasks.iter().all(|task| {
                            task.status == deadreckon_core::plan::PlanTaskStatus::Pending
                                && task.attempts.is_empty()
                                && task.child_run_id.is_none()
                        }));
                        let events = deadreckon_core::read_plan_events(&paths, &plan.plan_id)
                            .expect("plan events");
                        assert_eq!(
                            events
                                .iter()
                                .filter(|event| {
                                    matches!(
                                        event.event,
                                        deadreckon_core::PlanEventKind::RootBudgetExhausted { .. }
                                    )
                                })
                                .count(),
                            1
                        );
                        assert!(!events.iter().any(|event| {
                            matches!(
                                event.event,
                                deadreckon_core::PlanEventKind::TaskStarted { .. }
                                    | deadreckon_core::PlanEventKind::TaskRunDiscovered { .. }
                            )
                        }));
                    }
                    JobShape::LegacyCampaign => {
                        let campaign = deadreckon_core::campaign::read_campaign(
                            &paths.plan_dir(job.job_id.as_ref()),
                        )
                        .expect("failed campaign");
                        assert_eq!(
                            campaign.status,
                            deadreckon_core::campaign::CampaignStatus::Failed
                        );
                        assert!(campaign.sub_goals.iter().all(|sub| {
                            sub.status == deadreckon_core::campaign::SubGoalStatus::Pending
                                && sub.sub_plan_id.is_none()
                                && sub.result_run_id.is_none()
                        }));
                        let events = deadreckon_core::campaign::read_campaign_events(
                            &paths.plan_dir(job.job_id.as_ref()),
                        )
                        .expect("campaign events");
                        assert_eq!(
                            events
                                .iter()
                                .filter(|event| event.kind == "budget_exhausted")
                                .count(),
                            1
                        );
                        assert!(!events.iter().any(|event| {
                            matches!(
                                event.kind.as_str(),
                                "sub_launch_prepared" | "sub_launched" | "sub_recovered"
                            )
                        }));
                    }
                    _ => unreachable!(),
                }
            }
        }
    }

    #[tokio::test]
    async fn persisted_advanced_budget_terminal_survives_child_sidecar_removal() {
        for shape in [JobShape::Graph, JobShape::LegacyCampaign] {
            for stop_reason in [StopReason::SpendCap, StopReason::WallCap] {
                for attempt_already_stopped in [false, true] {
                    let temp = TempDir::new().expect("tempdir");
                    let (paths, mut job) = match shape {
                        JobShape::Graph => {
                            let (paths, job) =
                                graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Failed);
                            let dimension = match stop_reason {
                                StopReason::SpendCap => {
                                    deadreckon_core::plan::BudgetDimension::Spend
                                }
                                StopReason::WallCap => deadreckon_core::plan::BudgetDimension::Wall,
                                _ => unreachable!(),
                            };
                            deadreckon_core::append_plan_event(
                                &paths,
                                job.job_id.as_ref(),
                                deadreckon_core::PlanEventKind::RootBudgetExhausted {
                                    dimension,
                                    reason: format!(
                                        "persisted {stop_reason:?} before supervisor restart"
                                    ),
                                },
                            )
                            .expect("typed Plan budget event");
                            (paths, job)
                        }
                        JobShape::LegacyCampaign => {
                            let (paths, job, _) = campaign_fixture(
                                &temp,
                                deadreckon_core::campaign::CampaignStatus::Failed,
                            );
                            deadreckon_core::campaign::append_campaign_event(
                                &paths.plan_dir(job.job_id.as_ref()),
                                "budget_exhausted",
                                json!({
                                    "reason": format!(
                                        "persisted {stop_reason:?} before supervisor restart"
                                    ),
                                    "phase": "child_execution",
                                    "stop_reason": stop_reason,
                                }),
                            )
                            .expect("typed Campaign budget event");
                            (paths, job)
                        }
                        JobShape::Single | JobShape::LegacyChain => unreachable!(),
                    };
                    job.policy.max_attempts = 2;
                    fs::write(
                        paths.job_json(job.job_id.as_ref()),
                        serde_json::to_vec_pretty(&job).expect("job JSON"),
                    )
                    .expect("two-attempt job");
                    let token = claim_started_attempt(&paths, &job, 1);
                    if attempt_already_stopped {
                        append_attempt_stopped(
                            &paths,
                            &token,
                            stop_reason,
                            json!({ "reason": "crashed before terminal event" }),
                        )
                        .expect("persisted attempt stop");
                    }
                    fs::remove_file(paths.job_lease(job.job_id.as_ref()))
                        .expect("simulate lost supervisor checkpoint");

                    supervise_one_job(
                        &paths,
                        &instance(temp.path().join("must-not-launch")),
                        job.job_id.as_ref(),
                    )
                    .await
                    .expect("restart classification");

                    let view = JobView::load(&paths, job.job_id.as_ref()).expect("job view");
                    assert_eq!(
                        view.projection.outcome,
                        Some(deadreckon_protocol::JobOutcome::BudgetExhausted)
                    );
                    assert_eq!(view.projection.stop_reason, Some(stop_reason));
                    let history =
                        read_job_history(&paths.job_events(job.job_id.as_ref())).expect("history");
                    assert_eq!(
                        history
                            .events()
                            .iter()
                            .filter(|event| event.kind == JobEventKind::AttemptStopped)
                            .count(),
                        1
                    );
                    assert_eq!(
                        history
                            .events()
                            .iter()
                            .filter(|event| event.kind == JobEventKind::BudgetExhausted)
                            .count(),
                        1
                    );
                    assert!(history.events().iter().all(|event| {
                        !matches!(
                            event.kind,
                            JobEventKind::Blocked
                                | JobEventKind::Failed
                                | JobEventKind::RetryScheduled
                        )
                    }));
                }
            }
        }
    }

    #[test]
    fn cancellation_wins_when_it_arrives_during_budget_finalization() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Failed);
        let token = claim_started_attempt(&paths, &job, 1);
        append_control_event(
            &paths,
            &token,
            JobEventKind::CancelRequested,
            "cancel-during-budget-finalization".to_string(),
            json!({ "stop_reason": StopReason::CancelRequested }),
        )
        .expect("cancel request");

        finish_advanced_budget_attempt(
            &paths,
            &token,
            ChildExit {
                status: None,
                adopted: true,
            },
            StopReason::SpendCap,
            "plan",
            "budget finalization lost the cancellation race",
        )
        .expect("cancelled finalization");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::Cancelled)
        );
        assert_eq!(
            view.projection.stop_reason,
            Some(StopReason::CancelRequested)
        );
        let history = read_job_history(&paths.job_events(job.job_id.as_ref())).expect("history");
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| event.kind == JobEventKind::Cancelled)
                .count(),
            1
        );
        let attempt_stops = history
            .events()
            .iter()
            .filter(|event| event.kind == JobEventKind::AttemptStopped)
            .collect::<Vec<_>>();
        assert_eq!(attempt_stops.len(), 1);
        assert_eq!(
            attempt_stops[0]
                .detail
                .get("stop_reason")
                .and_then(Value::as_str),
            Some("cancel_requested")
        );
        assert!(
            history
                .events()
                .iter()
                .all(|event| event.kind != JobEventKind::BudgetExhausted)
        );
    }

    #[test]
    fn cancellation_wins_after_budget_attempt_stop_but_before_terminal() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Failed);
        let token = claim_started_attempt(&paths, &job, 1);
        append_attempt_stopped(
            &paths,
            &token,
            StopReason::WallCap,
            json!({ "reason": "budget attempt stop was durable" }),
        )
        .expect("budget attempt stop");
        append_control_event(
            &paths,
            &token,
            JobEventKind::CancelRequested,
            "cancel-before-budget-terminal".to_string(),
            json!({ "stop_reason": StopReason::CancelRequested }),
        )
        .expect("cancel request");
        append_terminal_event(
            &paths,
            &token,
            JobEventKind::BudgetExhausted,
            StopReason::WallCap,
            json!({ "reason": "must be suppressed by cancellation" }),
        )
        .expect("cancelled terminal");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::Cancelled)
        );
        assert_eq!(
            view.projection.stop_reason,
            Some(StopReason::CancelRequested)
        );
        let history = read_job_history(&paths.job_events(job.job_id.as_ref())).expect("history");
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| event.kind == JobEventKind::AttemptStopped)
                .count(),
            1
        );
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| is_terminal_kind(event.kind))
                .count(),
            1
        );
        assert!(
            history
                .events()
                .iter()
                .all(|event| event.kind != JobEventKind::BudgetExhausted)
        );
    }

    fn is_terminal_kind(kind: JobEventKind) -> bool {
        matches!(
            kind,
            JobEventKind::Verified
                | JobEventKind::NeedsReview
                | JobEventKind::Blocked
                | JobEventKind::BudgetExhausted
                | JobEventKind::DeadlineReached
                | JobEventKind::Cancelled
                | JobEventKind::Failed
        )
    }

    #[tokio::test]
    async fn advanced_budget_exhaustion_keeps_spend_and_wall_reasons_distinct() {
        for shape in [JobShape::Graph, JobShape::LegacyCampaign] {
            for stop_reason in [StopReason::SpendCap, StopReason::WallCap] {
                let temp = TempDir::new().expect("tempdir");
                let (paths, job) = match shape {
                    JobShape::Graph => {
                        let (paths, job) =
                            graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Pending);
                        let mut plan =
                            deadreckon_core::plan::load_plan(&paths, job.job_id.as_ref())
                                .expect("plan");
                        plan.status = deadreckon_core::plan::PlanStatus::Failed;
                        deadreckon_core::plan::save_plan(&paths, &plan).expect("failed plan");
                        deadreckon_core::append_plan_event(
                            &paths,
                            &plan.plan_id,
                            deadreckon_core::PlanEventKind::TaskBudgetExhausted {
                                task_id: plan.tasks[0].task_id.clone(),
                                task_index: 0,
                                dimension: match stop_reason {
                                    StopReason::SpendCap => {
                                        deadreckon_core::plan::BudgetDimension::Spend
                                    }
                                    StopReason::WallCap => {
                                        deadreckon_core::plan::BudgetDimension::Wall
                                    }
                                    _ => unreachable!(),
                                },
                                reason: format!("{stop_reason:?} exhausted"),
                            },
                        )
                        .expect("typed Plan budget event");
                        (paths, job)
                    }
                    JobShape::LegacyCampaign => {
                        let (paths, job, mut campaign) = campaign_fixture(
                            &temp,
                            deadreckon_core::campaign::CampaignStatus::Pending,
                        );
                        campaign.status = deadreckon_core::campaign::CampaignStatus::Failed;
                        let campaign_dir = paths.plan_dir(job.job_id.as_ref());
                        deadreckon_core::campaign::write_campaign(&campaign_dir, &campaign)
                            .expect("failed campaign");
                        deadreckon_core::campaign::append_campaign_event(
                            &campaign_dir,
                            "budget_exhausted",
                            json!({
                                "reason": format!("{stop_reason:?} exhausted"),
                                "stop_reason": stop_reason,
                            }),
                        )
                        .expect("typed Campaign budget event");
                        (paths, job)
                    }
                    _ => unreachable!(),
                };
                let token = claim_started_attempt(&paths, &job, 1);
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
                .expect("budget classification");

                let view = JobView::load(&paths, job.job_id.as_ref()).expect("budget view");
                assert_eq!(
                    view.projection.outcome,
                    Some(deadreckon_protocol::JobOutcome::BudgetExhausted)
                );
                assert_eq!(view.projection.stop_reason, Some(stop_reason));
                let history =
                    read_job_history(&paths.job_events(job.job_id.as_ref())).expect("history");
                assert_eq!(
                    history
                        .events()
                        .iter()
                        .filter(|event| event.kind == JobEventKind::BudgetExhausted)
                        .count(),
                    1
                );
                assert!(
                    !history
                        .events()
                        .iter()
                        .any(|event| event.kind == JobEventKind::Failed)
                );
                assert_eq!(
                    history
                        .events()
                        .iter()
                        .filter(|event| is_terminal_kind(event.kind))
                        .count(),
                    1
                );
                assert!(
                    history
                        .events()
                        .iter()
                        .all(|event| event.job_id == job.job_id)
                );
                match shape {
                    JobShape::Graph => {
                        let plan = deadreckon_core::plan::load_plan(&paths, job.job_id.as_ref())
                            .expect("same Plan");
                        assert_eq!(plan.plan_id, job.job_id.as_ref());
                    }
                    JobShape::LegacyCampaign => {
                        let campaign = deadreckon_core::campaign::read_campaign(
                            &paths.plan_dir(job.job_id.as_ref()),
                        )
                        .expect("same Campaign");
                        assert_eq!(campaign.campaign_id, job.job_id.as_ref());
                    }
                    _ => unreachable!(),
                }
            }
        }
    }

    #[tokio::test]
    async fn running_advanced_cancellation_never_retries_or_reclassifies() {
        for shape in [JobShape::Graph, JobShape::LegacyCampaign] {
            let temp = TempDir::new().expect("tempdir");
            let (paths, mut job) = match shape {
                JobShape::Graph => graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Forked),
                JobShape::LegacyCampaign => {
                    let (paths, job, _) =
                        campaign_fixture(&temp, deadreckon_core::campaign::CampaignStatus::Forked);
                    (paths, job)
                }
                _ => unreachable!(),
            };
            job.policy.max_attempts = 2;
            fs::write(
                paths.job_json(job.job_id.as_ref()),
                serde_json::to_vec_pretty(&job).expect("job JSON"),
            )
            .expect("two-attempt job");
            let token = claim_started_attempt(&paths, &job, 1);
            append_control_event(
                &paths,
                &token,
                JobEventKind::CancelRequested,
                format!("cancel-running-{shape:?}"),
                json!({ "stop_reason": StopReason::CancelRequested }),
            )
            .expect("cancel request");

            let decision = reconcile_child_exit(
                &paths,
                &job,
                &token,
                ChildExit {
                    status: None,
                    adopted: true,
                },
                1,
                2,
            )
            .await
            .expect("cancel reconciliation");
            assert_eq!(decision, ChildReconciliation::Finished);

            let history = read_job_history(&paths.job_events(job.job_id.as_ref()))
                .expect("cancelled history");
            let cancel_index = history
                .events()
                .iter()
                .position(|event| event.kind == JobEventKind::CancelRequested)
                .expect("cancel event");
            assert!(history.events()[cancel_index + 1..].iter().all(|event| {
                event.kind != JobEventKind::RetryScheduled
                    && event.kind != JobEventKind::AttemptStarted
            }));
            assert_eq!(
                history
                    .events()
                    .iter()
                    .filter(|event| is_terminal_kind(event.kind))
                    .count(),
                1
            );
            assert!(
                history
                    .events()
                    .iter()
                    .all(|event| event.job_id == job.job_id)
            );

            let view = JobView::load(&paths, job.job_id.as_ref()).expect("cancelled view");
            assert_eq!(view.projection.attempt_count, 1);
            assert_eq!(
                view.projection.outcome,
                Some(deadreckon_protocol::JobOutcome::Cancelled)
            );
            assert_eq!(
                view.projection.stop_reason,
                Some(StopReason::CancelRequested)
            );
            let before = history.events().len();
            assert!(finish_cancel_requested(&paths, &token, None).expect("idempotent cancel"));
            assert_eq!(
                read_job_history(&paths.job_events(job.job_id.as_ref()))
                    .expect("replayed history")
                    .events()
                    .len(),
                before
            );
            assert_eq!(
                fs::read_dir(paths.jobs_dir())
                    .expect("jobs")
                    .filter_map(std::result::Result::ok)
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .count(),
                1
            );
        }
    }

    #[tokio::test]
    async fn adopted_advanced_children_use_the_same_bounded_recovery_path() {
        for shape in [JobShape::Graph, JobShape::LegacyCampaign] {
            let temp = TempDir::new().expect("tempdir");
            let (paths, mut job) = match shape {
                JobShape::Graph => graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Forked),
                JobShape::LegacyCampaign => {
                    let (paths, job, _) =
                        campaign_fixture(&temp, deadreckon_core::campaign::CampaignStatus::Forked);
                    (paths, job)
                }
                _ => unreachable!(),
            };
            job.policy.max_attempts = 2;
            fs::write(
                paths.job_json(job.job_id.as_ref()),
                serde_json::to_vec_pretty(&job).expect("job JSON"),
            )
            .expect("two-attempt job");
            let token = claim_started_attempt(&paths, &job, 1);

            let decision = reconcile_child_exit(
                &paths,
                &job,
                &token,
                ChildExit {
                    status: None,
                    adopted: true,
                },
                1,
                2,
            )
            .await
            .expect("adopted recovery");
            assert_eq!(decision, ChildReconciliation::Retry);

            let history =
                read_job_history(&paths.job_events(job.job_id.as_ref())).expect("retry history");
            assert_eq!(
                history
                    .events()
                    .iter()
                    .filter(|event| event.kind == JobEventKind::RetryScheduled)
                    .count(),
                1
            );
            assert!(
                !history
                    .events()
                    .iter()
                    .any(|event| is_terminal_kind(event.kind))
            );
            let view = JobView::load(&paths, job.job_id.as_ref()).expect("retry view");
            assert_eq!(view.projection.attempt_count, 1);
            assert_eq!(view.projection.outcome, None);

            append_control_event(
                &paths,
                &token,
                JobEventKind::AttemptStarted,
                format!("last-attempt-{shape:?}"),
                attempt_detail(&job, 2),
            )
            .expect("last attempt");
            let decision = reconcile_child_exit(
                &paths,
                &job,
                &token,
                ChildExit {
                    status: None,
                    adopted: true,
                },
                2,
                2,
            )
            .await
            .expect("last-attempt classification");
            assert_eq!(decision, ChildReconciliation::Finished);
            let history =
                read_job_history(&paths.job_events(job.job_id.as_ref())).expect("final history");
            assert_eq!(
                history
                    .events()
                    .iter()
                    .filter(|event| is_terminal_kind(event.kind))
                    .count(),
                1
            );
            assert!(
                history
                    .events()
                    .iter()
                    .all(|event| event.job_id == job.job_id)
            );
            assert_eq!(
                fs::read_dir(paths.jobs_dir())
                    .expect("jobs")
                    .filter_map(std::result::Result::ok)
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .count(),
                1
            );
        }
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
        assert_eq!(
            view.projection.stop_reason,
            Some(StopReason::CorruptHistory)
        );
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
        use deadreckon_core::plan::{Plan, PlanMode, PlanProviders, PlanRole, PlanTask, save_plan};

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
            let mut first = PlanTask::new(0, "first", "work", PlanRole::Child, None);
            first.child_run_id = Some(leaf.run_id.clone());
            let mut second = PlanTask::new(1, "second", "reuse", PlanRole::Child, None);
            second.child_run_id = Some(leaf.run_id.clone());
            let mut subplan = Plan::new(
                sub.goal.clone(),
                PlanMode::FullPlan,
                vec![first, second],
                PlanProviders::default(),
                None,
                "test",
            )
            .expect("subplan");
            subplan.plan_id = format!("campaign-subplan-{index}");
            save_plan(&paths, &subplan).expect("subplan state");
            super::super::graph_job::record_plan_planner_accounting(&paths, &subplan.plan_id, None)
                .expect("subplan planner accounting");
            sub.sub_plan_id = Some(subplan.plan_id);
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
            goal_coverage: vec![deadreckon_protocol::GoalCoverage {
                claim: "approved campaign goal".to_string(),
                status: deadreckon_protocol::GoalCoverageStatus::Met,
                evidence: vec![
                    "approved-goal".to_string(),
                    "deterministic-gate".to_string(),
                ],
            }],
            missing: Vec::new(),
            input_sha256: parent_semantic_input_sha256(&parent, &job),
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
        assert_eq!(
            view.projection.stop_reason,
            Some(StopReason::CorruptHistory)
        );
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
        let mut plan = deadreckon_core::load_plan(&paths, job.job_id.as_ref()).expect("graph plan");
        for task in &mut plan.tasks {
            task.child_run_id = Some(merged.run_id.clone());
        }
        deadreckon_core::plan::save_plan(&paths, &plan).expect("graph accounting links");

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
            goal_coverage: vec![deadreckon_protocol::GoalCoverage {
                claim: "approved graph goal".to_string(),
                status: deadreckon_protocol::GoalCoverageStatus::Met,
                evidence: vec![
                    "approved-goal".to_string(),
                    "deterministic-gate".to_string(),
                ],
            }],
            missing: Vec::new(),
            input_sha256: parent_semantic_input_sha256(&parent, &job),
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
    async fn graph_parent_semantic_revise_uses_a_fenced_attempt_then_verifies_the_same_job() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut job) = graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Merged);
        update_job_attempt_limit(&paths, &mut job, 3);
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
        fs::write(merged.working_dir.join("result.txt"), "initial parent\n")
            .expect("merged result");
        merged
            .set_phase_status(
                deadreckon_core::PhaseId(60),
                deadreckon_core::PhaseStatus::Completed,
            )
            .expect("merged complete");
        save_state(&merged).expect("merged state");
        let mut plan = deadreckon_core::load_plan(&paths, job.job_id.as_ref()).expect("graph plan");
        for task in &mut plan.tasks {
            task.child_run_id = Some(merged.run_id.clone());
        }
        deadreckon_core::plan::save_plan(&paths, &plan).expect("graph accounting links");

        let authority: JobAuthority = serde_json::from_slice(
            &fs::read(paths.job_authority(job.job_id.as_ref())).expect("authority"),
        )
        .expect("authority json");
        let mut parent =
            super::super::graph_job::prepare_parent_result_run(&paths, &job, &authority, &merged)
                .expect("parent");
        let key = deadreckon_core::read_gate_key(&paths, job.job_id.as_ref()).expect("gate key");
        deadreckon_core::write_native_acceptance_marker_with_results_and_key(
            &parent.run_root,
            parent.run_id.clone(),
            parent.working_dir.clone(),
            vec![deadreckon_core::AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: true,
                must_pass: true,
                detail: "initial parent exists".to_string(),
                command: None,
                cwd: None,
                duration_ms: Some(1),
                stdout: None,
                stderr: None,
            }],
            &key,
            deadreckon_core::AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("initial marker");

        let owner = instance(PathBuf::from("/opt/deadreckon")).owner;
        let claim =
            claim_job_lease(&paths, &job.job_id, &owner, Utc::now(), LEASE_TTL).expect("claim");
        let token = claim.token();
        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "graph-revise-attempt-1".to_string(),
            attempt_detail(&job, 1),
        )
        .expect("attempt one");
        let first_launch = Uuid::new_v4().to_string();
        append_test_child_link(&paths, &token, 1, &first_launch);

        let marker = deadreckon_core::validate_acceptance_marker(&parent).expect("initial marker");
        let revise = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job.job_id.clone(),
            run_id: RunId(job.job_id.as_ref().to_string()),
            judged_at: Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Revise,
            summary: "the parent must explain the repaired result".to_string(),
            goal_coverage: vec![deadreckon_protocol::GoalCoverage {
                claim: "approved graph goal".to_string(),
                status: deadreckon_protocol::GoalCoverageStatus::Missing,
                evidence: vec!["initial-result".to_string()],
            }],
            missing: vec!["repair explanation".to_string()],
            input_sha256: parent_semantic_input_sha256(&parent, &job),
            spend_usd: 0.0,
        };
        deadreckon_runtime::persist_semantic_judgment(&parent.run_root, &revise)
            .expect("revise judgment");
        let requested = super::super::graph_job::request_parent_repair_for_test(
            &paths,
            &job,
            &mut parent,
            &merged,
            &marker,
            &revise,
            &plan.providers,
        )
        .expect("repair request");
        let super::super::graph_job::ParentCompletion::ReviseRequested {
            reason,
            round,
            intent_path,
            intent_sha256,
            judgment_path,
            judgment_sha256,
        } = requested
        else {
            panic!("revise must create a durable repair request")
        };
        schedule_parent_semantic_repair(
            &paths,
            &job,
            &token,
            &ChildExit {
                status: None,
                adopted: true,
            },
            "plan",
            plan.merged_run_id.as_deref(),
            &reason,
            round,
            &intent_path,
            &intent_sha256,
            &judgment_path,
            &judgment_sha256,
        )
        .expect("schedule repair");
        assert!(latest_job_event_is_retry_scheduled(&paths, job.job_id.as_ref()).expect("retry"));

        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "graph-revise-attempt-2".to_string(),
            attempt_detail(&job, 2),
        )
        .expect("attempt two");
        let second_launch = Uuid::new_v4().to_string();
        append_test_child_link(&paths, &token, 2, &second_launch);
        let baseline = parent_result_tree_sha256(&parent);
        fs::write(
            parent.working_dir.join("result.txt"),
            "repaired parent with explanation\n",
        )
        .expect("repair mutation");
        parent.turn = parent.turn.saturating_add(1);
        parent.status = RunStatus::Executing;
        save_state(&parent).expect("repaired parent state");
        super::super::graph_job::install_parent_repair_candidate_for_test(
            &paths,
            &job,
            &parent,
            2,
            &second_launch,
            token.epoch,
            baseline,
        )
        .expect("fenced repair candidate");
        deadreckon_core::write_native_acceptance_marker_with_results_and_key(
            &parent.run_root,
            parent.run_id.clone(),
            parent.working_dir.clone(),
            vec![deadreckon_core::AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: true,
                must_pass: true,
                detail: "repaired parent exists".to_string(),
                command: None,
                cwd: None,
                duration_ms: Some(1),
                stdout: None,
                stderr: None,
            }],
            &key,
            deadreckon_core::AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("post-repair marker");
        let achieved = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job.job_id.clone(),
            run_id: RunId(job.job_id.as_ref().to_string()),
            judged_at: Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Achieved,
            summary: "the repaired parent now satisfies the approved goal".to_string(),
            goal_coverage: vec![deadreckon_protocol::GoalCoverage {
                claim: "approved graph goal".to_string(),
                status: deadreckon_protocol::GoalCoverageStatus::Met,
                evidence: vec!["repaired-result".to_string()],
            }],
            missing: Vec::new(),
            input_sha256: parent_semantic_input_sha256(&parent, &job),
            spend_usd: 0.0,
        };
        deadreckon_runtime::persist_semantic_judgment(&parent.run_root, &achieved)
            .expect("achieved judgment");

        let mut expired =
            deadreckon_core::load_job_lease(&paths, &job.job_id).expect("active lease");
        expired.expires_at = Utc::now() - chrono::Duration::seconds(1);
        fs::write(
            paths.job_lease(job.job_id.as_ref()),
            serde_json::to_vec_pretty(&expired).expect("expired lease json"),
        )
        .expect("expire abandoned supervisor lease");
        let recovery_owner = LeaseOwner {
            owner_id: "replacement-supervisor".to_string(),
            boot_id: token.boot_id.clone(),
            pid: std::process::id(),
            process_group: std::process::id(),
        };
        let recovered =
            claim_job_lease(&paths, &job.job_id, &recovery_owner, Utc::now(), LEASE_TTL)
                .expect("replacement supervisor reclaims candidate-ready repair");
        assert!(matches!(
            recovered.disposition,
            deadreckon_core::LeaseClaimDisposition::Reclaimed(
                deadreckon_core::LeaseReclaimReason::Expired
            )
        ));
        let recovery_token = recovered.token();
        assert!(recovery_token.epoch > token.epoch);
        assert!(
            advanced_artifact_waits_for_terminal_classification(&paths, &job),
            "replacement supervisor did not recognize durable repair evidence"
        );
        classify_advanced_attempt(
            &paths,
            &job,
            &recovery_token,
            ChildExit {
                status: None,
                adopted: true,
            },
        )
        .await
        .expect("repair classification");
        let view = JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::Verified)
        );
        assert_eq!(view.projection.attempt_count, 2);
        let history = read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("recovered repair history");
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| event.kind == JobEventKind::AttemptStarted)
                .count(),
            2,
            "candidate recovery launched a duplicate attempt"
        );
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| event.kind == JobEventKind::RetryScheduled)
                .count(),
            1,
            "candidate recovery scheduled an extra retry"
        );
        let finish = super::super::lifecycle::finish_job_state(&paths, &view)
            .expect("finish repaired parent");
        assert_eq!(finish.run_id, job.job_id.as_ref());
        assert_eq!(
            fs::read_to_string(finish.working_dir.join("result.txt")).expect("repaired output"),
            "repaired parent with explanation\n"
        );
        deadreckon_core::validate_completion_receipt(&paths, &finish)
            .expect("repair receipt remains valid");
    }

    #[test]
    fn parent_semantic_repair_stops_at_attempt_limit_without_scheduling_another_worker() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Forked);
        executing_attempt(&paths, &job);
        let token = claim_started_attempt(&paths, &job, 1);
        let intent_path = paths
            .job_dir(job.job_id.as_ref())
            .join("parent-repair.json");
        let judgment_path = paths
            .job_dir(job.job_id.as_ref())
            .join("archived-revise-judgment.json");

        schedule_parent_semantic_repair(
            &paths,
            &job,
            &token,
            &ChildExit {
                status: None,
                adopted: true,
            },
            "plan",
            Some("result-run"),
            "the parent still misses an approved requirement",
            1,
            &intent_path,
            "sha256:intent",
            &judgment_path,
            "sha256:judgment",
        )
        .expect("classify final permitted attempt");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("attempt-limited view");
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::RetryExhausted)
        );
        assert_eq!(view.projection.stop_reason, Some(StopReason::AttemptLimit));
        let history = read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("attempt-limited history");
        assert!(
            history
                .events()
                .iter()
                .all(|event| event.kind != JobEventKind::RetryScheduled),
            "final permitted repair attempt scheduled another worker"
        );
    }

    #[test]
    fn cancellation_wins_over_parent_semantic_repair_retry() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut job) = graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Forked);
        update_job_attempt_limit(&paths, &mut job, 2);
        executing_attempt(&paths, &job);
        let token = claim_started_attempt(&paths, &job, 1);
        append_control_event(
            &paths,
            &token,
            JobEventKind::CancelRequested,
            "cancel-parent-repair".to_string(),
            json!({ "stop_reason": StopReason::CancelRequested }),
        )
        .expect("cancel request");

        schedule_parent_semantic_repair(
            &paths,
            &job,
            &token,
            &ChildExit {
                status: None,
                adopted: true,
            },
            "plan",
            Some("result-run"),
            "semantic repair was ready to retry",
            1,
            &paths
                .job_dir(job.job_id.as_ref())
                .join("parent-repair.json"),
            "sha256:intent",
            &paths
                .job_dir(job.job_id.as_ref())
                .join("archived-revise-judgment.json"),
            "sha256:judgment",
        )
        .expect("cancelled repair classification");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("cancelled repair view");
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::Cancelled)
        );
        assert_eq!(
            view.projection.stop_reason,
            Some(StopReason::CancelRequested)
        );
        let history = read_job_history(&paths.job_events(job.job_id.as_ref()))
            .expect("cancelled repair history");
        assert!(
            history
                .events()
                .iter()
                .all(|event| event.kind != JobEventKind::RetryScheduled),
            "cancelled parent repair scheduled another worker"
        );
    }

    #[tokio::test]
    async fn graph_parent_semantic_revise_twice_archives_each_round_then_verifies_the_same_job() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, mut job) = graph_fixture(&temp, deadreckon_core::plan::PlanStatus::Merged);
        update_job_attempt_limit(&paths, &mut job, 3);
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
        fs::write(merged.working_dir.join("result.txt"), "initial parent\n")
            .expect("merged result");
        merged
            .set_phase_status(
                deadreckon_core::PhaseId(60),
                deadreckon_core::PhaseStatus::Completed,
            )
            .expect("merged complete");
        save_state(&merged).expect("merged state");
        let mut plan = deadreckon_core::load_plan(&paths, job.job_id.as_ref()).expect("graph plan");
        for task in &mut plan.tasks {
            task.child_run_id = Some(merged.run_id.clone());
        }
        deadreckon_core::plan::save_plan(&paths, &plan).expect("graph accounting links");

        let authority: JobAuthority = serde_json::from_slice(
            &fs::read(paths.job_authority(job.job_id.as_ref())).expect("authority"),
        )
        .expect("authority json");
        let mut parent =
            super::super::graph_job::prepare_parent_result_run(&paths, &job, &authority, &merged)
                .expect("parent");
        let key = deadreckon_core::read_gate_key(&paths, job.job_id.as_ref()).expect("gate key");
        deadreckon_core::write_native_acceptance_marker_with_results_and_key(
            &parent.run_root,
            parent.run_id.clone(),
            parent.working_dir.clone(),
            vec![deadreckon_core::AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: true,
                must_pass: true,
                detail: "initial parent exists".to_string(),
                command: None,
                cwd: None,
                duration_ms: Some(1),
                stdout: None,
                stderr: None,
            }],
            &key,
            deadreckon_core::AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("initial marker");

        let owner = instance(PathBuf::from("/opt/deadreckon")).owner;
        let claim =
            claim_job_lease(&paths, &job.job_id, &owner, Utc::now(), LEASE_TTL).expect("claim");
        let token = claim.token();
        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "graph-revise-twice-attempt-1".to_string(),
            attempt_detail(&job, 1),
        )
        .expect("attempt one");
        let first_launch = Uuid::new_v4().to_string();
        append_test_child_link(&paths, &token, 1, &first_launch);

        let initial_marker =
            deadreckon_core::validate_acceptance_marker(&parent).expect("initial marker");
        let first_revise = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job.job_id.clone(),
            run_id: RunId(job.job_id.as_ref().to_string()),
            judged_at: Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Revise,
            summary: "the parent needs its first repair".to_string(),
            goal_coverage: vec![deadreckon_protocol::GoalCoverage {
                claim: "approved graph goal".to_string(),
                status: deadreckon_protocol::GoalCoverageStatus::Missing,
                evidence: vec!["initial-result".to_string()],
            }],
            missing: vec!["first repair".to_string()],
            input_sha256: parent_semantic_input_sha256(&parent, &job),
            spend_usd: 0.0,
        };
        deadreckon_runtime::persist_semantic_judgment(&parent.run_root, &first_revise)
            .expect("first revise judgment");
        let first_requested = super::super::graph_job::request_parent_repair_for_test(
            &paths,
            &job,
            &mut parent,
            &merged,
            &initial_marker,
            &first_revise,
            &plan.providers,
        )
        .expect("first repair request");
        let super::super::graph_job::ParentCompletion::ReviseRequested {
            reason: first_reason,
            round: first_round,
            intent_path: first_intent_path,
            intent_sha256: first_intent_sha256,
            judgment_path: first_judgment_path,
            judgment_sha256: first_judgment_sha256,
        } = first_requested
        else {
            panic!("first revise must create a durable repair request")
        };
        assert_eq!(first_round, 1);
        let first_intent_bytes = fs::read(&first_intent_path).expect("first active intent");
        schedule_parent_semantic_repair(
            &paths,
            &job,
            &token,
            &ChildExit {
                status: None,
                adopted: true,
            },
            "plan",
            plan.merged_run_id.as_deref(),
            &first_reason,
            first_round,
            &first_intent_path,
            &first_intent_sha256,
            &first_judgment_path,
            &first_judgment_sha256,
        )
        .expect("schedule first repair");

        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "graph-revise-twice-attempt-2".to_string(),
            attempt_detail(&job, 2),
        )
        .expect("attempt two");
        let second_launch = Uuid::new_v4().to_string();
        append_test_child_link(&paths, &token, 2, &second_launch);
        let first_baseline = parent_result_tree_sha256(&parent);
        fs::write(
            parent.working_dir.join("result.txt"),
            "first repair still needs revision\n",
        )
        .expect("first repair mutation");
        parent.turn = parent.turn.saturating_add(1);
        parent.status = RunStatus::Executing;
        save_state(&parent).expect("first repaired parent state");
        super::super::graph_job::install_parent_repair_candidate_for_test(
            &paths,
            &job,
            &parent,
            2,
            &second_launch,
            token.epoch,
            first_baseline,
        )
        .expect("first fenced repair candidate");
        deadreckon_core::write_native_acceptance_marker_with_results_and_key(
            &parent.run_root,
            parent.run_id.clone(),
            parent.working_dir.clone(),
            vec![deadreckon_core::AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: true,
                must_pass: true,
                detail: "first repaired parent exists".to_string(),
                command: None,
                cwd: None,
                duration_ms: Some(1),
                stdout: None,
                stderr: None,
            }],
            &key,
            deadreckon_core::AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("first post-repair marker");
        let first_repair_marker =
            deadreckon_core::validate_acceptance_marker(&parent).expect("first repair marker");
        let second_revise = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job.job_id.clone(),
            run_id: RunId(job.job_id.as_ref().to_string()),
            judged_at: Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Revise,
            summary: "the parent needs one final repair".to_string(),
            goal_coverage: vec![deadreckon_protocol::GoalCoverage {
                claim: "approved graph goal".to_string(),
                status: deadreckon_protocol::GoalCoverageStatus::Missing,
                evidence: vec!["first-repair-result".to_string()],
            }],
            missing: vec!["final explanation".to_string()],
            input_sha256: parent_semantic_input_sha256(&parent, &job),
            spend_usd: 0.0,
        };
        deadreckon_runtime::persist_semantic_judgment(&parent.run_root, &second_revise)
            .expect("second revise judgment");
        let second_requested = super::super::graph_job::request_parent_repair_for_test(
            &paths,
            &job,
            &mut parent,
            &merged,
            &first_repair_marker,
            &second_revise,
            &plan.providers,
        )
        .expect("second repair request");
        let super::super::graph_job::ParentCompletion::ReviseRequested {
            reason: second_reason,
            round: second_round,
            intent_path: second_intent_path,
            intent_sha256: second_intent_sha256,
            judgment_path: second_judgment_path,
            judgment_sha256: second_judgment_sha256,
        } = second_requested
        else {
            panic!("second revise must create a second durable repair request")
        };
        assert_eq!(second_round, 2);

        let repairs_root = parent.run_root.join("proofs").join("parent-repairs");
        let first_archive = repairs_root.join("round-1");
        let second_archive = repairs_root.join("round-2");
        assert_eq!(
            fs::read(first_archive.join("intent.json")).expect("archived first intent"),
            first_intent_bytes
        );
        let mut first_round_binding = String::new();
        for name in [
            "intent.json",
            "final-attempt.json",
            "candidate.json",
            "pre-repair-marker.json",
            "revise-judgment.json",
        ] {
            let digest =
                deadreckon_core::flight::sha256_file(&first_archive.join(name)).expect(name);
            first_round_binding.push_str(name);
            first_round_binding.push('=');
            first_round_binding.push_str(&digest);
            first_round_binding.push('\n');
        }
        let first_round_binding = deadreckon_core::flight::sha256_text(&first_round_binding);
        let second_intent: serde_json::Value =
            serde_json::from_slice(&fs::read(&second_intent_path).expect("second active intent"))
                .expect("second intent json");
        assert_eq!(second_intent["round"], 2);
        assert_eq!(
            second_intent["previous_round_sha256"].as_str(),
            Some(first_round_binding.as_str())
        );
        for path in [
            second_archive.join("pre-repair-marker.json"),
            second_archive.join("revise-judgment.json"),
        ] {
            assert!(
                path.is_file(),
                "missing second round archive {}",
                path.display()
            );
        }
        let archived_round_bytes = [
            first_archive.join("intent.json"),
            first_archive.join("final-attempt.json"),
            first_archive.join("candidate.json"),
            first_archive.join("pre-repair-marker.json"),
            first_archive.join("revise-judgment.json"),
            second_archive.join("pre-repair-marker.json"),
            second_archive.join("revise-judgment.json"),
        ]
        .map(|path| {
            let bytes = fs::read(&path).expect("immutable round evidence");
            (path, bytes)
        });

        schedule_parent_semantic_repair(
            &paths,
            &job,
            &token,
            &ChildExit {
                status: None,
                adopted: true,
            },
            "plan",
            plan.merged_run_id.as_deref(),
            &second_reason,
            second_round,
            &second_intent_path,
            &second_intent_sha256,
            &second_judgment_path,
            &second_judgment_sha256,
        )
        .expect("schedule second repair");

        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "graph-revise-twice-attempt-3".to_string(),
            attempt_detail(&job, 3),
        )
        .expect("attempt three");
        let third_launch = Uuid::new_v4().to_string();
        append_test_child_link(&paths, &token, 3, &third_launch);
        let second_baseline = parent_result_tree_sha256(&parent);
        fs::write(
            parent.working_dir.join("result.txt"),
            "final repaired parent with complete explanation\n",
        )
        .expect("final repair mutation");
        parent.turn = parent.turn.saturating_add(1);
        parent.status = RunStatus::Executing;
        save_state(&parent).expect("final repaired parent state");
        super::super::graph_job::install_parent_repair_candidate_for_test(
            &paths,
            &job,
            &parent,
            3,
            &third_launch,
            token.epoch,
            second_baseline,
        )
        .expect("second fenced repair candidate");
        deadreckon_core::write_native_acceptance_marker_with_results_and_key(
            &parent.run_root,
            parent.run_id.clone(),
            parent.working_dir.clone(),
            vec![deadreckon_core::AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: true,
                must_pass: true,
                detail: "final repaired parent exists".to_string(),
                command: None,
                cwd: None,
                duration_ms: Some(1),
                stdout: None,
                stderr: None,
            }],
            &key,
            deadreckon_core::AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("final post-repair marker");
        let achieved = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job.job_id.clone(),
            run_id: RunId(job.job_id.as_ref().to_string()),
            judged_at: Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Achieved,
            summary: "the twice-repaired parent satisfies the approved goal".to_string(),
            goal_coverage: vec![deadreckon_protocol::GoalCoverage {
                claim: "approved graph goal".to_string(),
                status: deadreckon_protocol::GoalCoverageStatus::Met,
                evidence: vec!["final-repaired-result".to_string()],
            }],
            missing: Vec::new(),
            input_sha256: parent_semantic_input_sha256(&parent, &job),
            spend_usd: 0.0,
        };
        deadreckon_runtime::persist_semantic_judgment(&parent.run_root, &achieved)
            .expect("achieved judgment");

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
        .expect("final repair classification");
        let view = JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::Verified)
        );
        assert_eq!(view.projection.attempt_count, 3);
        for (path, bytes) in archived_round_bytes {
            assert_eq!(
                fs::read(&path).expect("archived round after completion"),
                bytes,
                "immutable repair archive changed at {}",
                path.display()
            );
        }
        let finish = super::super::lifecycle::finish_job_state(&paths, &view)
            .expect("finish twice-repaired parent");
        assert_eq!(finish.run_id, job.job_id.as_ref());
        assert_eq!(
            fs::read_to_string(finish.working_dir.join("result.txt")).expect("repaired output"),
            "final repaired parent with complete explanation\n"
        );
        deadreckon_core::validate_completion_receipt(&paths, &finish)
            .expect("twice-repaired receipt remains valid");

        let archived_judgment = first_archive.join("revise-judgment.json");
        let original_judgment =
            fs::read(&archived_judgment).expect("sealed archived judgment bytes");
        let mut tampered_judgment = original_judgment.clone();
        tampered_judgment.push(b'\n');
        fs::write(&archived_judgment, tampered_judgment)
            .expect("tamper archived judgment after sealing");
        let error = deadreckon_core::validate_completion_receipt(&paths, &finish)
            .expect_err("post-seal repair archive tampering must invalidate finish");
        assert!(
            error
                .to_string()
                .contains("parent repair archive no longer matches the signed round chain"),
            "{error}"
        );
        assert!(
            super::super::lifecycle::finish_job_state(&paths, &view).is_err(),
            "finish accepted a receipt whose repair archive changed after sealing"
        );
        fs::write(&archived_judgment, original_judgment)
            .expect("restore archived judgment after tamper test");
        deadreckon_core::validate_completion_receipt(&paths, &finish)
            .expect("restored repair archive revalidates the receipt");
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            for (index, archived_path) in [
                first_archive.join("intent.json"),
                first_archive.join("candidate.json"),
                first_archive.join("revise-judgment.json"),
            ]
            .into_iter()
            .enumerate()
            {
                let external = temp
                    .path()
                    .join(format!("archived-repair-proof-{index}.json"));
                fs::rename(&archived_path, &external).expect("move archived regular proof");
                symlink(&external, &archived_path)
                    .expect("substitute byte-identical archived proof symlink");
                let error = deadreckon_core::validate_completion_receipt(&paths, &finish)
                    .expect_err("archived repair symlink must invalidate finish");
                assert!(
                    error.to_string().contains("regular non-symlink"),
                    "{}: {error}",
                    archived_path.display()
                );
                assert!(
                    super::super::lifecycle::finish_job_state(&paths, &view).is_err(),
                    "finish accepted a symlinked archived repair proof"
                );
                fs::remove_file(&archived_path).expect("remove archived proof symlink");
                fs::rename(&external, &archived_path).expect("restore archived regular proof");
                deadreckon_core::validate_completion_receipt(&paths, &finish)
                    .expect("restored archived regular proof revalidates");
            }
        }
    }

    #[tokio::test]
    async fn campaign_parent_semantic_revise_repairs_only_the_parent_then_verifies_the_same_job() {
        use deadreckon_core::plan::{Plan, PlanMode, PlanProviders, PlanRole, PlanTask, save_plan};

        let temp = TempDir::new().expect("tempdir");
        let (paths, mut job, mut campaign) =
            campaign_fixture(&temp, deadreckon_core::campaign::CampaignStatus::Merged);
        update_job_attempt_limit(&paths, &mut job, 3);

        let mut leaf_snapshots = Vec::new();
        for (index, sub) in campaign.sub_goals.iter_mut().enumerate() {
            let run_id = format!("campaign-revise-leaf-{index}");
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
            fs::write(
                leaf.working_dir.join(format!("leaf-{index}.txt")),
                format!("successful child {index}\n"),
            )
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

            let mut first = PlanTask::new(0, "first", "work", PlanRole::Child, None);
            first.child_run_id = Some(leaf.run_id.clone());
            let mut second = PlanTask::new(1, "second", "reuse", PlanRole::Child, None);
            second.child_run_id = Some(leaf.run_id.clone());
            let mut subplan = Plan::new(
                sub.goal.clone(),
                PlanMode::FullPlan,
                vec![first, second],
                PlanProviders::default(),
                None,
                "test",
            )
            .expect("subplan");
            subplan.plan_id = format!("campaign-revise-subplan-{index}");
            save_plan(&paths, &subplan).expect("subplan state");
            super::super::graph_job::record_plan_planner_accounting(&paths, &subplan.plan_id, None)
                .expect("subplan planner accounting");
            sub.sub_plan_id = Some(subplan.plan_id);
            sub.result_run_id = Some(run_id.clone());
            leaf_snapshots.push((
                run_id,
                fs::read(leaf.state_path()).expect("leaf state bytes"),
                parent_result_tree_sha256(&leaf),
            ));
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
                goal: job.goal.clone(),
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
        fs::write(
            merged.working_dir.join("campaign.txt"),
            "initial campaign parent\n",
        )
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
        let mut parent =
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
                detail: "initial campaign parent exists".to_string(),
                command: None,
                cwd: None,
                duration_ms: Some(1),
                stdout: None,
                stderr: None,
            }],
            &key,
            deadreckon_core::AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("initial marker");

        let owner = instance(PathBuf::from("/opt/deadreckon")).owner;
        let claim =
            claim_job_lease(&paths, &job.job_id, &owner, Utc::now(), LEASE_TTL).expect("claim");
        let token = claim.token();
        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "campaign-revise-attempt-1".to_string(),
            attempt_detail(&job, 1),
        )
        .expect("attempt one");
        let first_launch = Uuid::new_v4().to_string();
        append_test_child_link(&paths, &token, 1, &first_launch);

        let marker = deadreckon_core::validate_acceptance_marker(&parent).expect("initial marker");
        let revise = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job.job_id.clone(),
            run_id: RunId(job.job_id.as_ref().to_string()),
            judged_at: Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Revise,
            summary: "the campaign parent must explain the aggregate result".to_string(),
            goal_coverage: vec![deadreckon_protocol::GoalCoverage {
                claim: "approved campaign goal".to_string(),
                status: deadreckon_protocol::GoalCoverageStatus::Missing,
                evidence: vec!["initial-campaign-parent".to_string()],
            }],
            missing: vec!["aggregate repair explanation".to_string()],
            input_sha256: parent_semantic_input_sha256(&parent, &job),
            spend_usd: 0.0,
        };
        deadreckon_runtime::persist_semantic_judgment(&parent.run_root, &revise)
            .expect("revise judgment");
        let requested = super::super::graph_job::request_parent_repair_for_test(
            &paths,
            &job,
            &mut parent,
            &merged,
            &marker,
            &revise,
            &campaign.providers,
        )
        .expect("repair request");
        let super::super::graph_job::ParentCompletion::ReviseRequested {
            reason,
            round,
            intent_path,
            intent_sha256,
            judgment_path,
            judgment_sha256,
        } = requested
        else {
            panic!("revise must create a durable campaign repair request")
        };
        schedule_parent_semantic_repair(
            &paths,
            &job,
            &token,
            &ChildExit {
                status: None,
                adopted: true,
            },
            "campaign",
            campaign.merged_run_id.as_deref(),
            &reason,
            round,
            &intent_path,
            &intent_sha256,
            &judgment_path,
            &judgment_sha256,
        )
        .expect("schedule campaign repair");
        assert!(latest_job_event_is_retry_scheduled(&paths, job.job_id.as_ref()).expect("retry"));

        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "campaign-revise-attempt-2".to_string(),
            attempt_detail(&job, 2),
        )
        .expect("attempt two");
        let second_launch = Uuid::new_v4().to_string();
        append_test_child_link(&paths, &token, 2, &second_launch);
        let baseline = parent_result_tree_sha256(&parent);
        fs::write(
            parent.working_dir.join("campaign.txt"),
            "repaired campaign parent with aggregate explanation\n",
        )
        .expect("repair mutation");
        parent.turn = parent.turn.saturating_add(1);
        parent.status = RunStatus::Executing;
        save_state(&parent).expect("repaired parent state");
        super::super::graph_job::install_parent_repair_candidate_for_test(
            &paths,
            &job,
            &parent,
            2,
            &second_launch,
            token.epoch,
            baseline,
        )
        .expect("fenced repair candidate");

        let manifest: Value = serde_json::from_slice(
            &fs::read(deadreckon_core::parent_repair_manifest_path_for_run_root(
                &parent.run_root,
            ))
            .expect("repair manifest"),
        )
        .expect("manifest json");
        let candidate: Value = serde_json::from_slice(
            &fs::read(deadreckon_core::parent_repair_candidate_path_for_run_root(
                &parent.run_root,
            ))
            .expect("repair candidate"),
        )
        .expect("candidate json");
        for proof in [&manifest, &candidate] {
            assert_eq!(proof.get("attempt").and_then(Value::as_u64), Some(2));
            assert_eq!(
                proof.get("launch_id").and_then(Value::as_str),
                Some(second_launch.as_str())
            );
            assert_eq!(
                proof.get("lease_epoch").and_then(Value::as_u64),
                Some(token.epoch)
            );
        }

        deadreckon_core::write_native_acceptance_marker_with_results_and_key(
            &parent.run_root,
            parent.run_id.clone(),
            parent.working_dir.clone(),
            vec![deadreckon_core::AcceptanceCheckResult {
                kind: "file_exists".to_string(),
                passed: true,
                must_pass: true,
                detail: "repaired campaign parent exists".to_string(),
                command: None,
                cwd: None,
                duration_ms: Some(1),
                stdout: None,
                stderr: None,
            }],
            &key,
            deadreckon_core::AcceptanceContainment::contained("sandbox-exec"),
        )
        .expect("post-repair marker");
        let achieved = SemanticJudgment {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job.job_id.clone(),
            run_id: RunId(job.job_id.as_ref().to_string()),
            judged_at: Utc::now(),
            provider: "independent-test-judge".to_string(),
            model: "test-model".to_string(),
            decision: SemanticDecision::Achieved,
            summary: "the repaired campaign parent satisfies the approved goal".to_string(),
            goal_coverage: vec![deadreckon_protocol::GoalCoverage {
                claim: "approved campaign goal".to_string(),
                status: deadreckon_protocol::GoalCoverageStatus::Met,
                evidence: vec![
                    "repaired-campaign-parent".to_string(),
                    "preserved-worst-of-rollup".to_string(),
                ],
            }],
            missing: Vec::new(),
            input_sha256: parent_semantic_input_sha256(&parent, &job),
            spend_usd: 0.0,
        };
        deadreckon_runtime::persist_semantic_judgment(&parent.run_root, &achieved)
            .expect("achieved judgment");

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
        .expect("campaign repair classification");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("job view");
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::Verified)
        );
        assert_eq!(view.projection.stop_reason, Some(StopReason::Verified));
        assert_eq!(view.projection.attempt_count, 2);

        for (run_id, state_bytes, tree_sha256) in leaf_snapshots {
            let leaf = load_run(&paths, &run_id).expect("successful leaf remains");
            assert_eq!(
                fs::read(leaf.state_path()).expect("leaf state bytes after repair"),
                state_bytes,
                "campaign parent repair reran or rewrote successful child {run_id}"
            );
            assert_eq!(
                parent_result_tree_sha256(&leaf),
                tree_sha256,
                "campaign parent repair changed successful child output {run_id}"
            );
        }
        let persisted_campaign =
            deadreckon_core::campaign::read_campaign(&campaign_dir).expect("campaign after repair");
        let persisted_rollup =
            deadreckon_core::campaign::read_campaign_rollup(&campaign_dir).expect("rollup");
        assert_eq!(persisted_rollup, rollup);
        assert!(deadreckon_core::campaign::campaign_can_complete(
            &persisted_campaign,
            &persisted_rollup
        ));
        let parent = load_run(&paths, job.job_id.as_ref()).expect("repaired parent");
        let parent_rollup: deadreckon_core::campaign::CampaignRollup = serde_json::from_slice(
            &fs::read(deadreckon_core::campaign::rollup_path_at_run_root(
                &parent.run_root,
            ))
            .expect("parent rollup"),
        )
        .expect("parent rollup json");
        assert_eq!(parent_rollup, rollup);

        let finish = super::super::lifecycle::finish_job_state(&paths, &view)
            .expect("finish repaired campaign parent");
        assert_eq!(finish.run_id, job.job_id.as_ref());
        assert_eq!(
            fs::read_to_string(finish.working_dir.join("campaign.txt"))
                .expect("repaired campaign output"),
            "repaired campaign parent with aggregate explanation\n"
        );
        deadreckon_core::validate_completion_receipt(&paths, &finish)
            .expect("campaign repair receipt remains valid");
    }

    #[tokio::test]
    async fn persisted_achieved_graph_judgment_cannot_seal_over_cap_after_crash() {
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
        let mut plan = deadreckon_core::load_plan(&paths, job.job_id.as_ref()).expect("graph plan");
        for task in &mut plan.tasks {
            task.child_run_id = Some(merged.run_id.clone());
        }
        deadreckon_core::plan::save_plan(&paths, &plan).expect("graph accounting links");

        let authority: JobAuthority = serde_json::from_slice(
            &fs::read(paths.job_authority(job.job_id.as_ref())).expect("authority"),
        )
        .expect("authority json");
        let mut parent =
            super::super::graph_job::prepare_parent_result_run(&paths, &job, &authority, &merged)
                .expect("parent");
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
            goal_coverage: vec![deadreckon_protocol::GoalCoverage {
                claim: "approved graph goal".to_string(),
                status: deadreckon_protocol::GoalCoverageStatus::Met,
                evidence: vec![
                    "approved-goal".to_string(),
                    "deterministic-gate".to_string(),
                ],
            }],
            missing: Vec::new(),
            input_sha256: "sha256:test-evidence".to_string(),
            spend_usd: 0.25,
        };
        deadreckon_runtime::persist_semantic_judgment(&parent.run_root, &judgment)
            .expect("judgment");
        parent.total_spend_usd = job.policy.max_spend_usd + 0.01;
        save_state(&parent).expect("over-cap accounting");

        let completion =
            super::super::graph_job::complete_merged_plan_parent(&paths, &job, &authority, &plan)
                .await
                .expect("completion");

        let super::super::graph_job::ParentCompletion::BudgetExhausted { stop_reason, .. } =
            completion
        else {
            panic!("over-cap persisted judgment must not seal")
        };
        assert_eq!(stop_reason, StopReason::SpendCap);
        assert!(!paths.job_receipt(job.job_id.as_ref()).exists());
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
        let mut command = build_leaf_command(&paths, &job, &launch, Path::new("/opt/deadreckon"));
        apply_durable_scope_root(&mut command, &launch.plan);
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
        assert_eq!(
            command
                .get_envs()
                .find(|(key, _)| *key == "DEADRECKON_SCOPE_ROOT")
                .and_then(|(_, value)| value),
            Some(job.source_cwd.as_os_str())
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
    fn root_run_replays_the_normalized_copy_source_exactly_once() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = fixture(&temp, 1);
        let mut launch = load_launch_inputs(&paths, &job).expect("launch");
        let mut signals = launch.plan.signals.as_object().cloned().unwrap_or_default();
        signals.insert(
            "watchkeeper_source".to_string(),
            serde_json::to_value(super::super::job::DurableSource {
                mode: super::super::job::DurableSourceMode::Copy,
                from: Some(job.source_cwd.clone()),
                allow_dirty: false,
            })
            .expect("source json"),
        );
        launch.plan.signals = Value::Object(signals);

        let command = build_leaf_command(&paths, &job, &launch, Path::new("/opt/deadreckon"));
        let args = command.get_args().map(OsString::from).collect::<Vec<_>>();
        let from = args.iter().position(|arg| arg == "--from").expect("--from");
        assert_eq!(
            args.get(from + 1),
            Some(&job.source_cwd.as_os_str().to_os_string())
        );
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

    #[test]
    fn guarded_launcher_requires_its_current_fenced_child_link() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = fixture(&temp, 2);
        let owner = LeaseOwner {
            owner_id: "launch-owner".to_string(),
            boot_id: "launch-boot".to_string(),
            pid: std::process::id(),
            process_group: std::process::id(),
        };
        let claim =
            claim_job_lease(&paths, &job.job_id, &owner, Utc::now(), LEASE_TTL).expect("claim");
        let token = claim.token();
        let prepared = PreparedChildLaunch::new(1);
        append_control_event(
            &paths,
            &token,
            JobEventKind::ChildLaunchPrepared,
            "prepared-launch".to_string(),
            child_launch_prepared_detail(&job, &prepared),
        )
        .expect("prepared");
        append_control_event(
            &paths,
            &token,
            JobEventKind::AttemptStarted,
            "started-attempt".to_string(),
            attempt_detail(&job, 1),
        )
        .expect("started");
        let process_start = process_start_identity(std::process::id());

        assert!(
            !launcher_link_is_durable(
                &paths,
                job.job_id.as_ref(),
                1,
                &prepared.launch_id,
                &prepared.release_token_sha256,
                std::process::id(),
                process_start.as_deref(),
            )
            .expect("unlinked refusal")
        );

        let metadata = SupervisorChildMetadata {
            process: SupervisedProcess {
                pid: std::process::id(),
                #[cfg(unix)]
                pgid: Some(std::process::id()),
                #[cfg(not(unix))]
                pgid: None,
            },
            launch_id: Some(prepared.launch_id.clone()),
            attempt: Some(1),
            release_token_sha256: Some(prepared.release_token_sha256.clone()),
            boot_id: Some(boot_identity()),
            process_start_identity: process_start.clone(),
        };
        append_control_event(
            &paths,
            &token,
            JobEventKind::ChildLinked,
            "linked-launch".to_string(),
            child_link_detail(&job, &metadata, false, Some(1)),
        )
        .expect("linked");
        assert!(
            launcher_link_is_durable(
                &paths,
                job.job_id.as_ref(),
                1,
                &prepared.launch_id,
                &prepared.release_token_sha256,
                std::process::id(),
                process_start.as_deref(),
            )
            .expect("linked authorization")
        );

        let replacement = LeaseOwner {
            owner_id: "replacement".to_string(),
            boot_id: "replacement-boot".to_string(),
            pid: std::process::id(),
            process_group: std::process::id(),
        };
        claim_job_lease(&paths, &job.job_id, &replacement, Utc::now(), LEASE_TTL)
            .expect("reclaim by new boot");
        assert!(
            !launcher_link_is_durable(
                &paths,
                job.job_id.as_ref(),
                1,
                &prepared.launch_id,
                &prepared.release_token_sha256,
                std::process::id(),
                process_start.as_deref(),
            )
            .expect("stale link refusal")
        );
    }

    #[test]
    fn private_release_ack_is_required_and_bound_to_its_token_preimage() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = fixture(&temp, 2);
        let token = claim_started_attempt(&paths, &job, 1);
        let prepared = PreparedChildLaunch::new(1);
        append_control_event(
            &paths,
            &token,
            JobEventKind::ChildLaunchPrepared,
            "prepared-after-test-attempt".to_string(),
            child_launch_prepared_detail(&job, &prepared),
        )
        .expect("prepared");
        let projection = JobView::load(&paths, job.job_id.as_ref())
            .expect("view")
            .projection;
        let recovery =
            recoverable_unlinked_guarded_launch(&paths, job.job_id.as_ref(), &projection)
                .expect("unreleased launch")
                .expect("recoverable");
        assert_eq!(recovery.attempt, 1);
        assert!(recovery.attempt_started);

        let process_start = process_start_identity(std::process::id());
        let metadata = SupervisorChildMetadata {
            process: SupervisedProcess {
                pid: std::process::id(),
                #[cfg(unix)]
                pgid: Some(std::process::id()),
                #[cfg(not(unix))]
                pgid: None,
            },
            launch_id: Some(prepared.launch_id.clone()),
            attempt: Some(1),
            release_token_sha256: Some(prepared.release_token_sha256.clone()),
            boot_id: Some(boot_identity()),
            process_start_identity: process_start.clone(),
        };
        append_control_event(
            &paths,
            &token,
            JobEventKind::ChildLinked,
            "linked-for-ack".to_string(),
            child_link_detail(&job, &metadata, false, Some(1)),
        )
        .expect("linked");

        write_release_ack(
            &paths,
            &SupervisorReleaseAck {
                launch_protocol: GUARDED_LAUNCH_PROTOCOL.to_string(),
                job_id: job.job_id.as_ref().to_string(),
                attempt: 1,
                launch_id: prepared.launch_id.clone(),
                release_token_sha256: Some(deadreckon_core::flight::sha256_text(
                    "forged-visible-digest-preimage",
                )),
                legacy_release_token: None,
                pid: std::process::id(),
                process_start_identity: process_start.clone(),
                acknowledged_at: Utc::now(),
            },
        )
        .expect("forged ack fixture");
        let error = recoverable_unlinked_guarded_launch(&paths, job.job_id.as_ref(), &projection)
            .expect_err("a forged acknowledgement must fail closed");
        assert!(
            error
                .to_string()
                .contains("acknowledgement failed validation"),
            "{error}"
        );

        write_release_ack(
            &paths,
            &SupervisorReleaseAck {
                launch_protocol: GUARDED_LAUNCH_PROTOCOL.to_string(),
                job_id: job.job_id.as_ref().to_string(),
                attempt: 1,
                launch_id: prepared.launch_id.clone(),
                release_token_sha256: Some(prepared.release_token_sha256.clone()),
                legacy_release_token: None,
                pid: std::process::id(),
                process_start_identity: process_start,
                acknowledged_at: Utc::now(),
            },
        )
        .expect("real ack fixture");
        assert!(
            recoverable_unlinked_guarded_launch(&paths, job.job_id.as_ref(), &projection)
                .expect("acknowledged launch")
                .is_none()
        );
    }

    #[test]
    fn recovery_waits_for_a_linked_launcher_to_settle_before_relaunching() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = fixture(&temp, 2);
        let token = claim_started_attempt(&paths, &job, 1);
        let prepared = PreparedChildLaunch::new(1);
        append_control_event(
            &paths,
            &token,
            JobEventKind::ChildLaunchPrepared,
            "prepared-before-release-race".to_string(),
            child_launch_prepared_detail(&job, &prepared),
        )
        .expect("prepared");
        let process_start = process_start_identity(std::process::id());
        let child = SupervisorChildMetadata {
            process: SupervisedProcess {
                pid: std::process::id(),
                #[cfg(unix)]
                pgid: Some(std::process::id()),
                #[cfg(not(unix))]
                pgid: None,
            },
            launch_id: Some(prepared.launch_id.clone()),
            attempt: Some(1),
            release_token_sha256: Some(prepared.release_token_sha256.clone()),
            boot_id: Some(boot_identity()),
            process_start_identity: process_start.clone(),
        };
        append_control_event(
            &paths,
            &token,
            JobEventKind::ChildLinked,
            "linked-before-release-race".to_string(),
            child_link_detail(&job, &child, false, Some(1)),
        )
        .expect("linked");
        let projection = JobView::load(&paths, job.job_id.as_ref())
            .expect("projection")
            .projection;
        let recovery =
            recoverable_unlinked_guarded_launch(&paths, job.job_id.as_ref(), &projection)
                .expect("unacknowledged launch")
                .expect("recoverable launch");

        let ack_paths = paths.clone();
        let ack_job_id = job.job_id.as_ref().to_string();
        let ack_launch_id = prepared.launch_id.clone();
        let ack_token = prepared.release_token.clone();
        let acknowledgement = thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            write_release_ack(
                &ack_paths,
                &SupervisorReleaseAck {
                    launch_protocol: GUARDED_LAUNCH_PROTOCOL.to_string(),
                    job_id: ack_job_id,
                    attempt: 1,
                    launch_id: ack_launch_id,
                    release_token_sha256: Some(deadreckon_core::flight::sha256_text(&ack_token)),
                    legacy_release_token: None,
                    pid: std::process::id(),
                    process_start_identity: process_start,
                    acknowledged_at: Utc::now(),
                },
            )
            .expect("release acknowledgement");
        });
        let mut stored_child = Some(child);
        assert_eq!(
            prepare_unlinked_launch_recovery(
                &paths,
                job.job_id.as_ref(),
                &mut stored_child,
                &recovery,
            )
            .expect("settled launch"),
            UnlinkedLaunchDisposition::RecheckAcknowledgement
        );
        acknowledgement.join().expect("acknowledgement writer");
        assert!(
            stored_child.is_some(),
            "released child must remain adoptable"
        );
        assert!(
            recoverable_unlinked_guarded_launch(&paths, job.job_id.as_ref(), &projection)
                .expect("validated acknowledgement")
                .is_none(),
            "a valid acknowledgement forbids relaunching the logical attempt"
        );
    }

    #[tokio::test]
    async fn forged_release_ack_stops_with_an_explicit_corrupt_history_reason() {
        let temp = TempDir::new().expect("tempdir");
        let (paths, job) = fixture(&temp, 2);
        let token = claim_started_attempt(&paths, &job, 1);
        let prepared = PreparedChildLaunch::new(1);
        append_control_event(
            &paths,
            &token,
            JobEventKind::ChildLaunchPrepared,
            "prepared-for-forged-ack".to_string(),
            child_launch_prepared_detail(&job, &prepared),
        )
        .expect("prepared");
        let process_start = process_start_identity(std::process::id());
        let metadata = SupervisorChildMetadata {
            process: SupervisedProcess {
                pid: std::process::id(),
                #[cfg(unix)]
                pgid: Some(std::process::id()),
                #[cfg(not(unix))]
                pgid: None,
            },
            launch_id: Some(prepared.launch_id.clone()),
            attempt: Some(1),
            release_token_sha256: Some(prepared.release_token_sha256.clone()),
            boot_id: Some(boot_identity()),
            process_start_identity: process_start.clone(),
        };
        append_control_event(
            &paths,
            &token,
            JobEventKind::ChildLinked,
            "linked-for-forged-ack".to_string(),
            child_link_detail(&job, &metadata, false, Some(1)),
        )
        .expect("linked");
        write_release_ack(
            &paths,
            &SupervisorReleaseAck {
                launch_protocol: GUARDED_LAUNCH_PROTOCOL.to_string(),
                job_id: job.job_id.as_ref().to_string(),
                attempt: 1,
                launch_id: prepared.launch_id,
                release_token_sha256: Some(deadreckon_core::flight::sha256_text(
                    "not-the-private-release-token",
                )),
                legacy_release_token: None,
                pid: std::process::id(),
                process_start_identity: process_start,
                acknowledged_at: Utc::now(),
            },
        )
        .expect("forged acknowledgement fixture");

        let recovering_instance = SupervisorInstance {
            owner: LeaseOwner {
                owner_id: "forged-ack-recovery".to_string(),
                boot_id: "replacement-boot".to_string(),
                pid: std::process::id(),
                process_group: std::process::id(),
            },
            executable: temp.path().join("must-not-launch"),
        };
        supervise_one_job(&paths, &recovering_instance, job.job_id.as_ref())
            .await
            .expect("forged acknowledgement is classified");

        let view = JobView::load(&paths, job.job_id.as_ref()).expect("blocked view");
        assert_eq!(
            view.projection.outcome,
            Some(deadreckon_protocol::JobOutcome::Blocked)
        );
        assert_eq!(
            view.projection.stop_reason,
            Some(StopReason::CorruptHistory)
        );
    }

    #[test]
    fn child_adoption_requires_boot_and_process_start_identity() {
        let process_start = process_start_identity(std::process::id());
        let metadata = SupervisorChildMetadata {
            process: SupervisedProcess {
                pid: std::process::id(),
                #[cfg(unix)]
                pgid: Some(std::process::id()),
                #[cfg(not(unix))]
                pgid: None,
            },
            launch_id: Some(Uuid::new_v4().to_string()),
            attempt: Some(1),
            release_token_sha256: Some("sha256:test".to_string()),
            boot_id: Some(boot_identity()),
            process_start_identity: process_start.clone(),
        };
        assert_eq!(
            child_identity_is_current(&metadata),
            process_start.is_some(),
            "supported platforms must match the exact current process identity"
        );
        let mut reused = metadata;
        reused.process_start_identity = Some("different-process-start".to_string());
        assert!(
            !child_identity_is_current(&reused),
            "a live reused PID must not be adopted"
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
