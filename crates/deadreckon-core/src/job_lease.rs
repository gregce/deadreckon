//! Fenced, renewable ownership for durable jobs.
//!
//! A lease is a rebuildable checkpoint. The append-only lease-acquired or
//! lease-reclaimed event is written first while holding the same per-job lock
//! used by fenced worker events.

use std::fs;
use std::io::Write;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use deadreckon_protocol::{
    JobEvent, JobEventKind, JobEventSequence, JobId, JobLease, JobSchemaVersion,
};
use fs2::FileExt;
use serde_json::{Map, Value};
use tempfile::NamedTempFile;
use uuid::Uuid;

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::job::{
    JOB_CONTROL_LOCK, JobProjection, append_job_event_locked, open_job_control_lock,
    read_job_history, reduce_job_history,
};
use crate::paths::DeadreckonPaths;
use crate::state::atomic_write_json;

/// Process identity supplied by one supervisor instance.
///
/// `pid` and `process_group` are evidence for later reconciliation, not proof
/// that this owner still holds the lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseOwner {
    pub owner_id: String,
    pub boot_id: String,
    pub pid: u32,
    pub process_group: u32,
}

/// The capability a worker must present for every lifecycle commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseToken {
    pub job_id: JobId,
    pub owner_id: String,
    pub epoch: u64,
    pub boot_id: String,
}

/// Event metadata committed with one fenced controller-owned JSON projection.
#[derive(Debug, Clone, PartialEq)]
pub struct FencedJobJsonEvent {
    pub kind: JobEventKind,
    pub causation_id: String,
    pub detail: Value,
}

/// Result of atomically reserving one controller-owned JSON authority path.
#[derive(Debug, Clone, PartialEq)]
pub enum CreateFencedJobJsonDisposition {
    Created(JobProjection),
    AlreadyExists,
}

impl From<&JobLease> for LeaseToken {
    fn from(lease: &JobLease) -> Self {
        Self {
            job_id: lease.job_id.clone(),
            owner_id: lease.owner_id.clone(),
            epoch: lease.epoch,
            boot_id: lease.boot_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseReclaimReason {
    Expired,
    BootIdentityChanged,
    MissingCheckpoint,
}

impl LeaseReclaimReason {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::BootIdentityChanged => "boot_identity_changed",
            Self::MissingCheckpoint => "missing_checkpoint",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseClaimDisposition {
    Acquired,
    Reclaimed(LeaseReclaimReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseClaim {
    pub lease: JobLease,
    pub disposition: LeaseClaimDisposition,
}

impl LeaseClaim {
    pub fn token(&self) -> LeaseToken {
        LeaseToken::from(&self.lease)
    }
}

/// Claim an unowned or reclaimable job.
///
/// Calls serialize on the job control lock. A current, unexpired lease from
/// the same boot cannot be stolen even if the claimant happens to reuse its
/// PID. A changed boot identity makes the old process identity impossible and
/// permits immediate reclamation.
pub fn claim_job_lease(
    paths: &DeadreckonPaths,
    job_id: &JobId,
    owner: &LeaseOwner,
    now: DateTime<Utc>,
    ttl: Duration,
) -> Result<LeaseClaim> {
    validate_owner(owner)?;
    let expires_at = lease_expiry(now, ttl)?;
    let job_dir = paths.job_dir(job_id.as_ref());
    fs::create_dir_all(&job_dir).with_path(&job_dir)?;
    let lock_path = job_dir.join(JOB_CONTROL_LOCK);
    let lock = open_job_control_lock(&lock_path)?;
    FileExt::lock_exclusive(&lock).with_path(&lock_path)?;

    let prior = load_job_lease_optional(paths, job_id)?;
    let history = read_job_history(&paths.job_events(job_id.as_ref()))?;
    if !history.caveats.is_empty() {
        return Err(lease_error(
            job_id,
            "cannot claim after a torn final job event",
        ));
    }
    let projection = reduce_job_history(job_id, &history)?;
    let checkpoint_epoch = prior.as_ref().map_or(0, |lease| lease.epoch);
    let durable_epoch = projection.current_lease_epoch.max(checkpoint_epoch);

    let disposition = match prior.as_ref() {
        Some(lease) if lease.epoch == durable_epoch && lease.boot_id != owner.boot_id => {
            LeaseClaimDisposition::Reclaimed(LeaseReclaimReason::BootIdentityChanged)
        }
        Some(lease) if lease.epoch == durable_epoch && lease.expires_at <= now => {
            LeaseClaimDisposition::Reclaimed(LeaseReclaimReason::Expired)
        }
        Some(lease) if lease.epoch == durable_epoch => {
            return Err(lease_error(
                job_id,
                &format!(
                    "live lease held by owner {} at epoch {} until {}; try: deadreckon attach {}",
                    lease.owner_id, lease.epoch, lease.expires_at, job_id
                ),
            ));
        }
        _ if durable_epoch > 0 => {
            LeaseClaimDisposition::Reclaimed(LeaseReclaimReason::MissingCheckpoint)
        }
        _ => LeaseClaimDisposition::Acquired,
    };

    let epoch = durable_epoch
        .checked_add(1)
        .ok_or_else(|| lease_error(job_id, "lease epoch exhausted"))?;
    let lease = JobLease {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: job_id.clone(),
        owner_id: owner.owner_id.clone(),
        epoch,
        acquired_at: now,
        heartbeat_at: now,
        expires_at,
        boot_id: owner.boot_id.clone(),
        pid: owner.pid,
        process_group: owner.process_group,
        child_pid: None,
    };
    let event = lease_event(
        &lease,
        projection
            .last_sequence
            .checked_add(1)
            .and_then(JobEventSequence::new)
            .ok_or_else(|| lease_error(job_id, "job event sequence exhausted"))?,
        disposition,
        now,
    )?;

    // The event is lifecycle truth. The lease file is only its checkpoint.
    append_job_event_locked(paths, &event)?;
    atomic_write_json(&paths.job_lease(job_id.as_ref()), &lease)?;

    Ok(LeaseClaim { lease, disposition })
}

/// Renew the current lease without changing execution phase or event history.
pub fn heartbeat_job_lease(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    now: DateTime<Utc>,
    ttl: Duration,
) -> Result<JobLease> {
    let expires_at = lease_expiry(now, ttl)?;
    let lock_path = paths.job_dir(token.job_id.as_ref()).join(JOB_CONTROL_LOCK);
    let lock = open_job_control_lock(&lock_path)?;
    FileExt::lock_exclusive(&lock).with_path(&lock_path)?;

    let mut lease = load_job_lease(paths, &token.job_id)?;
    validate_token(&lease, token, now)?;
    lease.heartbeat_at = now;
    lease.expires_at = expires_at;
    atomic_write_json(&paths.job_lease(token.job_id.as_ref()), &lease)?;
    Ok(lease)
}

/// Append one worker lifecycle fact only when its lease epoch is still current.
pub fn append_fenced_job_event(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    now: DateTime<Utc>,
    event: &JobEvent,
) -> Result<JobProjection> {
    let lock_path = paths.job_dir(token.job_id.as_ref()).join(JOB_CONTROL_LOCK);
    let lock = open_job_control_lock(&lock_path)?;
    FileExt::lock_exclusive(&lock).with_path(&lock_path)?;

    let lease = load_job_lease(paths, &token.job_id)?;
    validate_token(&lease, token, now)?;
    if event.job_id != token.job_id {
        return Err(lease_error(
            &token.job_id,
            &format!("event belongs to {}", event.job_id),
        ));
    }
    if event.lease_epoch != token.epoch {
        return Err(lease_error(
            &token.job_id,
            &format!(
                "event lease epoch {} does not match token epoch {}",
                event.lease_epoch, token.epoch
            ),
        ));
    }
    let history = read_job_history(&paths.job_events(event.job_id.as_ref()))?;
    let projection = reduce_job_history(&event.job_id, &history)?;
    if projection.is_terminal() {
        return Ok(projection);
    }
    if projection.stop_reason == Some(deadreckon_protocol::StopReason::CancelRequested) {
        let cancellation_stop = event.kind == JobEventKind::AttemptStopped
            && event.detail.get("stop_reason").and_then(Value::as_str) == Some("cancel_requested");
        if event.kind != JobEventKind::Cancelled && !cancellation_stop {
            return Ok(projection);
        }
    }
    append_job_event_locked(paths, event)
}

/// Append one worker lifecycle fact using the next sequence chosen while the
/// Job control lock is held.
///
/// Callers that already possess a complete immutable event should use
/// [`append_fenced_job_event`]. Concurrent supervised children should use this
/// seam instead: deriving a sequence before taking the lock lets two valid
/// children race with the same sequence and turns ordinary concurrency into a
/// false corrupt-history failure.
pub fn append_next_fenced_job_event(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    now: DateTime<Utc>,
    kind: JobEventKind,
    causation_id: String,
    detail: Value,
) -> Result<JobProjection> {
    let lock_path = paths.job_dir(token.job_id.as_ref()).join(JOB_CONTROL_LOCK);
    let lock = open_job_control_lock(&lock_path)?;
    FileExt::lock_exclusive(&lock).with_path(&lock_path)?;

    let lease = load_job_lease(paths, &token.job_id)?;
    validate_token(&lease, token, now)?;
    let history = read_job_history(&paths.job_events(token.job_id.as_ref()))?;
    let projection = reduce_job_history(&token.job_id, &history)?;
    if projection.is_terminal() {
        return Ok(projection);
    }
    if projection.stop_reason == Some(deadreckon_protocol::StopReason::CancelRequested) {
        let cancellation_stop = kind == JobEventKind::AttemptStopped
            && detail.get("stop_reason").and_then(Value::as_str) == Some("cancel_requested");
        if kind != JobEventKind::Cancelled && !cancellation_stop {
            return Ok(projection);
        }
    }
    let sequence = projection
        .last_sequence
        .checked_add(1)
        .and_then(JobEventSequence::new)
        .ok_or_else(|| lease_error(&token.job_id, "job event sequence exhausted"))?;
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
    append_job_event_locked(paths, &event)
}

/// Run one controller-owned mutation while the current Job lease is validated
/// and its control lock is held. This is the common authority boundary for
/// derived orchestration state that lives outside the Job directory (for
/// example an owned Plan projection).
pub(crate) fn with_fenced_job_control<T>(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    now: DateTime<Utc>,
    mutate: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let lock_path = paths.job_dir(token.job_id.as_ref()).join(JOB_CONTROL_LOCK);
    let lock = open_job_control_lock(&lock_path)?;
    FileExt::lock_exclusive(&lock).with_path(&lock_path)?;

    let lease = load_job_lease(paths, &token.job_id)?;
    validate_token(&lease, token, now)?;
    let history = read_job_history(&paths.job_events(token.job_id.as_ref()))?;
    let projection = reduce_job_history(&token.job_id, &history)?;
    if projection.is_terminal()
        || projection.stop_reason == Some(deadreckon_protocol::StopReason::CancelRequested)
    {
        return Err(lease_error(
            &token.job_id,
            "cannot mutate an owned artifact after the Job stopped",
        ));
    }
    mutate()
}

/// Replace one controller-owned JSON projection and append the event that
/// authorizes that exact replacement while holding the Job control lock.
///
/// The projection is written first. A crash before the event therefore leaves
/// an uncommitted projection that consumers must refuse as authority. Once the
/// event is durable, lease reclamation cannot interleave with either write.
pub fn replace_fenced_job_json_and_append_event<T: serde::Serialize>(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    now: DateTime<Utc>,
    artifact_path: &std::path::Path,
    event: FencedJobJsonEvent,
    value: &T,
) -> Result<JobProjection> {
    let job_dir = paths.job_dir(token.job_id.as_ref());
    let relative_artifact = artifact_path.strip_prefix(&job_dir).map_err(|_| {
        lease_error(
            &token.job_id,
            "fenced artifact path is outside the protected Job directory",
        )
    })?;
    if relative_artifact.as_os_str().is_empty()
        || relative_artifact
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(lease_error(
            &token.job_id,
            "fenced artifact path is not a normalized protected Job child",
        ));
    }
    let lock_path = job_dir.join(JOB_CONTROL_LOCK);
    let lock = open_job_control_lock(&lock_path)?;
    FileExt::lock_exclusive(&lock).with_path(&lock_path)?;

    let lease = load_job_lease(paths, &token.job_id)?;
    validate_token(&lease, token, now)?;
    let history = read_job_history(&paths.job_events(token.job_id.as_ref()))?;
    let projection = reduce_job_history(&token.job_id, &history)?;
    if projection.is_terminal()
        || projection.stop_reason == Some(deadreckon_protocol::StopReason::CancelRequested)
    {
        return Err(lease_error(
            &token.job_id,
            "cannot replace a fenced artifact after the Job stopped",
        ));
    }
    let sequence = projection
        .last_sequence
        .checked_add(1)
        .and_then(JobEventSequence::new)
        .ok_or_else(|| lease_error(&token.job_id, "job event sequence exhausted"))?;
    let event = JobEvent {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: token.job_id.clone(),
        sequence,
        event_id: Uuid::new_v4().to_string(),
        causation_id: event.causation_id,
        timestamp: now,
        lease_epoch: token.epoch,
        kind: event.kind,
        detail: event.detail,
    };
    atomic_write_json(artifact_path, value)?;
    append_job_event_locked(paths, &event)
}

/// Create one controller-owned JSON authority and append its authorizing event
/// while holding the Job control lock.
///
/// Unlike [`replace_fenced_job_json_and_append_event`], this operation never
/// replaces an existing path. It is the launch-boundary primitive for child
/// authorities whose first durable identity must be chosen exactly once.
/// The artifact is written before the event, so a crash between the writes
/// leaves an uncommitted authority that recovery must reject rather than
/// silently replacing.
pub fn create_fenced_job_json_and_append_event<T: serde::Serialize>(
    paths: &DeadreckonPaths,
    token: &LeaseToken,
    now: DateTime<Utc>,
    artifact_path: &std::path::Path,
    event: FencedJobJsonEvent,
    value: &T,
) -> Result<CreateFencedJobJsonDisposition> {
    let job_dir = paths.job_dir(token.job_id.as_ref());
    let relative_artifact = artifact_path.strip_prefix(&job_dir).map_err(|_| {
        lease_error(
            &token.job_id,
            "fenced artifact path is outside the protected Job directory",
        )
    })?;
    if relative_artifact.as_os_str().is_empty()
        || relative_artifact
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(lease_error(
            &token.job_id,
            "fenced artifact path is not a normalized protected Job child",
        ));
    }
    let lock_path = job_dir.join(JOB_CONTROL_LOCK);
    let lock = open_job_control_lock(&lock_path)?;
    FileExt::lock_exclusive(&lock).with_path(&lock_path)?;

    let lease = load_job_lease(paths, &token.job_id)?;
    validate_token(&lease, token, now)?;
    let history = read_job_history(&paths.job_events(token.job_id.as_ref()))?;
    let projection = reduce_job_history(&token.job_id, &history)?;
    if projection.is_terminal()
        || projection.stop_reason == Some(deadreckon_protocol::StopReason::CancelRequested)
    {
        return Err(lease_error(
            &token.job_id,
            "cannot create a fenced artifact after the Job stopped",
        ));
    }
    let sequence = projection
        .last_sequence
        .checked_add(1)
        .and_then(JobEventSequence::new)
        .ok_or_else(|| lease_error(&token.job_id, "job event sequence exhausted"))?;
    let event = JobEvent {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: token.job_id.clone(),
        sequence,
        event_id: Uuid::new_v4().to_string(),
        causation_id: event.causation_id,
        timestamp: now,
        lease_epoch: token.epoch,
        kind: event.kind,
        detail: event.detail,
    };
    if !persist_new_json(artifact_path, value)? {
        return Ok(CreateFencedJobJsonDisposition::AlreadyExists);
    }
    append_job_event_locked(paths, &event).map(CreateFencedJobJsonDisposition::Created)
}

fn persist_new_json(path: &std::path::Path, value: &impl serde::Serialize) -> Result<bool> {
    let parent = path.parent().ok_or_else(|| {
        DeadreckonError::InvalidInput(format!("path has no parent: {}", path.display()))
    })?;
    fs::create_dir_all(parent).with_path(parent)?;
    let mut temp = NamedTempFile::new_in(parent).with_path(parent)?;
    serde_json::to_writer_pretty(&mut temp, value).with_json_path(path)?;
    temp.write_all(b"\n").with_path(path)?;
    temp.as_file_mut().sync_all().with_path(path)?;
    match temp.persist_noclobber(path) {
        Ok(file) => {
            file.sync_all().with_path(path)?;
            sync_parent(parent)?;
            Ok(true)
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(DeadreckonError::Io {
            path: path.to_path_buf(),
            source: error.error,
        }),
    }
}

#[cfg(unix)]
fn sync_parent(parent: &std::path::Path) -> Result<()> {
    fs::File::open(parent)
        .with_path(parent)?
        .sync_all()
        .with_path(parent)
}

#[cfg(not(unix))]
fn sync_parent(_parent: &std::path::Path) -> Result<()> {
    Ok(())
}

pub fn load_job_lease(paths: &DeadreckonPaths, job_id: &JobId) -> Result<JobLease> {
    let path = paths.job_lease(job_id.as_ref());
    let raw = fs::read(&path).with_path(&path)?;
    let lease: JobLease = serde_json::from_slice(&raw).with_json_path(&path)?;
    if lease.job_id != *job_id {
        return Err(lease_error(
            job_id,
            &format!("lease checkpoint belongs to {}", lease.job_id),
        ));
    }
    Ok(lease)
}

fn load_job_lease_optional(paths: &DeadreckonPaths, job_id: &JobId) -> Result<Option<JobLease>> {
    let path = paths.job_lease(job_id.as_ref());
    match fs::read(&path) {
        Ok(raw) => {
            let lease: JobLease = serde_json::from_slice(&raw).with_json_path(&path)?;
            if lease.job_id != *job_id {
                return Err(lease_error(
                    job_id,
                    &format!("lease checkpoint belongs to {}", lease.job_id),
                ));
            }
            Ok(Some(lease))
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(DeadreckonError::Io { path, source }),
    }
}

fn validate_owner(owner: &LeaseOwner) -> Result<()> {
    if owner.owner_id.trim().is_empty() {
        return Err(DeadreckonError::InvalidInput(
            "job lease owner_id must not be empty".to_string(),
        ));
    }
    if owner.boot_id.trim().is_empty() {
        return Err(DeadreckonError::InvalidInput(
            "job lease boot_id must not be empty".to_string(),
        ));
    }
    if owner.pid == 0 || owner.process_group == 0 {
        return Err(DeadreckonError::InvalidInput(
            "job lease pid and process_group must be non-zero".to_string(),
        ));
    }
    Ok(())
}

fn validate_token(lease: &JobLease, token: &LeaseToken, now: DateTime<Utc>) -> Result<()> {
    if lease.job_id != token.job_id
        || lease.owner_id != token.owner_id
        || lease.epoch != token.epoch
        || lease.boot_id != token.boot_id
    {
        return Err(lease_error(
            &token.job_id,
            &format!(
                "stale lease token for owner {} epoch {}",
                token.owner_id, token.epoch
            ),
        ));
    }
    if lease.expires_at <= now {
        return Err(lease_error(
            &token.job_id,
            &format!(
                "lease epoch {} expired at {}",
                lease.epoch, lease.expires_at
            ),
        ));
    }
    Ok(())
}

fn lease_expiry(now: DateTime<Utc>, ttl: Duration) -> Result<DateTime<Utc>> {
    if ttl.is_zero() {
        return Err(DeadreckonError::InvalidInput(
            "job lease ttl must be greater than zero".to_string(),
        ));
    }
    let delta = TimeDelta::from_std(ttl).map_err(|_| {
        DeadreckonError::InvalidInput("job lease ttl is outside chrono range".to_string())
    })?;
    now.checked_add_signed(delta)
        .ok_or_else(|| DeadreckonError::InvalidInput("job lease expiry overflow".to_string()))
}

fn lease_event(
    lease: &JobLease,
    sequence: JobEventSequence,
    disposition: LeaseClaimDisposition,
    now: DateTime<Utc>,
) -> Result<JobEvent> {
    let mut detail = match serde_json::to_value(lease).map_err(|source| DeadreckonError::Json {
        path: std::path::PathBuf::from("lease.json"),
        source,
    })? {
        Value::Object(detail) => detail,
        _ => Map::new(),
    };
    if let LeaseClaimDisposition::Reclaimed(reason) = disposition {
        detail.insert(
            "reclaim_reason".to_string(),
            Value::String(reason.as_str().to_string()),
        );
    }
    let event_id = Uuid::new_v4().to_string();
    Ok(JobEvent {
        schema_version: JobSchemaVersion::CURRENT,
        job_id: lease.job_id.clone(),
        sequence,
        event_id,
        causation_id: format!(
            "lease-claim:{}:{}:{}",
            lease.job_id, lease.owner_id, lease.epoch
        ),
        timestamp: now,
        lease_epoch: lease.epoch,
        kind: match disposition {
            LeaseClaimDisposition::Acquired => JobEventKind::LeaseAcquired,
            LeaseClaimDisposition::Reclaimed(_) => JobEventKind::LeaseReclaimed,
        },
        detail: Value::Object(detail),
    })
}

fn lease_error(job_id: &JobId, detail: &str) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!("job lease {}: {detail}", job_id))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    use chrono::{DateTime, TimeDelta, Utc};
    use deadreckon_protocol::{JobEvent, JobEventKind, JobEventSequence, JobId, JobSchemaVersion};
    use serde_json::json;
    use tempfile::TempDir;

    use super::{
        CreateFencedJobJsonDisposition, FencedJobJsonEvent, LeaseClaimDisposition, LeaseOwner,
        LeaseReclaimReason, append_fenced_job_event, append_next_fenced_job_event, claim_job_lease,
        create_fenced_job_json_and_append_event, heartbeat_job_lease,
        replace_fenced_job_json_and_append_event,
    };
    use crate::job::{load_job_projection, read_job_history};
    use crate::paths::DeadreckonPaths;

    fn at(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("timestamp")
            .with_timezone(&Utc)
    }

    fn owner(name: &str, boot_id: &str, pid: u32) -> LeaseOwner {
        LeaseOwner {
            owner_id: name.to_string(),
            boot_id: boot_id.to_string(),
            pid,
            process_group: pid,
        }
    }

    fn paths(temp: &TempDir) -> DeadreckonPaths {
        DeadreckonPaths::from_home(temp.path())
    }

    #[test]
    fn only_one_supervisor_can_claim_a_job() {
        let temp = TempDir::new().expect("tempdir");
        let paths = Arc::new(paths(&temp));
        let barrier = Arc::new(Barrier::new(3));
        let job_id = JobId("job-race".to_string());
        let now = at("2026-07-29T00:00:00Z");
        let mut workers = Vec::new();

        for index in 1..=2 {
            let paths = Arc::clone(&paths);
            let barrier = Arc::clone(&barrier);
            let job_id = job_id.clone();
            workers.push(thread::spawn(move || {
                barrier.wait();
                claim_job_lease(
                    &paths,
                    &job_id,
                    &owner(&format!("supervisor-{index}"), "boot-a", 4000 + index),
                    now,
                    Duration::from_secs(15),
                )
            }));
        }
        barrier.wait();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker"))
            .collect::<Vec<_>>();

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        let history = read_job_history(&paths.job_events(job_id.as_ref())).expect("history");
        assert_eq!(history.events().len(), 1);
    }

    #[test]
    fn concurrent_fenced_facts_receive_distinct_sequences_under_the_lock() {
        let temp = TempDir::new().expect("tempdir");
        let paths = Arc::new(paths(&temp));
        let job_id = JobId("job-concurrent-facts".to_string());
        let now = at("2026-07-29T00:00:00Z");
        let token = claim_job_lease(
            &paths,
            &job_id,
            &owner("supervisor-a", "boot-a", 4050),
            now,
            Duration::from_secs(15),
        )
        .expect("claim")
        .token();
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();

        for index in 1..=2 {
            let paths = Arc::clone(&paths);
            let barrier = Arc::clone(&barrier);
            let token = token.clone();
            workers.push(thread::spawn(move || {
                barrier.wait();
                append_next_fenced_job_event(
                    &paths,
                    &token,
                    now,
                    JobEventKind::CampaignSubAuthorityChanged,
                    format!("concurrent-fact-{index}"),
                    json!({ "index": index }),
                )
            }));
        }
        barrier.wait();
        for worker in workers {
            worker.join().expect("worker").expect("append fact");
        }

        let history = read_job_history(&paths.job_events(job_id.as_ref())).expect("history");
        assert_eq!(
            history
                .events()
                .iter()
                .map(|event| event.sequence.get())
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn expired_lease_reclaim_increments_epoch() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(&temp);
        let job_id = JobId("job-expiry".to_string());
        let now = at("2026-07-29T00:00:00Z");
        let first = claim_job_lease(
            &paths,
            &job_id,
            &owner("supervisor-a", "boot-a", 4100),
            now,
            Duration::from_secs(15),
        )
        .expect("first claim");
        let second = claim_job_lease(
            &paths,
            &job_id,
            &owner("supervisor-b", "boot-a", 4200),
            now + TimeDelta::seconds(15),
            Duration::from_secs(15),
        )
        .expect("reclaim");

        assert_eq!(first.lease.epoch, 1);
        assert_eq!(second.lease.epoch, 2);
        assert_eq!(
            second.disposition,
            LeaseClaimDisposition::Reclaimed(LeaseReclaimReason::Expired)
        );
    }

    #[test]
    fn stale_worker_cannot_commit_with_old_epoch() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(&temp);
        let job_id = JobId("job-fence".to_string());
        let now = at("2026-07-29T00:00:00Z");
        let stale = claim_job_lease(
            &paths,
            &job_id,
            &owner("supervisor-a", "boot-a", 4300),
            now,
            Duration::from_secs(15),
        )
        .expect("first claim")
        .token();
        claim_job_lease(
            &paths,
            &job_id,
            &owner("supervisor-b", "boot-a", 4400),
            now + TimeDelta::seconds(16),
            Duration::from_secs(15),
        )
        .expect("reclaim");
        let event = JobEvent {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: job_id.clone(),
            sequence: JobEventSequence::new(3).expect("sequence"),
            event_id: "stale-terminal".to_string(),
            causation_id: "stale-terminal".to_string(),
            timestamp: now + TimeDelta::seconds(17),
            lease_epoch: stale.epoch,
            kind: JobEventKind::Verified,
            detail: json!({}),
        };

        let error = append_fenced_job_event(&paths, &stale, now + TimeDelta::seconds(17), &event)
            .expect_err("stale worker must be fenced");
        assert!(error.to_string().contains("stale lease token"));
        let history = read_job_history(&paths.job_events(job_id.as_ref())).expect("history");
        assert_eq!(history.events().len(), 2);
    }

    #[test]
    fn stale_worker_cannot_replace_a_fenced_json_projection() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(&temp);
        let job_id = JobId("job-artifact-fence".to_string());
        let now = at("2026-07-29T00:00:00Z");
        let stale = claim_job_lease(
            &paths,
            &job_id,
            &owner("supervisor-a", "boot-a", 4350),
            now,
            Duration::from_secs(15),
        )
        .expect("first claim")
        .token();
        claim_job_lease(
            &paths,
            &job_id,
            &owner("supervisor-b", "boot-a", 4450),
            now + TimeDelta::seconds(16),
            Duration::from_secs(15),
        )
        .expect("reclaim");
        let artifact = paths.job_dir(job_id.as_ref()).join("authority.json");

        let error = replace_fenced_job_json_and_append_event(
            &paths,
            &stale,
            now + TimeDelta::seconds(17),
            &artifact,
            FencedJobJsonEvent {
                kind: JobEventKind::CampaignSubAuthorityChanged,
                causation_id: "stale-artifact".to_string(),
                detail: json!({"transition": "prepared"}),
            },
            &json!({"authority": "stale"}),
        )
        .expect_err("stale worker must not replace the projection");

        assert!(error.to_string().contains("stale lease token"));
        assert!(!artifact.exists(), "stale projection must not be written");
        let create_error = create_fenced_job_json_and_append_event(
            &paths,
            &stale,
            now + TimeDelta::seconds(17),
            &artifact,
            FencedJobJsonEvent {
                kind: JobEventKind::RepairChildAuthorityChanged,
                causation_id: "stale-create".to_string(),
                detail: json!({"transition": "process_prepared"}),
            },
            &json!({"authority": "stale"}),
        )
        .expect_err("stale worker must not create an authority");
        assert!(create_error.to_string().contains("stale lease token"));
        assert!(!artifact.exists(), "stale authority must not be created");
        let history = read_job_history(&paths.job_events(job_id.as_ref())).expect("history");
        assert_eq!(history.events().len(), 2);
    }

    #[test]
    fn fenced_json_authority_is_created_exactly_once() {
        let temp = TempDir::new().expect("tempdir");
        let paths = Arc::new(paths(&temp));
        let job_id = JobId("job-create-authority".to_string());
        let now = at("2026-07-29T00:00:00Z");
        let token = claim_job_lease(
            &paths,
            &job_id,
            &owner("supervisor-a", "boot-a", 4355),
            now,
            Duration::from_secs(15),
        )
        .expect("claim")
        .token();
        let artifact = paths.job_dir(job_id.as_ref()).join("authority.json");
        let barrier = Arc::new(Barrier::new(3));
        let mut creators = Vec::new();
        for authority in ["first", "second"] {
            let paths = Arc::clone(&paths);
            let token = token.clone();
            let artifact = artifact.clone();
            let barrier = Arc::clone(&barrier);
            creators.push(thread::spawn(move || {
                barrier.wait();
                (
                    authority,
                    create_fenced_job_json_and_append_event(
                        &paths,
                        &token,
                        now + TimeDelta::seconds(1),
                        &artifact,
                        FencedJobJsonEvent {
                            kind: JobEventKind::RepairChildAuthorityChanged,
                            causation_id: format!("create-authority:{authority}"),
                            detail: json!({
                                "transition": "process_prepared",
                                "authority": authority,
                            }),
                        },
                        &json!({"authority": authority}),
                    )
                    .expect("create classification"),
                )
            }));
        }
        barrier.wait();
        let results = creators
            .into_iter()
            .map(|creator| creator.join().expect("creator"))
            .collect::<Vec<_>>();
        assert_eq!(
            results
                .iter()
                .filter(|(_, result)| matches!(result, CreateFencedJobJsonDisposition::Created(_)))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|(_, result)| *result == CreateFencedJobJsonDisposition::AlreadyExists)
                .count(),
            1
        );
        let winner = results
            .iter()
            .find_map(|(authority, result)| {
                matches!(result, CreateFencedJobJsonDisposition::Created(_)).then_some(*authority)
            })
            .expect("winner");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &fs::read(&artifact).expect("authority bytes")
            )
            .expect("authority JSON"),
            json!({"authority": winner})
        );
        let history = read_job_history(&paths.job_events(job_id.as_ref())).expect("history");
        assert_eq!(
            history
                .events()
                .iter()
                .filter(|event| event.kind == JobEventKind::RepairChildAuthorityChanged)
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn fenced_json_create_preserves_existing_files_and_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("tempdir");
        let paths = paths(&temp);
        let job_id = JobId("job-create-noclobber".to_string());
        let now = at("2026-07-29T00:00:00Z");
        let token = claim_job_lease(
            &paths,
            &job_id,
            &owner("supervisor-a", "boot-a", 4356),
            now,
            Duration::from_secs(15),
        )
        .expect("claim")
        .token();
        let event = || FencedJobJsonEvent {
            kind: JobEventKind::RepairChildAuthorityChanged,
            causation_id: "create-noclobber".to_string(),
            detail: json!({"transition": "process_prepared"}),
        };

        let regular = paths.job_dir(job_id.as_ref()).join("regular.json");
        fs::write(&regular, b"operator bytes\n").expect("existing regular file");
        assert_eq!(
            create_fenced_job_json_and_append_event(
                &paths,
                &token,
                now + TimeDelta::seconds(1),
                &regular,
                event(),
                &json!({"authority": "replacement"}),
            )
            .expect("regular classification"),
            CreateFencedJobJsonDisposition::AlreadyExists
        );
        assert_eq!(
            fs::read(&regular).expect("regular bytes"),
            b"operator bytes\n"
        );

        let target = temp.path().join("foreign-target.json");
        fs::write(&target, b"foreign bytes\n").expect("foreign target");
        let link = paths.job_dir(job_id.as_ref()).join("symlink.json");
        symlink(&target, &link).expect("authority symlink");
        assert_eq!(
            create_fenced_job_json_and_append_event(
                &paths,
                &token,
                now + TimeDelta::seconds(2),
                &link,
                event(),
                &json!({"authority": "replacement"}),
            )
            .expect("symlink classification"),
            CreateFencedJobJsonDisposition::AlreadyExists
        );
        assert_eq!(fs::read(&target).expect("target bytes"), b"foreign bytes\n");
        let history = read_job_history(&paths.job_events(job_id.as_ref())).expect("history");
        assert_eq!(
            history.events().len(),
            1,
            "only lease acquisition is durable"
        );
    }

    #[test]
    fn fenced_json_projection_cannot_escape_the_job_directory() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(&temp);
        let job_id = JobId("job-artifact-path".to_string());
        let now = at("2026-07-29T00:00:00Z");
        let token = claim_job_lease(
            &paths,
            &job_id,
            &owner("supervisor-a", "boot-a", 4360),
            now,
            Duration::from_secs(15),
        )
        .expect("claim")
        .token();
        let outside = temp.path().join("outside.json");

        let error = replace_fenced_job_json_and_append_event(
            &paths,
            &token,
            now + TimeDelta::seconds(1),
            &outside,
            FencedJobJsonEvent {
                kind: JobEventKind::CampaignSubAuthorityChanged,
                causation_id: "escaped-artifact".to_string(),
                detail: json!({"transition": "prepared"}),
            },
            &json!({"authority": "escaped"}),
        )
        .expect_err("fenced projection must remain under its Job directory");

        assert!(error.to_string().contains("outside"), "{error}");
        assert!(!outside.exists(), "escaped projection must not be written");
    }

    #[test]
    fn boot_identity_change_reclaims_even_when_pid_is_reused() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(&temp);
        let job_id = JobId("job-boot".to_string());
        let now = at("2026-07-29T00:00:00Z");
        claim_job_lease(
            &paths,
            &job_id,
            &owner("supervisor-a", "boot-a", 4500),
            now,
            Duration::from_secs(15),
        )
        .expect("first claim");
        let reclaimed = claim_job_lease(
            &paths,
            &job_id,
            &owner("supervisor-b", "boot-b", 4500),
            now + TimeDelta::seconds(1),
            Duration::from_secs(15),
        )
        .expect("boot reclaim");

        assert_eq!(reclaimed.lease.epoch, 2);
        assert_eq!(reclaimed.lease.pid, 4500);
        assert_eq!(
            reclaimed.disposition,
            LeaseClaimDisposition::Reclaimed(LeaseReclaimReason::BootIdentityChanged)
        );
    }

    #[test]
    fn heartbeat_renews_without_phase_transition() {
        let temp = TempDir::new().expect("tempdir");
        let paths = paths(&temp);
        let job_id = JobId("job-heartbeat".to_string());
        let now = at("2026-07-29T00:00:00Z");
        let claim = claim_job_lease(
            &paths,
            &job_id,
            &owner("supervisor-a", "boot-a", 4600),
            now,
            Duration::from_secs(15),
        )
        .expect("claim");
        let before = load_job_projection(&paths, job_id.as_ref()).expect("projection");

        let renewed = heartbeat_job_lease(
            &paths,
            &claim.token(),
            now + TimeDelta::seconds(2),
            Duration::from_secs(15),
        )
        .expect("heartbeat");
        let after = load_job_projection(&paths, job_id.as_ref()).expect("projection");

        assert_eq!(after.phase, before.phase);
        assert_eq!(after.last_sequence, before.last_sequence);
        assert_eq!(renewed.heartbeat_at, now + TimeDelta::seconds(2));
        assert_eq!(renewed.expires_at, now + TimeDelta::seconds(17));
    }
}
