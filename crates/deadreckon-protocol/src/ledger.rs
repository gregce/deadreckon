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

/// Default `kind` for spend rows written before the field existed and for the
/// run loop's own turns. The live narrator writes `"narrator"` instead.
pub fn spend_kind_loop() -> String {
    "loop".to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SpendRecord {
    pub timestamp: DateTime<Utc>,
    pub turn: u32,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    pub total_cost_usd: f64,
    pub cap_usd: Option<f64>,
    #[serde(default)]
    pub subscription: bool,
    #[serde(default)]
    pub estimated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_time_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_time_cap_seconds: Option<f64>,
    /// `"loop"` for the run loop's own turns, `"narrator"` for live-narration
    /// calls. Defaulted so legacy spend.jsonl rows still parse.
    #[serde(default = "spend_kind_loop")]
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TraceRecord {
    pub timestamp: DateTime<Utc>,
    pub run_id: String,
    pub turn: u32,
    pub event: String,
    pub latency_ms: Option<u128>,
    pub detail: Value,
}
