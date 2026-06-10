use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::time::Duration;

use deadreckon_core::{
    DeadreckonPaths, Plan, PlanEvent, PlanEventKind, RUN_EVENTS_JSONL, RunEvent, load_plan,
    load_run,
};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::broadcast;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) enum PlanFeedEvent {
    Plan {
        event: PlanEvent,
    },
    ChildRun {
        task_id: String,
        run_id: String,
        event: RunEvent,
    },
    RepairRun {
        run_id: String,
        event: RunEvent,
    },
    Snapshot {
        plan: Box<Plan>,
    },
    Warning {
        message: String,
    },
}

#[derive(Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct PlanEventBus {
    sender: broadcast::Sender<PlanFeedEvent>,
}

impl PlanEventBus {
    #[cfg(test)]
    pub(crate) fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    #[cfg(test)]
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<PlanFeedEvent> {
        self.sender.subscribe()
    }

    #[cfg(test)]
    pub(crate) fn emit_plan(&self, event: PlanEvent) {
        let _ = self.sender.send(PlanFeedEvent::Plan { event });
    }

    pub(crate) fn file_tail(paths: DeadreckonPaths, plan_id: impl Into<String>) -> PlanEventFeed {
        PlanEventFeed::file_tail(paths, plan_id.into())
    }

    #[cfg(test)]
    pub(crate) fn from_broadcast(
        receiver: broadcast::Receiver<PlanFeedEvent>,
        paths: DeadreckonPaths,
        plan_id: impl Into<String>,
    ) -> PlanEventFeed {
        PlanEventFeed::from_broadcast(receiver, paths, plan_id.into())
    }
}

pub(crate) struct PlanEventFeed {
    paths: DeadreckonPaths,
    plan_id: String,
    source: PlanEventSource,
    child_tails: BTreeMap<(String, String), JsonlTail<RunEvent>>,
    repair_tails: BTreeMap<String, JsonlTail<RunEvent>>,
    seen: BTreeSet<String>,
    last_snapshot_key: Option<String>,
}

enum PlanEventSource {
    #[cfg_attr(not(test), allow(dead_code))]
    Broadcast {
        receiver: broadcast::Receiver<PlanFeedEvent>,
        replay: JsonlTail<PlanEvent>,
    },
    FileTail(JsonlTail<PlanEvent>),
}

impl PlanEventFeed {
    fn file_tail(paths: DeadreckonPaths, plan_id: String) -> Self {
        Self {
            source: PlanEventSource::FileTail(JsonlTail::new(paths.plan_events(&plan_id))),
            paths,
            plan_id,
            child_tails: BTreeMap::new(),
            repair_tails: BTreeMap::new(),
            seen: BTreeSet::new(),
            last_snapshot_key: None,
        }
    }

    #[cfg(test)]
    fn from_broadcast(
        receiver: broadcast::Receiver<PlanFeedEvent>,
        paths: DeadreckonPaths,
        plan_id: String,
    ) -> Self {
        Self {
            source: PlanEventSource::Broadcast {
                receiver,
                replay: JsonlTail::new(paths.plan_events(&plan_id)),
            },
            paths,
            plan_id,
            child_tails: BTreeMap::new(),
            repair_tails: BTreeMap::new(),
            seen: BTreeSet::new(),
            last_snapshot_key: None,
        }
    }

    pub(crate) async fn refresh(&mut self, wait: Duration) -> Vec<PlanFeedEvent> {
        let mut events = Vec::new();
        self.read_plan_source(wait, &mut events).await;
        self.discover_runs_from_plan_events(&events);
        if let Ok(plan) = load_plan(&self.paths, &self.plan_id) {
            self.discover_runs_from_plan(&plan);
            self.maybe_push_snapshot(plan, &mut events);
        }
        self.discover_repair_run_record();
        self.read_child_and_repair_events(&mut events);
        self.dedupe(events)
    }

    async fn read_plan_source(&mut self, wait: Duration, events: &mut Vec<PlanFeedEvent>) {
        match &mut self.source {
            PlanEventSource::Broadcast { receiver, replay } => {
                read_plan_replay(replay, events);
                drain_receiver(receiver, events);
                if events.is_empty() && !wait.is_zero() {
                    match tokio::time::timeout(wait, receiver.recv()).await {
                        Ok(Ok(event)) => events.push(event),
                        Ok(Err(broadcast::error::RecvError::Lagged(_)))
                        | Ok(Err(broadcast::error::RecvError::Closed))
                        | Err(_) => {}
                    }
                }
                read_plan_replay(replay, events);
                drain_receiver(receiver, events);
            }
            PlanEventSource::FileTail(replay) => {
                read_plan_replay(replay, events);
            }
        }
    }

    fn discover_runs_from_plan_events(&mut self, events: &[PlanFeedEvent]) {
        for event in events {
            match event {
                PlanFeedEvent::Plan {
                    event:
                        PlanEvent {
                            event:
                                PlanEventKind::TaskRunDiscovered {
                                    task_id,
                                    run_id: Some(run_id),
                                    ..
                                },
                            ..
                        },
                } => self.register_child_run(task_id, run_id),
                PlanFeedEvent::Plan {
                    event:
                        PlanEvent {
                            event: PlanEventKind::MergeRepairRunDiscovered { run_id, .. },
                            ..
                        },
                } => self.register_repair_run(run_id),
                _ => {}
            }
        }
    }

    fn discover_runs_from_plan(&mut self, plan: &Plan) {
        for task in &plan.tasks {
            if let Some(run_id) = task.child_run_id.as_deref() {
                self.register_child_run(&task.task_id, run_id);
            }
            let sidecar = self
                .paths
                .plan_dir(&plan.plan_id)
                .join("launch")
                .join(&task.task_id)
                .join("run-id");
            if let Ok(raw) = fs::read_to_string(sidecar) {
                let run_id = raw.trim();
                if !run_id.is_empty() {
                    self.register_child_run(&task.task_id, run_id);
                }
            }
        }
    }

    fn discover_repair_run_record(&mut self) {
        let path = self
            .paths
            .merge_proofs(&self.plan_id)
            .join("repair-run.json");
        let Ok(raw) = fs::read_to_string(path) else {
            return;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return;
        };
        if let Some(run_id) = value.get("run_id").and_then(serde_json::Value::as_str) {
            self.register_repair_run(run_id);
        }
    }

    fn register_child_run(&mut self, task_id: &str, run_id: &str) {
        let key = (task_id.to_string(), run_id.to_string());
        if self.child_tails.contains_key(&key) {
            return;
        }
        if let Ok(state) = load_run(&self.paths, run_id) {
            self.child_tails
                .insert(key, JsonlTail::new(state.run_root.join(RUN_EVENTS_JSONL)));
        }
    }

    fn register_repair_run(&mut self, run_id: &str) {
        if self.repair_tails.contains_key(run_id) {
            return;
        }
        if let Ok(state) = load_run(&self.paths, run_id) {
            self.repair_tails.insert(
                run_id.to_string(),
                JsonlTail::new(state.run_root.join(RUN_EVENTS_JSONL)),
            );
        }
    }

    fn read_child_and_repair_events(&mut self, events: &mut Vec<PlanFeedEvent>) {
        for ((task_id, run_id), tail) in &mut self.child_tails {
            let task_id = task_id.clone();
            let run_id = run_id.clone();
            for event in tail.read_new().into_iter().flatten() {
                events.push(PlanFeedEvent::ChildRun {
                    task_id: task_id.clone(),
                    run_id: run_id.clone(),
                    event,
                });
            }
        }
        for (run_id, tail) in &mut self.repair_tails {
            let run_id = run_id.clone();
            for event in tail.read_new().into_iter().flatten() {
                events.push(PlanFeedEvent::RepairRun {
                    run_id: run_id.clone(),
                    event,
                });
            }
        }
    }

    fn maybe_push_snapshot(&mut self, plan: Plan, events: &mut Vec<PlanFeedEvent>) {
        let key = snapshot_key(&plan);
        if self.last_snapshot_key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.last_snapshot_key = Some(key);
        events.push(PlanFeedEvent::Snapshot {
            plan: Box::new(plan),
        });
    }

    fn dedupe(&mut self, events: Vec<PlanFeedEvent>) -> Vec<PlanFeedEvent> {
        events
            .into_iter()
            .filter(|event| {
                let key = serde_json::to_string(event).unwrap_or_else(|_| format!("{event:?}"));
                self.seen.insert(key)
            })
            .collect()
    }
}

fn drain_receiver(
    receiver: &mut broadcast::Receiver<PlanFeedEvent>,
    events: &mut Vec<PlanFeedEvent>,
) {
    loop {
        match receiver.try_recv() {
            Ok(event) => events.push(event),
            Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(broadcast::error::TryRecvError::Empty)
            | Err(broadcast::error::TryRecvError::Closed) => break,
        }
    }
}

fn read_plan_replay(replay: &mut JsonlTail<PlanEvent>, events: &mut Vec<PlanFeedEvent>) {
    match replay.read_new() {
        Ok(plan_events) => events.extend(
            plan_events
                .into_iter()
                .map(|event| PlanFeedEvent::Plan { event }),
        ),
        Err(error) => events.push(PlanFeedEvent::Warning {
            message: format!("plan event replay failed: {error}"),
        }),
    }
}

fn snapshot_key(plan: &Plan) -> String {
    let tasks = plan
        .tasks
        .iter()
        .map(|task| {
            format!(
                "{}:{:?}:{:?}:{:?}",
                task.task_id, task.status, task.child_run_id, task.summary_path
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    format!(
        "{}:{:?}:{:?}:{}",
        plan.plan_id, plan.status, plan.merged_run_id, tasks
    )
}

pub(crate) struct JsonlTail<T> {
    path: PathBuf,
    offset: u64,
    partial: String,
    _marker: PhantomData<T>,
}

impl<T> JsonlTail<T>
where
    T: DeserializeOwned,
{
    pub(crate) fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            offset: 0,
            partial: String::new(),
            _marker: PhantomData,
        }
    }

    pub(crate) fn read_new(&mut self) -> std::io::Result<Vec<T>> {
        let Some(mut file) = self.open_at_offset()? else {
            return Ok(Vec::new());
        };
        let mut raw = String::new();
        file.read_to_string(&mut raw)?;
        self.offset = file.stream_position()?;
        if raw.is_empty() {
            return Ok(Vec::new());
        }
        let mut buffer = String::new();
        std::mem::swap(&mut buffer, &mut self.partial);
        buffer.push_str(&raw);
        let complete = buffer.ends_with('\n');
        let mut lines = buffer.lines().collect::<Vec<_>>();
        if !complete {
            self.partial = lines.pop().unwrap_or_default().to_string();
        }
        Ok(lines
            .into_iter()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<T>(line).ok())
            .collect())
    }

    fn open_at_offset(&mut self) -> std::io::Result<Option<fs::File>> {
        if !self.path.exists() {
            self.offset = 0;
            self.partial.clear();
            return Ok(None);
        }
        let metadata = fs::metadata(&self.path)?;
        if metadata.len() < self.offset {
            self.offset = 0;
            self.partial.clear();
        }
        let mut file = fs::File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        Ok(Some(file))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use chrono::Utc;
    use deadreckon_core::{
        CapabilityPreview, DeadreckonPaths, NetworkCapability, Plan, PlanEvent, PlanEventKind,
        PlanMode, PlanProviders, PlanRole, PlanStatus, PlanTask, RunEventKind, RunOptions,
        append_plan_event, create_run, emit_event, save_plan,
    };
    use tempfile::TempDir;

    use super::{PlanEventBus, PlanFeedEvent};

    #[tokio::test]
    async fn plan_event_bus_replays_existing_jsonl_before_live_events() {
        let (_temp, paths, plan) = plan_fixture();
        save_plan(&paths, &plan).expect("save plan");
        append_plan_event(&paths, &plan.plan_id, PlanEventKind::PlanStarted)
            .expect("append started");
        let bus = PlanEventBus::new(8);
        let mut feed =
            PlanEventBus::from_broadcast(bus.subscribe(), paths.clone(), plan.plan_id.clone());
        bus.emit_plan(PlanEvent {
            timestamp: Utc::now(),
            plan_id: plan.plan_id.clone(),
            event: PlanEventKind::TaskReady {
                task_id: "task-0".to_string(),
                task_index: 0,
            },
        });

        let events = feed.refresh(Duration::from_millis(10)).await;

        assert!(matches!(
            events.first(),
            Some(PlanFeedEvent::Plan {
                event: PlanEvent {
                    event: PlanEventKind::PlanStarted,
                    ..
                }
            })
        ));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                PlanFeedEvent::Plan {
                    event: PlanEvent {
                        event: PlanEventKind::TaskReady { task_id, .. },
                        ..
                    }
                } if task_id == "task-0"
            )
        }));
    }

    #[tokio::test]
    async fn plan_event_bus_dedupes_replay_and_broadcast_events() {
        let (_temp, paths, plan) = plan_fixture();
        save_plan(&paths, &plan).expect("save plan");
        let event = PlanEvent {
            timestamp: Utc::now(),
            plan_id: plan.plan_id.clone(),
            event: PlanEventKind::PlanStarted,
        };
        std::fs::write(
            paths.plan_events(&plan.plan_id),
            format!("{}\n", serde_json::to_string(&event).expect("json")),
        )
        .expect("write event");
        let bus = PlanEventBus::new(8);
        let mut feed =
            PlanEventBus::from_broadcast(bus.subscribe(), paths.clone(), plan.plan_id.clone());
        bus.emit_plan(event);

        let events = feed.refresh(Duration::from_millis(10)).await;
        let started = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    PlanFeedEvent::Plan {
                        event: PlanEvent {
                            event: PlanEventKind::PlanStarted,
                            ..
                        }
                    }
                )
            })
            .count();

        assert_eq!(started, 1, "{events:#?}");
    }

    #[tokio::test]
    async fn plan_event_bus_tolerates_partial_jsonl_line() {
        let (_temp, paths, plan) = plan_fixture();
        save_plan(&paths, &plan).expect("save plan");
        let event = PlanEvent {
            timestamp: Utc::now(),
            plan_id: plan.plan_id.clone(),
            event: PlanEventKind::PlanStarted,
        };
        let raw = serde_json::to_string(&event).expect("json");
        std::fs::write(paths.plan_events(&plan.plan_id), &raw[..raw.len() / 2]).expect("partial");
        let mut feed = PlanEventBus::file_tail(paths.clone(), plan.plan_id.clone());

        let first = feed.refresh(Duration::ZERO).await;
        std::fs::write(paths.plan_events(&plan.plan_id), format!("{raw}\n")).expect("complete");
        let second = feed.refresh(Duration::ZERO).await;

        assert!(
            !first
                .iter()
                .any(|event| matches!(event, PlanFeedEvent::Plan { .. }))
        );
        assert!(second.iter().any(|event| {
            matches!(
                event,
                PlanFeedEvent::Plan {
                    event: PlanEvent {
                        event: PlanEventKind::PlanStarted,
                        ..
                    }
                }
            )
        }));
    }

    #[tokio::test]
    async fn plan_event_bus_emits_snapshot_after_plan_status_change() {
        let (_temp, paths, mut plan) = plan_fixture();
        save_plan(&paths, &plan).expect("save plan");
        let mut feed = PlanEventBus::file_tail(paths.clone(), plan.plan_id.clone());

        let first = feed.refresh(Duration::ZERO).await;
        plan.status = PlanStatus::Forked;
        save_plan(&paths, &plan).expect("save updated plan");
        let second = feed.refresh(Duration::ZERO).await;

        assert!(
            first
                .iter()
                .any(|event| matches!(event, PlanFeedEvent::Snapshot { .. }))
        );
        assert!(second.iter().any(|event| {
            matches!(
                event,
                PlanFeedEvent::Snapshot { plan } if plan.status == PlanStatus::Forked
            )
        }));
    }

    #[tokio::test]
    async fn plan_event_bus_adds_child_stream_when_run_discovered() {
        let (temp, paths, plan) = plan_fixture();
        save_plan(&paths, &plan).expect("save plan");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "child".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        append_plan_event(
            &paths,
            &plan.plan_id,
            PlanEventKind::TaskRunDiscovered {
                task_id: "task-0".to_string(),
                task_index: 0,
                run_id: Some(state.run_id.clone()),
                pid: None,
            },
        )
        .expect("discover");
        emit_event(&state, None, RunEventKind::TurnStarted { turn: 1 }).expect("turn");
        let mut feed = PlanEventBus::file_tail(paths.clone(), plan.plan_id.clone());

        let events = feed.refresh(Duration::ZERO).await;

        assert!(events.iter().any(|event| {
            matches!(
                event,
                PlanFeedEvent::ChildRun {
                    task_id,
                    run_id,
                    event,
                } if task_id == "task-0"
                    && run_id == &state.run_id
                    && matches!(event.event, RunEventKind::TurnStarted { turn: 1 })
            )
        }));
    }

    #[tokio::test]
    async fn plan_event_bus_tags_merge_repair_run_events() {
        let (temp, paths, plan) = plan_fixture();
        save_plan(&paths, &plan).expect("save plan");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "repair".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run");
        append_plan_event(
            &paths,
            &plan.plan_id,
            PlanEventKind::MergeRepairRunDiscovered {
                run_id: state.run_id.clone(),
                pid: None,
            },
        )
        .expect("repair");
        emit_event(
            &state,
            None,
            RunEventKind::RunCompleted {
                status: "completed".to_string(),
            },
        )
        .expect("complete");
        let mut feed = PlanEventBus::file_tail(paths.clone(), plan.plan_id.clone());

        let events = feed.refresh(Duration::ZERO).await;

        assert!(events.iter().any(|event| {
            matches!(
                event,
                PlanFeedEvent::RepairRun { run_id, event }
                    if run_id == &state.run_id
                        && matches!(event.event, RunEventKind::RunCompleted { ref status } if status == "completed")
            )
        }));
    }

    fn plan_fixture() -> (TempDir, DeadreckonPaths, Plan) {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let mut tasks = vec![
            PlanTask::new(
                0,
                "first",
                "do first",
                PlanRole::Child,
                Some("smoke".to_string()),
            ),
            PlanTask::new(
                1,
                "second",
                "do second",
                PlanRole::Child,
                Some("smoke".to_string()),
            ),
        ];
        tasks[1].depends_on = vec!["task-0".to_string()];
        let mut plan = Plan::new(
            "build",
            PlanMode::FullPlan,
            tasks,
            PlanProviders {
                planner: Some("smoke:planner".to_string()),
                default_child: Some("smoke".to_string()),
                coder: None,
                reviewer: None,
                children: Default::default(),
                ..PlanProviders::default()
            },
            Some("scope".to_string()),
            "0.1.0",
        )
        .expect("plan");
        plan.capability_preview = CapabilityPreview {
            network: NetworkCapability::Allowlist,
            deploy: false,
            global_install: false,
            filesystem: Vec::new(),
            notes: Vec::new(),
        };
        (temp, paths, plan)
    }
}
