use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use deadreckon_protocol::{
    Job, JobEvent, JobEventKind, JobId, JobOutcome, JobPhase, JobSchemaVersion, StopReason,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::campaign::{Campaign, CampaignStatus};
use crate::chain::{Chain, ChainStatus};
use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::paths::DeadreckonPaths;
use crate::plan::{Plan, PlanStatus};
use crate::run_view::RunView;
use crate::state::{PipelineState, RunStatus, atomic_write_json};

pub const JOB_JSON: &str = "job.json";
pub const JOB_EVENTS_JSONL: &str = "job-events.jsonl";
pub const JOB_PROJECTION_JSON: &str = "projection.json";
pub const JOB_CONTROL_LOCK: &str = "control.lock";

/// The latest successful operator delivery folded from the Job event history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobDelivery {
    pub kind: JobDeliveryKind,
    pub destination: PathBuf,
    pub resulting_revision: Option<String>,
    pub delivered_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobDeliveryKind {
    Applied,
    Exported,
}

/// A rebuildable checkpoint over the append-only job lifecycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobProjection {
    pub schema_version: JobSchemaVersion,
    pub job_id: JobId,
    pub phase: JobPhase,
    pub outcome: Option<JobOutcome>,
    pub stop_reason: Option<StopReason>,
    pub last_sequence: u64,
    pub current_lease_epoch: u64,
    pub attempt_count: u32,
    pub child_run_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<JobDelivery>,
    pub updated_at: Option<DateTime<Utc>>,
    pub caveats: Vec<String>,
}

impl JobProjection {
    fn empty(job_id: JobId) -> Self {
        Self {
            schema_version: JobSchemaVersion::CURRENT,
            job_id,
            phase: JobPhase::Queued,
            outcome: None,
            stop_reason: None,
            last_sequence: 0,
            current_lease_epoch: 0,
            attempt_count: 0,
            child_run_ids: Vec::new(),
            delivery: None,
            updated_at: None,
            caveats: Vec::new(),
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.phase == JobPhase::Terminal
    }
}

/// Operator read model: one lifecycle plus the existing evidence-rich attempts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobView {
    pub job: Job,
    pub projection: JobProjection,
    pub attempts: Vec<RunView>,
    pub missing_attempts: Vec<String>,
    /// A terminal lifecycle fact remains immutable, but its external proof may
    /// later be deleted or tampered with. Status surfaces must expose that
    /// distinction instead of continuing to present an unvalidated Verified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_receipt_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyJobKind {
    Run,
    Plan,
    Chain,
    Campaign,
}

/// Honest read-only compatibility projection. It deliberately does not claim
/// a durable Job, lease epoch, or typed causal outcome that the old artifact
/// never recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyJobView {
    pub id: String,
    pub kind: LegacyJobKind,
    pub goal: String,
    pub phase: JobPhase,
    pub outcome: Option<JobOutcome>,
    pub stop_reason: Option<StopReason>,
    pub updated_at: DateTime<Utc>,
    pub precision: String,
}

impl LegacyJobView {
    fn new(
        id: String,
        kind: LegacyJobKind,
        goal: String,
        phase: JobPhase,
        terminal_failure: bool,
        updated_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            kind,
            goal,
            phase,
            outcome: terminal_failure.then_some(JobOutcome::Failed),
            stop_reason: (phase == JobPhase::Terminal || phase == JobPhase::Waiting)
                .then_some(StopReason::LegacyUnknown),
            updated_at,
            precision: "legacy_snapshot".to_string(),
        }
    }
}

pub fn legacy_run_job_view(state: &PipelineState) -> LegacyJobView {
    let (phase, failed) = match state.status {
        RunStatus::Pending | RunStatus::Planned => (JobPhase::Queued, false),
        RunStatus::Executing => (JobPhase::Running, false),
        RunStatus::Completed | RunStatus::Killed => (JobPhase::Terminal, false),
        RunStatus::Failed => (JobPhase::Terminal, true),
    };
    LegacyJobView::new(
        state.run_id.clone(),
        LegacyJobKind::Run,
        state.goal.clone(),
        phase,
        failed,
        state.updated_at,
    )
}

pub fn legacy_plan_job_view(plan: &Plan) -> LegacyJobView {
    let (phase, failed) = match plan.status {
        PlanStatus::Pending => (JobPhase::Queued, false),
        PlanStatus::Forked => (JobPhase::Running, false),
        PlanStatus::Merged => (JobPhase::Terminal, false),
        PlanStatus::Failed => (JobPhase::Terminal, true),
    };
    LegacyJobView::new(
        plan.plan_id.clone(),
        LegacyJobKind::Plan,
        plan.root_goal.clone(),
        phase,
        failed,
        plan.merged_at.or(plan.forked_at).unwrap_or(plan.created_at),
    )
}

pub fn legacy_chain_job_view(chain: &Chain) -> LegacyJobView {
    let (phase, failed) = match chain.status {
        ChainStatus::Pending => (JobPhase::Queued, false),
        ChainStatus::Running => (JobPhase::Running, false),
        ChainStatus::Paused => (JobPhase::Waiting, false),
        ChainStatus::Completed | ChainStatus::Killed | ChainStatus::Undone => {
            (JobPhase::Terminal, false)
        }
        ChainStatus::Failed => (JobPhase::Terminal, true),
    };
    LegacyJobView::new(
        chain.chain_id.clone(),
        LegacyJobKind::Chain,
        chain.root_goal.clone(),
        phase,
        failed,
        chain
            .completed_at
            .or(chain.started_at)
            .unwrap_or(chain.created_at),
    )
}

pub fn legacy_campaign_job_view(campaign: &Campaign) -> LegacyJobView {
    let (phase, failed) = match campaign.status {
        CampaignStatus::Pending => (JobPhase::Queued, false),
        CampaignStatus::Forked => (JobPhase::Running, false),
        CampaignStatus::Merged | CampaignStatus::Killed => (JobPhase::Terminal, false),
        CampaignStatus::Failed => (JobPhase::Terminal, true),
    };
    LegacyJobView::new(
        campaign.campaign_id.clone(),
        LegacyJobKind::Campaign,
        campaign.root_goal.clone(),
        phase,
        failed,
        campaign
            .merged_at
            .or(campaign.forked_at)
            .unwrap_or(campaign.created_at),
    )
}

impl JobView {
    pub fn load(paths: &DeadreckonPaths, job_id: &str) -> Result<Self> {
        let job = load_job(paths, job_id)?;
        let history = read_job_history(&paths.job_events(job_id))?;
        let projection = reduce_job_history(&job.job_id, &history)?;
        let mut attempts = Vec::new();
        let mut missing_attempts = Vec::new();
        for run_id in &projection.child_run_ids {
            match RunView::load(paths, &job.scope, run_id) {
                Ok(view) => attempts.push(view),
                Err(_) => missing_attempts.push(run_id.clone()),
            }
        }
        let verified_receipt_error = if projection.outcome == Some(JobOutcome::Verified) {
            match crate::state::load_run(paths, job_id)
                .and_then(|state| crate::completion::validate_completion_receipt(paths, &state))
            {
                Ok(receipt) => {
                    let terminal = history
                        .events()
                        .iter()
                        .rev()
                        .find(|event| event.kind == JobEventKind::Verified);
                    let receipt_sha256 = crate::flight::sha256_file(&paths.job_receipt(job_id));
                    match (terminal, receipt_sha256) {
                        (Some(terminal), Ok(receipt_sha256))
                            if terminal
                                .detail
                                .get("receipt_sha256")
                                .and_then(Value::as_str)
                                == Some(receipt_sha256.as_str())
                                && terminal.detail.get("attempt").and_then(Value::as_u64)
                                    == Some(u64::from(receipt.attempt))
                                && terminal
                                    .detail
                                    .get("receipt_run_id")
                                    .and_then(Value::as_str)
                                    == Some(receipt.run_id.as_ref())
                                && terminal
                                    .detail
                                    .get("outer_launch_id")
                                    .and_then(Value::as_str)
                                    == Some(receipt.outer_launch_id.as_str())
                                && terminal
                                    .detail
                                    .get("result_run_id")
                                    .and_then(Value::as_str)
                                    .is_some_and(|value| !value.trim().is_empty()) =>
                        {
                            None
                        }
                        (Some(_), Ok(_)) => Some(
                            "terminal Verified fact is not bound to the current signed receipt"
                                .to_string(),
                        ),
                        (None, _) => Some(
                            "terminal projection has no durable Verified lifecycle fact"
                                .to_string(),
                        ),
                        (_, Err(error)) => Some(error.to_string()),
                    }
                }
                Err(error) => Some(error.to_string()),
            }
        } else {
            None
        };
        Ok(Self {
            job,
            projection,
            attempts,
            missing_attempts,
            verified_receipt_error,
        })
    }
}

/// Parsed lifecycle rows plus a bounded account of recoverable read damage.
#[derive(Debug, Clone)]
pub struct JobHistory {
    events: Vec<JobEvent>,
    raw_lines: Vec<Vec<u8>>,
    pub caveats: Vec<String>,
}

impl JobHistory {
    pub fn events(&self) -> &[JobEvent] {
        &self.events
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

/// Write immutable job identity once. A changed replacement is never allowed.
pub fn write_job(paths: &DeadreckonPaths, job: &Job) -> Result<()> {
    let path = paths.job_json(job.job_id.as_ref());
    if path.exists() {
        let existing = load_job(paths, job.job_id.as_ref())?;
        return if existing == *job {
            Ok(())
        } else {
            Err(history_corrupt(
                job.job_id.as_ref(),
                "job.json is immutable and already contains different bytes",
            ))
        };
    }
    atomic_write_json(&path, job)
}

pub fn load_job(paths: &DeadreckonPaths, job_id: &str) -> Result<Job> {
    let path = paths.job_json(job_id);
    let raw = fs::read(&path).with_path(&path)?;
    let job: Job = serde_json::from_slice(&raw).with_json_path(&path)?;
    if job.job_id.as_ref() != job_id {
        return Err(history_corrupt(
            job_id,
            &format!("job.json contains identity {}", job.job_id),
        ));
    }
    Ok(job)
}

/// Read complete JSONL rows. An unterminated final row is a torn append: it is
/// ignored and surfaced as a caveat, while damage to any completed row fails.
pub fn read_job_history(path: &Path) -> Result<JobHistory> {
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let terminated = raw.is_empty() || raw.ends_with(b"\n");
    let complete_len = if terminated {
        raw.len()
    } else {
        raw.iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |index| index + 1)
    };
    let mut events = Vec::new();
    let mut raw_lines = Vec::new();
    for (index, line) in raw[..complete_len]
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .enumerate()
    {
        let event = serde_json::from_slice::<JobEvent>(line).map_err(|source| {
            DeadreckonError::InvalidInput(format!(
                "job history corrupt at {} row {}: {source}; try: deadreckon show <id> --json",
                path.display(),
                index + 1
            ))
        })?;
        events.push(event);
        raw_lines.push(line.to_vec());
    }
    let caveats = (!terminated)
        .then(|| {
            format!(
                "ignored torn final job event row ({} bytes)",
                raw.len().saturating_sub(complete_len)
            )
        })
        .into_iter()
        .collect();
    Ok(JobHistory {
        events,
        raw_lines,
        caveats,
    })
}

/// Fold history in append order. Exact repeated event bytes are idempotent;
/// conflicting IDs, identity changes and sequence gaps are corruption.
pub fn reduce_job_history(job_id: &JobId, history: &JobHistory) -> Result<JobProjection> {
    let mut projection = JobProjection::empty(job_id.clone());
    projection.caveats.clone_from(&history.caveats);
    let mut expected = 1_u64;
    let mut seen = BTreeMap::<&str, &[u8]>::new();
    let mut child_ids = BTreeSet::new();
    let mut undo_operations = BTreeMap::new();

    for (event, raw) in history.events.iter().zip(&history.raw_lines) {
        if event.job_id != *job_id {
            return Err(history_corrupt(
                job_id.as_ref(),
                &format!("event belongs to {}", event.job_id),
            ));
        }
        if let Some(prior) = seen.get(event.event_id.as_str()) {
            if *prior == raw.as_slice() {
                continue;
            }
            return Err(history_corrupt(
                job_id.as_ref(),
                &format!("event id {} has conflicting bytes", event.event_id),
            ));
        }
        if event.sequence.get() != expected {
            return Err(history_corrupt(
                job_id.as_ref(),
                &format!(
                    "expected event sequence {expected}, found {}",
                    event.sequence.get()
                ),
            ));
        }
        seen.insert(event.event_id.as_str(), raw.as_slice());
        expected += 1;
        projection.last_sequence = event.sequence.get();
        projection.updated_at = Some(event.timestamp);
        reduce_event(&mut projection, event, &mut child_ids, &mut undo_operations)?;
    }
    projection.child_run_ids = child_ids.into_iter().collect();
    Ok(projection)
}

/// Append one control fact while holding the per-job lock, then checkpoint the
/// projection. The event is durable before the projection it authorizes.
pub fn append_job_event(paths: &DeadreckonPaths, event: &JobEvent) -> Result<JobProjection> {
    fs::create_dir_all(paths.job_dir(event.job_id.as_ref()))
        .with_path(paths.job_dir(event.job_id.as_ref()))?;
    let lock_path = paths.job_dir(event.job_id.as_ref()).join(JOB_CONTROL_LOCK);
    let lock = open_job_control_lock(&lock_path)?;
    lock.lock_exclusive().with_path(&lock_path)?;
    append_job_event_locked(paths, event)
}

/// Append while the caller holds this job's control lock. Lease operations
/// use this seam so ownership validation and the authorized event are one
/// serialized transaction.
pub(crate) fn append_job_event_locked(
    paths: &DeadreckonPaths,
    event: &JobEvent,
) -> Result<JobProjection> {
    let events_path = paths.job_events(event.job_id.as_ref());
    let history = read_job_history(&events_path)?;
    if !history.caveats.is_empty() {
        return Err(history_corrupt(
            event.job_id.as_ref(),
            "cannot append after a torn final event row",
        ));
    }
    for (existing, raw) in history.events.iter().zip(&history.raw_lines) {
        if existing.event_id != event.event_id {
            continue;
        }
        let candidate = serde_json::to_vec(event).map_err(|source| DeadreckonError::Json {
            path: events_path.clone(),
            source,
        })?;
        if candidate == *raw {
            return reduce_job_history(&event.job_id, &history);
        }
        return Err(history_corrupt(
            event.job_id.as_ref(),
            &format!(
                "event id {} already exists with different bytes",
                event.event_id
            ),
        ));
    }
    let expected = history
        .events
        .last()
        .map_or(1, |prior| prior.sequence.get() + 1);
    if event.sequence.get() != expected {
        return Err(history_corrupt(
            event.job_id.as_ref(),
            &format!(
                "append expected sequence {expected}, received {}",
                event.sequence.get()
            ),
        ));
    }

    append_synced_json_line(&events_path, event)?;
    let rebuilt = read_job_history(&events_path)?;
    let projection = reduce_job_history(&event.job_id, &rebuilt)?;
    atomic_write_json(&paths.job_projection(event.job_id.as_ref()), &projection)?;
    Ok(projection)
}

pub fn rebuild_job_projection(paths: &DeadreckonPaths, job_id: &str) -> Result<JobProjection> {
    let job = load_job(paths, job_id)?;
    let history = read_job_history(&paths.job_events(job_id))?;
    let projection = reduce_job_history(&job.job_id, &history)?;
    atomic_write_json(&paths.job_projection(job_id), &projection)?;
    Ok(projection)
}

pub fn load_job_projection(paths: &DeadreckonPaths, job_id: &str) -> Result<JobProjection> {
    let path = paths.job_projection(job_id);
    let raw = fs::read(&path).with_path(&path)?;
    serde_json::from_slice(&raw).with_json_path(path)
}

pub(crate) fn open_job_control_lock(path: &Path) -> Result<File> {
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .with_path(path)
}

fn append_synced_json_line(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut line = serde_json::to_vec(value).map_err(|source| DeadreckonError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    line.push(b'\n');
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_path(path)?;
    file.write_all(&line).with_path(path)?;
    file.sync_all().with_path(path)
}

fn reduce_event(
    projection: &mut JobProjection,
    event: &JobEvent,
    child_ids: &mut BTreeSet<String>,
    undo_operations: &mut BTreeMap<String, UndoEventState>,
) -> Result<()> {
    use JobEventKind::{
        AttemptStarted, AttemptStopped, Blocked, CancelRequested, Cancelled, ChildLaunchPrepared,
        ChildLinked, Created, DeadlineReached, DeterministicGateFailed, DeterministicGatePassed,
        Failed, LeaseAcquired, LeaseReclaimed, NeedsReview, Queued, RetryScheduled,
        SemanticJudgeAchieved, SemanticJudgeRevise, SemanticJudgeUncertain, Verified,
    };

    if projection.is_terminal()
        && !matches!(
            event.kind,
            JobEventKind::ResultApplied
                | JobEventKind::ResultExported
                | JobEventKind::UndoStarted
                | JobEventKind::UndoCompleted
                | JobEventKind::UndoFailed
        )
    {
        return Err(history_corrupt(
            event.job_id.as_ref(),
            "lifecycle history continued after its terminal outcome",
        ));
    }

    match event.kind {
        Created | Queued => projection.phase = JobPhase::Queued,
        LeaseAcquired | LeaseReclaimed => {
            if event.lease_epoch <= projection.current_lease_epoch {
                return Err(history_corrupt(
                    event.job_id.as_ref(),
                    "lease epoch did not increase",
                ));
            }
            projection.current_lease_epoch = event.lease_epoch;
            projection.phase = JobPhase::Running;
        }
        ChildLaunchPrepared => projection.phase = JobPhase::Running,
        AttemptStarted => {
            projection.phase = JobPhase::Running;
            projection.attempt_count = projection.attempt_count.saturating_add(1);
        }
        ChildLinked => {
            if let Some(run_id) = detail_string(&event.detail, "run_id") {
                child_ids.insert(run_id.to_string());
            }
        }
        AttemptStopped => {
            projection.phase = JobPhase::Waiting;
            let reason = detail_stop_reason(event)?;
            // Operator cancellation is sticky once durably requested. A
            // concurrently exiting worker must not overwrite it with its own
            // completion/failure reason before terminal classification.
            if projection.stop_reason != Some(StopReason::CancelRequested) {
                projection.stop_reason = reason;
            }
        }
        RetryScheduled => {
            projection.phase = JobPhase::Waiting;
            if projection.stop_reason != Some(StopReason::CancelRequested) {
                projection.stop_reason = None;
            }
        }
        DeterministicGatePassed => projection.phase = JobPhase::VerifyingMeaning,
        DeterministicGateFailed => projection.phase = JobPhase::Running,
        SemanticJudgeAchieved | SemanticJudgeUncertain => {
            projection.phase = JobPhase::VerifyingMeaning;
        }
        SemanticJudgeRevise => projection.phase = JobPhase::Running,
        NeedsReview => terminal(
            projection,
            event,
            JobOutcome::NeedsReview,
            detail_stop_reason(event)?.unwrap_or(StopReason::SemanticUncertain),
        )?,
        Blocked => terminal(
            projection,
            event,
            JobOutcome::Blocked,
            detail_stop_reason(event)?.unwrap_or(StopReason::OperatorInputRequired),
        )?,
        JobEventKind::BudgetExhausted => terminal(
            projection,
            event,
            JobOutcome::BudgetExhausted,
            detail_stop_reason(event)?.unwrap_or(StopReason::SpendCap),
        )?,
        DeadlineReached => terminal(
            projection,
            event,
            JobOutcome::DeadlineReached,
            StopReason::Deadline,
        )?,
        CancelRequested => {
            projection.phase = JobPhase::Waiting;
            projection.stop_reason = Some(StopReason::CancelRequested);
        }
        Cancelled => terminal(
            projection,
            event,
            JobOutcome::Cancelled,
            StopReason::CancelRequested,
        )?,
        Failed => {
            let reason = detail_stop_reason(event)?.unwrap_or(StopReason::FatalProvider);
            let outcome = if reason == StopReason::AttemptLimit {
                JobOutcome::RetryExhausted
            } else {
                JobOutcome::Failed
            };
            terminal(projection, event, outcome, reason)?;
        }
        Verified => terminal(
            projection,
            event,
            JobOutcome::Verified,
            StopReason::Verified,
        )?,
        JobEventKind::ResultApplied => {
            if projection.outcome != Some(JobOutcome::Verified) || projection.delivery.is_some() {
                return Err(history_corrupt(
                    event.job_id.as_ref(),
                    "applied delivery must follow one Verified result and may be recorded only once",
                ));
            }
            projection.delivery = Some(job_delivery(event, JobDeliveryKind::Applied)?);
        }
        JobEventKind::ResultExported => {
            if projection.outcome != Some(JobOutcome::Verified) || projection.delivery.is_some() {
                return Err(history_corrupt(
                    event.job_id.as_ref(),
                    "exported delivery must follow one Verified result and may be recorded only once",
                ));
            }
            projection.delivery = Some(job_delivery(event, JobDeliveryKind::Exported)?);
        }
        JobEventKind::ContractApproved
        | JobEventKind::WorkspacePrepared
        | JobEventKind::CampaignSubAuthorityChanged
        | JobEventKind::RepairChildAuthorityChanged => {}
        JobEventKind::UndoStarted | JobEventKind::UndoCompleted | JobEventKind::UndoFailed => {
            reduce_undo_event(projection, event, undo_operations)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UndoEventState {
    Started,
    Completed,
    Failed,
}

fn reduce_undo_event(
    projection: &JobProjection,
    event: &JobEvent,
    operations: &mut BTreeMap<String, UndoEventState>,
) -> Result<()> {
    if projection.outcome != Some(JobOutcome::Verified)
        || projection.delivery.as_ref().map(|delivery| delivery.kind)
            != Some(JobDeliveryKind::Applied)
    {
        return Err(history_corrupt(
            event.job_id.as_ref(),
            "undo lifecycle facts require one previously applied Verified delivery",
        ));
    }
    let operation_id = detail_string(&event.detail, "operation_id")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            history_corrupt(
                event.job_id.as_ref(),
                "undo lifecycle fact omitted its operation_id",
            )
        })?
        .to_string();
    match event.kind {
        JobEventKind::UndoStarted => {
            if operations
                .values()
                .any(|state| matches!(state, UndoEventState::Started | UndoEventState::Completed))
                || operations
                    .insert(operation_id, UndoEventState::Started)
                    .is_some()
            {
                return Err(history_corrupt(
                    event.job_id.as_ref(),
                    "undo lifecycle contains a duplicate, concurrent, or post-completion start",
                ));
            }
        }
        JobEventKind::UndoCompleted | JobEventKind::UndoFailed => {
            if operations.get(&operation_id) != Some(&UndoEventState::Started) {
                return Err(history_corrupt(
                    event.job_id.as_ref(),
                    "undo terminal fact has no unique matching start",
                ));
            }
            if event.kind == JobEventKind::UndoCompleted
                && detail_string(&event.detail, "undo_revision")
                    .is_none_or(|revision| revision.trim().is_empty())
            {
                return Err(history_corrupt(
                    event.job_id.as_ref(),
                    "completed undo omitted its resulting revision",
                ));
            }
            operations.insert(
                operation_id,
                if event.kind == JobEventKind::UndoCompleted {
                    UndoEventState::Completed
                } else {
                    UndoEventState::Failed
                },
            );
        }
        _ => unreachable!("caller filters undo event kinds"),
    }
    Ok(())
}

fn terminal(
    projection: &mut JobProjection,
    event: &JobEvent,
    outcome: JobOutcome,
    reason: StopReason,
) -> Result<()> {
    if !outcome.accepts_stop_reason(reason) {
        return Err(history_corrupt(
            event.job_id.as_ref(),
            &format!(
                "terminal event {:?} cannot classify the Job as {:?} with stop reason {:?}",
                event.kind, outcome, reason
            ),
        ));
    }
    if projection.stop_reason == Some(StopReason::CancelRequested)
        && outcome != JobOutcome::Cancelled
        && !(outcome == JobOutcome::Blocked && reason == StopReason::LostContainment)
    {
        return Err(history_corrupt(
            event.job_id.as_ref(),
            "a durable cancellation request was overwritten by another terminal outcome",
        ));
    }
    projection.phase = JobPhase::Terminal;
    projection.outcome = Some(outcome);
    projection.stop_reason = Some(reason);
    Ok(())
}

fn detail_string<'a>(detail: &'a Value, name: &str) -> Option<&'a str> {
    detail.get(name).and_then(Value::as_str)
}

fn detail_stop_reason(event: &JobEvent) -> Result<Option<StopReason>> {
    let Some(value) = event.detail.get("stop_reason") else {
        return Ok(None);
    };
    serde_json::from_value(value.clone())
        .map(Some)
        .map_err(|_| {
            history_corrupt(
                event.job_id.as_ref(),
                &format!("event {} has an invalid typed stop_reason", event.event_id),
            )
        })
}

fn job_delivery(event: &JobEvent, kind: JobDeliveryKind) -> Result<JobDelivery> {
    let destination = detail_string(&event.detail, "destination").ok_or_else(|| {
        history_corrupt(
            event.job_id.as_ref(),
            "delivery event omitted string destination",
        )
    })?;
    let resulting_revision = match event.detail.get("resulting_revision") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => {
            return Err(history_corrupt(
                event.job_id.as_ref(),
                "delivery event has non-string resulting revision",
            ));
        }
    };
    Ok(JobDelivery {
        kind,
        destination: PathBuf::from(destination),
        resulting_revision,
        delivered_at: event.timestamp,
    })
}

fn history_corrupt(job_id: &str, detail: &str) -> DeadreckonError {
    DeadreckonError::InvalidInput(format!(
        "job history corrupt for {job_id}: {detail}; try: deadreckon show {job_id} --json"
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::Utc;
    use deadreckon_protocol::{
        Job, JobEvent, JobEventKind, JobEventSequence, JobId, JobOutcome, JobPolicy,
        JobSchemaVersion, JobShape, SemanticJudgeMode, StopReason,
    };
    use serde_json::json;
    use tempfile::TempDir;

    use super::{
        JobHistory, JobView, append_job_event, legacy_campaign_job_view, legacy_chain_job_view,
        legacy_plan_job_view, legacy_run_job_view, load_job_projection, read_job_history,
        rebuild_job_projection, reduce_job_history, write_job,
    };
    use crate::campaign::{Campaign, SubGoal, SubGoalStatus};
    use crate::chain::{ApplyMode, ApplyStrategy, BranchPolicy, Chain, ChainNewOptions, OnFail};
    use crate::paths::DeadreckonPaths;
    use crate::plan::{Plan, PlanMode, PlanProviders, PlanRole, PlanTask};
    use crate::state::{RunOptions, create_run};

    fn job() -> Job {
        Job {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId("job-1".to_string()),
            scope: "scope".to_string(),
            goal: "finish it".to_string(),
            shape: JobShape::Single,
            created_at: Utc::now(),
            source_cwd: "/tmp/source".into(),
            launch_plan_sha256: "sha256:launch".to_string(),
            authority_sha256: "sha256:authority".to_string(),
            policy: JobPolicy {
                max_spend_usd: 1.0,
                max_wall_seconds: 60,
                max_attempts: 3,
                deadline: None,
                semantic_judge: SemanticJudgeMode::Required,
                execution: Some(deadreckon_protocol::JobExecutionPolicy::workspace_only(
                    "sandbox-exec",
                )),
            },
        }
    }

    fn event(sequence: u64, event_id: &str, kind: JobEventKind) -> JobEvent {
        JobEvent {
            schema_version: JobSchemaVersion::CURRENT,
            job_id: JobId("job-1".to_string()),
            sequence: JobEventSequence::new(sequence).expect("nonzero sequence"),
            event_id: event_id.to_string(),
            causation_id: event_id.to_string(),
            timestamp: Utc::now(),
            lease_epoch: 0,
            kind,
            detail: json!({}),
        }
    }

    #[test]
    fn job_event_sequence_reduces_deterministically() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("events.jsonl");
        let first = event(1, "created", JobEventKind::Created);
        let second = event(2, "queued", JobEventKind::Queued);
        fs::write(
            &path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&first).expect("first"),
                serde_json::to_string(&second).expect("second")
            ),
        )
        .expect("write");
        let history = read_job_history(&path).expect("history");

        let left = reduce_job_history(&first.job_id, &history).expect("projection");
        let right = reduce_job_history(&first.job_id, &history).expect("projection");
        assert_eq!(left, right);
        assert_eq!(left.last_sequence, 2);
    }

    #[test]
    fn child_launch_preparation_does_not_consume_a_logical_attempt() {
        let events = vec![
            event(1, "created", JobEventKind::Created),
            event(2, "prepared", JobEventKind::ChildLaunchPrepared),
            event(3, "started", JobEventKind::AttemptStarted),
        ];
        let history = JobHistory {
            raw_lines: events
                .iter()
                .map(|event| serde_json::to_vec(event).expect("serialize event"))
                .collect(),
            events,
            caveats: Vec::new(),
        };

        let projection = reduce_job_history(&JobId("job-1".into()), &history).expect("projection");
        assert_eq!(projection.attempt_count, 1);
        assert_eq!(projection.phase, deadreckon_protocol::JobPhase::Running);
    }

    #[test]
    fn attempt_limit_projects_retry_exhausted_not_generic_failure() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("events.jsonl");
        let created = event(1, "created", JobEventKind::Created);
        let mut failed = event(2, "failed", JobEventKind::Failed);
        failed.detail = json!({ "stop_reason": StopReason::AttemptLimit });
        fs::write(
            &path,
            format!(
                "{}\n{}\n",
                serde_json::to_string(&created).expect("created"),
                serde_json::to_string(&failed).expect("failed")
            ),
        )
        .expect("write");
        let history = read_job_history(&path).expect("history");

        let projection = reduce_job_history(&JobId("job-1".into()), &history).expect("projection");
        assert_eq!(projection.outcome, Some(JobOutcome::RetryExhausted));
        assert_eq!(projection.stop_reason, Some(StopReason::AttemptLimit));
    }

    #[test]
    fn delivery_event_projects_factual_after_state_without_changing_verdict() {
        let created = event(1, "created", JobEventKind::Created);
        let verified = event(2, "verified", JobEventKind::Verified);
        let mut delivered = event(3, "delivered", JobEventKind::ResultApplied);
        delivered.detail = json!({
            "destination": "/tmp/delivered",
            "resulting_revision": "abc123",
        });
        let events = vec![created, verified, delivered.clone()];
        let raw_lines = events
            .iter()
            .map(|event| serde_json::to_vec(event).expect("serialize event"))
            .collect();
        let history = JobHistory {
            events,
            raw_lines,
            caveats: Vec::new(),
        };

        let projection = reduce_job_history(&JobId("job-1".into()), &history).expect("projection");

        assert_eq!(projection.outcome, Some(JobOutcome::Verified));
        assert_eq!(projection.stop_reason, Some(StopReason::Verified));
        let delivery = projection.delivery.expect("delivery");
        assert_eq!(delivery.kind, super::JobDeliveryKind::Applied);
        assert_eq!(
            delivery.destination,
            std::path::PathBuf::from("/tmp/delivered")
        );
        assert_eq!(delivery.resulting_revision.as_deref(), Some("abc123"));
        assert_eq!(delivery.delivered_at, delivered.timestamp);
    }

    #[test]
    fn malformed_delivery_event_fails_projection_closed() {
        let created = event(1, "created", JobEventKind::Created);
        let verified = event(2, "verified", JobEventKind::Verified);
        let delivered = event(3, "delivered", JobEventKind::ResultExported);
        let events = vec![created, verified, delivered];
        let raw_lines = events
            .iter()
            .map(|event| serde_json::to_vec(event).expect("serialize event"))
            .collect();
        let history = JobHistory {
            events,
            raw_lines,
            caveats: Vec::new(),
        };

        let error = reduce_job_history(&JobId("job-1".into()), &history)
            .expect_err("delivery without destination must fail closed")
            .to_string();

        assert!(error.contains("delivery event omitted string destination"));
    }

    #[test]
    fn spend_wall_deadline_retry_cancel_blocked_needs_review_are_distinct() {
        let cases = [
            (
                "spend",
                JobEventKind::BudgetExhausted,
                Some(StopReason::SpendCap),
                JobOutcome::BudgetExhausted,
                StopReason::SpendCap,
            ),
            (
                "wall",
                JobEventKind::BudgetExhausted,
                Some(StopReason::WallCap),
                JobOutcome::BudgetExhausted,
                StopReason::WallCap,
            ),
            (
                "deadline",
                JobEventKind::DeadlineReached,
                None,
                JobOutcome::DeadlineReached,
                StopReason::Deadline,
            ),
            (
                "retry",
                JobEventKind::Failed,
                Some(StopReason::AttemptLimit),
                JobOutcome::RetryExhausted,
                StopReason::AttemptLimit,
            ),
            (
                "cancel",
                JobEventKind::Cancelled,
                None,
                JobOutcome::Cancelled,
                StopReason::CancelRequested,
            ),
            (
                "blocked",
                JobEventKind::Blocked,
                Some(StopReason::OperatorInputRequired),
                JobOutcome::Blocked,
                StopReason::OperatorInputRequired,
            ),
            (
                "needs-review",
                JobEventKind::NeedsReview,
                Some(StopReason::SemanticUnavailable),
                JobOutcome::NeedsReview,
                StopReason::SemanticUnavailable,
            ),
            (
                "supervisor-failure",
                JobEventKind::Failed,
                Some(StopReason::SupervisorFailure),
                JobOutcome::Failed,
                StopReason::SupervisorFailure,
            ),
        ];

        let mut terminal_classifications = std::collections::HashSet::new();
        for (name, kind, detail_reason, expected_outcome, expected_reason) in cases {
            let created = event(1, &format!("{name}-created"), JobEventKind::Created);
            let mut stopped = event(2, &format!("{name}-stopped"), kind);
            if let Some(reason) = detail_reason {
                stopped.detail = json!({ "stop_reason": reason });
            }
            let events = vec![created, stopped];
            let raw_lines = events
                .iter()
                .map(|event| serde_json::to_vec(event).expect("serialize event"))
                .collect();
            let history = JobHistory {
                events,
                raw_lines,
                caveats: Vec::new(),
            };

            let projection =
                reduce_job_history(&JobId("job-1".into()), &history).expect("projection");
            assert_eq!(projection.phase, deadreckon_protocol::JobPhase::Terminal);
            assert_eq!(projection.outcome, Some(expected_outcome), "{name}");
            assert_eq!(projection.stop_reason, Some(expected_reason), "{name}");
            assert!(
                terminal_classifications.insert((expected_outcome, expected_reason)),
                "{name} reused an existing outcome and stop-reason classification"
            );
        }
    }

    #[test]
    fn incompatible_terminal_outcome_and_stop_reason_pairs_fail_closed() {
        let cases = [
            (
                "corrupt-is-not-blocked",
                JobEventKind::Blocked,
                StopReason::CorruptHistory,
            ),
            (
                "lost-containment-is-not-failed",
                JobEventKind::Failed,
                StopReason::LostContainment,
            ),
            (
                "supervisor-failure-is-not-blocked",
                JobEventKind::Blocked,
                StopReason::SupervisorFailure,
            ),
            (
                "deadline-is-not-budget",
                JobEventKind::BudgetExhausted,
                StopReason::Deadline,
            ),
            (
                "gate-failure-is-not-review",
                JobEventKind::NeedsReview,
                StopReason::FatalGate,
            ),
        ];

        for (name, kind, reason) in cases {
            let created = event(1, &format!("{name}-created"), JobEventKind::Created);
            let mut stopped = event(2, &format!("{name}-stopped"), kind);
            stopped.detail = json!({ "stop_reason": reason });
            let events = vec![created, stopped];
            let history = JobHistory {
                raw_lines: events
                    .iter()
                    .map(|event| serde_json::to_vec(event).expect("serialize event"))
                    .collect(),
                events,
                caveats: Vec::new(),
            };

            let error = reduce_job_history(&JobId("job-1".into()), &history)
                .expect_err("incompatible terminal pair must fail closed")
                .to_string();
            assert!(error.contains("cannot classify"), "{name}: {error}");
        }
    }

    #[test]
    fn malformed_typed_stop_reason_fails_closed() {
        let created = event(1, "created", JobEventKind::Created);
        let mut failed = event(2, "failed", JobEventKind::Failed);
        failed.detail = json!({ "stop_reason": "definitely_not_a_reason" });
        let events = vec![created, failed];
        let history = JobHistory {
            raw_lines: events
                .iter()
                .map(|event| serde_json::to_vec(event).expect("serialize event"))
                .collect(),
            events,
            caveats: Vec::new(),
        };

        let error = reduce_job_history(&JobId("job-1".into()), &history)
            .expect_err("malformed stop reason must fail closed")
            .to_string();
        assert!(error.contains("invalid typed stop_reason"), "{error}");
    }

    #[test]
    fn cancellation_remains_sticky_across_a_racing_verified_attempt_stop() {
        let created = event(1, "created", JobEventKind::Created);
        let cancelled_requested = event(2, "cancel-requested", JobEventKind::CancelRequested);
        let mut stopped = event(3, "stopped", JobEventKind::AttemptStopped);
        stopped.detail = json!({ "stop_reason": StopReason::Verified });
        let cancelled = event(4, "cancelled", JobEventKind::Cancelled);
        let events = vec![created, cancelled_requested, stopped, cancelled];
        let history = JobHistory {
            raw_lines: events
                .iter()
                .map(|event| serde_json::to_vec(event).expect("serialize event"))
                .collect(),
            events,
            caveats: Vec::new(),
        };

        let projection = reduce_job_history(&JobId("job-1".into()), &history).expect("cancel wins");
        assert_eq!(projection.outcome, Some(JobOutcome::Cancelled));
        assert_eq!(projection.stop_reason, Some(StopReason::CancelRequested));
    }

    #[test]
    fn terminal_verified_cannot_overwrite_a_durable_cancellation_request() {
        let events = vec![
            event(1, "created", JobEventKind::Created),
            event(2, "cancel-requested", JobEventKind::CancelRequested),
            event(3, "verified", JobEventKind::Verified),
        ];
        let history = JobHistory {
            raw_lines: events
                .iter()
                .map(|event| serde_json::to_vec(event).expect("serialize event"))
                .collect(),
            events,
            caveats: Vec::new(),
        };

        let error = reduce_job_history(&JobId("job-1".into()), &history)
            .expect_err("verified must not overwrite cancellation")
            .to_string();
        assert!(
            error.contains("cancellation request was overwritten"),
            "{error}"
        );
    }

    #[test]
    fn duplicate_event_id_is_idempotent_only_when_bytes_match() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("events.jsonl");
        let first =
            serde_json::to_string(&event(1, "same", JobEventKind::Created)).expect("serialize");
        fs::write(&path, format!("{first}\n{first}\n")).expect("write");
        let history = read_job_history(&path).expect("history");
        assert_eq!(
            reduce_job_history(&JobId("job-1".into()), &history)
                .expect("exact replay")
                .last_sequence,
            1
        );

        let conflicting =
            serde_json::to_string(&event(2, "same", JobEventKind::Queued)).expect("serialize");
        fs::write(&path, format!("{first}\n{conflicting}\n")).expect("write conflict");
        let history = read_job_history(&path).expect("history");
        assert!(reduce_job_history(&JobId("job-1".into()), &history).is_err());
    }

    #[test]
    fn event_gap_is_reported_not_hidden() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("events.jsonl");
        let gap = serde_json::to_string(&event(2, "gap", JobEventKind::Queued)).expect("serialize");
        fs::write(&path, format!("{gap}\n")).expect("write");
        let history = read_job_history(&path).expect("history");
        let error = reduce_job_history(&JobId("job-1".into()), &history)
            .expect_err("gap must be corruption")
            .to_string();
        assert!(error.contains("expected event sequence 1, found 2"));
    }

    #[test]
    fn partial_final_event_is_ignored_with_a_caveat() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("events.jsonl");
        let first =
            serde_json::to_string(&event(1, "created", JobEventKind::Created)).expect("serialize");
        fs::write(&path, format!("{first}\n{{\"schema_version\":")).expect("write torn row");
        let history = read_job_history(&path).expect("history");
        assert_eq!(history.events().len(), 1);
        assert_eq!(history.caveats.len(), 1);
    }

    #[test]
    fn job_projection_rebuild_matches_saved_projection() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        let job = job();
        write_job(&paths, &job).expect("job");
        append_job_event(&paths, &event(1, "created", JobEventKind::Created)).expect("created");
        append_job_event(&paths, &event(2, "queued", JobEventKind::Queued)).expect("queued");
        let saved = load_job_projection(&paths, "job-1").expect("saved projection");
        let rebuilt = rebuild_job_projection(&paths, "job-1").expect("rebuilt projection");
        assert_eq!(saved, rebuilt);
    }

    #[test]
    fn job_view_run_facts_match_run_view() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        let state = create_run(
            &paths,
            RunOptions {
                goal: "attempt".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "deadreckon".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: Some(60.0),
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        let mut identity = job();
        identity.scope.clone_from(&state.scope);
        write_job(&paths, &identity).expect("job");
        append_job_event(&paths, &event(1, "created", JobEventKind::Created)).expect("created");
        let mut linked = event(2, "linked", JobEventKind::ChildLinked);
        linked.detail = json!({ "run_id": state.run_id });
        append_job_event(&paths, &linked).expect("linked");

        let view = JobView::load(&paths, "job-1").expect("job view");
        let direct = crate::RunView::load(&paths, &state.scope, &state.run_id).expect("run view");
        assert_eq!(view.attempts, vec![direct]);
        assert!(view.missing_attempts.is_empty());
    }

    #[test]
    fn legacy_run_plan_chain_campaign_keep_their_ids_without_writes() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path());
        let state = create_run(
            &paths,
            RunOptions {
                goal: "legacy run".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "deadreckon".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("legacy-run".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        let plan = Plan::new(
            "legacy plan",
            PlanMode::FullPlan,
            vec![
                PlanTask::new(0, "first task", "do first", PlanRole::Child, None),
                PlanTask::new(1, "second task", "do second", PlanRole::Child, None),
            ],
            PlanProviders::default(),
            Some(state.scope.clone()),
            "test",
        )
        .expect("plan");
        let chain = Chain::new(ChainNewOptions {
            root_goal: "legacy chain".to_string(),
            goals: vec!["first".to_string(), "second".to_string()],
            scope: state.scope.clone(),
            base_branch: "main".to_string(),
            base_sha: "abc".to_string(),
            cwd: temp.path().to_path_buf(),
            provider: None,
            model: None,
            sandbox: "none".to_string(),
            branch_policy: BranchPolicy::Stack,
            apply_mode: ApplyMode::Manual,
            apply_strategy: ApplyStrategy::Squash,
            apply_allowlist: Vec::new(),
            on_fail: OnFail::Stop,
            circuit_breaker_threshold: 2,
            max_spend_usd: None,
            max_wall_seconds: None,
            deadreckon_version: "test".to_string(),
        })
        .expect("chain");
        let campaign = Campaign::new(
            "legacy campaign",
            vec![
                SubGoal {
                    sub_id: "sub-1".to_string(),
                    goal: "one".to_string(),
                    task_key: "one".to_string(),
                    sub_plan_id: None,
                    result_run_id: None,
                    scope: None,
                    status: SubGoalStatus::Pending,
                },
                SubGoal {
                    sub_id: "sub-2".to_string(),
                    goal: "two".to_string(),
                    task_key: "two".to_string(),
                    sub_plan_id: None,
                    result_run_id: None,
                    scope: None,
                    status: SubGoalStatus::Pending,
                },
            ],
            PlanProviders::default(),
            0,
            None,
            None,
            "test",
        )
        .expect("campaign");

        let views = [
            legacy_run_job_view(&state),
            legacy_plan_job_view(&plan),
            legacy_chain_job_view(&chain),
            legacy_campaign_job_view(&campaign),
        ];
        assert_eq!(
            views.map(|view| view.id),
            [
                state.run_id,
                plan.plan_id,
                chain.chain_id,
                campaign.campaign_id,
            ]
        );
        assert!(
            !paths.jobs_dir().exists(),
            "read-only adapters must not invent durable job artifacts"
        );
    }
}
