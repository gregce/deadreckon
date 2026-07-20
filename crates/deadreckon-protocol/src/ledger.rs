//! Persisted ledger vocabulary.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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
    DocsCheckpoint {
        turn: u32,
        path: PathBuf,
        status: String,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunEvent {
    pub timestamp: DateTime<Utc>,
    pub run_id: String,
    pub event: RunEventKind,
}
