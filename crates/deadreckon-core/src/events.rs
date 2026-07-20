use chrono::Utc;
pub use deadreckon_protocol::{RunEvent, RunEventKind};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::error::Result;
use crate::state::{PipelineState, append_json_line};

pub const RUN_EVENTS_JSONL: &str = "events.jsonl";

#[derive(Clone)]
pub struct RunEventBus {
    sender: broadcast::Sender<RunEvent>,
}

impl RunEventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RunEvent> {
        self.sender.subscribe()
    }

    pub fn emit(&self, state: &PipelineState, event: RunEventKind) -> Result<()> {
        emit_event(state, Some(&self.sender), event)
    }

    pub fn sender(&self) -> broadcast::Sender<RunEvent> {
        self.sender.clone()
    }
}

pub fn emit_event(
    state: &PipelineState,
    sender: Option<&broadcast::Sender<RunEvent>>,
    event: RunEventKind,
) -> Result<()> {
    let event = RunEvent {
        timestamp: Utc::now(),
        run_id: state.run_id.clone(),
        event,
    };
    append_json_line(&state.run_root.join(RUN_EVENTS_JSONL), &event)?;
    if let Some(sender) = sender {
        let _ = sender.send(event);
    }
    Ok(())
}

pub fn event_preview(value: impl AsRef<str>) -> String {
    let value = value.as_ref().replace('\n', " ");
    const MAX: usize = 160;
    if value.len() <= MAX {
        value
    } else {
        format!("{}...", &value[..MAX])
    }
}

pub fn tool_args_json(command_or_path: impl Into<String>) -> Value {
    json!({ "value": command_or_path.into() })
}
