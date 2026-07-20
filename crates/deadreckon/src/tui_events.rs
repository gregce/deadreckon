use std::collections::BTreeSet;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::Duration;

use deadreckon_protocol::RunEvent;
use tokio::sync::broadcast;

pub(crate) struct TuiEventFeed {
    source: TuiEventSource,
    seen: BTreeSet<String>,
}

enum TuiEventSource {
    #[cfg_attr(not(test), allow(dead_code))]
    Broadcast {
        receiver: broadcast::Receiver<RunEvent>,
        replay: JsonlTail,
    },
    FileTail(JsonlTail),
}

impl TuiEventFeed {
    #[cfg(test)]
    pub(crate) fn from_broadcast(
        receiver: broadcast::Receiver<RunEvent>,
        replay_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            source: TuiEventSource::Broadcast {
                receiver,
                replay: JsonlTail::new(replay_path.into()),
            },
            seen: BTreeSet::new(),
        }
    }

    pub(crate) fn file_tail(path: impl Into<PathBuf>) -> Self {
        Self {
            source: TuiEventSource::FileTail(JsonlTail::new(path.into())),
            seen: BTreeSet::new(),
        }
    }

    pub(crate) async fn refresh(&mut self, wait: Duration) -> std::io::Result<Vec<RunEvent>> {
        let mut events = Vec::new();
        match &mut self.source {
            TuiEventSource::Broadcast { receiver, replay } => {
                drain_receiver(receiver, &mut events);
                if events.is_empty() && !wait.is_zero() {
                    match tokio::time::timeout(wait, receiver.recv()).await {
                        Ok(Ok(event)) => events.push(event),
                        Ok(Err(broadcast::error::RecvError::Lagged(_)))
                        | Ok(Err(broadcast::error::RecvError::Closed)) => {
                            events.extend(replay.read_new()?);
                        }
                        Err(_) => {}
                    }
                }
                drain_receiver(receiver, &mut events);
                if events.is_empty() {
                    events.extend(replay.read_new()?);
                } else {
                    replay.skip_to_end();
                }
            }
            TuiEventSource::FileTail(replay) => {
                events.extend(replay.read_new()?);
            }
        }
        Ok(self.dedupe(events))
    }

    #[cfg(test)]
    fn is_broadcast(&self) -> bool {
        matches!(self.source, TuiEventSource::Broadcast { .. })
    }
}

fn drain_receiver(receiver: &mut broadcast::Receiver<RunEvent>, events: &mut Vec<RunEvent>) {
    loop {
        match receiver.try_recv() {
            Ok(event) => events.push(event),
            Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
            Err(broadcast::error::TryRecvError::Empty)
            | Err(broadcast::error::TryRecvError::Closed) => break,
        }
    }
}

impl TuiEventFeed {
    fn dedupe(&mut self, events: Vec<RunEvent>) -> Vec<RunEvent> {
        events
            .into_iter()
            .filter(|event| {
                let key = serde_json::to_string(event).unwrap_or_else(|_| {
                    format!("{}:{}", event.timestamp.to_rfc3339(), event.run_id)
                });
                self.seen.insert(key)
            })
            .collect()
    }
}

struct JsonlTail {
    path: PathBuf,
    offset: u64,
    partial: String,
}

impl JsonlTail {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            offset: 0,
            partial: String::new(),
        }
    }

    fn read_new(&mut self) -> std::io::Result<Vec<RunEvent>> {
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
        let mut lines = buffer.lines().map(str::to_string).collect::<Vec<_>>();
        if !complete {
            self.partial = lines.pop().unwrap_or_default();
        }
        Ok(lines
            .into_iter()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<RunEvent>(&line).ok())
            .collect())
    }

    fn skip_to_end(&mut self) {
        self.offset = fs::metadata(&self.path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        self.partial.clear();
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
    use deadreckon_core::events::emit_event;
    use deadreckon_core::paths::DeadreckonPaths;
    use deadreckon_core::state::{RunOptions, create_run};
    use deadreckon_core::{RUN_EVENTS_JSONL, RunEventBus};
    use deadreckon_protocol::{RunEvent, RunEventKind};
    use tempfile::TempDir;

    use super::TuiEventFeed;

    #[tokio::test]
    async fn same_process_attach_uses_broadcast_channel() {
        let temp = TempDir::new().expect("tempdir");
        let state = test_state(&temp);
        let bus = RunEventBus::new(8);
        let mut feed =
            TuiEventFeed::from_broadcast(bus.subscribe(), state.run_root.join(RUN_EVENTS_JSONL));

        bus.emit(&state, RunEventKind::TurnStarted { turn: 1 })
            .expect("emit");
        let events = feed
            .refresh(Duration::from_millis(50))
            .await
            .expect("events");

        assert!(feed.is_broadcast());
        assert!(matches!(
            events.first().map(|event| &event.event),
            Some(RunEventKind::TurnStarted { turn: 1 })
        ));
    }

    #[tokio::test]
    async fn cross_process_attach_replays_events_jsonl() {
        let temp = TempDir::new().expect("tempdir");
        let state = test_state(&temp);
        emit_event(&state, None, RunEventKind::TurnStarted { turn: 1 }).expect("turn");
        emit_event(
            &state,
            None,
            RunEventKind::ToolCallStarted {
                turn: 1,
                tool_call_id: "tool-1".to_string(),
                tool_name: "bash".to_string(),
                args: serde_json::json!({"value":"true"}),
            },
        )
        .expect("tool");

        let mut feed = TuiEventFeed::file_tail(state.run_root.join(RUN_EVENTS_JSONL));
        let first = feed.refresh(Duration::ZERO).await.expect("first");
        let second = feed.refresh(Duration::ZERO).await.expect("second");

        assert_eq!(first.len(), 2);
        assert!(second.is_empty());
    }

    #[tokio::test]
    async fn kill_mid_turn_surfaces_run_completed_event_within_200ms() {
        let temp = TempDir::new().expect("tempdir");
        let state = test_state(&temp);
        let path = state.run_root.join(RUN_EVENTS_JSONL);
        let mut feed = TuiEventFeed::file_tail(&path);
        let state_for_task = state.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(40)).await;
            emit_event(
                &state_for_task,
                None,
                RunEventKind::RunCompleted {
                    status: "killed".to_string(),
                },
            )
            .expect("emit");
        });

        let deadline = tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                for event in feed
                    .refresh(Duration::from_millis(10))
                    .await
                    .expect("events")
                {
                    if matches!(
                        event.event,
                        RunEventKind::RunCompleted { ref status } if status == "killed"
                    ) {
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        assert!(
            deadline.is_ok(),
            "run_completed(killed) missed 200ms attach SLA"
        );
    }

    #[tokio::test]
    async fn attach_falls_back_to_file_replay_when_broadcast_unavailable() {
        let temp = TempDir::new().expect("tempdir");
        let state = test_state(&temp);
        let bus = RunEventBus::new(1);
        let receiver = bus.subscribe();
        emit_event(&state, None, RunEventKind::TurnStarted { turn: 7 }).expect("file event");
        drop(bus);

        let mut feed =
            TuiEventFeed::from_broadcast(receiver, state.run_root.join(RUN_EVENTS_JSONL));
        let events = feed
            .refresh(Duration::from_millis(50))
            .await
            .expect("events");

        assert!(matches!(
            events.first().map(|event| &event.event),
            Some(RunEventKind::TurnStarted { turn: 7 })
        ));
    }

    fn test_state(temp: &TempDir) -> deadreckon_core::PipelineState {
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        create_run(
            &paths,
            RunOptions {
                goal: "tui events".to_string(),
                cwd: temp.path().to_path_buf(),
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: Some(1.0),
                max_wall_seconds: None,
                run_id: None,
                codebase: None,
            },
        )
        .expect("run")
    }

    #[test]
    fn partial_jsonl_line_waits_for_completion() {
        let temp = TempDir::new().expect("tempdir");
        let state = test_state(&temp);
        let path = state.run_root.join(RUN_EVENTS_JSONL);
        let event = RunEvent {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            event: RunEventKind::TurnStarted { turn: 3 },
        };
        let raw = serde_json::to_string(&event).expect("json");
        std::fs::write(&path, &raw[..raw.len() / 2]).expect("partial");

        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let mut feed = TuiEventFeed::file_tail(&path);
        let first = runtime
            .block_on(feed.refresh(Duration::ZERO))
            .expect("first");
        std::fs::write(&path, format!("{raw}\n")).expect("complete");
        let second = runtime
            .block_on(feed.refresh(Duration::ZERO))
            .expect("second");

        assert!(first.is_empty());
        assert_eq!(second.len(), 1);
    }
}
