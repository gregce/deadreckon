use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::paths::{DeadreckonPaths, sanitize_slug};
use crate::state::{append_json_line, atomic_write_json};

pub const PLAN_JSON: &str = "plan.json";
pub const COORDINATOR_JSON: &str = "coordinator.json";
pub const PLAN_MESSAGES_JSONL: &str = "messages.jsonl";
pub const PLAN_EVENTS_JSONL: &str = "plan-events.jsonl";
pub const WORKER_SPECS_DIR: &str = "worker-specs";
pub const SUMMARIES_DIR: &str = "summaries";
pub const PLAN_CHILD_PARENT_JSON: &str = ".deadreckon/parent.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanMode {
    FullPlan,
    Review,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
            worker_spec: PathBuf::from(format!("{WORKER_SPECS_DIR}/{task_id}.md")),
            summary_path: None,
            review_status: None,
            child_run_id: None,
            child_scope: None,
            status: PlanTaskStatus::Pending,
        }
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
            schema_version: 1,
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
        let completed = self
            .tasks
            .iter()
            .filter(|task| task.status == PlanTaskStatus::Completed)
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
                        .all(|dependency| completed.contains(dependency.as_str()))
            })
            .map(|(index, _)| index)
            .collect()
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
    TaskKilled {
        task_id: String,
        task_index: usize,
        run_id: Option<String>,
    },
    MergeStarted,
    MergeConflict {
        conflict_count: usize,
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
    }
    detect_task_cycle(tasks)?;
    Ok(())
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
            PlanEventKind::TaskKilled {
                task_id: "task-1".to_string(),
                task_index: 1,
                run_id: None,
            },
            PlanEventKind::MergeStarted,
            PlanEventKind::MergeConflict { conflict_count: 2 },
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
