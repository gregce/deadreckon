use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use deadreckon_protocol::{RunEvent, RunEventKind, SpendRecord, TraceRecord};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::artifacts::{DiffSummary, snapshot_diff};
use crate::docs::{DocKind, doc_path_for_kind};
use crate::error::{DeadreckonError, IoContext, JsonContext, Result};
use crate::events::RUN_EVENTS_JSONL;
use crate::gate::{
    AcceptanceCheckResult, AcceptanceMarker, AcceptanceProgressEntry,
    acceptance_progress_path_for_run_root, acceptance_spec_path_for_run_root,
    gate_nonce_path_for_run_root, marker_path_for_run_root, validate_acceptance_marker,
};
use crate::paths::DeadreckonPaths;
use crate::state::{PipelineState, RunStatus, load_state, spend_summary};
use crate::tamper::{AcceptanceTamper, acceptance_tamper_path_for_run_root};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunView {
    pub id: RunIdentity,
    pub goal: String,
    pub status: RunStatus,
    pub verdict: VerdictBand,
    pub signature: SignatureFact,
    pub sandbox: SandboxFact,
    pub spend: SpendBand,
    pub wall_secs: Option<u64>,
    pub provider: String,
    pub changed: DiffSummary,
    pub why: WhyBand,
    pub turns: Vec<TurnView>,
    pub proof: ProofBand,
    pub missing: Vec<Artifact>,
}

impl RunView {
    pub fn load(paths: &DeadreckonPaths, scope: &str, run_id: &str) -> Result<Self> {
        let state_path = paths.run_root(scope, run_id).join("state.json");
        let state = load_state(&state_path)?;
        Self::from_state(&state)
    }

    pub fn from_state(state: &PipelineState) -> Result<Self> {
        let mut missing = Vec::new();
        let traces = read_jsonl_or_missing::<TraceRecord>(
            &state.run_root.join("traces.jsonl"),
            Artifact::Traces,
            &mut missing,
        )?;
        let spend_records = read_jsonl_or_missing::<SpendRecord>(
            &state.run_root.join("spend.jsonl"),
            Artifact::Spend,
            &mut missing,
        )?;
        let events = read_jsonl_or_missing::<RunEvent>(
            &state.run_root.join(RUN_EVENTS_JSONL),
            Artifact::Events,
            &mut missing,
        )?;
        record_file_presence(
            &state.run_root.join("provenance.jsonl"),
            Artifact::Provenance,
            &mut missing,
        );
        record_file_presence(
            &acceptance_spec_path_for_run_root(&state.run_root),
            Artifact::Acceptance,
            &mut missing,
        );
        record_file_presence(
            &state.run_root.join("launch-plan.json"),
            Artifact::LaunchPlan,
            &mut missing,
        );
        record_file_presence(
            &state.run_root.join("seams.json"),
            Artifact::Seams,
            &mut missing,
        );
        record_file_presence(
            &gate_nonce_path_for_run_root(&state.run_root),
            Artifact::GateNonce,
            &mut missing,
        );
        let history_path = state.run_root.join("history.json");
        let history = read_history(&history_path, &mut missing)?;
        let sandbox = sandbox_fact(state, &mut missing)?;
        let proof = proof_band(state, &mut missing)?;
        let signature = signature_fact(&proof);
        let spend = spend_band(state)?;
        let changed = full_run_diff(state, &mut missing);
        let why = why_band(state, &mut missing);
        let turns = turn_views(
            state,
            &traces,
            &spend_records,
            &events,
            &history,
            &mut missing,
        );
        Ok(Self {
            id: RunIdentity {
                scope: state.scope.clone(),
                run_id: state.run_id.clone(),
                short: short_id(&state.run_id),
            },
            goal: state.goal.clone(),
            status: state.status,
            verdict: verdict_band(state, &proof),
            signature,
            sandbox,
            spend,
            wall_secs: wall_secs(state),
            provider: state
                .provider
                .clone()
                .unwrap_or_else(|| "unknown".to_string()),
            changed,
            why,
            turns,
            proof,
            missing,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunIdentity {
    pub scope: String,
    pub run_id: String,
    pub short: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerdictBand {
    pub state: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignatureFact {
    pub status: SignatureStatus,
    pub marker_path: Option<PathBuf>,
    pub tamper_path: Option<PathBuf>,
    pub tamper_verdict: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureStatus {
    Valid,
    Invalid,
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxFact {
    pub backend: String,
    pub path: Option<PathBuf>,
    pub tools: Vec<String>,
    pub fallback_note: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpendBand {
    pub total_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub wall_seconds: f64,
    pub turns: usize,
    pub subscription_turns: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhyBand {
    pub narrative_path: Option<PathBuf>,
    pub decisions_path: Option<PathBuf>,
    pub narrative_excerpt: Option<String>,
    pub decision_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnView {
    pub n: u32,
    pub did: String,
    pub diff: DiffSummary,
    pub spend_delta: Money,
    pub check: Option<CheckOutcome>,
    pub exchange_ref: Option<ExchangeRef>,
    pub sandbox_events: Vec<SandboxEvent>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Money {
    pub usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub wall_seconds: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckOutcome {
    pub status: String,
    pub index: usize,
    pub total: usize,
    pub result: Option<AcceptanceCheckResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExchangeRef {
    pub path: PathBuf,
    pub index: usize,
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SandboxEvent {
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    pub summary: String,
    pub raw: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProofBand {
    pub marker_path: Option<PathBuf>,
    pub marker_valid: bool,
    pub marker: Option<AcceptanceMarker>,
    pub checks: Vec<AcceptanceCheckResult>,
    pub progress_path: Option<PathBuf>,
    pub tamper_path: Option<PathBuf>,
    pub tamper: Option<AcceptanceTamper>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Artifact {
    State,
    History,
    Spend,
    Traces,
    Events,
    Provenance,
    Acceptance,
    Sandbox,
    LaunchPlan,
    Seams,
    Proofs,
    GateNonce,
    Snapshot(u32),
    Doc(RunViewDocKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunViewDocKind {
    Narrative,
    AsBuilt,
    Decisions,
    Delta,
}

impl From<DocKind> for RunViewDocKind {
    fn from(kind: DocKind) -> Self {
        match kind {
            DocKind::Narrative => Self::Narrative,
            DocKind::AsBuilt => Self::AsBuilt,
            DocKind::Decisions => Self::Decisions,
            DocKind::Delta => Self::Delta,
        }
    }
}

fn read_jsonl_or_missing<T>(
    path: &Path,
    artifact: Artifact,
    missing: &mut Vec<Artifact>,
) -> Result<Vec<T>>
where
    T: serde::de::DeserializeOwned,
{
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            missing.push(artifact);
            return Ok(Vec::new());
        }
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    let mut values = Vec::new();
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        if let Ok(value) = serde_json::from_str::<T>(line) {
            values.push(value);
        }
    }
    Ok(values)
}

fn record_file_presence(path: &Path, artifact: Artifact, missing: &mut Vec<Artifact>) {
    if !path.exists() {
        missing.push(artifact);
    }
}

fn read_history(path: &Path, missing: &mut Vec<Artifact>) -> Result<Vec<String>> {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            missing.push(Artifact::History);
            return Ok(Vec::new());
        }
        Err(source) => {
            return Err(DeadreckonError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    serde_json::from_str(&raw).map_err(|source| DeadreckonError::Json {
        path: path.to_path_buf(),
        source,
    })
}

fn sandbox_fact(state: &PipelineState, missing: &mut Vec<Artifact>) -> Result<SandboxFact> {
    let path = state.run_root.join("sandbox.toml");
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            missing.push(Artifact::Sandbox);
            return Ok(SandboxFact {
                backend: state.sandbox.clone(),
                path: None,
                tools: Vec::new(),
                fallback_note: Some("sandbox.toml absent; using state sandbox label".to_string()),
            });
        }
        Err(source) => return Err(DeadreckonError::Io { path, source }),
    };
    let value = toml::from_str::<toml::Value>(&raw).map_err(|source| {
        DeadreckonError::InvalidInput(format!("invalid sandbox.toml: {source}"))
    })?;
    let tools = value
        .get("tools")
        .and_then(toml::Value::as_table)
        .map(|table| table.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    Ok(SandboxFact {
        backend: state.sandbox.clone(),
        path: Some(path),
        tools,
        fallback_note: None,
    })
}

fn proof_band(state: &PipelineState, missing: &mut Vec<Artifact>) -> Result<ProofBand> {
    let proofs_dir = state.run_root.join("proofs");
    if !proofs_dir.exists() {
        missing.push(Artifact::Proofs);
    }
    let marker_path = marker_path_for_run_root(&state.run_root);
    let marker = if marker_path.exists() {
        Some(
            serde_json::from_slice::<AcceptanceMarker>(
                &fs::read(&marker_path).with_path(&marker_path)?,
            )
            .with_json_path(&marker_path)?,
        )
    } else {
        None
    };
    let marker_valid = marker.is_some() && validate_acceptance_marker(state).is_ok();
    let progress_path = acceptance_progress_path_for_run_root(&state.run_root);
    let progress = read_jsonl_or_missing::<AcceptanceProgressEntry>(
        &progress_path,
        Artifact::Proofs,
        missing,
    )?;
    let checks = marker
        .as_ref()
        .map(|marker| marker.checks.clone())
        .unwrap_or_else(|| {
            progress
                .iter()
                .filter_map(|entry| entry.result.clone())
                .collect::<Vec<_>>()
        });
    let tamper_path = acceptance_tamper_path_for_run_root(&state.run_root);
    let tamper = crate::tamper::read_acceptance_tamper_for_run_root(&state.run_root)?;
    let tamper_path = tamper_path.exists().then_some(tamper_path);
    Ok(ProofBand {
        marker_path: marker.as_ref().map(|_| marker_path),
        marker_valid,
        marker,
        checks,
        progress_path: progress_path.exists().then_some(progress_path),
        tamper_path,
        tamper,
    })
}

fn signature_fact(proof: &ProofBand) -> SignatureFact {
    let status = if proof.marker_path.is_none() {
        SignatureStatus::Absent
    } else if proof.marker_valid {
        SignatureStatus::Valid
    } else {
        SignatureStatus::Invalid
    };
    SignatureFact {
        status,
        marker_path: proof.marker_path.clone(),
        tamper_path: proof.tamper_path.clone(),
        tamper_verdict: proof
            .tamper
            .as_ref()
            .map(|tamper| format!("{:?}", tamper.verdict)),
    }
}

fn verdict_band(state: &PipelineState, proof: &ProofBand) -> VerdictBand {
    let state_label = if proof.marker_path.is_some() && proof.marker_valid {
        "VERIFIED"
    } else if proof.marker_path.is_some() {
        "REGRESSED"
    } else {
        "UNVERIFIED"
    };
    VerdictBand {
        state: state_label.to_string(),
        summary: format!(
            "{} run with {} proof check(s)",
            state.status,
            proof.checks.len()
        ),
    }
}

fn spend_band(state: &PipelineState) -> Result<SpendBand> {
    let summary = spend_summary(state)?;
    Ok(SpendBand {
        total_usd: summary.total_usd,
        input_tokens: summary.input_tokens,
        output_tokens: summary.output_tokens,
        wall_seconds: summary.wall_seconds,
        turns: summary.turns,
        subscription_turns: summary.subscription_turns,
    })
}

fn full_run_diff(state: &PipelineState, missing: &mut Vec<Artifact>) -> DiffSummary {
    if state.turn == 0 {
        return DiffSummary::default();
    }
    if !snapshot_path(state, 0).exists() {
        missing.push(Artifact::Snapshot(0));
        return DiffSummary::default();
    }
    if !snapshot_path(state, state.turn).exists() {
        missing.push(Artifact::Snapshot(state.turn));
        return DiffSummary::default();
    }
    snapshot_diff(state, 0, state.turn).unwrap_or_default()
}

fn why_band(state: &PipelineState, missing: &mut Vec<Artifact>) -> WhyBand {
    let narrative_path = doc_path_for_kind(&state.working_dir, DocKind::Narrative);
    let decisions_path = doc_path_for_kind(&state.working_dir, DocKind::Decisions);
    for kind in [
        DocKind::Narrative,
        DocKind::AsBuilt,
        DocKind::Decisions,
        DocKind::Delta,
    ] {
        if doc_path_for_kind(&state.working_dir, kind).is_none() {
            missing.push(Artifact::Doc(kind.into()));
        }
    }
    let narrative_excerpt = narrative_path
        .as_ref()
        .and_then(|path| first_substantive_line(path).ok().flatten());
    let decision_refs = decisions_path
        .as_ref()
        .map(|path| decision_lines(path).unwrap_or_default())
        .unwrap_or_default();
    WhyBand {
        narrative_path,
        decisions_path,
        narrative_excerpt,
        decision_refs,
    }
}

fn turn_views(
    state: &PipelineState,
    traces: &[TraceRecord],
    spend_records: &[SpendRecord],
    events: &[RunEvent],
    history: &[String],
    missing: &mut Vec<Artifact>,
) -> Vec<TurnView> {
    let progress = read_jsonl_or_missing::<AcceptanceProgressEntry>(
        &acceptance_progress_path_for_run_root(&state.run_root),
        Artifact::Proofs,
        missing,
    )
    .unwrap_or_default();
    (1..=state.turn)
        .map(|turn| TurnView {
            n: turn,
            did: turn_did(turn, traces),
            diff: turn_diff(state, turn, missing),
            spend_delta: turn_spend(turn, spend_records),
            check: turn_check(turn, state.turn, &progress),
            exchange_ref: history
                .get(turn.saturating_sub(1) as usize)
                .map(|entry| ExchangeRef {
                    path: state.run_root.join("history.json"),
                    index: turn.saturating_sub(1) as usize,
                    preview: one_line(entry, 160),
                }),
            sandbox_events: turn_sandbox_events(turn, events),
        })
        .collect()
}

fn turn_diff(state: &PipelineState, turn: u32, missing: &mut Vec<Artifact>) -> DiffSummary {
    let from = turn.saturating_sub(1);
    if !snapshot_path(state, from).exists() {
        missing.push(Artifact::Snapshot(from));
        return DiffSummary::default();
    }
    if !snapshot_path(state, turn).exists() {
        missing.push(Artifact::Snapshot(turn));
        return DiffSummary::default();
    }
    snapshot_diff(state, from, turn).unwrap_or_default()
}

fn snapshot_path(state: &PipelineState, turn: u32) -> PathBuf {
    state
        .run_root
        .join("snapshots")
        .join(format!("turn-{turn}"))
}

fn turn_did(turn: u32, traces: &[TraceRecord]) -> String {
    traces
        .iter()
        .rev()
        .find(|trace| trace.turn == turn)
        .map(|trace| {
            format!(
                "{} {}",
                trace.event,
                one_line(&trace.detail.to_string(), 120)
            )
        })
        .unwrap_or_else(|| "no trace recorded".to_string())
}

fn turn_spend(turn: u32, records: &[SpendRecord]) -> Money {
    records
        .iter()
        .filter(|record| record.turn == turn && record.kind == "loop")
        .fold(Money::default(), |mut total, record| {
            total.usd += record.cost_usd;
            total.input_tokens = total.input_tokens.saturating_add(record.input_tokens);
            total.output_tokens = total.output_tokens.saturating_add(record.output_tokens);
            total.wall_seconds += record.wall_time_seconds.unwrap_or(0.0);
            total
        })
}

fn turn_check(
    turn: u32,
    last_turn: u32,
    progress: &[AcceptanceProgressEntry],
) -> Option<CheckOutcome> {
    if turn != last_turn {
        return None;
    }
    progress.last().map(|entry| CheckOutcome {
        status: entry.status.clone(),
        index: entry.index,
        total: entry.total,
        result: entry.result.clone(),
    })
}

fn turn_sandbox_events(turn: u32, events: &[RunEvent]) -> Vec<SandboxEvent> {
    events
        .iter()
        .filter_map(|event| {
            let event_turn = run_event_turn(&event.event)?;
            if event_turn != turn {
                return None;
            }
            let raw = serde_json::to_value(&event.event).ok()?;
            Some(SandboxEvent {
                timestamp: event.timestamp,
                kind: run_event_kind_label(&event.event).to_string(),
                summary: run_event_summary(&event.event),
                raw,
            })
        })
        .collect()
}

fn run_event_turn(event: &RunEventKind) -> Option<u32> {
    match event {
        RunEventKind::TurnStarted { turn }
        | RunEventKind::ToolCallStarted { turn, .. }
        | RunEventKind::ToolCallResult { turn, .. }
        | RunEventKind::TokenUsageDelta { turn, .. }
        | RunEventKind::SpendDelta { turn, .. }
        | RunEventKind::DocsCheckpoint { turn, .. } => Some(*turn),
        RunEventKind::RunCompleted { .. } | RunEventKind::RunPromoted { .. } => None,
        RunEventKind::Error { turn, .. } => *turn,
    }
}

fn run_event_kind_label(event: &RunEventKind) -> &'static str {
    match event {
        RunEventKind::TurnStarted { .. } => "turn_started",
        RunEventKind::ToolCallStarted { .. } => "tool_call_started",
        RunEventKind::ToolCallResult { .. } => "tool_call_result",
        RunEventKind::TokenUsageDelta { .. } => "token_usage_delta",
        RunEventKind::SpendDelta { .. } => "spend_delta",
        RunEventKind::DocsCheckpoint { .. } => "docs_checkpoint",
        RunEventKind::RunCompleted { .. } => "run_completed",
        RunEventKind::RunPromoted { .. } => "run_promoted",
        RunEventKind::Error { .. } => "error",
    }
}

fn run_event_summary(event: &RunEventKind) -> String {
    match event {
        RunEventKind::TurnStarted { turn } => format!("turn {turn} started"),
        RunEventKind::ToolCallStarted {
            turn, tool_name, ..
        } => format!("turn {turn} tool {tool_name} started"),
        RunEventKind::ToolCallResult {
            turn,
            tool_call_id,
            status,
            preview,
        } => format!(
            "turn {turn} tool {tool_call_id} {status}: {}",
            one_line(preview, 80)
        ),
        RunEventKind::TokenUsageDelta {
            turn,
            input_tokens,
            output_tokens,
        } => format!("turn {turn} tokens +{input_tokens}/+{output_tokens}"),
        RunEventKind::SpendDelta {
            turn,
            cost_usd,
            total_cost_usd,
            ..
        } => format!("turn {turn} spend +{cost_usd:.4} total {total_cost_usd:.4}"),
        RunEventKind::DocsCheckpoint { turn, status, path } => {
            format!("turn {turn} docs {status} {}", path.display())
        }
        RunEventKind::RunCompleted { status } => format!("run completed {status}"),
        RunEventKind::RunPromoted { library_dir } => {
            format!("run promoted {}", library_dir.display())
        }
        RunEventKind::Error { turn, message } => {
            format!(
                "turn {} error {}",
                turn.unwrap_or_default(),
                one_line(message, 120)
            )
        }
    }
}

fn first_substantive_line(path: &Path) -> Result<Option<String>> {
    let raw = fs::read_to_string(path).with_path(path)?;
    Ok(raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("---"))
        .map(|line| one_line(line, 160)))
}

fn decision_lines(path: &Path) -> Result<Vec<String>> {
    let raw = fs::read_to_string(path).with_path(path)?;
    Ok(raw
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.starts_with('-')
                || line.starts_with('*')
                || line.to_ascii_lowercase().contains("decision")
        })
        .take(12)
        .map(|line| one_line(line, 160))
        .collect())
}

fn wall_secs(state: &PipelineState) -> Option<u64> {
    let seconds = (state.updated_at - state.started_at).num_seconds();
    u64::try_from(seconds).ok()
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn one_line(value: &str, max: usize) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        return collapsed;
    }
    let mut out = collapsed
        .chars()
        .take(max.saturating_sub(3))
        .collect::<String>();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::Utc;
    use tempfile::TempDir;

    use deadreckon_protocol::{RunEventKind, SpendRecord, TraceRecord};

    use crate::artifacts::{append_spend, append_trace, snapshot_working};
    use crate::events::emit_event;
    use crate::gate::{AcceptanceCheckResult, AcceptanceProgressEntry};
    use crate::state::{RunOptions, create_run, save_state};
    use crate::{DeadreckonPaths, RunStatus};

    use super::{Artifact, RunView};

    fn fixture_run() -> (TempDir, crate::PipelineState, DeadreckonPaths) {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let cwd = std::env::current_dir().expect("cwd");
        let mut state = create_run(
            &paths,
            RunOptions {
                goal: "logbook fixture".to_string(),
                cwd,
                sandbox: "none".to_string(),
                provider: Some("smoke".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("run-view-fixture".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        fs::write(state.working_dir.join("README.md"), "before\n").expect("readme");
        snapshot_working(&state, 0).expect("snapshot 0");
        fs::write(state.working_dir.join("README.md"), "before\nafter\n").expect("readme");
        state.turn = 1;
        state.status = RunStatus::Completed;
        save_state(&state).expect("save");
        snapshot_working(&state, 1).expect("snapshot 1");
        fs::write(
            state.run_root.join("history.json"),
            serde_json::to_string_pretty(&vec!["model exchange for turn one"]).expect("json"),
        )
        .expect("history");
        fs::write(
            state.run_root.join("sandbox.toml"),
            "version = 1\n\n[tools.bash]\nread = []\nwrite = []\nnetwork = []\n",
        )
        .expect("sandbox");
        fs::write(
            state.run_root.join("acceptance.yaml"),
            "name: fixture\nchecks: []\n",
        )
        .expect("acceptance");
        fs::write(state.run_root.join("launch-plan.json"), "{}\n").expect("launch");
        fs::write(state.run_root.join("seams.json"), "{}\n").expect("seams");
        fs::write(state.run_root.join("gate/nonce"), "secret").expect("nonce");
        append_spend(
            &state,
            &SpendRecord {
                timestamp: Utc::now(),
                turn: 1,
                provider: "smoke".to_string(),
                model: "model".to_string(),
                input_tokens: 5,
                output_tokens: 7,
                cost_usd: 0.01,
                total_cost_usd: 0.01,
                cap_usd: None,
                subscription: false,
                estimated: false,
                wall_time_seconds: Some(2.0),
                wall_time_cap_seconds: None,
                kind: "loop".to_string(),
            },
        )
        .expect("spend");
        append_trace(
            &state,
            &TraceRecord {
                timestamp: Utc::now(),
                run_id: state.run_id.clone(),
                turn: 1,
                event: "provider_done".to_string(),
                latency_ms: None,
                detail: serde_json::json!({"summary":"changed readme"}),
            },
        )
        .expect("trace");
        emit_event(
            &state,
            None,
            RunEventKind::ToolCallResult {
                turn: 1,
                tool_call_id: "tool-1".to_string(),
                status: "ok".to_string(),
                preview: "wrote README".to_string(),
            },
        )
        .expect("event");
        fs::create_dir_all(state.run_root.join("proofs")).expect("proofs");
        crate::state::append_json_line(
            &crate::gate::acceptance_progress_path_for_run_root(&state.run_root),
            &AcceptanceProgressEntry {
                checked_at: Utc::now(),
                status: "passed".to_string(),
                index: 1,
                total: 1,
                result: Some(AcceptanceCheckResult {
                    kind: "shell".to_string(),
                    passed: true,
                    must_pass: true,
                    detail: "ok".to_string(),
                    command: Some("true".to_string()),
                    cwd: None,
                    duration_ms: None,
                    stdout: None,
                    stderr: None,
                }),
            },
        )
        .expect("progress");
        (temp, state, paths)
    }

    #[test]
    fn run_view_load_assembles_all_bands_from_fixture_run() {
        let (_temp, state, paths) = fixture_run();

        let view = RunView::load(&paths, &state.scope, &state.run_id).expect("view");

        assert_eq!(view.id.run_id, state.run_id);
        assert_eq!(view.goal, "logbook fixture");
        assert_eq!(view.provider, "smoke");
        assert_eq!(view.changed.files_changed, 1);
        assert_eq!(view.turns.len(), 1);
        assert_eq!(
            view.turns[0].exchange_ref.as_ref().expect("exchange").index,
            0
        );
        assert_eq!(view.turns[0].sandbox_events.len(), 1);
        assert_eq!(view.proof.checks.len(), 1);
    }

    #[test]
    fn run_view_load_records_absent_files_in_missing_not_panic() {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "partial run".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "none".to_string(),
                provider: None,
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("partial-run".to_string()),
                codebase: None,
            },
        )
        .expect("run");

        let view = RunView::load(&paths, &state.scope, &state.run_id).expect("view");

        assert!(view.missing.contains(&Artifact::History));
        assert!(view.missing.contains(&Artifact::Events));
        assert!(view.missing.contains(&Artifact::Sandbox));
    }

    #[test]
    fn run_view_serializes_stable_json_shape() {
        let (_temp, state, paths) = fixture_run();
        let view = RunView::load(&paths, &state.scope, &state.run_id).expect("view");

        let value = serde_json::to_value(&view).expect("json");

        assert_eq!(value["id"]["short"], "run-view");
        assert!(value.get("verdict").is_some(), "{value}");
        assert!(value.get("signature").is_some(), "{value}");
        assert!(value.get("changed").is_some(), "{value}");
        assert!(value.get("turns").is_some(), "{value}");
        assert!(value.get("proof").is_some(), "{value}");
    }

    #[test]
    fn run_view_sandbox_fact_names_backend_from_sandbox_toml() {
        let (_temp, state, paths) = fixture_run();
        let view = RunView::load(&paths, &state.scope, &state.run_id).expect("view");

        assert_eq!(view.sandbox.backend, "none");
        assert_eq!(view.sandbox.tools, vec!["bash"]);
    }

    #[test]
    fn run_view_turn_carries_model_exchange_ref_from_history() {
        let (_temp, state, paths) = fixture_run();
        let view = RunView::load(&paths, &state.scope, &state.run_id).expect("view");

        let exchange = view.turns[0].exchange_ref.as_ref().expect("exchange");
        assert!(exchange.preview.contains("model exchange"));
    }

    #[test]
    fn run_view_turn_carries_sandbox_events_from_events_jsonl() {
        let (_temp, state, paths) = fixture_run();
        let view = RunView::load(&paths, &state.scope, &state.run_id).expect("view");

        assert_eq!(view.turns[0].sandbox_events[0].kind, "tool_call_result");
        assert!(
            view.turns[0].sandbox_events[0]
                .summary
                .contains("wrote README")
        );
    }
}
