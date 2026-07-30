use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::glossary::{plan_status_label, plan_task_status_label};
use crate::paths::{DeadreckonPaths, sanitize_slug};
use crate::state::{append_json_line, atomic_write_json};

pub const PLAN_JSON: &str = "plan.json";
pub const COORDINATOR_JSON: &str = "coordinator.json";
pub const PLAN_MESSAGES_JSONL: &str = "messages.jsonl";
pub const PLAN_EVENTS_JSONL: &str = "plan-events.jsonl";
pub const PLAN_DOCS_DIR: &str = "docs";
pub const PLAN_NARRATIVE: &str = "PLAN-NARRATIVE.md";
pub const WORKER_SPECS_DIR: &str = "worker-specs";
pub const SUMMARIES_DIR: &str = "summaries";
pub const PLAN_CHILD_PARENT_JSON: &str = ".deadreckon/parent.json";

/// Schema 2 adds the execution-policy fields (`apply`, `on_fail`,
/// `max_attempts`, the apply/branch settings) and per-node `subplan` /
/// `attempts`. Every addition is `#[serde(default)]`, so schema-1 files load
/// unchanged and no migration step is required.
pub const PLAN_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanMode {
    FullPlan,
    Review,
}

/// Immutable accounting for the provider call that created a root Plan or
/// Campaign decomposition.
///
/// This lives in the root artifact as well as the richer accounting sidecar so
/// a crash after the artifact write cannot make recovery silently forget
/// planner spend or wall time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RootPlannerAccounting {
    pub schema_version: u32,
    pub planner_invoked: bool,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub subscription: bool,
    pub wall_seconds: f64,
    pub recorded_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetDimension {
    Spend,
    Wall,
}

impl PlanMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FullPlan => "full-plan",
            Self::Review => "review",
        }
    }
}

/// When a plan's node results reach the operator's branch.
///
/// `AtEnd` is the historical behavior: every node runs, then `merge` composes
/// one result. `PerNode` lands each node as its gate passes, so later nodes
/// build on earlier ones — the execution model `chain` owns today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ApplyWhen {
    #[default]
    AtEnd,
    PerNode,
}

impl ApplyWhen {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AtEnd => "at-end",
            Self::PerNode => "per-node",
        }
    }
}

/// Which ref a per-node apply builds on. Owned here rather than in `chain`
/// because a plan is the execution unit; `chain` re-exports for compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BranchPolicy {
    #[default]
    Stack,
    Base,
    Merge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApplyMode {
    #[default]
    Auto,
    Preview,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ApplyStrategy {
    #[default]
    Squash,
    Merge,
    CherryPick,
}

/// What a plan does with the remaining graph once a node has exhausted its
/// attempts. `Skip` is the plan default: an unattended run should keep the
/// independent work moving rather than pause for an operator who walked away.
/// `chain` sets `Stop` explicitly to preserve its historical semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnFail {
    Stop,
    #[default]
    Skip,
    Continue,
}

/// Default retry budget for a node that fails its done contract. The first
/// run plus two retries, each seeded with the gate's complaint.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 3;

/// Default consecutive-node-failure count that halts a plan outright.
pub const DEFAULT_CIRCUIT_BREAKER_THRESHOLD: u32 = 2;

/// The hard ceiling on subplan nesting. A top-level plan sits at depth 0; a
/// plan reached through one `PlanTask::subplan` hop sits at depth 1. Mirrors
/// the guard `campaign::guard_campaign_depth` enforces today.
pub const MAX_SUBPLAN_DEPTH: u32 = 2;

fn default_max_attempts() -> u32 {
    DEFAULT_MAX_ATTEMPTS
}

fn default_circuit_breaker_threshold() -> u32 {
    DEFAULT_CIRCUIT_BREAKER_THRESHOLD
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    Forked,
    Merged,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanTaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Killed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanRole {
    Child,
    Coder,
    Reviewer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkCapability {
    Deny,
    Allowlist,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityPreview {
    pub network: NetworkCapability,
    pub deploy: bool,
    pub global_install: bool,
    pub filesystem: Vec<String>,
    pub notes: Vec<String>,
}

impl Default for CapabilityPreview {
    fn default() -> Self {
        Self {
            network: NetworkCapability::Deny,
            deploy: false,
            global_install: false,
            filesystem: Vec::new(),
            notes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PlanProviders {
    pub planner: Option<String>,
    pub default_child: Option<String>,
    pub coder: Option<String>,
    pub reviewer: Option<String>,
    #[serde(default)]
    pub children: BTreeMap<u32, String>,
    // Per-role models (additive; pre-rider plan.json deserializes with all
    // of these empty). None means the provider's own default — no --model
    // argument reaches the child.
    #[serde(default)]
    pub planner_model: Option<String>,
    #[serde(default)]
    pub default_child_model: Option<String>,
    #[serde(default)]
    pub coder_model: Option<String>,
    #[serde(default)]
    pub reviewer_model: Option<String>,
    #[serde(default)]
    pub child_models: BTreeMap<u32, String>,
}

/// One execution of a node. `child_run_id` records the run that counts (the
/// last attempt); this records every attempt, so `status` can report what a
/// self-healing plan actually tried and what each attempt cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskAttempt {
    /// 1-based. Attempt 1 is the original run; 2 and beyond are retries
    /// seeded with the previous attempt's gate failure.
    pub attempt: u32,
    /// The run this attempt produced. `None` when the child process never got
    /// far enough to create one — a spawn or source-preparation failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub status: PlanTaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_reason: Option<String>,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub spend_usd: f64,
}

impl TaskAttempt {
    pub fn new(attempt: u32, run_id: Option<String>) -> Self {
        Self {
            attempt,
            run_id,
            status: PlanTaskStatus::Running,
            failure_reason: None,
            started_at: Utc::now(),
            finished_at: None,
            spend_usd: 0.0,
        }
    }

    /// A finished attempt that did not satisfy the done contract.
    ///
    /// Stamps both endpoints with the recording time. When the attempt has a
    /// real run behind it, the caller overrides `started_at`/`finished_at`
    /// from the run's own state — otherwise durations read as zero, which
    /// deliberately under-counts (see `attempts_wall_seconds`).
    pub fn failed(
        attempt: u32,
        run_id: Option<String>,
        failure_reason: Option<String>,
        spend_usd: f64,
    ) -> Self {
        Self {
            attempt,
            run_id,
            status: PlanTaskStatus::Failed,
            failure_reason,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            spend_usd,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanTask {
    pub index: u32,
    pub task_id: String,
    pub subject: String,
    pub goal: String,
    pub active_form: String,
    pub provider: Option<String>,
    pub role: PlanRole,
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// When set, this node is executed as its own plan rather than as a single
    /// run: the value is a `plan_id` whose merged result becomes this node's
    /// result. The mechanism `campaign` reaches for today, without the
    /// separate id space or the hardcoded parallel sub-shape.
    #[serde(default)]
    pub subplan: Option<String>,
    /// Every run this node has taken, oldest first. Empty on plans written
    /// before self-healing; `child_run_id` remains the authoritative pointer
    /// to the run whose result counts.
    #[serde(default)]
    pub attempts: Vec<TaskAttempt>,
    pub worker_spec: PathBuf,
    pub summary_path: Option<PathBuf>,
    pub review_status: Option<String>,
    pub child_run_id: Option<String>,
    pub child_scope: Option<String>,
    pub status: PlanTaskStatus,
}

impl PlanTask {
    pub fn new(
        index: u32,
        subject: impl Into<String>,
        goal: impl Into<String>,
        role: PlanRole,
        provider: Option<String>,
    ) -> Self {
        let task_id = format!("task-{index}");
        let subject = subject.into();
        let active_form = if subject.trim().is_empty() {
            format!("Running child {index}")
        } else {
            subject.clone()
        };
        Self {
            index,
            task_id: task_id.clone(),
            subject,
            goal: goal.into(),
            active_form,
            provider,
            role,
            depends_on: Vec::new(),
            subplan: None,
            attempts: Vec::new(),
            worker_spec: PathBuf::from(format!("{WORKER_SPECS_DIR}/{task_id}.md")),
            summary_path: None,
            review_status: None,
            child_run_id: None,
            child_scope: None,
            status: PlanTaskStatus::Pending,
        }
    }

    /// Runs this node has already taken, including the first.
    pub fn attempts_used(&self) -> u32 {
        self.attempts.len() as u32
    }

    /// Whether this node has budget left for another run under `max_attempts`.
    pub fn may_retry(&self, max_attempts: u32) -> bool {
        self.attempts_used() < max_attempts
    }

    /// The run a retry should continue from: the most recent failed attempt
    /// that produced one. A retry extends that run so the agent keeps its
    /// working tree and context instead of starting the node over.
    pub fn retry_parent_run_id(&self) -> Option<&str> {
        let last = self.attempts.last()?;
        if last.status != PlanTaskStatus::Failed {
            return None;
        }
        last.run_id.as_deref()
    }

    /// Why the most recent attempt failed — the text a retry is seeded with.
    pub fn last_failure_reason(&self) -> Option<&str> {
        self.attempts.last()?.failure_reason.as_deref()
    }

    /// Everything this node has spent across all of its attempts.
    pub fn attempts_spend_usd(&self) -> f64 {
        self.attempts.iter().map(|attempt| attempt.spend_usd).sum()
    }

    /// Wall-clock seconds this node has consumed across all attempts, from
    /// the attempts that recorded both endpoints. An attempt missing either
    /// stamp contributes zero — undercounting keeps a retry possible, while
    /// overcounting would refuse retries on missing data.
    pub fn attempts_wall_seconds(&self) -> f64 {
        self.attempts
            .iter()
            .filter_map(|attempt| {
                let finished = attempt.finished_at?;
                let elapsed = (finished - attempt.started_at).num_milliseconds();
                (elapsed > 0).then_some(elapsed as f64 / 1000.0)
            })
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub schema_version: u32,
    pub plan_id: String,
    pub root_goal: String,
    pub mode: PlanMode,
    pub n: u32,
    pub providers: PlanProviders,
    pub capability_preview: CapabilityPreview,
    pub tasks: Vec<PlanTask>,
    pub parent_scope: Option<String>,
    #[serde(default)]
    pub parent_cwd: Option<PathBuf>,
    #[serde(default)]
    pub acceptance_path: Option<PathBuf>,
    // --- Execution policy (schema 2) --------------------------------------
    // Every field below defaults to the pre-schema-2 behavior, so a plan.json
    // written by an older binary deserializes unchanged.
    /// When node results reach the branch. `AtEnd` preserves today's
    /// run-everything-then-merge behavior.
    #[serde(default)]
    pub apply: ApplyWhen,
    #[serde(default)]
    pub branch_policy: BranchPolicy,
    #[serde(default)]
    pub apply_strategy: ApplyStrategy,
    #[serde(default)]
    pub apply_allowlist: Vec<String>,
    /// What happens to the rest of the graph once a node is out of attempts.
    #[serde(default)]
    pub on_fail: OnFail,
    /// Total runs a node may take, including the first. See
    /// [`DEFAULT_MAX_ATTEMPTS`].
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    /// Consecutive node failures that halt the plan. 0 disables the breaker.
    #[serde(default = "default_circuit_breaker_threshold")]
    pub circuit_breaker_threshold: u32,
    /// Set when this plan is a node's subplan. The lineage this forms is what
    /// bounds nesting depth; see [`MAX_SUBPLAN_DEPTH`].
    #[serde(default)]
    pub parent_plan_id: Option<String>,
    /// Durable Job that exclusively owns this Plan's executable lifecycle.
    ///
    /// `parent_plan_id` describes graph topology; this field is the authority
    /// boundary shared by a root Plan and every descendant. Older standalone
    /// Plans omit it and remain unowned compatibility artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_job_id: Option<String>,
    /// Crash-safe copy of root-planner usage. The separate accounting sidecar
    /// remains the reporting format; this copy lets the supervisor reconstruct
    /// it without inventing zero usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_planner_accounting: Option<RootPlannerAccounting>,
    /// The process supervising this plan's fork, while one is. A Forked plan
    /// whose conductor is dead is resumable; one whose conductor is alive is
    /// not — the same liveness model chain's conductor uses.
    #[serde(default)]
    pub conductor_pid: Option<u32>,
    pub status: PlanStatus,
    pub created_at: DateTime<Utc>,
    pub forked_at: Option<DateTime<Utc>>,
    pub merged_at: Option<DateTime<Utc>>,
    pub merged_run_id: Option<String>,
    pub deadreckon_version: String,
}

impl Plan {
    pub fn new(
        root_goal: impl Into<String>,
        mode: PlanMode,
        tasks: Vec<PlanTask>,
        providers: PlanProviders,
        parent_scope: Option<String>,
        deadreckon_version: impl Into<String>,
    ) -> Result<Self> {
        validate_task_graph(&tasks)?;
        Ok(Self {
            schema_version: PLAN_SCHEMA_VERSION,
            plan_id: Uuid::new_v4().simple().to_string(),
            root_goal: root_goal.into(),
            mode,
            n: tasks.len() as u32,
            providers,
            capability_preview: CapabilityPreview::default(),
            tasks,
            parent_scope,
            parent_cwd: None,
            acceptance_path: None,
            apply: ApplyWhen::default(),
            branch_policy: BranchPolicy::default(),
            apply_strategy: ApplyStrategy::default(),
            apply_allowlist: Vec::new(),
            on_fail: OnFail::default(),
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            circuit_breaker_threshold: DEFAULT_CIRCUIT_BREAKER_THRESHOLD,
            parent_plan_id: None,
            owner_job_id: None,
            root_planner_accounting: None,
            conductor_pid: None,
            status: PlanStatus::Pending,
            created_at: Utc::now(),
            forked_at: None,
            merged_at: None,
            merged_run_id: None,
            deadreckon_version: deadreckon_version.into(),
        })
    }

    pub fn task_by_id(&self, task_id: &str) -> Option<&PlanTask> {
        self.tasks.iter().find(|task| task.task_id == task_id)
    }

    pub fn task_by_id_mut(&mut self, task_id: &str) -> Option<&mut PlanTask> {
        self.tasks.iter_mut().find(|task| task.task_id == task_id)
    }

    pub fn ready_pending_task_indices(&self) -> Vec<usize> {
        self.ready_pending_task_indices_for(self.on_fail)
    }

    /// Readiness under a failure policy.
    ///
    /// Under `Stop` and `Skip`, a node waits for every dependency to complete —
    /// a dependent of a failed node stays blocked, which is the safe default.
    /// `Continue` is the operator saying the remaining work does not need the
    /// failed node's output, so a terminally-failed dependency stops blocking.
    pub fn ready_pending_task_indices_for(&self, on_fail: OnFail) -> Vec<usize> {
        let satisfied = self
            .tasks
            .iter()
            .filter(|task| {
                task.status == PlanTaskStatus::Completed
                    || (on_fail == OnFail::Continue
                        && matches!(task.status, PlanTaskStatus::Failed | PlanTaskStatus::Killed))
            })
            .map(|task| task.task_id.as_str())
            .collect::<BTreeSet<_>>();
        self.tasks
            .iter()
            .enumerate()
            .filter(|(_, task)| {
                task.status == PlanTaskStatus::Pending
                    && task
                        .depends_on
                        .iter()
                        .all(|dependency| satisfied.contains(dependency.as_str()))
            })
            .map(|(index, _)| index)
            .collect()
    }

    /// Total spend recorded across every attempt of every node.
    pub fn attempts_spend_usd(&self) -> f64 {
        self.tasks
            .iter()
            .map(|task| task.attempts_spend_usd())
            .sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanMessageKind {
    Progress,
    Blocker,
    ReviewRequest,
    ReviewResponse,
    CapabilityRequest,
    ShutdownRequest,
    ShutdownResponse,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanMessage {
    pub schema_version: u32,
    pub ts: DateTime<Utc>,
    pub request_id: Option<String>,
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub kind: PlanMessageKind,
    pub summary: String,
    #[serde(default)]
    pub body: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanEventKind {
    PlanCreated {
        mode: PlanMode,
        task_count: usize,
    },
    PlanStarted,
    TaskReady {
        task_id: String,
        task_index: usize,
    },
    TaskStarted {
        task_id: String,
        task_index: usize,
    },
    TaskRunDiscovered {
        task_id: String,
        task_index: usize,
        run_id: Option<String>,
        pid: Option<u32>,
    },
    TaskCompleted {
        task_id: String,
        task_index: usize,
        run_id: Option<String>,
        status: String,
    },
    TaskBlocked {
        task_id: String,
        task_index: usize,
        reason: String,
    },
    TaskFailed {
        task_id: String,
        task_index: usize,
        reason: String,
    },
    /// A node missed its done contract but still has attempts left, so the
    /// plan is running it again rather than pausing for an operator.
    TaskRetrying {
        task_id: String,
        task_index: usize,
        /// The attempt about to start, 1-based.
        attempt: u32,
        max_attempts: u32,
        /// The failure the retry is seeded with.
        reason: String,
        /// The run the retry extends, when the failed attempt produced one.
        parent_run_id: Option<String>,
    },
    /// A child could not be retried because its approved spend or wall
    /// allowance was exhausted. The dimension is persisted as data so the
    /// parent Job never has to infer a stop reason from prose.
    TaskBudgetExhausted {
        task_id: String,
        task_index: usize,
        dimension: BudgetDimension,
        reason: String,
    },
    /// The provider call that decomposed the root goal consumed the approved
    /// tree allowance before any child was launched.
    RootBudgetExhausted {
        dimension: BudgetDimension,
        reason: String,
    },
    /// Consecutive node failures reached `circuit_breaker_threshold`; the
    /// plan stopped launching work rather than spend the rest of the budget.
    CircuitBreakerTripped {
        consecutive_failures: u32,
        threshold: u32,
    },
    /// Under `ApplyWhen::PerNode`, this node's gated result landed on the
    /// operator's branch, so nodes that follow start from a tree containing it.
    TaskApplied {
        task_id: String,
        task_index: usize,
        run_id: String,
    },
    TaskKilled {
        task_id: String,
        task_index: usize,
        run_id: Option<String>,
    },
    MergeStarted,
    MergeConflict {
        conflict_count: usize,
    },
    MergeRepairPlanned {
        conflict_count: usize,
        provider: Option<String>,
    },
    MergeRepairStarted {
        mode: String,
    },
    MergeRepairRunDiscovered {
        run_id: String,
        pid: Option<u32>,
    },
    MergeRepaired {
        strategy: String,
        repair_run_id: Option<String>,
    },
    MergeRepairFailed {
        reason: String,
    },
    MergeCompleted {
        merged_run_id: String,
    },
    PlanCompleted,
    PlanFailed {
        reason: String,
    },
    PlanKilled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanEvent {
    pub timestamp: DateTime<Utc>,
    pub plan_id: String,
    pub event: PlanEventKind,
}

impl PlanMessage {
    pub fn new(
        from: impl Into<String>,
        to: impl Into<String>,
        kind: PlanMessageKind,
        summary: impl Into<String>,
        body: Value,
    ) -> Result<Self> {
        let request_id = match kind {
            PlanMessageKind::ReviewRequest
            | PlanMessageKind::ReviewResponse
            | PlanMessageKind::ShutdownRequest
            | PlanMessageKind::ShutdownResponse => Some(Uuid::new_v4().simple().to_string()),
            PlanMessageKind::Progress
            | PlanMessageKind::Blocker
            | PlanMessageKind::CapabilityRequest => None,
        };
        Ok(Self {
            schema_version: 1,
            ts: Utc::now(),
            request_id,
            from: from.into(),
            to: to.into(),
            kind,
            summary: summary.into(),
            body,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanChildMarker {
    pub schema_version: u32,
    pub kind: String,
    pub parent_plan_id: String,
    pub parent_scope: String,
    pub parent_goal: String,
    pub task_id: String,
    pub child_index: u32,
    pub task_goal: String,
    pub worker_spec: PathBuf,
    pub provider: Option<String>,
    pub role: PlanRole,
    pub created_at: DateTime<Utc>,
    pub deadreckon_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinatorChild {
    pub child_index: u32,
    pub task_id: String,
    pub run_id: Option<String>,
    pub pid: Option<u32>,
    pub scope: Option<String>,
    pub provider: Option<String>,
    pub role: PlanRole,
    pub status: PlanTaskStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinatorState {
    pub schema_version: u32,
    pub plan_id: String,
    pub coordinator_pid: u32,
    pub started_at: DateTime<Utc>,
    pub children: Vec<CoordinatorChild>,
}

pub fn validate_task_count(count: usize) -> Result<()> {
    match count {
        0 | 1 => Err(DeadreckonError::InvalidInput(
            "plan must have >= 2 children\ntry: deadreckon run \"<the only child>\"".to_string(),
        )),
        2..=6 => Ok(()),
        _ => Err(DeadreckonError::InvalidInput(format!(
            "plan capped at 6 children; got {count}\ntry: split the goal into a chain"
        ))),
    }
}

pub fn validate_task_graph(tasks: &[PlanTask]) -> Result<()> {
    validate_task_count(tasks.len())?;
    let mut ids = BTreeSet::new();
    let mut subjects = BTreeSet::new();
    for (expected_index, task) in tasks.iter().enumerate() {
        if task.index != expected_index as u32 {
            return Err(DeadreckonError::InvalidInput(format!(
                "child {} has index {}; expected {expected_index}",
                task.task_id, task.index
            )));
        }
        validate_nonempty(&task.task_id, "task_id")?;
        validate_nonempty(&task.subject, "subject")?;
        validate_nonempty(&task.goal, "goal")?;
        if !ids.insert(task.task_id.as_str()) {
            return Err(DeadreckonError::InvalidInput(format!(
                "duplicate child id {}",
                task.task_id
            )));
        }
        let normalized_subject = task
            .subject
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        if !subjects.insert(normalized_subject) {
            return Err(DeadreckonError::InvalidInput(format!(
                "duplicate child subject {}",
                task.subject
            )));
        }
    }
    for task in tasks {
        for dependency in &task.depends_on {
            if !ids.contains(dependency.as_str()) {
                return Err(DeadreckonError::InvalidInput(format!(
                    "child {} depends on unknown child {dependency}",
                    task.task_id
                )));
            }
            if dependency == &task.task_id {
                return Err(DeadreckonError::InvalidInput(format!(
                    "child {} depends on itself",
                    task.task_id
                )));
            }
        }
        if let Some(subplan) = task.subplan.as_deref() {
            validate_nonempty(subplan, "subplan")?;
            if subplan == task.task_id {
                return Err(DeadreckonError::InvalidInput(format!(
                    "child {} names itself as its subplan",
                    task.task_id
                )));
            }
        }
    }
    detect_task_cycle(tasks)?;
    Ok(())
}

/// Guard a subplan launch against the nesting cap.
///
/// `current_depth` is the depth of the plan requesting the subplan (0 for a
/// top-level plan). The subplan would sit at `current_depth + 1`; if that
/// reaches [`MAX_SUBPLAN_DEPTH`] it is refused. Mirrors
/// `campaign::guard_campaign_depth`, which this replaces once subplans land.
pub fn guard_subplan_depth(current_depth: u32) -> Result<()> {
    if current_depth + 1 >= MAX_SUBPLAN_DEPTH {
        return Err(DeadreckonError::InvalidInput(format!(
            "subplan refused: nesting cap {MAX_SUBPLAN_DEPTH} reached\n\
             a plan at depth {current_depth} cannot launch another subplan"
        )));
    }
    Ok(())
}

/// Walk `parent_plan_id` to this plan's nesting depth. Stops at
/// [`MAX_SUBPLAN_DEPTH`] hops so a corrupted lineage cannot loop forever.
pub fn plan_depth(paths: &DeadreckonPaths, plan: &Plan) -> u32 {
    let mut depth = 0;
    let mut parent = plan.parent_plan_id.clone();
    while let Some(parent_id) = parent {
        depth += 1;
        if depth >= MAX_SUBPLAN_DEPTH {
            break;
        }
        parent = match load_plan(paths, &parent_id) {
            Ok(parent_plan) => parent_plan.parent_plan_id,
            Err(_) => break,
        };
    }
    depth
}

pub fn save_plan(paths: &DeadreckonPaths, plan: &Plan) -> Result<()> {
    atomic_write_json(&paths.plan_json(&plan.plan_id), plan)
}

pub fn load_plan(paths: &DeadreckonPaths, plan_id: &str) -> Result<Plan> {
    let path = paths.plan_json(plan_id);
    let raw = fs::read(&path).with_path(&path)?;
    serde_json::from_slice(&raw).with_json_path(&path)
}

pub fn append_plan_message(
    paths: &DeadreckonPaths,
    plan_id: &str,
    message: &PlanMessage,
) -> Result<()> {
    validate_message_request_id(message)?;
    append_json_line(&paths.plan_messages(plan_id), message)
}

pub fn append_plan_event(
    paths: &DeadreckonPaths,
    plan_id: &str,
    event: PlanEventKind,
) -> Result<()> {
    let event = PlanEvent {
        timestamp: Utc::now(),
        plan_id: plan_id.to_string(),
        event,
    };
    append_json_line(&paths.plan_events(plan_id), &event)
}

pub fn read_plan_events(paths: &DeadreckonPaths, plan_id: &str) -> Result<Vec<PlanEvent>> {
    let path = paths.plan_events(plan_id);
    match fs::read_to_string(&path) {
        Ok(raw) => raw
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).with_json_path(&path))
            .collect(),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(DeadreckonError::Io { path, source }),
    }
}

pub fn read_plan_messages(paths: &DeadreckonPaths, plan_id: &str) -> Result<Vec<PlanMessage>> {
    let path = paths.plan_messages(plan_id);
    match fs::read_to_string(&path) {
        Ok(raw) => raw
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).with_json_path(&path))
            .collect(),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(DeadreckonError::Io { path, source }),
    }
}

pub fn write_worker_spec(
    paths: &DeadreckonPaths,
    plan_id: &str,
    task_id: &str,
    spec: &str,
) -> Result<PathBuf> {
    let path = paths.worker_spec(plan_id, task_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_path(parent)?;
    }
    fs::write(&path, spec).with_path(&path)?;
    Ok(path)
}

pub fn write_child_summary(
    paths: &DeadreckonPaths,
    plan_id: &str,
    task_id: &str,
    summary: &str,
) -> Result<PathBuf> {
    let path = paths.child_summary(plan_id, task_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_path(parent)?;
    }
    fs::write(&path, summary).with_path(&path)?;
    Ok(path)
}

pub fn plan_narrative_path(paths: &DeadreckonPaths, plan_id: &str) -> PathBuf {
    paths
        .plan_dir(plan_id)
        .join(PLAN_DOCS_DIR)
        .join(PLAN_NARRATIVE)
}

pub fn write_plan_narrative(paths: &DeadreckonPaths, plan: &Plan) -> Result<PathBuf> {
    let path = plan_narrative_path(paths, &plan.plan_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_path(parent)?;
    }

    let mut out = String::new();
    out.push_str("# Plan Narrative\n\n");
    out.push_str(&format!("**Plan ID:** `{}`\n", plan.plan_id));
    out.push_str(&format!("**Status:** {}\n", plan_status_label(plan.status)));
    out.push_str(&format!("**Goal:** {}\n", plan.root_goal));
    out.push_str(&format!("**Mode:** {}\n", plan.mode.as_str()));
    if let Some(run_id) = plan.merged_run_id.as_deref() {
        out.push_str(&format!("**Result run:** `{run_id}`\n"));
    }
    out.push_str("\n## Reading Order\n\n");
    out.push_str(
        "Start with this plan narrative, then read the child summaries below for the work each child completed.\n\n",
    );
    out.push_str("## Child Summaries\n\n");

    for task in &plan.tasks {
        out.push_str(&format!("### Child {}: {}\n\n", task.index, task.subject));
        out.push_str(&format!("- Child id: `{}`\n", task.task_id));
        out.push_str(&format!("- Role: {}\n", plan_role_label(task.role)));
        out.push_str(&format!(
            "- Status: {}\n",
            plan_task_status_label(task.status)
        ));
        if let Some(run_id) = task.child_run_id.as_deref() {
            out.push_str(&format!("- Run: `{run_id}`\n"));
        }
        if task.depends_on.is_empty() {
            out.push_str("- Dependencies: none\n");
        } else {
            out.push_str(&format!("- Dependencies: {}\n", task.depends_on.join(", ")));
        }
        out.push('\n');

        let summary_path = task
            .summary_path
            .as_ref()
            .map(|relative| paths.plan_dir(&plan.plan_id).join(relative))
            .unwrap_or_else(|| paths.child_summary(&plan.plan_id, &task.task_id));
        let summary = fs::read_to_string(&summary_path)
            .unwrap_or_else(|_| "No child summary was recorded for this child.".to_string());
        out.push_str(summary.trim());
        out.push_str("\n\n");
    }

    fs::write(&path, out).with_path(&path)?;
    Ok(path)
}

fn plan_role_label(role: PlanRole) -> &'static str {
    match role {
        PlanRole::Child => "child",
        PlanRole::Coder => "coder",
        PlanRole::Reviewer => "reviewer",
    }
}

pub fn write_plan_child_marker(working_dir: &Path, marker: &PlanChildMarker) -> Result<PathBuf> {
    let path = working_dir.join(PLAN_CHILD_PARENT_JSON);
    atomic_write_json(&path, marker)?;
    Ok(path)
}

pub fn write_coordinator_state(
    paths: &DeadreckonPaths,
    plan_id: &str,
    state: &CoordinatorState,
) -> Result<()> {
    atomic_write_json(&paths.coordinator_json(plan_id), state)
}

pub fn plan_task_key(plan_id: &str) -> String {
    format!("plan--{plan_id}")
}

pub fn worker_spec_relative_path(task_id: &str) -> PathBuf {
    PathBuf::from(format!("{WORKER_SPECS_DIR}/{}.md", sanitize_slug(task_id)))
}

pub fn child_summary_relative_path(task_id: &str) -> PathBuf {
    PathBuf::from(format!("{SUMMARIES_DIR}/{}.md", sanitize_slug(task_id)))
}

fn validate_nonempty(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(DeadreckonError::InvalidInput(format!(
            "child {field} must be non-empty"
        )));
    }
    Ok(())
}

fn validate_message_request_id(message: &PlanMessage) -> Result<()> {
    let required = matches!(
        message.kind,
        PlanMessageKind::ReviewRequest
            | PlanMessageKind::ReviewResponse
            | PlanMessageKind::ShutdownRequest
            | PlanMessageKind::ShutdownResponse
    );
    if required && message.request_id.as_deref().unwrap_or_default().is_empty() {
        return Err(DeadreckonError::InvalidInput(format!(
            "message {:?} requires request_id",
            message.kind
        )));
    }
    Ok(())
}

fn detect_task_cycle(tasks: &[PlanTask]) -> Result<()> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mark {
        Visiting,
        Done,
    }
    fn visit<'a>(
        id: &'a str,
        by_id: &BTreeMap<&'a str, &'a PlanTask>,
        marks: &mut BTreeMap<&'a str, Mark>,
    ) -> Result<()> {
        match marks.get(id) {
            Some(Mark::Visiting) => {
                return Err(DeadreckonError::InvalidInput(format!(
                    "child dependency cycle at {id}\ntry: remove one depends_on edge"
                )));
            }
            Some(Mark::Done) => return Ok(()),
            None => {}
        }
        marks.insert(id, Mark::Visiting);
        if let Some(task) = by_id.get(id) {
            for dependency in &task.depends_on {
                visit(dependency, by_id, marks)?;
            }
        }
        marks.insert(id, Mark::Done);
        Ok(())
    }

    let by_id = tasks
        .iter()
        .map(|task| (task.task_id.as_str(), task))
        .collect::<BTreeMap<_, _>>();
    let mut marks = BTreeMap::new();
    for task in tasks {
        visit(&task.task_id, &by_id, &mut marks)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn sample_plan(temp: &TempDir) -> Plan {
        let mut task0 = PlanTask::new(
            0,
            "Create app shell",
            "Create the app shell",
            PlanRole::Child,
            Some("cli:claude-code".to_string()),
        );
        task0.worker_spec = worker_spec_relative_path(&task0.task_id);
        let mut task1 = PlanTask::new(
            1,
            "Add tests",
            "Add smoke tests",
            PlanRole::Child,
            Some("cli:codex".to_string()),
        );
        task1.depends_on = vec![task0.task_id.clone()];
        task1.worker_spec = worker_spec_relative_path(&task1.task_id);
        Plan::new(
            "build a tiny app",
            PlanMode::FullPlan,
            vec![task0, task1],
            PlanProviders {
                planner: Some("cli:codex".to_string()),
                default_child: Some("cli:claude-code".to_string()),
                coder: None,
                reviewer: None,
                children: BTreeMap::from([(1, "cli:codex".to_string())]),
                ..PlanProviders::default()
            },
            Some("scope-a".to_string()),
            "0.1.0",
        )
        .map(|mut plan| {
            plan.plan_id = format!(
                "plan-{}",
                temp.path().file_name().unwrap().to_string_lossy()
            );
            plan
        })
        .expect("plan")
    }

    #[test]
    fn plan_json_serializes_roundtrip() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let plan = sample_plan(&temp);

        save_plan(&paths, &plan).expect("save");
        let loaded = load_plan(&paths, &plan.plan_id).expect("load");

        assert_eq!(loaded, plan);
        assert!(paths.plan_json(&plan.plan_id).ends_with(PLAN_JSON));
    }

    #[test]
    fn plan_task_dag_rejects_cycles() {
        let mut task0 = PlanTask::new(0, "A", "A", PlanRole::Child, None);
        let mut task1 = PlanTask::new(1, "B", "B", PlanRole::Child, None);
        task0.depends_on = vec![task1.task_id.clone()];
        task1.depends_on = vec![task0.task_id.clone()];

        let err = validate_task_graph(&[task0, task1]).expect_err("cycle");

        assert!(err.to_string().contains("cycle"), "{err}");
        assert!(err.to_string().contains("try:"), "{err}");
    }

    #[test]
    fn plan_json_preserves_provider_role_assignments() {
        let temp = TempDir::new().expect("tempdir");
        let plan = sample_plan(&temp);
        let json = serde_json::to_string_pretty(&plan).expect("json");
        let decoded = serde_json::from_str::<Plan>(&json).expect("decode");

        assert_eq!(decoded.providers.planner.as_deref(), Some("cli:codex"));
        assert_eq!(
            decoded.providers.default_child.as_deref(),
            Some("cli:claude-code")
        );
        assert_eq!(
            decoded.providers.children.get(&1).map(String::as_str),
            Some("cli:codex")
        );
    }

    #[test]
    fn child_parent_json_plan_kind() {
        let temp = TempDir::new().expect("tempdir");
        let marker = PlanChildMarker {
            schema_version: 1,
            kind: "plan_child".to_string(),
            parent_plan_id: "plan-123".to_string(),
            parent_scope: "scope-a".to_string(),
            parent_goal: "build a tiny app".to_string(),
            task_id: "task-1".to_string(),
            child_index: 1,
            task_goal: "add behavior".to_string(),
            worker_spec: PathBuf::from("worker-specs/task-1.md"),
            provider: Some("cli:codex".to_string()),
            role: PlanRole::Child,
            created_at: Utc::now(),
            deadreckon_version: "0.1.0".to_string(),
        };

        let path = write_plan_child_marker(temp.path(), &marker).expect("marker");
        let decoded =
            serde_json::from_slice::<PlanChildMarker>(&std::fs::read(path).expect("read"))
                .expect("decode");

        assert_eq!(decoded.kind, "plan_child");
        assert_eq!(decoded.parent_plan_id, "plan-123");
        assert_eq!(decoded.task_id, "task-1");
        assert_eq!(decoded.provider.as_deref(), Some("cli:codex"));
        assert_eq!(decoded.role, PlanRole::Child);
    }

    #[test]
    fn plan_messages_jsonl_roundtrips_typed_requests() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let plan = sample_plan(&temp);
        save_plan(&paths, &plan).expect("save");
        let progress = PlanMessage::new(
            "coordinator",
            "task-0",
            PlanMessageKind::Progress,
            "task started",
            serde_json::json!({ "status": "running" }),
        )
        .expect("progress");
        let review = PlanMessage::new(
            "coordinator",
            "task-1",
            PlanMessageKind::ReviewRequest,
            "review requested",
            serde_json::json!({ "run": "abc" }),
        )
        .expect("review");
        let review_response = PlanMessage::new(
            "task-1",
            "coordinator",
            PlanMessageKind::ReviewResponse,
            "review completed",
            serde_json::json!({ "findings": 0 }),
        )
        .expect("review response");
        let shutdown = PlanMessage::new(
            "coordinator",
            "task-0",
            PlanMessageKind::ShutdownRequest,
            "shutdown requested",
            serde_json::json!({ "reason": "test" }),
        )
        .expect("shutdown");
        let shutdown_response = PlanMessage::new(
            "task-0",
            "coordinator",
            PlanMessageKind::ShutdownResponse,
            "shutdown completed",
            serde_json::json!({ "ok": true }),
        )
        .expect("shutdown response");

        append_plan_message(&paths, &plan.plan_id, &progress).expect("append progress");
        append_plan_message(&paths, &plan.plan_id, &review).expect("append review");
        append_plan_message(&paths, &plan.plan_id, &review_response)
            .expect("append review response");
        append_plan_message(&paths, &plan.plan_id, &shutdown).expect("append shutdown");
        append_plan_message(&paths, &plan.plan_id, &shutdown_response)
            .expect("append shutdown response");
        let messages = read_plan_messages(&paths, &plan.plan_id).expect("read");

        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0].kind, PlanMessageKind::Progress);
        assert!(messages[1].request_id.is_some());
        assert_eq!(messages[2].kind, PlanMessageKind::ReviewResponse);
        assert!(messages[2].request_id.is_some());
        assert_eq!(messages[3].kind, PlanMessageKind::ShutdownRequest);
        assert!(messages[3].request_id.is_some());
        assert_eq!(messages[4].kind, PlanMessageKind::ShutdownResponse);
        assert!(messages[4].request_id.is_some());
    }

    #[test]
    fn plan_event_jsonl_roundtrips_all_kinds() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let plan = sample_plan(&temp);
        save_plan(&paths, &plan).expect("save");
        let events = vec![
            PlanEventKind::PlanCreated {
                mode: PlanMode::FullPlan,
                task_count: 2,
            },
            PlanEventKind::PlanStarted,
            PlanEventKind::TaskReady {
                task_id: "task-0".to_string(),
                task_index: 0,
            },
            PlanEventKind::TaskStarted {
                task_id: "task-0".to_string(),
                task_index: 0,
            },
            PlanEventKind::TaskRunDiscovered {
                task_id: "task-0".to_string(),
                task_index: 0,
                run_id: Some("run-0".to_string()),
                pid: Some(123),
            },
            PlanEventKind::TaskCompleted {
                task_id: "task-0".to_string(),
                task_index: 0,
                run_id: Some("run-0".to_string()),
                status: "completed".to_string(),
            },
            PlanEventKind::TaskBlocked {
                task_id: "task-1".to_string(),
                task_index: 1,
                reason: "waiting".to_string(),
            },
            PlanEventKind::TaskFailed {
                task_id: "task-1".to_string(),
                task_index: 1,
                reason: "red".to_string(),
            },
            PlanEventKind::TaskRetrying {
                task_id: "task-1".to_string(),
                task_index: 1,
                attempt: 2,
                max_attempts: 3,
                reason: "acceptance failed after turn 8".to_string(),
                parent_run_id: Some("run-1".to_string()),
            },
            PlanEventKind::CircuitBreakerTripped {
                consecutive_failures: 2,
                threshold: 2,
            },
            PlanEventKind::TaskApplied {
                task_id: "task-1".to_string(),
                task_index: 1,
                run_id: "run-1".to_string(),
            },
            PlanEventKind::TaskKilled {
                task_id: "task-1".to_string(),
                task_index: 1,
                run_id: None,
            },
            PlanEventKind::MergeStarted,
            PlanEventKind::MergeConflict { conflict_count: 2 },
            PlanEventKind::MergeRepairPlanned {
                conflict_count: 1,
                provider: Some("cli:codex".to_string()),
            },
            PlanEventKind::MergeRepairStarted {
                mode: "auto".to_string(),
            },
            PlanEventKind::MergeRepairRunDiscovered {
                run_id: "repair".to_string(),
                pid: Some(456),
            },
            PlanEventKind::MergeRepaired {
                strategy: "spawn_repair_child".to_string(),
                repair_run_id: Some("repair".to_string()),
            },
            PlanEventKind::MergeRepairFailed {
                reason: "unsafe".to_string(),
            },
            PlanEventKind::MergeCompleted {
                merged_run_id: "merged".to_string(),
            },
            PlanEventKind::PlanCompleted,
            PlanEventKind::PlanFailed {
                reason: "blocked".to_string(),
            },
            PlanEventKind::PlanKilled,
        ];

        for event in events.clone() {
            append_plan_event(&paths, &plan.plan_id, event).expect("append event");
        }
        let loaded = read_plan_events(&paths, &plan.plan_id).expect("read events");

        assert_eq!(loaded.len(), events.len());
        for (loaded, expected) in loaded.iter().zip(events) {
            assert_eq!(loaded.plan_id, plan.plan_id);
            assert_eq!(loaded.event, expected);
        }
    }

    #[test]
    fn append_plan_event_writes_under_plan_dir() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let plan = sample_plan(&temp);
        save_plan(&paths, &plan).expect("save");

        append_plan_event(&paths, &plan.plan_id, PlanEventKind::PlanStarted).expect("append event");

        let path = paths.plan_events(&plan.plan_id);
        assert!(path.starts_with(paths.plan_dir(&plan.plan_id)));
        assert!(path.ends_with(PLAN_EVENTS_JSONL));
        assert!(path.is_file());
    }

    #[test]
    fn plan_event_kind_uses_snake_case_tags() {
        let event = PlanEvent {
            timestamp: Utc::now(),
            plan_id: "plan-1".to_string(),
            event: PlanEventKind::TaskRunDiscovered {
                task_id: "task-0".to_string(),
                task_index: 0,
                run_id: Some("run-1".to_string()),
                pid: Some(7),
            },
        };
        let value = serde_json::to_value(event).expect("json");

        assert_eq!(
            value
                .pointer("/event/kind")
                .and_then(serde_json::Value::as_str),
            Some("task_run_discovered")
        );
    }

    #[test]
    fn worker_spec_paths_are_plan_local() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let plan_id = "abc";
        let spec_path = paths.worker_spec(plan_id, "../task-0");
        let summary_path = paths.child_summary(plan_id, "../task-0");

        assert!(spec_path.starts_with(paths.plan_dir(plan_id)));
        assert!(summary_path.starts_with(paths.plan_dir(plan_id)));
        assert!(!spec_path.to_string_lossy().contains(".."));
        assert!(!summary_path.to_string_lossy().contains(".."));
    }
}

#[cfg(test)]
mod plan_providers_compat_tests {
    use super::PlanProviders;

    #[test]
    fn pre_rider_plan_providers_json_deserializes_with_empty_model_fields() {
        // plan.json written before per-role models existed must parse with
        // every model field defaulted.
        let raw = r#"{
            "planner": "smoke",
            "default_child": "smoke",
            "coder": null,
            "reviewer": null,
            "children": {"1": "cli:codex"}
        }"#;
        let providers: PlanProviders = serde_json::from_str(raw).expect("parse");
        assert_eq!(providers.planner.as_deref(), Some("smoke"));
        assert_eq!(providers.planner_model, None);
        assert_eq!(providers.default_child_model, None);
        assert_eq!(providers.coder_model, None);
        assert_eq!(providers.reviewer_model, None);
        assert!(providers.child_models.is_empty());
        assert_eq!(
            providers.children.get(&1).map(String::as_str),
            Some("cli:codex")
        );
    }
}

#[cfg(test)]
mod plan_schema2_compat_tests {
    use super::*;

    /// A schema-1 plan.json — written before execution policy existed — must
    /// load with every schema-2 field defaulted to the old behavior. This is
    /// what makes schema 2 a no-migration change.
    const SCHEMA_1_PLAN: &str = r#"{
        "schema_version": 1,
        "plan_id": "a8ed1c09341a43ab8ccd374cdaac6813",
        "root_goal": "add auth, add billing, and add an audit log",
        "mode": "full_plan",
        "n": 1,
        "providers": {"planner": "smoke", "default_child": "smoke",
                      "coder": null, "reviewer": null, "children": {}},
        "capability_preview": {"network": "deny", "deploy": false,
                               "global_install": false, "filesystem": [], "notes": []},
        "tasks": [{
            "index": 0, "task_id": "task-0", "subject": "Create foundation",
            "goal": "create the foundation", "active_form": "Creating foundation",
            "provider": "smoke", "role": "child", "depends_on": [],
            "worker_spec": "worker-specs/task-0.md", "summary_path": null,
            "review_status": null, "child_run_id": null, "child_scope": null,
            "status": "pending"
        }],
        "parent_scope": null,
        "status": "pending",
        "created_at": "2026-07-25T15:13:03.491323273Z",
        "forked_at": null, "merged_at": null, "merged_run_id": null,
        "deadreckon_version": "0.7.0"
    }"#;

    #[test]
    fn schema_1_plan_json_loads_with_pre_schema_2_behavior() {
        let plan: Plan = serde_json::from_str(SCHEMA_1_PLAN).expect("parse schema-1 plan");

        assert_eq!(
            plan.schema_version, 1,
            "the file's own version is preserved"
        );
        assert_eq!(
            plan.apply,
            ApplyWhen::AtEnd,
            "an existing plan must keep merging at the end, not start applying per node"
        );
        assert_eq!(plan.branch_policy, BranchPolicy::Stack);
        assert_eq!(plan.apply_strategy, ApplyStrategy::Squash);
        assert!(plan.apply_allowlist.is_empty());
        assert_eq!(plan.on_fail, OnFail::Skip);
        assert_eq!(plan.max_attempts, DEFAULT_MAX_ATTEMPTS);
        assert_eq!(
            plan.circuit_breaker_threshold,
            DEFAULT_CIRCUIT_BREAKER_THRESHOLD
        );
        assert_eq!(plan.parent_plan_id, None);
        assert_eq!(plan.tasks[0].subplan, None);
        assert!(plan.tasks[0].attempts.is_empty());
    }

    #[test]
    fn new_plans_are_written_at_schema_2() {
        let plan = Plan::new(
            "goal",
            PlanMode::FullPlan,
            vec![
                PlanTask::new(0, "one", "do one", PlanRole::Child, None),
                PlanTask::new(1, "two", "do two", PlanRole::Child, None),
            ],
            PlanProviders::default(),
            None,
            "0.7.0",
        )
        .expect("plan");

        assert_eq!(plan.schema_version, PLAN_SCHEMA_VERSION);
        assert_eq!(plan.apply, ApplyWhen::AtEnd);
        assert_eq!(plan.max_attempts, DEFAULT_MAX_ATTEMPTS);
    }

    #[test]
    fn schema_2_plan_survives_a_save_load_round_trip() {
        let mut plan = Plan::new(
            "goal",
            PlanMode::FullPlan,
            vec![
                PlanTask::new(0, "one", "do one", PlanRole::Child, None),
                PlanTask::new(1, "two", "do two", PlanRole::Child, None),
            ],
            PlanProviders::default(),
            None,
            "0.7.0",
        )
        .expect("plan");
        plan.apply = ApplyWhen::PerNode;
        plan.on_fail = OnFail::Stop;
        plan.max_attempts = 5;
        plan.parent_plan_id = Some("parent".to_string());
        plan.tasks[0].subplan = Some("sub".to_string());
        plan.tasks[0]
            .attempts
            .push(TaskAttempt::new(1, Some("run-1".to_string())));

        let raw = serde_json::to_string(&plan).expect("serialize");
        let parsed: Plan = serde_json::from_str(&raw).expect("deserialize");

        assert_eq!(parsed, plan);
    }

    #[test]
    fn a_node_may_not_name_itself_as_its_subplan() {
        let mut first = PlanTask::new(0, "one", "do one", PlanRole::Child, None);
        first.subplan = Some("task-0".to_string());
        let second = PlanTask::new(1, "two", "do two", PlanRole::Child, None);

        let err = validate_task_graph(&[first, second]).expect_err("self-subplan must refuse");

        assert!(err.to_string().contains("subplan"), "{err}");
    }

    fn two_node_plan() -> Plan {
        let mut second = PlanTask::new(1, "two", "do two", PlanRole::Child, None);
        second.depends_on = vec!["task-0".to_string()];
        Plan::new(
            "goal",
            PlanMode::FullPlan,
            vec![
                PlanTask::new(0, "one", "do one", PlanRole::Child, None),
                second,
            ],
            PlanProviders::default(),
            None,
            "0.7.0",
        )
        .expect("plan")
    }

    #[test]
    fn a_node_may_retry_until_max_attempts_is_reached() {
        let mut task = PlanTask::new(0, "one", "do one", PlanRole::Child, None);
        assert!(task.may_retry(DEFAULT_MAX_ATTEMPTS), "no attempts used yet");

        for attempt in 1..=DEFAULT_MAX_ATTEMPTS {
            task.attempts.push(TaskAttempt::failed(
                attempt,
                Some(format!("run-{attempt}")),
                Some("acceptance failed".to_string()),
                0.5,
            ));
        }

        assert!(
            !task.may_retry(DEFAULT_MAX_ATTEMPTS),
            "the attempt budget is spent"
        );
        assert_eq!(task.attempts_used(), DEFAULT_MAX_ATTEMPTS);
        assert_eq!(task.attempts_spend_usd(), 1.5);
    }

    #[test]
    fn a_retry_continues_the_failed_run_and_carries_its_reason() {
        let mut task = PlanTask::new(0, "one", "do one", PlanRole::Child, None);
        task.attempts.push(TaskAttempt::failed(
            1,
            Some("run-1".to_string()),
            Some("acceptance failed after turn 8: billing.rs missing".to_string()),
            0.4,
        ));

        assert_eq!(task.retry_parent_run_id(), Some("run-1"));
        assert_eq!(
            task.last_failure_reason(),
            Some("acceptance failed after turn 8: billing.rs missing")
        );
    }

    #[test]
    fn a_spawn_failure_leaves_no_tree_for_the_retry_to_resume_from() {
        let mut task = PlanTask::new(0, "one", "do one", PlanRole::Child, None);
        task.attempts.push(TaskAttempt::failed(
            1,
            None,
            Some("child process failed to start".to_string()),
            0.0,
        ));

        assert_eq!(
            task.retry_parent_run_id(),
            None,
            "no run means no working tree to resume; the retry starts from the plan source"
        );
    }

    #[test]
    fn skip_keeps_dependents_blocked_but_continue_releases_them() {
        let mut plan = two_node_plan();
        plan.tasks[0].status = PlanTaskStatus::Failed;

        assert!(
            plan.ready_pending_task_indices_for(OnFail::Skip).is_empty(),
            "skip must not run work that depends on a failed node"
        );
        assert_eq!(
            plan.ready_pending_task_indices_for(OnFail::Continue),
            vec![1],
            "continue is the operator saying the dependent does not need that output"
        );
    }

    #[test]
    fn readiness_defaults_to_the_plans_own_failure_policy() {
        let mut plan = two_node_plan();
        plan.tasks[0].status = PlanTaskStatus::Failed;
        plan.on_fail = OnFail::Continue;

        assert_eq!(plan.ready_pending_task_indices(), vec![1]);
    }

    #[test]
    fn subplan_depth_guard_refuses_past_the_cap() {
        assert!(
            guard_subplan_depth(0).is_ok(),
            "a top-level plan may nest once"
        );
        assert!(
            guard_subplan_depth(1).is_err(),
            "a plan already one level deep may not nest again"
        );
    }
}
