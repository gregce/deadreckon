use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::sync::broadcast;

use crate::error::Result;
use crate::state::{PipelineState, append_json_line};

pub const RUN_EVENTS_JSONL: &str = "events.jsonl";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunEventKind {
    TurnStarted {
        turn: u32,
    },
    ToolCallStarted {
        turn: u32,
        tool_call_id: String,
        tool_name: String,
        args: Value,
    },
    ToolCallResult {
        turn: u32,
        tool_call_id: String,
        status: String,
        preview: String,
    },
    TokenUsageDelta {
        turn: u32,
        input_tokens: u64,
        output_tokens: u64,
    },
    SpendDelta {
        turn: u32,
        cost_usd: f64,
        total_cost_usd: f64,
        wall_time_seconds: Option<f64>,
    },
    RunCompleted {
        status: String,
    },
    RunPromoted {
        library_dir: PathBuf,
    },
    Error {
        turn: Option<u32>,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEvent {
    pub timestamp: DateTime<Utc>,
    pub run_id: String,
    pub event: RunEventKind,
}

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
