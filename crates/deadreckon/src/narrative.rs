use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use clap::ValueEnum;
use deadreckon_core::flight::{FlightEvent, read_flight_events};
use deadreckon_core::{
    DeadreckonPaths, Plan, PlanEvent, PlanEventKind, PlanMessage, PlanMessageKind, PlanStatus,
    PlanTaskStatus, RUN_EVENTS_JSONL, RunEvent, RunEventKind, RunStatus, SpendRecord, TraceRecord,
    TurnRecord, acceptance_progress_path_for_run_root, load_run, marker_path_for_run_root,
    plan_status_label, plan_task_status_label, run_status_label,
};
use regex::Regex;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::plan_event_bus::PlanFeedEvent;

const NARRATIVE_DIR: &str = "narrative";
const NARRATIVE_STATE_JSON: &str = "state.json";
const NARRATIVE_SNAPSHOTS_JSONL: &str = "snapshots.jsonl";
const ARCHITECTURE_GRAPH_JSON: &str = "architecture-graph.json";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttachViewMode {
    #[default]
    Activity,
    Narrative,
    Split,
}

impl AttachViewMode {
    pub(crate) fn is_narrative(self) -> bool {
        matches!(self, Self::Narrative | Self::Split)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NarrativeVisualMode {
    #[default]
    Architecture,
    Agents,
    Files,
    Evidence,
    None,
}

impl NarrativeVisualMode {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Architecture => Self::Agents,
            Self::Agents => Self::Files,
            Self::Files => Self::Evidence,
            Self::Evidence => Self::None,
            Self::None => Self::Architecture,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Architecture => "architecture",
            Self::Agents => "agents",
            Self::Files => "files",
            Self::Evidence => "evidence",
            Self::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LiveFileFact {
    pub(crate) path: String,
    pub(crate) bytes: u64,
    pub(crate) modified_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ParentPlanFact {
    pub(crate) plan_id: String,
    pub(crate) task_id: String,
}

#[derive(Debug)]
pub(crate) struct RunNarrativeInput<'a> {
    pub(crate) state: &'a deadreckon_core::PipelineState,
    pub(crate) spend: &'a [SpendRecord],
    pub(crate) traces: &'a [TraceRecord],
    pub(crate) events: &'a [RunEvent],
    pub(crate) live_files: Vec<LiveFileFact>,
    pub(crate) file_count: usize,
    pub(crate) total_bytes: u64,
    pub(crate) acceptance_summary: String,
    pub(crate) provider_activity: &'a [String],
    pub(crate) parent_plan: Option<ParentPlanFact>,
}

#[derive(Debug)]
pub(crate) struct PlanNarrativeInput<'a> {
    pub(crate) paths: &'a DeadreckonPaths,
    pub(crate) plan: &'a Plan,
    pub(crate) messages: &'a [PlanMessage],
    pub(crate) plan_events: &'a [PlanEvent],
    pub(crate) feed_events: &'a [PlanFeedEvent],
    pub(crate) selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NarrativeScope {
    Run,
    Plan,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NarrativeStatus {
    Fresh,
    Stale,
    Failed,
    Disabled,
    Redacted,
    Deterministic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct NarrativeState {
    pub(crate) version: u32,
    pub(crate) scope: NarrativeScope,
    pub(crate) target_id: String,
    pub(crate) latest_snapshot_id: String,
    pub(crate) latest_status: NarrativeStatus,
    pub(crate) latest_created_at: Option<DateTime<Utc>>,
    pub(crate) latest_covered: NarrativeCoverage,
    pub(crate) cadence: NarrativeCadence,
    pub(crate) provider: NarrativeProviderState,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct NarrativeCoverage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) run_event_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) trace_count: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) flight_event_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) checkpoint_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) plan_event_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) child_runs: BTreeMap<String, NarrativeChildCoverage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) repair_run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) doc_inputs_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) architecture_graph_hash: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NarrativeChildCoverage {
    pub(crate) run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) run_event_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) flight_event_seq: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct NarrativeCadence {
    pub(crate) mode: String,
    pub(crate) min_seconds_between_provider_calls: u64,
    pub(crate) quiet_seconds: u64,
    pub(crate) max_provider_calls_per_attach: u32,
}

impl Default for NarrativeCadence {
    fn default() -> Self {
        Self {
            mode: "event-driven".to_string(),
            min_seconds_between_provider_calls: 45,
            quiet_seconds: 30,
            max_provider_calls_per_attach: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct NarrativeProviderState {
    pub(crate) route: Option<String>,
    pub(crate) source: String,
    pub(crate) model: Option<String>,
    pub(crate) calls: u32,
    pub(crate) cost_usd: f64,
    pub(crate) subscription_seconds: f64,
}

/// Who wrote a snapshot. `Live` = the in-process narrator the run spawns;
/// `Attach` = an on-demand refresh issued from the attach TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NarrativeSource {
    Live,
    Attach,
}

/// Schema-2 continuity fields carried by a live narration beat. Absent on
/// legacy schema-1 snapshots (which deserialize with `live: None`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LiveBeat {
    pub(crate) beat_seq: u64,
    pub(crate) covers_turn: u32,
    pub(crate) source: NarrativeSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rolling_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct NarrativeSnapshot {
    pub(crate) version: u32,
    pub(crate) snapshot_id: String,
    pub(crate) scope: NarrativeScope,
    pub(crate) target_id: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) status: NarrativeStatus,
    pub(crate) source_window: NarrativeSourceWindow,
    pub(crate) coverage: NarrativeSnapshotCoverage,
    pub(crate) headline: String,
    pub(crate) current_work: Vec<NarrativeClaim>,
    pub(crate) architecture_notes: Vec<NarrativeClaim>,
    pub(crate) risks: Vec<NarrativeClaim>,
    pub(crate) next_likely: Vec<NarrativeClaim>,
    pub(crate) citations: Vec<NarrativeCitation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) plan_status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) agent_table: Vec<NarrativeAgentRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) coordination_notes: Vec<NarrativeClaim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) live: Option<LiveBeat>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct NarrativeSourceWindow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) run_events: Option<SeqWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) traces: Option<IndexWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) flight_events: Option<SeqWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) plan_events: Option<SeqWindow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) checkpoints: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) docs_hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SeqWindow {
    pub(crate) from_seq: u64,
    pub(crate) to_seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IndexWindow {
    pub(crate) from_index: usize,
    pub(crate) to_index: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NarrativeSnapshotCoverage {
    pub(crate) skipped_events: usize,
    pub(crate) redacted_events: usize,
    pub(crate) known_gaps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NarrativeClaim {
    pub(crate) text: String,
    pub(crate) evidence: Vec<String>,
    pub(crate) confidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NarrativeCitation {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NarrativeAgentRow {
    pub(crate) task_id: String,
    pub(crate) role: String,
    pub(crate) provider: Option<String>,
    pub(crate) status: String,
    pub(crate) summary: String,
    pub(crate) evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ArchitectureGraph {
    pub(crate) version: u32,
    pub(crate) graph_id: String,
    pub(crate) scope: NarrativeScope,
    pub(crate) target_id: String,
    pub(crate) generated_at: DateTime<Utc>,
    pub(crate) source_window: NarrativeSourceWindow,
    pub(crate) default_visual: NarrativeVisualMode,
    pub(crate) nodes: Vec<ArchitectureNode>,
    pub(crate) edges: Vec<ArchitectureEdge>,
    pub(crate) groups: Vec<ArchitectureGroup>,
    pub(crate) layout: ArchitectureLayout,
    pub(crate) legend: Vec<ArchitectureLegendItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ArchitectureNode {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) weight: u32,
    pub(crate) evidence: Vec<String>,
    pub(crate) style_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ArchitectureEdge {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) label: String,
    pub(crate) kind: String,
    pub(crate) evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ArchitectureGroup {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) node_ids: Vec<String>,
    pub(crate) evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ArchitectureLayout {
    pub(crate) kind: String,
    pub(crate) root_ids: Vec<String>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ArchitectureLegendItem {
    pub(crate) style_token: String,
    pub(crate) meaning: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct NarrativeProjection {
    pub(crate) state: NarrativeState,
    pub(crate) snapshot: NarrativeSnapshot,
    pub(crate) graph: ArchitectureGraph,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct LatestNarrativeSnapshot {
    pub(crate) snapshot: Option<NarrativeSnapshot>,
    pub(crate) skipped_malformed_rows: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NarrativeRedactionReport {
    pub(crate) text: String,
    pub(crate) findings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NarrativeRefreshDecision {
    Eligible,
    NoProvider,
    OverBudget,
    TooSoon,
    CallLimitReached,
}

#[derive(Debug, Clone)]
pub(crate) struct NarrativeRefreshPolicy {
    pub(crate) provider_route: Option<String>,
    pub(crate) max_spend_usd: Option<f64>,
    pub(crate) manual: bool,
    pub(crate) meaningful_delta: bool,
    pub(crate) now: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NarrativeProviderRefresh {
    pub(crate) route: String,
    pub(crate) model: String,
    pub(crate) cost_usd: f64,
    pub(crate) subscription_seconds: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct NarrativePromptBundle {
    pub(crate) prompt: String,
    pub(crate) redaction: NarrativeRedactionReport,
    pub(crate) evidence_ids: Vec<String>,
    pub(crate) graph_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct ProviderNarrativeOutput {
    #[serde(default)]
    headline: Option<String>,
    #[serde(default)]
    current_work: Vec<NarrativeClaim>,
    #[serde(default)]
    architecture_notes: Vec<NarrativeClaim>,
    #[serde(default)]
    risks: Vec<NarrativeClaim>,
    #[serde(default)]
    next_likely: Vec<NarrativeClaim>,
    #[serde(default)]
    graph_labels: Vec<ProviderGraphLabel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ProviderGraphLabel {
    target_id: String,
    label: String,
}

pub(crate) fn ensure_run_projection(
    input: &RunNarrativeInput<'_>,
) -> crate::Result<NarrativeProjection> {
    let narrative_dir = input.state.run_root.join(NARRATIVE_DIR);
    let projection = build_run_projection(input);
    if let Some(current) = read_current_projection_if_covered(&projection, &narrative_dir) {
        persist_projection(&current, &narrative_dir)?;
        return Ok(current);
    }
    persist_projection(&projection, &narrative_dir)?;
    Ok(projection)
}

pub(crate) fn ensure_plan_projection(
    input: &PlanNarrativeInput<'_>,
) -> crate::Result<NarrativeProjection> {
    let plan_dir = input.paths.plan_dir(&input.plan.plan_id);
    let narrative_dir = plan_dir.join(NARRATIVE_DIR);
    let projection = build_plan_projection(input);
    if let Some(current) = read_current_projection_if_covered(&projection, &narrative_dir) {
        persist_projection(&current, &narrative_dir)?;
        return Ok(current);
    }
    persist_projection(&projection, &narrative_dir)?;
    Ok(projection)
}

pub(crate) fn persist_run_projection(
    state: &deadreckon_core::PipelineState,
    projection: &NarrativeProjection,
) -> crate::Result<()> {
    persist_projection(projection, &state.run_root.join(NARRATIVE_DIR))
}

pub(crate) fn persist_plan_projection(
    paths: &DeadreckonPaths,
    plan: &Plan,
    projection: &NarrativeProjection,
) -> crate::Result<()> {
    persist_projection(
        projection,
        &paths.plan_dir(&plan.plan_id).join(NARRATIVE_DIR),
    )
}

pub(crate) fn build_run_projection(input: &RunNarrativeInput<'_>) -> NarrativeProjection {
    let state = input.state;
    let flight_events = read_flight_events(state).unwrap_or_default();
    let mut files = collect_run_files(input, &flight_events);
    files.sort();
    files.dedup();
    let checkpoints = flight_events
        .iter()
        .filter_map(|event| event.checkpoint_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let run_prefix = short_id(&state.run_id);
    let state_evidence = format!("file:{}", state.state_path().display());
    let acceptance_evidence =
        acceptance_artifact_for_run(state).map(|(kind, path)| (kind, file_evidence_id(&path)));
    let latest_run_event = input.events.last();
    let latest_trace = input.traces.last();
    let latest_flight = flight_events.last();
    let latest_run_event_id = latest_run_event
        .map(|_| format!("run:{run_prefix}:event:{}", input.events.len()))
        .unwrap_or_else(|| state_evidence.clone());
    let latest_trace_id = latest_trace
        .map(|trace| format!("trace:{run_prefix}:turn-{}:{}", trace.turn, trace.event))
        .unwrap_or_else(|| state_evidence.clone());
    let latest_flight_id = latest_flight
        .map(|event| format!("flight:{run_prefix}:seq:{}", event.seq))
        .unwrap_or_else(|| latest_run_event_id.clone());
    let provider = state.provider.clone();
    let status = run_status_label(state.status).to_string();
    let phase = state
        .active_phase()
        .map(|phase| phase.name.clone())
        .unwrap_or_else(|| "phase".to_string());
    let headline = match state.status {
        RunStatus::Executing => format!(
            "Run {} is executing {} with {}.",
            run_prefix,
            phase,
            provider.as_deref().unwrap_or("the configured provider")
        ),
        RunStatus::Completed => format!("Run {run_prefix} completed and is ready for inspection."),
        RunStatus::Failed => format!("Run {run_prefix} failed; inspect risks and evidence."),
        RunStatus::Killed => format!("Run {run_prefix} was killed; work state is preserved."),
        RunStatus::Pending | RunStatus::Planned => {
            format!("Run {run_prefix} is preparing to execute.")
        }
    };
    let mut run_state_evidence = vec![state_evidence.clone()];
    if let Some((_, evidence_id)) = acceptance_evidence.as_ref() {
        run_state_evidence.push(evidence_id.clone());
    }
    let mut current_work = vec![claim(
        format!(
            "[{}] turn {} in {} ({})",
            status, state.turn, phase, input.acceptance_summary
        ),
        run_state_evidence,
        "high",
    )];
    if let Some(event) = latest_run_event {
        current_work.push(claim(
            format!("Latest run event: {}.", run_event_summary(&event.event)),
            vec![latest_run_event_id.clone()],
            "high",
        ));
    }
    if let Some(trace) = latest_trace {
        current_work.push(claim(
            format!("Latest trace: turn {} {}.", trace.turn, trace.event),
            vec![latest_trace_id],
            "high",
        ));
    }
    if let Some(event) = latest_flight {
        current_work.push(claim(
            format!(
                "Latest provider-native event: {}.",
                one_line(&event.summary, 140)
            ),
            vec![latest_flight_id],
            "high",
        ));
    } else if !input.provider_activity.is_empty() {
        current_work.push(claim(
            format!(
                "Live provider activity is available but has not been converted into durable flight rows yet: {}.",
                one_line(input.provider_activity.last().unwrap_or(&String::new()), 140)
            ),
            vec![state_evidence.clone()],
            "medium",
        ));
    }
    current_work.push(claim(
        format!(
            "Working tree view sees {} file(s), {} bytes, latest file {}.",
            input.file_count,
            input.total_bytes,
            input
                .live_files
                .first()
                .map(|file| {
                    let age = file
                        .modified_at
                        .map(|modified| {
                            Utc::now()
                                .signed_duration_since(modified)
                                .num_seconds()
                                .max(0)
                        })
                        .map(|seconds| format!("{seconds}s ago"))
                        .unwrap_or_else(|| "unknown age".to_string());
                    format!("{} ({} bytes, {age})", file.path, file.bytes)
                })
                .unwrap_or_else(|| "none yet".to_string())
        ),
        vec![format!("file:{}", state.working_dir.display())],
        "medium",
    ));
    let mut architecture_notes = Vec::new();
    if files.is_empty() {
        architecture_notes.push(claim(
            "No changed-file evidence is visible yet, so the architecture map remains sparse."
                .to_string(),
            vec![state_evidence.clone()],
            "medium",
        ));
    } else {
        architecture_notes.push(claim(
            format!(
                "The visible work clusters around {}.",
                file_cluster_summary(&files)
            ),
            file_evidence(&files),
            "medium",
        ));
    }
    if let Some(parent) = input.parent_plan.as_ref() {
        architecture_notes.push(claim(
            format!(
                "This run is attached as {} in plan {}.",
                parent.task_id,
                short_id(&parent.plan_id)
            ),
            vec![format!(
                "plan:{}:task:{}",
                short_id(&parent.plan_id),
                parent.task_id
            )],
            "high",
        ));
    }
    let mut risks = Vec::new();
    match state.status {
        RunStatus::Failed => risks.push(claim(
            state
                .failure_reason
                .clone()
                .unwrap_or_else(|| "Run failed; no failure reason was recorded.".to_string()),
            vec![state_evidence.clone()],
            "high",
        )),
        RunStatus::Killed => risks.push(claim(
            "Run was killed before normal completion.".to_string(),
            vec![state_evidence.clone()],
            "high",
        )),
        _ => {}
    }
    if matches!(input.acceptance_summary.as_str(), s if s.contains("failed")) {
        risks.push(claim(
            format!(
                "Acceptance status needs attention: {}.",
                input.acceptance_summary
            ),
            acceptance_evidence
                .as_ref()
                .map(|(_, evidence_id)| vec![evidence_id.clone()])
                .unwrap_or_else(|| vec![state_evidence.clone()]),
            "high",
        ));
    }
    risks.push(claim(
        "Provider-backed narration has not run yet; deterministic fallback facts are shown."
            .to_string(),
        vec![state_evidence],
        "high",
    ));
    let next_likely = vec![claim(
        if state.status == RunStatus::Executing {
            "Watch for the provider turn to finish, then inspect acceptance and changed files."
                .to_string()
        } else {
            "Use activity, docs, show, or apply/abandon lifecycle actions for the next inspection step."
                .to_string()
        },
        vec![latest_run_event_id.clone()],
        "low",
    )];
    let citations = run_citations(state, input.events, input.traces, &flight_events, &files);
    let source_window = NarrativeSourceWindow {
        run_events: seq_window(input.events.len() as u64),
        traces: index_window(input.traces.len()),
        flight_events: flight_events.last().map(|event| SeqWindow {
            from_seq: flight_events
                .first()
                .map(|first| first.seq)
                .unwrap_or(event.seq),
            to_seq: event.seq,
        }),
        checkpoints: checkpoints.clone(),
        files: files.clone(),
        docs_hash: docs_hash(state),
        ..NarrativeSourceWindow::default()
    };
    let graph = build_run_graph(
        state,
        &source_window,
        &files,
        &flight_events,
        &latest_run_event_id,
    );
    debug_assert!(validate_graph(&graph));
    let graph_hash = graph_content_hash(&graph);
    let coverage = NarrativeCoverage {
        run_event_seq: Some(input.events.len() as u64),
        trace_count: Some(input.traces.len()),
        flight_event_seq: latest_flight.map(|event| event.seq),
        checkpoint_id: checkpoints.last().cloned(),
        doc_inputs_hash: source_window.docs_hash.clone(),
        architecture_graph_hash: Some(graph_hash.clone()),
        ..NarrativeCoverage::default()
    };
    let snapshot_id = snapshot_id(&(
        NarrativeScope::Run,
        &state.run_id,
        &headline,
        &current_work,
        &architecture_notes,
        &risks,
        &next_likely,
        &graph_hash,
    ));
    let created_at = Utc::now();
    let snapshot = NarrativeSnapshot {
        version: 1,
        snapshot_id: snapshot_id.clone(),
        scope: NarrativeScope::Run,
        target_id: state.run_id.clone(),
        created_at,
        status: NarrativeStatus::Deterministic,
        source_window,
        coverage: NarrativeSnapshotCoverage::default(),
        headline,
        current_work,
        architecture_notes,
        risks,
        next_likely,
        citations,
        plan_status: None,
        agent_table: Vec::new(),
        coordination_notes: Vec::new(),
        live: None,
    };
    let narrative_state = NarrativeState {
        version: 1,
        scope: NarrativeScope::Run,
        target_id: state.run_id.clone(),
        latest_snapshot_id: snapshot_id,
        latest_status: NarrativeStatus::Deterministic,
        latest_created_at: Some(created_at),
        latest_covered: coverage,
        cadence: NarrativeCadence::default(),
        provider: NarrativeProviderState {
            route: provider,
            source: "deterministic".to_string(),
            model: None,
            calls: 0,
            cost_usd: input.spend.iter().map(|record| record.cost_usd).sum(),
            subscription_seconds: input
                .spend
                .iter()
                .filter_map(|record| record.wall_time_seconds)
                .sum(),
        },
        last_error: None,
    };
    NarrativeProjection {
        state: narrative_state,
        snapshot,
        graph,
    }
}

pub(crate) fn build_plan_projection(input: &PlanNarrativeInput<'_>) -> NarrativeProjection {
    let plan = input.plan;
    let plan_prefix = short_id(&plan.plan_id);
    let plan_evidence = format!(
        "file:{}",
        input
            .paths
            .plan_dir(&plan.plan_id)
            .join("plan.json")
            .display()
    );
    let latest_plan_event_id = input
        .plan_events
        .last()
        .map(|_| format!("plan:{plan_prefix}:event:{}", input.plan_events.len()))
        .unwrap_or_else(|| plan_evidence.clone());
    let completed = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .count();
    let running = plan
        .tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Running)
        .count();
    let failed = plan
        .tasks
        .iter()
        .filter(|task| matches!(task.status, PlanTaskStatus::Failed | PlanTaskStatus::Killed))
        .count();
    let headline = format!(
        "Plan {} is {} with {completed} done, {running} running, and {failed} blocked or failed.",
        plan_prefix,
        plan_status_label(plan.status)
    );
    let mut current_work = vec![claim(
        format!(
            "The plan is coordinating {} task(s) in {} mode.",
            plan.tasks.len(),
            plan.mode.as_str()
        ),
        vec![plan_evidence.clone()],
        "high",
    )];
    if let Some(event) = input.plan_events.last() {
        current_work.push(claim(
            format!(
                "Latest plan event: {}.",
                plan_event_summary_for_narrative(&event.event)
            ),
            vec![latest_plan_event_id.clone()],
            "high",
        ));
    }
    let selected_summary = plan
        .tasks
        .get(input.selected)
        .map(|task| {
            format!(
                "Selected child {} is {} and {}.",
                task.task_id,
                plan_task_status_label(task.status),
                task.child_run_id
                    .as_deref()
                    .map(|run_id| format!("maps to run {}", short_id(run_id)))
                    .unwrap_or_else(|| "has no run yet".to_string())
            )
        })
        .unwrap_or_else(|| "No selected child is available.".to_string());
    current_work.push(claim(
        selected_summary,
        vec![latest_plan_event_id.clone()],
        "high",
    ));
    let agent_table = plan
        .tasks
        .iter()
        .map(|task| NarrativeAgentRow {
            task_id: task.task_id.clone(),
            role: format!("{:?}", task.role).to_ascii_lowercase(),
            provider: task.provider.clone(),
            status: plan_task_status_label(task.status).to_string(),
            summary: task_summary(input.paths, task),
            evidence: task_evidence(input.paths, task),
        })
        .collect::<Vec<_>>();
    let mut coordination_notes = Vec::new();
    let dep_notes = plan
        .tasks
        .iter()
        .filter(|task| !task.depends_on.is_empty())
        .map(|task| format!("{} waits on {}", task.task_id, task.depends_on.join(",")))
        .collect::<Vec<_>>();
    if dep_notes.is_empty() {
        coordination_notes.push(claim(
            "No explicit task dependencies are blocking ready work.".to_string(),
            vec![plan_evidence.clone()],
            "medium",
        ));
    } else {
        coordination_notes.push(claim(
            dep_notes.join("; "),
            dep_notes
                .iter()
                .filter_map(|note| note.split_whitespace().next())
                .map(|task_id| format!("task:{task_id}:deps"))
                .collect(),
            "high",
        ));
    }
    let architecture_notes = vec![claim(
        "The visual map is derived from task roles, dependencies, provider routes, child runs, and repair/final-gate evidence."
            .to_string(),
        vec![plan_evidence.clone()],
        "high",
    )];
    let mut risks = Vec::new();
    for message in input
        .messages
        .iter()
        .rev()
        .filter(|message| message.kind == PlanMessageKind::Blocker)
        .take(3)
    {
        risks.push(claim(
            format!("Coordinator blocker: {}.", message.summary),
            vec![format!(
                "plan-message:{}:{}",
                plan_prefix,
                message.ts.timestamp()
            )],
            "high",
        ));
    }
    if failed > 0 {
        risks.push(claim(
            format!("{failed} child task(s) are failed or killed."),
            vec![latest_plan_event_id.clone()],
            "high",
        ));
    }
    risks.push(claim(
        "Provider-backed narration has not run yet; deterministic fallback facts are shown."
            .to_string(),
        vec![plan_evidence],
        "high",
    ));
    let next_likely = vec![claim(
        if plan.status == PlanStatus::Forked {
            "Expect running children to finish before merge/final gate work continues.".to_string()
        } else {
            "Use Enter on a child run for detail or inspect the plan feed for coordination events."
                .to_string()
        },
        vec![latest_plan_event_id],
        "low",
    )];
    let child_runs = child_coverage(input.paths, plan);
    let coverage = NarrativeCoverage {
        plan_event_seq: Some(input.plan_events.len() as u64),
        child_runs,
        repair_run_id: repair_run_id_from_feed(input.feed_events),
        ..NarrativeCoverage::default()
    };
    let mut source_files = plan
        .tasks
        .iter()
        .filter_map(|task| task.summary_path.as_ref())
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    source_files.extend(plan_child_graph_files(input.paths, plan));
    source_files.sort();
    source_files.dedup();
    let source_window = NarrativeSourceWindow {
        plan_events: seq_window(input.plan_events.len() as u64),
        files: source_files,
        ..NarrativeSourceWindow::default()
    };
    let graph = build_plan_graph(input.paths, plan, &source_window);
    debug_assert!(validate_graph(&graph));
    let graph_hash = graph_content_hash(&graph);
    let snapshot_id = snapshot_id(&(
        NarrativeScope::Plan,
        &plan.plan_id,
        &headline,
        &current_work,
        &architecture_notes,
        &risks,
        &next_likely,
        &graph_hash,
    ));
    let created_at = Utc::now();
    let snapshot = NarrativeSnapshot {
        version: 1,
        snapshot_id: snapshot_id.clone(),
        scope: NarrativeScope::Plan,
        target_id: plan.plan_id.clone(),
        created_at,
        status: NarrativeStatus::Deterministic,
        source_window,
        coverage: NarrativeSnapshotCoverage::default(),
        headline,
        current_work,
        architecture_notes,
        risks,
        next_likely,
        citations: plan_citations(input.paths, plan, input.plan_events),
        plan_status: Some(plan_status_label(plan.status).to_string()),
        agent_table,
        coordination_notes,
        live: None,
    };
    let mut latest_covered = coverage;
    latest_covered.architecture_graph_hash = Some(graph_hash);
    let narrative_state = NarrativeState {
        version: 1,
        scope: NarrativeScope::Plan,
        target_id: plan.plan_id.clone(),
        latest_snapshot_id: snapshot_id,
        latest_status: NarrativeStatus::Deterministic,
        latest_created_at: Some(created_at),
        latest_covered,
        cadence: NarrativeCadence::default(),
        provider: NarrativeProviderState {
            route: plan
                .providers
                .planner
                .clone()
                .or_else(|| plan.providers.default_child.clone())
                .or_else(|| plan.providers.coder.clone()),
            source: "deterministic".to_string(),
            model: None,
            calls: 0,
            cost_usd: 0.0,
            subscription_seconds: 0.0,
        },
        last_error: None,
    };
    NarrativeProjection {
        state: narrative_state,
        snapshot,
        graph,
    }
}

/// The calm foreground block: the headline plus the top `current_work` claims,
/// bounded to `max_lines`. The few-lines-max counterpart to
/// [`narrative_plain_lines`] for the live run surface (P8).
#[allow(dead_code)] // wired into the narrator task's foreground render in P8 integration
pub(crate) fn live_block_lines(snapshot: &NarrativeSnapshot, max_lines: usize) -> Vec<String> {
    if max_lines == 0 {
        return Vec::new();
    }
    let mut lines = Vec::with_capacity(max_lines);
    lines.push(snapshot.headline.clone());
    for claim in &snapshot.current_work {
        if lines.len() >= max_lines {
            break;
        }
        lines.push(format!("· {}", claim.text));
    }
    lines.truncate(max_lines);
    lines
}

pub(crate) fn narrative_plain_lines(
    projection: &NarrativeProjection,
    visual: NarrativeVisualMode,
) -> Vec<String> {
    let snapshot = &projection.snapshot;
    let mut lines = vec![
        format!(
            "Narrated {}  status {:?}  visual {}",
            snapshot.snapshot_id,
            projection.state.latest_status,
            visual.label()
        ),
        format!(
            "freshness: {} via {}  covered: {}{}",
            narrative_status_label(&projection.state.latest_status),
            projection.state.provider.source,
            coverage_label(&projection.state.latest_covered),
            projection
                .state
                .last_error
                .as_ref()
                .map(|error| format!("  error: {}", one_line(error, 120)))
                .unwrap_or_default()
        ),
        String::new(),
        snapshot.headline.clone(),
        String::new(),
    ];
    push_claim_section(&mut lines, "Current work", &snapshot.current_work);
    push_claim_section(&mut lines, "Architecture", &snapshot.architecture_notes);
    if !snapshot.agent_table.is_empty() {
        lines.push("Agents".to_string());
        for row in &snapshot.agent_table {
            lines.push(format!(
                "- {} [{}] {} {}",
                row.task_id,
                row.status,
                row.provider.as_deref().unwrap_or("-"),
                row.summary
            ));
        }
        lines.push(String::new());
    }
    if !snapshot.coordination_notes.is_empty() {
        push_claim_section(&mut lines, "Coordination", &snapshot.coordination_notes);
    }
    push_claim_section(&mut lines, "Risks", &snapshot.risks);
    push_claim_section(&mut lines, "Next likely", &snapshot.next_likely);
    if visual != NarrativeVisualMode::None {
        lines.push(format!("Visual: {}", visual.label()));
        lines.extend(graph_ascii_lines(&projection.graph, visual));
        lines.push(String::new());
    }
    lines.push("Evidence".to_string());
    for citation in snapshot.citations.iter().take(8) {
        lines.push(format!("- {} {}", citation.id, citation.summary));
    }
    lines
}

pub(crate) fn graph_ascii_lines(
    graph: &ArchitectureGraph,
    visual: NarrativeVisualMode,
) -> Vec<String> {
    if visual == NarrativeVisualMode::None {
        return Vec::new();
    }
    if graph.nodes.is_empty() {
        return vec!["[stale] not enough architecture evidence yet".to_string()];
    }
    match visual {
        NarrativeVisualMode::Agents => graph_agent_lines(graph),
        NarrativeVisualMode::Files => graph_file_lines(graph),
        NarrativeVisualMode::Evidence => graph_evidence_lines(graph),
        NarrativeVisualMode::Architecture => graph_architecture_lines(graph),
        NarrativeVisualMode::None => Vec::new(),
    }
}

pub(crate) fn validate_graph(graph: &ArchitectureGraph) -> bool {
    graph.nodes.iter().all(|node| !node.evidence.is_empty())
        && graph.edges.iter().all(|edge| !edge.evidence.is_empty())
}

pub(crate) fn read_latest_snapshot(narrative_dir: &Path) -> LatestNarrativeSnapshot {
    let path = narrative_dir.join(NARRATIVE_SNAPSHOTS_JSONL);
    let Ok(raw) = fs::read_to_string(path) else {
        return LatestNarrativeSnapshot {
            snapshot: None,
            skipped_malformed_rows: 0,
        };
    };
    let mut latest = None;
    let mut skipped = 0;
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str::<NarrativeSnapshot>(line) {
            Ok(snapshot) => latest = Some(snapshot),
            Err(_) => skipped += 1,
        }
    }
    LatestNarrativeSnapshot {
        snapshot: latest,
        skipped_malformed_rows: skipped,
    }
}

pub(crate) fn redact_for_provider(raw: &str) -> NarrativeRedactionReport {
    let mut findings = Vec::new();
    let mut text = strip_terminal_controls(raw, &mut findings);
    let rules = [
        (
            r#"(?i)(authorization|cookie|password|passwd|api[_-]?key|secret|token|access[_-]?token)\s*[:=]\s*['"]?[^'"\s,;]+"#,
            "<redacted-secret>",
            "secret-like assignment redacted",
        ),
        (
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
            "<redacted-private-key>",
            "private key block redacted",
        ),
        (
            r"\bgh[pousr]_[A-Za-z0-9_]{12,}\b",
            "<redacted-github-token>",
            "github token redacted",
        ),
        (
            r"\bsk-[A-Za-z0-9_-]{12,}\b",
            "<redacted-api-token>",
            "api token redacted",
        ),
        (
            r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b",
            "<redacted-email>",
            "email redacted",
        ),
    ];
    for (pattern, replacement, finding) in rules {
        let Ok(regex) = Regex::new(pattern) else {
            continue;
        };
        if regex.is_match(&text) {
            text = regex.replace_all(&text, replacement).to_string();
            findings.push(finding.to_string());
        }
    }
    findings.sort();
    findings.dedup();
    NarrativeRedactionReport { text, findings }
}

pub(crate) fn build_provider_prompt(
    projection: &NarrativeProjection,
) -> crate::Result<NarrativePromptBundle> {
    let evidence_ids = evidence_ids_for_projection(projection)
        .into_iter()
        .collect::<Vec<_>>();
    let graph_ids = graph_ids_for_projection(projection)
        .into_iter()
        .collect::<Vec<_>>();
    let prompt_value = json!({
        "output_schema": {
            "headline": "string",
            "current_work": [{"text": "string", "evidence": ["known evidence id"], "confidence": "high|medium|low"}],
            "architecture_notes": [{"text": "string", "evidence": ["known evidence id"], "confidence": "high|medium|low"}],
            "risks": [{"text": "string", "evidence": ["known evidence id"], "confidence": "high|medium|low"}],
            "next_likely": [{"text": "string", "evidence": ["known evidence id"], "confidence": "low"}],
            "graph_labels": [{"target_id": "known graph id", "label": "short label"}]
        },
        "allowed_evidence_ids": evidence_ids,
        "allowed_graph_ids": graph_ids,
        "snapshot": projection.snapshot,
        "graph": projection.graph,
    });
    let payload = serde_json::to_string_pretty(&prompt_value)?;
    let raw = format!(
        "You are a narrative projector over cited evidence, not a source of truth.\n\
Return exactly one raw JSON object matching the requested schema and nothing else.\n\
Do not write Markdown, code fences, commentary, or prose outside the JSON object.\n\
Cite every claim with ids from allowed_evidence_ids. Do not invent evidence, graph nodes, graph edges, files, or actions.\n\n\
Evidence payload:\n{payload}\n"
    );
    let redaction = redact_for_provider(&raw);
    Ok(NarrativePromptBundle {
        prompt: redaction.text.clone(),
        redaction,
        evidence_ids,
        graph_ids,
    })
}

pub(crate) fn provider_refresh_decision(
    state: &NarrativeState,
    policy: &NarrativeRefreshPolicy,
) -> NarrativeRefreshDecision {
    if policy.provider_route.as_deref().is_none_or(str::is_empty) {
        return NarrativeRefreshDecision::NoProvider;
    }
    if state.provider.calls >= state.cadence.max_provider_calls_per_attach {
        return NarrativeRefreshDecision::CallLimitReached;
    }
    if let Some(max_spend) = policy.max_spend_usd
        && state.provider.cost_usd >= max_spend
    {
        return NarrativeRefreshDecision::OverBudget;
    }
    if state.provider.calls > 0
        && !policy.manual
        && !policy.meaningful_delta
        && let Some(last) = state.latest_created_at
    {
        let elapsed = (policy.now - last).num_seconds().max(0) as u64;
        if elapsed < state.cadence.min_seconds_between_provider_calls {
            return NarrativeRefreshDecision::TooSoon;
        }
    }
    NarrativeRefreshDecision::Eligible
}

pub(crate) fn apply_provider_response(
    projection: &NarrativeProjection,
    raw_content: &str,
    provider: NarrativeProviderRefresh,
) -> crate::Result<NarrativeProjection> {
    let redacted_output = redact_for_provider(raw_content);
    if !redacted_output.findings.is_empty() {
        return Err(crate::CliError::Exit {
            code: 1,
            message: "narrative provider output contained sensitive or unsafe text".to_string(),
            hint: "inspect provider output and rely on deterministic narrative fallback"
                .to_string(),
        });
    }
    let output = parse_provider_narrative_output(raw_content)?;
    let allowed_evidence = evidence_ids_for_projection(projection);
    validate_provider_claims(&output, &allowed_evidence)?;
    let allowed_graph_ids = graph_ids_for_projection(projection);
    validate_provider_graph_labels(&output.graph_labels, &allowed_graph_ids)?;

    let mut next = projection.clone();
    if let Some(headline) = non_empty_redacted_line(output.headline.as_deref()) {
        next.snapshot.headline = headline;
    }
    if !output.current_work.is_empty() {
        next.snapshot.current_work = output.current_work;
    }
    if !output.architecture_notes.is_empty() {
        next.snapshot.architecture_notes = output.architecture_notes;
    }
    if !output.risks.is_empty() {
        next.snapshot.risks = output.risks;
    }
    if !output.next_likely.is_empty() {
        next.snapshot.next_likely = output.next_likely;
    }
    apply_graph_label_suggestions(&mut next.graph, &output.graph_labels);
    let graph_hash = graph_content_hash(&next.graph);
    next.state.latest_covered.architecture_graph_hash = Some(graph_hash.clone());
    next.snapshot.source_window = next.graph.source_window.clone();
    next.snapshot.status = NarrativeStatus::Fresh;
    next.state.latest_status = NarrativeStatus::Fresh;
    let created_at = Utc::now();
    next.snapshot.created_at = created_at;
    next.state.latest_created_at = Some(created_at);
    next.state.provider.route = Some(provider.route);
    next.state.provider.model = Some(provider.model);
    next.state.provider.source = "provider".to_string();
    next.state.provider.calls = next.state.provider.calls.saturating_add(1);
    next.state.provider.cost_usd += provider.cost_usd;
    next.state.provider.subscription_seconds += provider.subscription_seconds.unwrap_or_default();
    next.state.last_error = None;
    let snapshot_id = snapshot_id(&(
        &next.snapshot.scope,
        &next.snapshot.target_id,
        &next.snapshot.headline,
        &next.snapshot.current_work,
        &next.snapshot.architecture_notes,
        &next.snapshot.risks,
        &next.snapshot.next_likely,
        &graph_hash,
        &next.state.provider.route,
        next.state.provider.calls,
    ));
    next.snapshot.snapshot_id = snapshot_id.clone();
    next.state.latest_snapshot_id = snapshot_id;
    Ok(next)
}

pub(crate) fn projection_with_provider_failure(
    projection: &NarrativeProjection,
    route: Option<String>,
    error: impl Into<String>,
) -> NarrativeProjection {
    let mut next = projection.clone();
    let error = error.into();
    next.snapshot.status = NarrativeStatus::Stale;
    next.state.latest_status = NarrativeStatus::Stale;
    next.state.provider.route = route;
    next.state.provider.source = "provider_failed".to_string();
    next.state.last_error = Some(error);
    let created_at = Utc::now();
    next.snapshot.created_at = created_at;
    next.state.latest_created_at = Some(created_at);
    next.snapshot.risks.push(NarrativeClaim {
        text: "Provider-backed narration failed; deterministic facts remain visible.".to_string(),
        evidence: next
            .snapshot
            .citations
            .first()
            .map(|citation| vec![citation.id.clone()])
            .unwrap_or_else(|| vec!["state".to_string()]),
        confidence: "high".to_string(),
    });
    let graph_hash = graph_content_hash(&next.graph);
    next.snapshot.snapshot_id = snapshot_id(&(
        &next.snapshot.scope,
        &next.snapshot.target_id,
        &next.snapshot.status,
        &next.snapshot.headline,
        &next.snapshot.current_work,
        &next.snapshot.architecture_notes,
        &next.snapshot.risks,
        &next.snapshot.next_likely,
        &graph_hash,
        &next.state.provider.route,
        next.state.provider.calls,
        &next.state.last_error,
    ));
    next.state.latest_snapshot_id = next.snapshot.snapshot_id.clone();
    next
}

/// One turn fed to the live narrator's continuity prompt. Populated from a
/// `TurnRecord` by the windowing layer (P5); each carries the evidence id
/// `turn:{turn}` that a beat may cite.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // wired into the narrator task in P5/P6
pub(crate) struct LiveTurnInput {
    pub(crate) turn: u32,
    pub(crate) title: String,
    pub(crate) summary: String,
    pub(crate) tool_kind: String,
    pub(crate) outcome: String,
    pub(crate) files: Vec<String>,
}

impl LiveTurnInput {
    #[allow(dead_code)] // wired into the narrator task in P5/P6
    pub(crate) fn evidence_id(&self) -> String {
        format!("turn:{}", self.turn)
    }
}

/// Continuity metadata for a live beat: where it sits in the rolling story and
/// which provider produced it.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // wired into the narrator task in P5/P6
pub(crate) struct LiveBeatMeta {
    pub(crate) beat_seq: u64,
    pub(crate) covers_turn: u32,
    pub(crate) rolling_summary: Option<String>,
    pub(crate) provider: NarrativeProviderRefresh,
}

/// Evidence ids a live beat may cite: the prior snapshot's citations and claim
/// evidence (continuity) plus one `turn:{n}` id per windowed new turn (so the
/// narrator may assert genuinely new beats, not only relabel prior claims).
#[allow(dead_code)] // wired into the narrator task in P5/P6
pub(crate) fn live_allowed_evidence(
    previous: &NarrativeSnapshot,
    new_turns: &[LiveTurnInput],
) -> BTreeSet<String> {
    let mut ids = previous
        .citations
        .iter()
        .map(|citation| citation.id.clone())
        .collect::<BTreeSet<_>>();
    for claim in previous
        .current_work
        .iter()
        .chain(&previous.architecture_notes)
        .chain(&previous.risks)
        .chain(&previous.next_likely)
    {
        for evidence in &claim.evidence {
            ids.insert(evidence.clone());
        }
    }
    for turn in new_turns {
        ids.insert(turn.evidence_id());
    }
    ids
}

/// Build the live continuity prompt: amend and EXTEND the previous narrative
/// using only the windowed new turns plus the carried rolling summary. Unlike
/// the attach projector prompt, the model may add new beats tied to new-turn
/// evidence ids rather than only relabel a deterministic snapshot.
#[allow(dead_code)] // wired into the narrator task in P5/P6
pub(crate) fn build_live_narrator_prompt(
    previous: &NarrativeSnapshot,
    new_turns: &[LiveTurnInput],
    rolling_summary: Option<&str>,
) -> crate::Result<NarrativePromptBundle> {
    let evidence_ids = live_allowed_evidence(previous, new_turns)
        .into_iter()
        .collect::<Vec<_>>();
    let previous_narrative = json!({
        "headline": previous.headline,
        "current_work": previous.current_work,
        "architecture_notes": previous.architecture_notes,
        "risks": previous.risks,
        "next_likely": previous.next_likely,
    });
    let new_turns_json = new_turns
        .iter()
        .map(|turn| {
            json!({
                "turn": turn.turn,
                "evidence_id": turn.evidence_id(),
                "title": turn.title,
                "summary": turn.summary,
                "tool": turn.tool_kind,
                "outcome": turn.outcome,
                "files": turn.files,
            })
        })
        .collect::<Vec<_>>();
    let prompt_value = json!({
        "task": "amend_and_extend",
        "output_schema": {
            "headline": "string",
            "current_work": [{"text": "string", "evidence": ["known evidence id"], "confidence": "high|medium|low"}],
            "architecture_notes": [{"text": "string", "evidence": ["known evidence id"], "confidence": "high|medium|low"}],
            "risks": [{"text": "string", "evidence": ["known evidence id"], "confidence": "high|medium|low"}],
            "next_likely": [{"text": "string", "evidence": ["known evidence id"], "confidence": "low"}]
        },
        "allowed_evidence_ids": evidence_ids,
        "previous_narrative": previous_narrative,
        "rolling_summary": rolling_summary.unwrap_or(""),
        "new_turns": new_turns_json,
    });
    let payload = serde_json::to_string_pretty(&prompt_value)?;
    let raw = format!(
        "You are the live narrator for a coding run. Amend and EXTEND the previous narrative using only the new turns and the rolling summary — never regenerate from scratch.\n\
Append a beat describing what the latest turn(s) did; revise the headline, current_work, and next_likely; keep prior architecture_notes and risks unless the new turns contradict them.\n\
Return exactly one raw JSON object matching output_schema and nothing else. No Markdown, code fences, commentary, or prose outside the JSON object.\n\
Cite every claim with ids from allowed_evidence_ids (each new turn's evidence id is turn:N). Do not invent evidence, files, or actions.\n\n\
Payload:\n{payload}\n"
    );
    let redaction = redact_for_provider(&raw);
    Ok(NarrativePromptBundle {
        prompt: redaction.text.clone(),
        redaction,
        evidence_ids,
        graph_ids: Vec::new(),
    })
}

/// Merge a live provider response onto the previous snapshot, producing a NEW
/// beat snapshot (the caller appends it — the prior beat is never overwritten).
/// Claims are validated against [`live_allowed_evidence`], so a beat citing a
/// turn outside the window is rejected.
#[allow(dead_code)] // wired into the narrator task in P5/P6
pub(crate) fn apply_live_narrator_response(
    previous: &NarrativeSnapshot,
    new_turns: &[LiveTurnInput],
    raw_content: &str,
    meta: LiveBeatMeta,
) -> crate::Result<NarrativeSnapshot> {
    let redacted_output = redact_for_provider(raw_content);
    if !redacted_output.findings.is_empty() {
        return Err(crate::CliError::Exit {
            code: 1,
            message: "live narrator output contained sensitive or unsafe text".to_string(),
            hint: "rely on the deterministic narrative floor for this beat".to_string(),
        });
    }
    let output = parse_provider_narrative_output(raw_content)?;
    let allowed_evidence = live_allowed_evidence(previous, new_turns);
    validate_provider_claims(&output, &allowed_evidence)?;

    let mut next = previous.clone();
    if let Some(headline) = non_empty_redacted_line(output.headline.as_deref()) {
        next.headline = headline;
    }
    if !output.current_work.is_empty() {
        next.current_work = output.current_work;
    }
    if !output.architecture_notes.is_empty() {
        next.architecture_notes = output.architecture_notes;
    }
    if !output.risks.is_empty() {
        next.risks = output.risks;
    }
    if !output.next_likely.is_empty() {
        next.next_likely = output.next_likely;
    }
    next.status = NarrativeStatus::Fresh;
    next.created_at = Utc::now();
    next.live = Some(LiveBeat {
        beat_seq: meta.beat_seq,
        covers_turn: meta.covers_turn,
        source: NarrativeSource::Live,
        rolling_summary: meta.rolling_summary,
    });
    next.snapshot_id = snapshot_id(&(
        &next.scope,
        &next.target_id,
        &next.headline,
        &next.current_work,
        &next.architecture_notes,
        &next.risks,
        &next.next_likely,
        meta.beat_seq,
        meta.covers_turn,
        &meta.provider.route,
    ));
    Ok(next)
}

/// Append a live beat to `snapshots.jsonl`, never rewriting prior beats — the
/// rolling story is an append-only audit trail.
#[allow(dead_code)] // wired into the narrator task in P5/P6
pub(crate) fn append_narrative_snapshot(
    narrative_dir: &Path,
    snapshot: &NarrativeSnapshot,
) -> crate::Result<()> {
    fs::create_dir_all(narrative_dir)?;
    append_json_line(&narrative_dir.join(NARRATIVE_SNAPSHOTS_JSONL), snapshot)
}

/// Hard cap on the carried rolling summary. Bounding the carry — not the beat
/// history — is what keeps per-beat model input O(1) and total cost O(turns).
pub(crate) const ROLLING_SUMMARY_CAP: usize = 1200;

#[allow(dead_code)] // wired into the narrator task in P6
fn truncate_chars(text: &str, max: usize) -> String {
    let mut out: String = text.chars().take(max).collect();
    if text.chars().count() > max {
        out.push('…');
    }
    out
}

/// Fold the windowed turns into the prior rolling summary, bounded to `cap`
/// chars by keeping the most recent content (older context is elided). The
/// beat history in snapshots.jsonl stays whole; only this carry is bounded.
#[allow(dead_code)] // wired into the narrator task in P6
fn fold_rolling_summary(prev: Option<&str>, turns: &[LiveTurnInput], cap: usize) -> String {
    let mut summary = prev.unwrap_or_default().to_string();
    for turn in turns {
        if !summary.is_empty() {
            summary.push(' ');
        }
        summary.push_str(&format!(
            "t{}: {}",
            turn.turn,
            truncate_chars(turn.summary.trim(), 80)
        ));
    }
    let count = summary.chars().count();
    if count <= cap || cap == 0 {
        return summary;
    }
    let tail: String = summary.chars().skip(count - (cap - 1)).collect();
    format!("…{tail}")
}

/// Map a persisted `TurnRecord` to the live narrator's per-turn input.
#[allow(dead_code)] // wired into the narrator task in P6
pub(crate) fn turn_record_to_input(record: &TurnRecord) -> LiveTurnInput {
    let summary = if record.response_summary.trim().is_empty() {
        record.outcome.clone()
    } else {
        record.response_summary.clone()
    };
    LiveTurnInput {
        turn: record.turn,
        title: record.title.clone(),
        summary,
        tool_kind: record.tool_kind.clone(),
        outcome: record.outcome.clone(),
        files: record.files.iter().map(|file| file.path.clone()).collect(),
    }
}

/// Accumulates the turns seen since the last beat. The narrator feeds the model
/// only this window (plus the carried rolling summary), never the full trace —
/// so each beat's input is bounded and total cost is O(turns), not O(turns²).
#[allow(dead_code)] // wired into the narrator task in P6
pub(crate) struct NarratorWindow {
    last_covered_turn: u32,
    pending: Vec<LiveTurnInput>,
    rolling_summary: Option<String>,
    cap: usize,
}

#[allow(dead_code)] // wired into the narrator task in P6
impl NarratorWindow {
    pub(crate) fn new() -> Self {
        Self {
            last_covered_turn: 0,
            pending: Vec::new(),
            rolling_summary: None,
            cap: ROLLING_SUMMARY_CAP,
        }
    }

    /// Record a turn, ignoring ones already covered by a prior beat or already
    /// pending (so re-observed checkpoints never double-count).
    pub(crate) fn observe(&mut self, input: LiveTurnInput) {
        if input.turn <= self.last_covered_turn {
            return;
        }
        if self.pending.iter().any(|turn| turn.turn == input.turn) {
            return;
        }
        self.pending.push(input);
    }

    pub(crate) fn pending(&self) -> &[LiveTurnInput] {
        &self.pending
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(crate) fn rolling_summary(&self) -> Option<&str> {
        self.rolling_summary.as_deref()
    }

    pub(crate) fn covers_turn(&self) -> Option<u32> {
        self.pending.iter().map(|turn| turn.turn).max()
    }

    /// Commit a beat: fold the window into the bounded rolling summary, advance
    /// the covered watermark, and clear the window. Returns the covered turn.
    pub(crate) fn commit_beat(&mut self) -> Option<u32> {
        let covers = self.covers_turn()?;
        self.rolling_summary = Some(fold_rolling_summary(
            self.rolling_summary.as_deref(),
            &self.pending,
            self.cap,
        ));
        self.last_covered_turn = covers;
        self.pending.clear();
        Some(covers)
    }

    /// Chars of model input this beat would carry: the bounded rolling summary
    /// plus the pending window — independent of how many turns the run has run.
    pub(crate) fn beat_input_chars(&self) -> usize {
        let summary = self
            .rolling_summary
            .as_deref()
            .map(|text| text.chars().count())
            .unwrap_or(0);
        let turns: usize = self
            .pending
            .iter()
            .map(|turn| turn.title.chars().count() + turn.summary.chars().count())
            .sum();
        summary + turns
    }
}

/// A minimal starting snapshot for the live rolling story. The first beat
/// amends this; until then it is the calm "starting" state.
pub(crate) fn seed_live_snapshot(run_id: &str) -> NarrativeSnapshot {
    NarrativeSnapshot {
        version: 2,
        snapshot_id: format!("live-seed-{run_id}"),
        scope: NarrativeScope::Run,
        target_id: run_id.to_string(),
        created_at: Utc::now(),
        status: NarrativeStatus::Deterministic,
        source_window: NarrativeSourceWindow::default(),
        coverage: NarrativeSnapshotCoverage::default(),
        headline: "Run starting…".to_string(),
        current_work: Vec::new(),
        architecture_notes: Vec::new(),
        risks: Vec::new(),
        next_likely: Vec::new(),
        citations: Vec::new(),
        plan_status: None,
        agent_table: Vec::new(),
        coordination_notes: Vec::new(),
        live: None,
    }
}

/// Build a deterministic floor beat from the window with no provider call —
/// the always-available narration when no provider is credentialed, the budget
/// is exhausted, or a model beat failed. Each claim cites its turn evidence id.
pub(crate) fn build_live_floor_beat(
    previous: &NarrativeSnapshot,
    new_turns: &[LiveTurnInput],
    meta: LiveBeatMeta,
) -> NarrativeSnapshot {
    let mut next = previous.clone();
    if let Some(last) = new_turns.last() {
        next.headline = format!("turn {}: {}", last.turn, last.title);
    }
    if !new_turns.is_empty() {
        next.current_work = new_turns
            .iter()
            .map(|turn| NarrativeClaim {
                text: truncate_chars(turn.summary.trim(), 160),
                evidence: vec![turn.evidence_id()],
                confidence: "medium".to_string(),
            })
            .collect();
    }
    next.status = NarrativeStatus::Deterministic;
    next.created_at = Utc::now();
    next.live = Some(LiveBeat {
        beat_seq: meta.beat_seq,
        covers_turn: meta.covers_turn,
        source: NarrativeSource::Live,
        rolling_summary: meta.rolling_summary,
    });
    next.snapshot_id = snapshot_id(&(
        &next.scope,
        &next.target_id,
        &next.headline,
        &next.current_work,
        meta.beat_seq,
        meta.covers_turn,
        "floor",
    ));
    next
}

/// Read the `TurnRecord` for `turn` from an incremental docs JSONL file (the
/// path carried by a `DocsCheckpoint` event). Returns the last matching record.
pub(crate) fn read_turn_record(incremental_path: &Path, turn: u32) -> Option<TurnRecord> {
    let raw = fs::read_to_string(incremental_path).ok()?;
    let mut found = None;
    for line in raw.lines().filter(|line| !line.trim().is_empty()) {
        if let Ok(record) = serde_json::from_str::<TurnRecord>(line)
            && record.turn == turn
        {
            found = Some(record);
        }
    }
    found
}

fn read_current_projection_if_covered(
    candidate: &NarrativeProjection,
    narrative_dir: &Path,
) -> Option<NarrativeProjection> {
    let state = read_json::<NarrativeState>(&narrative_dir.join(NARRATIVE_STATE_JSON))?;
    let snapshot = read_latest_snapshot(narrative_dir).snapshot?;
    if snapshot.snapshot_id != state.latest_snapshot_id {
        return None;
    }
    if !same_narrative_input_coverage(&state.latest_covered, &candidate.state.latest_covered) {
        return provider_projection_for_newer_coverage(
            candidate,
            NarrativeProjection {
                state,
                snapshot,
                graph: read_json::<ArchitectureGraph>(&narrative_dir.join(ARCHITECTURE_GRAPH_JSON))
                    .unwrap_or_else(|| candidate.graph.clone()),
            },
        );
    }
    let graph = read_json::<ArchitectureGraph>(&narrative_dir.join(ARCHITECTURE_GRAPH_JSON))
        .unwrap_or_else(|| candidate.graph.clone());
    Some(NarrativeProjection {
        state,
        snapshot,
        graph,
    })
}

fn provider_projection_for_newer_coverage(
    candidate: &NarrativeProjection,
    current: NarrativeProjection,
) -> Option<NarrativeProjection> {
    if current.state.provider.source != "provider"
        || !matches!(
            current.state.latest_status,
            NarrativeStatus::Fresh | NarrativeStatus::Stale
        )
    {
        return None;
    }

    let mut next = current;
    next.state.latest_status = NarrativeStatus::Stale;
    next.state.latest_covered = candidate.state.latest_covered.clone();
    next.graph = candidate.graph.clone();
    if next.snapshot.snapshot_id != next.state.latest_snapshot_id {
        return None;
    }
    Some(next)
}

fn same_narrative_input_coverage(left: &NarrativeCoverage, right: &NarrativeCoverage) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.architecture_graph_hash = None;
    right.architecture_graph_hash = None;
    left == right
}

fn strip_terminal_controls(raw: &str, findings: &mut Vec<String>) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut stripped = false;
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            stripped = true;
            if matches!(chars.peek(), Some('[')) {
                chars.next();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        break;
                    }
                }
            }
            continue;
        }
        if ch.is_control() && !matches!(ch, '\n' | '\r' | '\t') {
            stripped = true;
            continue;
        }
        out.push(ch);
    }
    if stripped {
        findings.push("terminal control sequence redacted".to_string());
    }
    out
}

fn parse_provider_narrative_output(raw_content: &str) -> crate::Result<ProviderNarrativeOutput> {
    let mut last_error = None;
    for candidate in provider_json_candidates(raw_content) {
        match serde_json::from_str::<ProviderNarrativeOutput>(&candidate) {
            Ok(output) if provider_output_has_narrative_fields(&output) => return Ok(output),
            Ok(_) => {
                last_error = Some(
                    "provider JSON did not include headline, claim sections, or graph labels"
                        .to_string(),
                );
            }
            Err(err) => last_error = Some(err.to_string()),
        }
    }
    Err(crate::CliError::Exit {
        code: 1,
        message: format!(
            "narrative provider returned invalid JSON: {}",
            last_error.unwrap_or_else(|| "no JSON object found".to_string())
        ),
        hint: "press r later or use --no-narrative-provider for deterministic fallback".to_string(),
    })
}

fn provider_json_candidates(raw_content: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    push_unique_candidate(&mut candidates, raw_content.trim());
    for candidate in fenced_json_candidates(raw_content) {
        push_unique_candidate(&mut candidates, candidate.trim());
    }
    if let Some(candidate) = embedded_json_candidate(raw_content) {
        push_unique_candidate(&mut candidates, candidate.trim());
    }
    candidates
}

fn fenced_json_candidates(raw_content: &str) -> Vec<String> {
    let mut candidates = Vec::new();
    let mut in_fence = false;
    let mut current = Vec::new();
    for line in raw_content.lines() {
        if line.trim_start().starts_with("```") {
            if in_fence {
                candidates.push(current.join("\n"));
                current.clear();
                in_fence = false;
            } else {
                in_fence = true;
            }
            continue;
        }
        if in_fence {
            current.push(line);
        }
    }
    candidates
}

fn embedded_json_candidate(raw_content: &str) -> Option<String> {
    for (index, ch) in raw_content.char_indices() {
        if ch != '{' {
            continue;
        }
        let mut deserializer = serde_json::Deserializer::from_str(&raw_content[index..]);
        if let Ok(value) = Value::deserialize(&mut deserializer)
            && value.is_object()
            && let Ok(candidate) = serde_json::to_string(&value)
        {
            return Some(candidate);
        }
    }
    None
}

fn push_unique_candidate(candidates: &mut Vec<String>, candidate: &str) {
    if candidate.is_empty() {
        return;
    }
    if !candidates.iter().any(|existing| existing == candidate) {
        candidates.push(candidate.to_string());
    }
}

fn provider_output_has_narrative_fields(output: &ProviderNarrativeOutput) -> bool {
    output
        .headline
        .as_deref()
        .is_some_and(|headline| !headline.trim().is_empty())
        || !output.current_work.is_empty()
        || !output.architecture_notes.is_empty()
        || !output.risks.is_empty()
        || !output.next_likely.is_empty()
        || !output.graph_labels.is_empty()
}

fn validate_provider_claims(
    output: &ProviderNarrativeOutput,
    allowed_evidence: &BTreeSet<String>,
) -> crate::Result<()> {
    let mut errors = Vec::new();
    validate_claim_list(
        "current_work",
        &output.current_work,
        allowed_evidence,
        false,
        &mut errors,
    );
    validate_claim_list(
        "architecture_notes",
        &output.architecture_notes,
        allowed_evidence,
        false,
        &mut errors,
    );
    validate_claim_list("risks", &output.risks, allowed_evidence, false, &mut errors);
    validate_claim_list(
        "next_likely",
        &output.next_likely,
        allowed_evidence,
        true,
        &mut errors,
    );
    if errors.is_empty() {
        Ok(())
    } else {
        Err(crate::CliError::Exit {
            code: 1,
            message: format!(
                "narrative provider claims failed validation: {}",
                errors.join("; ")
            ),
            hint: "rely on deterministic fallback or refresh after more evidence exists"
                .to_string(),
        })
    }
}

fn validate_claim_list(
    section: &str,
    claims: &[NarrativeClaim],
    allowed_evidence: &BTreeSet<String>,
    low_only: bool,
    errors: &mut Vec<String>,
) {
    for (index, claim) in claims.iter().enumerate() {
        if claim.evidence.is_empty() {
            errors.push(format!("{section}[{index}] has no evidence"));
        }
        for evidence in &claim.evidence {
            if !allowed_evidence.contains(evidence) {
                errors.push(format!(
                    "{section}[{index}] cites unknown evidence {evidence}"
                ));
            }
        }
        if low_only && claim.confidence != "low" {
            errors.push(format!(
                "{section}[{index}] must use low confidence because it is predictive"
            ));
        }
        if claim.text.trim().is_empty() {
            errors.push(format!("{section}[{index}] has empty text"));
        }
    }
}

fn validate_provider_graph_labels(
    labels: &[ProviderGraphLabel],
    allowed_graph_ids: &BTreeSet<String>,
) -> crate::Result<()> {
    let unknown = labels
        .iter()
        .filter(|label| !allowed_graph_ids.contains(&label.target_id))
        .map(|label| label.target_id.clone())
        .collect::<Vec<_>>();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(crate::CliError::Exit {
            code: 1,
            message: format!(
                "narrative provider suggested unknown graph target(s): {}",
                unknown.join(", ")
            ),
            hint: "refresh after the architecture map has more deterministic evidence".to_string(),
        })
    }
}

fn evidence_ids_for_projection(projection: &NarrativeProjection) -> BTreeSet<String> {
    let mut ids = projection
        .snapshot
        .citations
        .iter()
        .map(|citation| citation.id.clone())
        .collect::<BTreeSet<_>>();
    for claim in projection
        .snapshot
        .current_work
        .iter()
        .chain(projection.snapshot.architecture_notes.iter())
        .chain(projection.snapshot.risks.iter())
        .chain(projection.snapshot.next_likely.iter())
        .chain(projection.snapshot.coordination_notes.iter())
    {
        ids.extend(claim.evidence.iter().cloned());
    }
    for row in &projection.snapshot.agent_table {
        ids.extend(row.evidence.iter().cloned());
    }
    for node in &projection.graph.nodes {
        ids.extend(node.evidence.iter().cloned());
    }
    for edge in &projection.graph.edges {
        ids.extend(edge.evidence.iter().cloned());
    }
    for group in &projection.graph.groups {
        ids.extend(group.evidence.iter().cloned());
    }
    ids
}

fn graph_ids_for_projection(projection: &NarrativeProjection) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    ids.extend(projection.graph.nodes.iter().map(|node| node.id.clone()));
    ids.extend(
        projection
            .graph
            .edges
            .iter()
            .map(|edge| format!("edge:{}:{}:{}", edge.from, edge.kind, edge.to)),
    );
    ids.extend(projection.graph.groups.iter().map(|group| group.id.clone()));
    ids
}

fn apply_graph_label_suggestions(graph: &mut ArchitectureGraph, labels: &[ProviderGraphLabel]) {
    for suggestion in labels {
        let Some(label) = non_empty_redacted_line(Some(&suggestion.label)) else {
            continue;
        };
        if let Some(node) = graph
            .nodes
            .iter_mut()
            .find(|node| node.id == suggestion.target_id)
        {
            node.label = label;
            continue;
        }
        if let Some(group) = graph
            .groups
            .iter_mut()
            .find(|group| group.id == suggestion.target_id)
        {
            group.label = label;
            continue;
        }
        if let Some(edge) = graph.edges.iter_mut().find(|edge| {
            format!("edge:{}:{}:{}", edge.from, edge.kind, edge.to) == suggestion.target_id
        }) {
            edge.label = label;
        }
    }
}

fn non_empty_redacted_line(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    let redacted = redact_for_provider(value);
    if redacted.findings.is_empty() && !redacted.text.trim().is_empty() {
        Some(one_line(&redacted.text, 220))
    } else {
        None
    }
}

fn persist_projection(projection: &NarrativeProjection, narrative_dir: &Path) -> crate::Result<()> {
    let latest = read_latest_snapshot(narrative_dir);
    if latest
        .snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.snapshot_id == projection.snapshot.snapshot_id)
    {
        write_json_pretty(&narrative_dir.join(NARRATIVE_STATE_JSON), &projection.state)?;
        write_json_pretty(
            &narrative_dir.join(ARCHITECTURE_GRAPH_JSON),
            &projection.graph,
        )?;
        return Ok(());
    }
    fs::create_dir_all(narrative_dir)?;
    append_json_line(
        &narrative_dir.join(NARRATIVE_SNAPSHOTS_JSONL),
        &projection.snapshot,
    )?;
    write_json_pretty(&narrative_dir.join(NARRATIVE_STATE_JSON), &projection.state)?;
    write_json_pretty(
        &narrative_dir.join(ARCHITECTURE_GRAPH_JSON),
        &projection.graph,
    )
}

fn build_run_graph(
    state: &deadreckon_core::PipelineState,
    source_window: &NarrativeSourceWindow,
    files: &[String],
    flight_events: &[FlightEvent],
    latest_evidence: &str,
) -> ArchitectureGraph {
    let run_node = format!("run:{}", short_id(&state.run_id));
    let provider_node = state
        .provider
        .as_deref()
        .map(|provider| format!("provider:{provider}"));
    let mut nodes = vec![ArchitectureNode {
        id: run_node.clone(),
        label: format!("run {}", short_id(&state.run_id)),
        kind: "run".to_string(),
        status: run_status_label(state.status).to_string(),
        weight: 5,
        evidence: vec![format!("file:{}", state.state_path().display())],
        style_token: style_for_run_status(state.status).to_string(),
    }];
    let mut edges = Vec::new();
    if let Some(provider_node) = provider_node.as_ref() {
        nodes.push(ArchitectureNode {
            id: provider_node.clone(),
            label: state
                .provider
                .clone()
                .unwrap_or_else(|| "provider".to_string()),
            kind: "provider".to_string(),
            status: "active".to_string(),
            weight: 3,
            evidence: vec![format!("file:{}", state.state_path().display())],
            style_token: "primary".to_string(),
        });
        edges.push(ArchitectureEdge {
            from: run_node.clone(),
            to: provider_node.clone(),
            label: "uses".to_string(),
            kind: "depends_on".to_string(),
            evidence: vec![format!("file:{}", state.state_path().display())],
        });
    }
    let mut group_nodes = Vec::new();
    for (index, file) in files.iter().take(8).enumerate() {
        let node_id = format!("file:{file}");
        nodes.push(ArchitectureNode {
            id: node_id.clone(),
            label: file.clone(),
            kind: "file".to_string(),
            status: if index < 3 { "active" } else { "neutral" }.to_string(),
            weight: 2,
            evidence: vec![format!("file:{file}")],
            style_token: if index < 3 { "primary" } else { "muted" }.to_string(),
        });
        edges.push(ArchitectureEdge {
            from: run_node.clone(),
            to: node_id.clone(),
            label: "touches".to_string(),
            kind: "writes".to_string(),
            evidence: vec![format!("file:{file}")],
        });
        group_nodes.push(node_id);
    }
    for event in flight_events.iter().rev().take(3) {
        if let Some(checkpoint_id) = event.checkpoint_id.as_ref() {
            let node_id = format!("checkpoint:{checkpoint_id}");
            nodes.push(ArchitectureNode {
                id: node_id.clone(),
                label: checkpoint_id.clone(),
                kind: "checkpoint".to_string(),
                status: "done".to_string(),
                weight: 2,
                evidence: vec![format!(
                    "flight:{}:seq:{}",
                    short_id(&state.run_id),
                    event.seq
                )],
                style_token: "success".to_string(),
            });
            edges.push(ArchitectureEdge {
                from: run_node.clone(),
                to: node_id,
                label: "records".to_string(),
                kind: "validates".to_string(),
                evidence: vec![format!(
                    "flight:{}:seq:{}",
                    short_id(&state.run_id),
                    event.seq
                )],
            });
        }
    }
    let warnings = if files.is_empty() && flight_events.is_empty() {
        vec!["not enough architecture evidence yet".to_string()]
    } else {
        Vec::new()
    };
    let graph_hash = stable_hash(&(source_window, &nodes, &edges, latest_evidence));
    ArchitectureGraph {
        version: 1,
        graph_id: format!("arch-{}", &graph_hash[..12.min(graph_hash.len())]),
        scope: NarrativeScope::Run,
        target_id: state.run_id.clone(),
        generated_at: Utc::now(),
        source_window: source_window.clone(),
        default_visual: NarrativeVisualMode::Architecture,
        nodes,
        edges,
        groups: if group_nodes.is_empty() {
            Vec::new()
        } else {
            vec![ArchitectureGroup {
                id: "group:changed-files".to_string(),
                label: "Changed files".to_string(),
                node_ids: group_nodes,
                evidence: file_evidence(files),
            }]
        },
        layout: ArchitectureLayout {
            kind: "layered-tree".to_string(),
            root_ids: vec![run_node],
            warnings,
        },
        legend: default_legend(),
    }
}

fn build_plan_graph(
    paths: &DeadreckonPaths,
    plan: &Plan,
    source_window: &NarrativeSourceWindow,
) -> ArchitectureGraph {
    let plan_prefix = short_id(&plan.plan_id);
    let root_id = format!("plan:{plan_prefix}");
    let plan_evidence = format!("plan:{plan_prefix}");
    let mut nodes = vec![ArchitectureNode {
        id: root_id.clone(),
        label: format!("plan {plan_prefix}"),
        kind: "run".to_string(),
        status: plan_status_label(plan.status).to_string(),
        weight: 5,
        evidence: vec![plan_evidence.clone()],
        style_token: style_for_plan_status(plan.status).to_string(),
    }];
    let mut edges = Vec::new();
    let mut child_file_node_ids = Vec::new();
    let mut child_file_group_evidence = BTreeSet::new();
    for task in &plan.tasks {
        let task_id = format!("task:{}", task.task_id);
        nodes.push(ArchitectureNode {
            id: task_id.clone(),
            label: format!(
                "{} {}",
                task.task_id,
                format!("{:?}", task.role).to_ascii_lowercase()
            ),
            kind: "task".to_string(),
            status: plan_task_status_label(task.status).to_string(),
            weight: 3,
            evidence: vec![format!("task:{}", task.task_id)],
            style_token: style_for_task_status(task.status).to_string(),
        });
        edges.push(ArchitectureEdge {
            from: root_id.clone(),
            to: task_id.clone(),
            label: "spawns".to_string(),
            kind: "spawns".to_string(),
            evidence: vec![format!("task:{}", task.task_id)],
        });
        if let Some(provider) = task.provider.as_ref() {
            let provider_id = format!("provider:{provider}");
            if !nodes.iter().any(|node| node.id == provider_id) {
                nodes.push(ArchitectureNode {
                    id: provider_id.clone(),
                    label: provider.clone(),
                    kind: "provider".to_string(),
                    status: "active".to_string(),
                    weight: 2,
                    evidence: vec![format!("task:{}", task.task_id)],
                    style_token: "primary".to_string(),
                });
            }
            edges.push(ArchitectureEdge {
                from: task_id.clone(),
                to: provider_id,
                label: "uses".to_string(),
                kind: "depends_on".to_string(),
                evidence: vec![format!("task:{}", task.task_id)],
            });
        }
        if let Some(run_id) = task.child_run_id.as_ref() {
            let run_node = format!("run:{}", short_id(run_id));
            nodes.push(ArchitectureNode {
                id: run_node.clone(),
                label: format!("run {}", short_id(run_id)),
                kind: "run".to_string(),
                status: "active".to_string(),
                weight: 2,
                evidence: vec![format!("child-run:{run_id}")],
                style_token: "primary".to_string(),
            });
            edges.push(ArchitectureEdge {
                from: task_id.clone(),
                to: run_node,
                label: "owns".to_string(),
                kind: "owns".to_string(),
                evidence: vec![format!("child-run:{run_id}")],
            });
            if let Some(child_graph) = latest_child_architecture_graph(paths, run_id) {
                for child_file in child_graph
                    .nodes
                    .iter()
                    .filter(|node| node.kind == "file")
                    .take(4)
                {
                    let node_hash = stable_hash(&(task.task_id.as_str(), &child_file.id));
                    let file_node_id = format!(
                        "child-file:{}:{}",
                        task.task_id,
                        &node_hash[..12.min(node_hash.len())]
                    );
                    let evidence = if child_file.evidence.is_empty() {
                        vec![format!("child-run:{run_id}")]
                    } else {
                        child_file.evidence.clone()
                    };
                    if !nodes.iter().any(|node| node.id == file_node_id) {
                        nodes.push(ArchitectureNode {
                            id: file_node_id.clone(),
                            label: child_file.label.clone(),
                            kind: "file".to_string(),
                            status: child_file.status.clone(),
                            weight: child_file.weight,
                            evidence: evidence.clone(),
                            style_token: child_file.style_token.clone(),
                        });
                        child_file_node_ids.push(file_node_id.clone());
                        child_file_group_evidence.extend(evidence.iter().cloned());
                    }
                    edges.push(ArchitectureEdge {
                        from: task_id.clone(),
                        to: file_node_id,
                        label: "touches".to_string(),
                        kind: "writes".to_string(),
                        evidence,
                    });
                }
            }
        }
        for dependency in &task.depends_on {
            edges.push(ArchitectureEdge {
                from: format!("task:{dependency}"),
                to: task_id.clone(),
                label: "blocks".to_string(),
                kind: "depends_on".to_string(),
                evidence: vec![format!("task:{}:deps", task.task_id)],
            });
        }
    }
    let graph_hash = stable_hash(&(source_window, &nodes, &edges));
    let mut groups = vec![ArchitectureGroup {
        id: "group:tasks".to_string(),
        label: "Plan tasks".to_string(),
        node_ids: plan
            .tasks
            .iter()
            .map(|task| format!("task:{}", task.task_id))
            .collect(),
        evidence: vec![plan_evidence],
    }];
    if !child_file_node_ids.is_empty() {
        groups.push(ArchitectureGroup {
            id: "group:child-files".to_string(),
            label: "Child file evidence".to_string(),
            node_ids: child_file_node_ids,
            evidence: child_file_group_evidence.into_iter().collect(),
        });
    }
    ArchitectureGraph {
        version: 1,
        graph_id: format!("arch-{}", &graph_hash[..12.min(graph_hash.len())]),
        scope: NarrativeScope::Plan,
        target_id: plan.plan_id.clone(),
        generated_at: Utc::now(),
        source_window: source_window.clone(),
        default_visual: NarrativeVisualMode::Agents,
        nodes,
        edges,
        groups,
        layout: ArchitectureLayout {
            kind: "swimlane".to_string(),
            root_ids: vec![root_id],
            warnings: Vec::new(),
        },
        legend: default_legend(),
    }
}

fn collect_run_files(input: &RunNarrativeInput<'_>, flight_events: &[FlightEvent]) -> Vec<String> {
    let mut files = input
        .live_files
        .iter()
        .take(12)
        .map(|file| file.path.clone())
        .collect::<Vec<_>>();
    files.extend(
        flight_events
            .iter()
            .flat_map(|event| event.files.iter())
            .map(|path| path.display().to_string()),
    );
    files
}

fn run_citations(
    state: &deadreckon_core::PipelineState,
    events: &[RunEvent],
    traces: &[TraceRecord],
    flight_events: &[FlightEvent],
    files: &[String],
) -> Vec<NarrativeCitation> {
    let prefix = short_id(&state.run_id);
    let mut citations = vec![NarrativeCitation {
        id: format!("file:{}", state.state_path().display()),
        kind: "state".to_string(),
        path: Some(state.state_path()),
        summary: "run state".to_string(),
    }];
    if let Some(event) = events.last() {
        citations.push(NarrativeCitation {
            id: format!("run:{prefix}:event:{}", events.len()),
            kind: "run_event".to_string(),
            path: Some(state.run_root.join(RUN_EVENTS_JSONL)),
            summary: run_event_summary(&event.event),
        });
    }
    if let Some(trace) = traces.last() {
        citations.push(NarrativeCitation {
            id: format!("trace:{prefix}:turn-{}:{}", trace.turn, trace.event),
            kind: "trace".to_string(),
            path: Some(state.run_root.join("traces.jsonl")),
            summary: format!("turn {} {}", trace.turn, trace.event),
        });
    }
    if let Some((kind, path)) = acceptance_artifact_for_run(state) {
        citations.push(NarrativeCitation {
            id: file_evidence_id(&path),
            kind,
            path: Some(path),
            summary: "acceptance evidence".to_string(),
        });
    }
    for event in flight_events.iter().rev().take(5) {
        citations.push(NarrativeCitation {
            id: format!("flight:{prefix}:seq:{}", event.seq),
            kind: "flight_event".to_string(),
            path: event
                .source_path
                .clone()
                .or_else(|| Some(state.run_root.join("flight-events.jsonl"))),
            summary: one_line(&event.summary, 120),
        });
    }
    for file in files.iter().take(5) {
        citations.push(NarrativeCitation {
            id: format!("file:{file}"),
            kind: "file".to_string(),
            path: Some(PathBuf::from(file)),
            summary: file.clone(),
        });
    }
    citations
}

fn acceptance_artifact_for_run(
    state: &deadreckon_core::PipelineState,
) -> Option<(String, PathBuf)> {
    let marker = marker_path_for_run_root(&state.run_root);
    if marker.exists() {
        return Some(("acceptance_marker".to_string(), marker));
    }
    let progress = acceptance_progress_path_for_run_root(&state.run_root);
    progress
        .exists()
        .then(|| ("acceptance_progress".to_string(), progress))
}

fn file_evidence_id(path: &Path) -> String {
    format!("file:{}", path.display())
}

fn plan_citations(
    paths: &DeadreckonPaths,
    plan: &Plan,
    events: &[PlanEvent],
) -> Vec<NarrativeCitation> {
    let prefix = short_id(&plan.plan_id);
    let mut citations = vec![NarrativeCitation {
        id: format!("file:{}", paths.plan_json(&plan.plan_id).display()),
        kind: "plan".to_string(),
        path: Some(paths.plan_json(&plan.plan_id)),
        summary: "plan state".to_string(),
    }];
    if let Some(event) = events.last() {
        citations.push(NarrativeCitation {
            id: format!("plan:{prefix}:event:{}", events.len()),
            kind: "plan_event".to_string(),
            path: Some(paths.plan_events(&plan.plan_id)),
            summary: plan_event_summary_for_narrative(&event.event),
        });
    }
    for task in &plan.tasks {
        citations.push(NarrativeCitation {
            id: format!("task:{}", task.task_id),
            kind: "task".to_string(),
            path: Some(paths.plan_dir(&plan.plan_id).join(&task.worker_spec)),
            summary: format!("{} {}", task.task_id, task.subject),
        });
        if let Some(run_id) = task.child_run_id.as_ref()
            && let Some((path, snapshot)) = latest_child_narrative_snapshot(paths, run_id)
        {
            citations.push(NarrativeCitation {
                id: child_narrative_evidence_id(run_id, &snapshot),
                kind: "child_narrative".to_string(),
                path: Some(path),
                summary: one_line(&snapshot.headline, 120),
            });
        }
    }
    citations
}

fn child_coverage(
    paths: &DeadreckonPaths,
    plan: &Plan,
) -> BTreeMap<String, NarrativeChildCoverage> {
    let mut children = BTreeMap::new();
    for task in &plan.tasks {
        let Some(run_id) = task.child_run_id.as_ref() else {
            continue;
        };
        let run_event_seq = load_run(paths, run_id)
            .ok()
            .and_then(|state| read_jsonl::<RunEvent>(&state.run_root.join(RUN_EVENTS_JSONL)).ok())
            .map(|events| events.len() as u64);
        let flight_event_seq = load_run(paths, run_id)
            .ok()
            .and_then(|state| read_flight_events(&state).ok())
            .and_then(|events| events.last().map(|event| event.seq));
        children.insert(
            task.task_id.clone(),
            NarrativeChildCoverage {
                run_id: run_id.clone(),
                run_event_seq,
                flight_event_seq,
            },
        );
    }
    children
}

fn repair_run_id_from_feed(feed_events: &[PlanFeedEvent]) -> Option<String> {
    feed_events.iter().rev().find_map(|event| match event {
        PlanFeedEvent::RepairRun { run_id, .. } => Some(run_id.clone()),
        PlanFeedEvent::Plan {
            event:
                PlanEvent {
                    event: PlanEventKind::MergeRepairRunDiscovered { run_id, .. },
                    ..
                },
        } => Some(run_id.clone()),
        _ => None,
    })
}

fn push_claim_section(lines: &mut Vec<String>, title: &str, claims: &[NarrativeClaim]) {
    if claims.is_empty() {
        return;
    }
    lines.push(title.to_string());
    for claim in claims {
        lines.push(format!("- {}  [{}]", claim.text, claim.evidence.join(", ")));
    }
    lines.push(String::new());
}

fn graph_architecture_lines(graph: &ArchitectureGraph) -> Vec<String> {
    let mut lines = Vec::new();
    for root in &graph.layout.root_ids {
        if let Some(node) = graph.nodes.iter().find(|node| &node.id == root) {
            lines.push(format!("[{}] {}", node.status, node.label));
            for edge in graph
                .edges
                .iter()
                .filter(|edge| edge.from == node.id)
                .take(8)
            {
                if let Some(child) = graph.nodes.iter().find(|node| node.id == edge.to) {
                    lines.push(format!(
                        "  -> {} [{}] {}",
                        edge.label, child.status, child.label
                    ));
                }
            }
        }
    }
    if lines.is_empty() {
        lines.push("[stale] not enough architecture evidence yet".to_string());
    }
    lines
}

fn graph_agent_lines(graph: &ArchitectureGraph) -> Vec<String> {
    let mut lines = graph
        .nodes
        .iter()
        .filter(|node| node.kind == "task")
        .map(|node| {
            let deps = graph
                .edges
                .iter()
                .filter(|edge| edge.to == node.id && edge.kind == "depends_on")
                .count();
            format!("[{}] {} deps={deps}", node.status, node.label)
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines = graph_architecture_lines(graph);
    }
    lines
}

fn graph_file_lines(graph: &ArchitectureGraph) -> Vec<String> {
    let mut lines = graph
        .nodes
        .iter()
        .filter(|node| node.kind == "file")
        .map(|node| format!("[{}] {}", node.status, node.label))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push("[stale] no file-level evidence yet".to_string());
    }
    lines
}

fn graph_evidence_lines(graph: &ArchitectureGraph) -> Vec<String> {
    let mut lines = graph
        .edges
        .iter()
        .take(10)
        .map(|edge| {
            format!(
                "{} -> {} [{}] {}",
                edge.from,
                edge.to,
                edge.kind,
                edge.evidence.join(", ")
            )
        })
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push("[stale] no edge evidence yet".to_string());
    }
    lines
}

fn claim(text: String, evidence: Vec<String>, confidence: impl Into<String>) -> NarrativeClaim {
    NarrativeClaim {
        text,
        evidence,
        confidence: confidence.into(),
    }
}

fn seq_window(count: u64) -> Option<SeqWindow> {
    (count > 0).then_some(SeqWindow {
        from_seq: 1,
        to_seq: count,
    })
}

fn index_window(count: usize) -> Option<IndexWindow> {
    (count > 0).then_some(IndexWindow {
        from_index: 0,
        to_index: count.saturating_sub(1),
    })
}

fn file_evidence(files: &[String]) -> Vec<String> {
    files
        .iter()
        .take(8)
        .map(|file| format!("file:{file}"))
        .collect()
}

fn file_cluster_summary(files: &[String]) -> String {
    let clusters = files
        .iter()
        .filter_map(|file| file.split('/').next())
        .filter(|part| !part.is_empty())
        .take(5)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if clusters.is_empty() {
        format!("{} changed path(s)", files.len())
    } else {
        format!("{} ({})", clusters.join(", "), files.len())
    }
}

fn task_summary(paths: &DeadreckonPaths, task: &deadreckon_core::PlanTask) -> String {
    let base = if task.subject.trim().is_empty() {
        task.active_form.clone()
    } else {
        task.subject.clone()
    };
    if let Some(run_id) = task.child_run_id.as_ref()
        && let Ok(state) = load_run(paths, run_id)
    {
        if let Some((_path, snapshot)) = latest_child_narrative_snapshot(paths, run_id) {
            return format!(
                "{}; child narrative {}: {}",
                base,
                snapshot.snapshot_id,
                one_line(&snapshot.headline, 120)
            );
        }
        return format!(
            "{}; run {} turn {} {}",
            base,
            short_id(run_id),
            state.turn,
            run_status_label(state.status)
        );
    }
    base
}

fn task_evidence(paths: &DeadreckonPaths, task: &deadreckon_core::PlanTask) -> Vec<String> {
    let mut evidence = vec![format!("task:{}", task.task_id)];
    if let Some(run_id) = task.child_run_id.as_ref()
        && let Some((_path, snapshot)) = latest_child_narrative_snapshot(paths, run_id)
    {
        evidence.push(child_narrative_evidence_id(run_id, &snapshot));
    }
    evidence
}

fn latest_child_narrative_snapshot(
    paths: &DeadreckonPaths,
    run_id: &str,
) -> Option<(PathBuf, NarrativeSnapshot)> {
    let state = load_run(paths, run_id).ok()?;
    let snapshots_path = state
        .run_root
        .join(NARRATIVE_DIR)
        .join(NARRATIVE_SNAPSHOTS_JSONL);
    let snapshot = read_latest_snapshot(&state.run_root.join(NARRATIVE_DIR)).snapshot?;
    Some((snapshots_path, snapshot))
}

fn latest_child_architecture_graph(
    paths: &DeadreckonPaths,
    run_id: &str,
) -> Option<ArchitectureGraph> {
    let state = load_run(paths, run_id).ok()?;
    read_json(
        &state
            .run_root
            .join(NARRATIVE_DIR)
            .join(ARCHITECTURE_GRAPH_JSON),
    )
}

fn plan_child_graph_files(paths: &DeadreckonPaths, plan: &Plan) -> Vec<String> {
    let mut files = plan
        .tasks
        .iter()
        .filter_map(|task| task.child_run_id.as_deref())
        .filter_map(|run_id| latest_child_architecture_graph(paths, run_id))
        .flat_map(|graph| {
            graph
                .nodes
                .into_iter()
                .filter(|node| node.kind == "file")
                .map(|node| node.label)
        })
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    files
}

fn child_narrative_evidence_id(run_id: &str, snapshot: &NarrativeSnapshot) -> String {
    format!(
        "child-narrative:{}:{}",
        short_id(run_id),
        snapshot.snapshot_id
    )
}

fn run_event_summary(event: &RunEventKind) -> String {
    match event {
        RunEventKind::TurnStarted { turn } => format!("turn {turn} started"),
        RunEventKind::ToolCallStarted {
            turn, tool_name, ..
        } => format!("turn {turn} {tool_name} started"),
        RunEventKind::ToolCallResult {
            turn,
            status,
            preview,
            ..
        } => format!("turn {turn} tool {status}: {}", one_line(preview, 120)),
        RunEventKind::TokenUsageDelta {
            turn,
            input_tokens,
            output_tokens,
        } => format!("turn {turn} tokens +{}", input_tokens + output_tokens),
        RunEventKind::SpendDelta {
            turn,
            wall_time_seconds,
            ..
        } => format!("turn {turn} wall {}s", wall_time_seconds.unwrap_or(0.0)),
        RunEventKind::DocsCheckpoint { turn, status, .. } => {
            format!("turn {turn} docs {status}")
        }
        RunEventKind::RunCompleted { status } => format!("run {status}"),
        RunEventKind::RunPromoted { library_dir } => {
            format!("promoted {}", library_dir.display())
        }
        RunEventKind::Error { turn, message } => {
            format!("turn {} error {message}", turn.unwrap_or_default())
        }
    }
}

fn plan_event_summary_for_narrative(event: &PlanEventKind) -> String {
    match event {
        PlanEventKind::PlanCreated { mode, task_count } => {
            format!(
                "plan created in {} mode with {task_count} task(s)",
                mode.as_str()
            )
        }
        PlanEventKind::PlanStarted => "plan started".to_string(),
        PlanEventKind::TaskReady { task_id, .. } => format!("{task_id} ready"),
        PlanEventKind::TaskStarted { task_id, .. } => format!("{task_id} started"),
        PlanEventKind::TaskRunDiscovered {
            task_id, run_id, ..
        } => format!(
            "{task_id} run discovered {}",
            run_id
                .as_deref()
                .map(short_id)
                .unwrap_or_else(|| "-".to_string())
        ),
        PlanEventKind::TaskCompleted {
            task_id, status, ..
        } => format!("{task_id} completed with {status}"),
        PlanEventKind::TaskBlocked {
            task_id, reason, ..
        } => format!("{task_id} blocked: {reason}"),
        PlanEventKind::TaskFailed {
            task_id, reason, ..
        } => format!("{task_id} failed: {reason}"),
        PlanEventKind::TaskKilled { task_id, .. } => format!("{task_id} killed"),
        PlanEventKind::MergeStarted => "merge started".to_string(),
        PlanEventKind::MergeConflict { conflict_count } => {
            format!("merge conflict count {conflict_count}")
        }
        PlanEventKind::MergeRepairPlanned { conflict_count, .. } => {
            format!("merge repair planned for {conflict_count} conflict(s)")
        }
        PlanEventKind::MergeRepairStarted { mode } => {
            format!("merge repair started in {mode} mode")
        }
        PlanEventKind::MergeRepairRunDiscovered { run_id, .. } => {
            format!("merge repair run {}", short_id(run_id))
        }
        PlanEventKind::MergeRepaired { strategy, .. } => {
            format!("merge repaired via {strategy}")
        }
        PlanEventKind::MergeRepairFailed { reason } => {
            format!("merge repair failed: {reason}")
        }
        PlanEventKind::MergeCompleted { merged_run_id } => {
            format!("merge completed run {}", short_id(merged_run_id))
        }
        PlanEventKind::PlanCompleted => "plan completed".to_string(),
        PlanEventKind::PlanFailed { reason } => format!("plan failed: {reason}"),
        PlanEventKind::PlanKilled => "plan killed".to_string(),
    }
}

fn coverage_label(coverage: &NarrativeCoverage) -> String {
    let mut parts = Vec::new();
    if let Some(seq) = coverage.run_event_seq {
        parts.push(format!("run event #{seq}"));
    }
    if let Some(seq) = coverage.flight_event_seq {
        parts.push(format!("flight #{seq}"));
    }
    if let Some(seq) = coverage.plan_event_seq {
        parts.push(format!("plan event #{seq}"));
    }
    if let Some(checkpoint) = coverage.checkpoint_id.as_ref() {
        parts.push(format!("checkpoint {checkpoint}"));
    }
    if parts.is_empty() {
        "state only".to_string()
    } else {
        parts.join(" / ")
    }
}

fn narrative_status_label(status: &NarrativeStatus) -> &'static str {
    match status {
        NarrativeStatus::Fresh => "fresh",
        NarrativeStatus::Stale => "stale",
        NarrativeStatus::Failed => "failed",
        NarrativeStatus::Disabled => "disabled",
        NarrativeStatus::Redacted => "redacted",
        NarrativeStatus::Deterministic => "deterministic fallback",
    }
}

fn docs_hash(state: &deadreckon_core::PipelineState) -> Option<String> {
    let docs_dir = state.working_dir.join(".deadreckon/docs");
    let mut hasher = Sha256::new();
    let mut found = false;
    for name in ["RUN-NARRATIVE.md", "RUN-DECISIONS.md", "RUN-AS-BUILT.md"] {
        let path = docs_dir.join(name);
        if let Ok(bytes) = fs::read(path) {
            hasher.update(name.as_bytes());
            hasher.update(bytes);
            found = true;
        }
    }
    found.then(|| format!("sha256:{}", hex_hash(hasher.finalize().as_slice())))
}

fn snapshot_id<T: Serialize>(parts: &T) -> String {
    let hash = stable_hash(parts);
    format!("nar-{}", &hash[..12.min(hash.len())])
}

fn stable_hash<T: Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_hash(hasher.finalize().as_slice())
}

fn graph_content_hash(graph: &ArchitectureGraph) -> String {
    stable_hash(&(
        graph.version,
        &graph.scope,
        &graph.target_id,
        &graph.source_window,
        &graph.default_visual,
        &graph.nodes,
        &graph.edges,
        &graph.groups,
        &graph.layout,
        &graph.legend,
    ))
}

fn hex_hash(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn style_for_run_status(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Completed => "success",
        RunStatus::Failed | RunStatus::Killed => "danger",
        RunStatus::Executing => "primary",
        RunStatus::Pending | RunStatus::Planned => "muted",
    }
}

fn style_for_plan_status(status: PlanStatus) -> &'static str {
    match status {
        PlanStatus::Merged => "success",
        PlanStatus::Failed => "danger",
        PlanStatus::Forked => "primary",
        PlanStatus::Pending => "muted",
    }
}

fn style_for_task_status(status: PlanTaskStatus) -> &'static str {
    match status {
        PlanTaskStatus::Completed => "success",
        PlanTaskStatus::Failed | PlanTaskStatus::Killed => "danger",
        PlanTaskStatus::Running => "primary",
        PlanTaskStatus::Pending => "muted",
    }
}

fn default_legend() -> Vec<ArchitectureLegendItem> {
    vec![
        ArchitectureLegendItem {
            style_token: "primary".to_string(),
            meaning: "active work".to_string(),
        },
        ArchitectureLegendItem {
            style_token: "success".to_string(),
            meaning: "done".to_string(),
        },
        ArchitectureLegendItem {
            style_token: "warning".to_string(),
            meaning: "risk or stale evidence".to_string(),
        },
        ArchitectureLegendItem {
            style_token: "danger".to_string(),
            meaning: "blocked or failed".to_string(),
        },
    ]
}

fn write_json_pretty<T: Serialize>(path: &Path, value: &T) -> crate::Result<()> {
    let parent = path.parent().ok_or_else(|| crate::CliError::Exit {
        code: 1,
        message: format!("path has no parent: {}", path.display()),
        hint: "rerun attach with a valid run or plan id".to_string(),
    })?;
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("narrative.json");
    let temp_path = parent.join(format!(".{file_name}.{}.tmp", Uuid::new_v4().simple()));
    let mut temp = File::create(&temp_path)?;
    serde_json::to_writer_pretty(&mut temp, value)?;
    temp.write_all(b"\n")?;
    temp.sync_all()?;
    drop(temp);
    fs::rename(&temp_path, path)?;
    Ok(())
}

fn append_json_line<T: Serialize>(path: &Path, value: &T) -> crate::Result<()> {
    let parent = path.parent().ok_or_else(|| crate::CliError::Exit {
        code: 1,
        message: format!("path has no parent: {}", path.display()),
        hint: "rerun attach with a valid run or plan id".to_string(),
    })?;
    fs::create_dir_all(parent)?;
    let mut file = File::options().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn read_jsonl<T: DeserializeOwned>(path: &Path) -> crate::Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

fn short_id(value: &str) -> String {
    value.chars().take(8).collect()
}

fn compact_whitespace(value: &str) -> String {
    value
        .split_whitespace()
        .fold(String::new(), |mut out, word| {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
            out
        })
}

fn one_line(value: &str, max_chars: usize) -> String {
    let compact = compact_whitespace(value);
    let mut chars = compact.chars();
    let shortened = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{shortened}...")
    } else {
        shortened
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deadreckon_core::{
        AcceptanceCheckResult, AcceptanceProgressEntry, PlanMode, PlanProviders, PlanRole,
        PlanTask, PlanTaskStatus, RunOptions, acceptance_progress_path_for_run_root, create_run,
    };
    use tempfile::TempDir;

    #[test]
    fn narrative_snapshot_schema2_roundtrips_and_reads_legacy_schema1() {
        // A legacy schema-1 snapshot (no `live` field) must still deserialize,
        // with `live` defaulting to None.
        let legacy = r#"{
            "version": 1,
            "snapshot_id": "snap-1",
            "scope": "run",
            "target_id": "run-abc",
            "created_at": "2026-06-15T00:00:00Z",
            "status": "deterministic",
            "source_window": {},
            "coverage": {"skipped_events": 0, "redacted_events": 0, "known_gaps": []},
            "headline": "Started",
            "current_work": [],
            "architecture_notes": [],
            "risks": [],
            "next_likely": [],
            "citations": []
        }"#;
        let parsed: NarrativeSnapshot =
            serde_json::from_str(legacy).expect("legacy schema-1 snapshot parses");
        assert!(
            parsed.live.is_none(),
            "legacy snapshot carries no live beat"
        );

        // A schema-2 live beat round-trips byte-for-byte through serde.
        let live = NarrativeSnapshot {
            live: Some(LiveBeat {
                beat_seq: 7,
                covers_turn: 14,
                source: NarrativeSource::Live,
                rolling_summary: Some("Through turn 14: wired the bus.".to_string()),
            }),
            ..parsed
        };
        let encoded = serde_json::to_string(&live).expect("encode schema-2");
        let round: NarrativeSnapshot = serde_json::from_str(&encoded).expect("round-trip");
        assert_eq!(round.live, live.live);
        let beat = round.live.expect("live beat present");
        assert_eq!(beat.beat_seq, 7);
        assert_eq!(beat.covers_turn, 14);
        assert_eq!(beat.source, NarrativeSource::Live);
    }

    fn live_previous_snapshot() -> NarrativeSnapshot {
        serde_json::from_str(
            r#"{
                "version": 2,
                "snapshot_id": "beat-0",
                "scope": "run",
                "target_id": "run-x",
                "created_at": "2026-06-15T00:00:00Z",
                "status": "deterministic",
                "source_window": {},
                "coverage": {"skipped_events": 0, "redacted_events": 0, "known_gaps": []},
                "headline": "Started run",
                "current_work": [],
                "architecture_notes": [],
                "risks": [],
                "next_likely": [],
                "citations": [{"id": "state", "kind": "state", "path": null, "summary": "run state"}]
            }"#,
        )
        .expect("previous snapshot parses")
    }

    fn live_turns() -> Vec<LiveTurnInput> {
        vec![LiveTurnInput {
            turn: 14,
            title: "Wire bus".to_string(),
            summary: "Added narrator task to run.rs".to_string(),
            tool_kind: "write_file".to_string(),
            outcome: "ok".to_string(),
            files: vec!["run.rs".to_string()],
        }]
    }

    fn live_meta() -> LiveBeatMeta {
        LiveBeatMeta {
            beat_seq: 1,
            covers_turn: 14,
            rolling_summary: Some("Through turn 14: wired the bus.".to_string()),
            provider: NarrativeProviderRefresh {
                route: "cli:claude-code".to_string(),
                model: "haiku".to_string(),
                cost_usd: 0.0,
                subscription_seconds: None,
            },
        }
    }

    #[test]
    fn live_prompt_includes_previous_narrative_and_only_window_turns() {
        let bundle = build_live_narrator_prompt(
            &live_previous_snapshot(),
            &live_turns(),
            Some("Through turn 13: set up plumbing."),
        )
        .expect("live prompt builds");
        assert!(
            bundle.prompt.contains("Started run"),
            "carries the previous narrative forward"
        );
        assert!(
            bundle.prompt.contains("Added narrator task to run.rs"),
            "includes the windowed new turn"
        );
        assert!(bundle.evidence_ids.contains(&"turn:14".to_string()));
        assert!(
            !bundle.evidence_ids.contains(&"turn:99".to_string()),
            "only window turns are citable"
        );
        assert!(!bundle.prompt.contains("turn:99"));
    }

    #[test]
    fn apply_live_response_appends_beat_does_not_overwrite() {
        let temp = TempDir::new().expect("tempdir");
        let dir = temp.path().join("narrative");
        let previous = live_previous_snapshot();
        append_narrative_snapshot(&dir, &previous).expect("write previous beat");

        let response = r#"{"headline":"Wired the bus","current_work":[{"text":"Added the narrator task","evidence":["turn:14"],"confidence":"high"}]}"#;
        let beat = apply_live_narrator_response(&previous, &live_turns(), response, live_meta())
            .expect("apply live response");
        assert_ne!(beat.snapshot_id, previous.snapshot_id, "new beat id");
        let live = beat.live.as_ref().expect("live beat metadata");
        assert_eq!(live.beat_seq, 1);
        assert_eq!(live.covers_turn, 14);
        assert_eq!(live.source, NarrativeSource::Live);
        assert_eq!(previous.headline, "Started run", "previous left untouched");

        append_narrative_snapshot(&dir, &beat).expect("append beat");
        let raw = std::fs::read_to_string(dir.join("snapshots.jsonl")).expect("read jsonl");
        assert_eq!(
            raw.lines().filter(|line| !line.trim().is_empty()).count(),
            2,
            "appended, not overwritten"
        );
        let latest = read_latest_snapshot(&dir).snapshot.expect("latest beat");
        assert_eq!(latest.snapshot_id, beat.snapshot_id);
    }

    #[test]
    fn live_beat_rejects_evidence_id_for_nonexistent_turn() {
        let response = r#"{"headline":"x","current_work":[{"text":"y","evidence":["turn:999"],"confidence":"high"}]}"#;
        let result = apply_live_narrator_response(
            &live_previous_snapshot(),
            &live_turns(),
            response,
            live_meta(),
        );
        assert!(
            result.is_err(),
            "a beat citing a turn outside the window is rejected"
        );
    }

    #[test]
    fn live_beat_accepts_new_turn_evidence_id() {
        let response = r#"{"headline":"x","current_work":[{"text":"y","evidence":["turn:14"],"confidence":"high"}]}"#;
        let result = apply_live_narrator_response(
            &live_previous_snapshot(),
            &live_turns(),
            response,
            live_meta(),
        );
        assert!(result.is_ok(), "a beat citing a windowed turn is accepted");
    }

    fn window_turn(turn: u32, summary: &str) -> LiveTurnInput {
        LiveTurnInput {
            turn,
            title: format!("turn {turn}"),
            summary: summary.to_string(),
            tool_kind: "bash".to_string(),
            outcome: "ok".to_string(),
            files: Vec::new(),
        }
    }

    #[test]
    fn foreground_block_is_bounded_to_narrate_lines() {
        let mut snapshot = live_previous_snapshot();
        snapshot.headline = "Working".to_string();
        snapshot.current_work = (0..10)
            .map(|index| NarrativeClaim {
                text: format!("claim {index}"),
                evidence: vec!["state".to_string()],
                confidence: "high".to_string(),
            })
            .collect();
        let lines = live_block_lines(&snapshot, 4);
        assert!(lines.len() <= 4, "never exceeds narrate_lines");
        assert_eq!(lines[0], "Working", "leads with the headline");
    }

    #[test]
    fn narrator_window_feeds_only_new_turns_not_full_trace() {
        let mut window = NarratorWindow::new();
        for turn in 1..=3 {
            window.observe(window_turn(turn, "did work"));
        }
        assert_eq!(
            window.pending().iter().map(|t| t.turn).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(window.commit_beat(), Some(3));
        assert!(!window.has_pending());

        window.observe(window_turn(3, "already covered")); // <= watermark, ignored
        window.observe(window_turn(4, "new work"));
        assert_eq!(
            window.pending().iter().map(|t| t.turn).collect::<Vec<_>>(),
            vec![4],
            "the window carries only the new turn, never the whole 1..4 trace"
        );
    }

    #[test]
    fn rolling_summary_stays_under_cap_over_120_turns() {
        let mut window = NarratorWindow::new();
        let long = "a fairly long per-turn summary sentence ".repeat(8);
        for turn in 1..=120u32 {
            window.observe(window_turn(turn, &long));
            window.commit_beat();
            let len = window
                .rolling_summary()
                .map(|s| s.chars().count())
                .unwrap_or(0);
            assert!(
                len <= ROLLING_SUMMARY_CAP,
                "rolling summary stays bounded at turn {turn}: {len}"
            );
        }
    }

    #[test]
    fn narrator_input_token_estimate_is_o_turns_not_o_turns_squared() {
        let mut window = NarratorWindow::new();
        let summary = "summary fragment ".repeat(16);
        let mut max_estimate = 0usize;
        for turn in 1..=120u32 {
            window.observe(window_turn(turn, &summary));
            max_estimate = max_estimate.max(window.beat_input_chars());
            window.commit_beat();
        }
        // Per-beat input is the bounded rolling summary plus a single window
        // turn — a constant ceiling regardless of run length. Accumulating the
        // full trace would be ~120x larger; this bound rules that out.
        assert!(
            max_estimate <= ROLLING_SUMMARY_CAP + 1000,
            "per-beat input stays O(1): {max_estimate}"
        );
    }

    #[test]
    fn architecture_graph_requires_evidence_on_nodes_and_edges() {
        let graph = ArchitectureGraph {
            version: 1,
            graph_id: "arch-test".to_string(),
            scope: NarrativeScope::Run,
            target_id: "run".to_string(),
            generated_at: Utc::now(),
            source_window: NarrativeSourceWindow::default(),
            default_visual: NarrativeVisualMode::Architecture,
            nodes: vec![ArchitectureNode {
                id: "node".to_string(),
                label: "node".to_string(),
                kind: "file".to_string(),
                status: "active".to_string(),
                weight: 1,
                evidence: vec!["file:src/main.rs".to_string()],
                style_token: "primary".to_string(),
            }],
            edges: vec![ArchitectureEdge {
                from: "node".to_string(),
                to: "other".to_string(),
                label: "touches".to_string(),
                kind: "writes".to_string(),
                evidence: vec!["file:src/main.rs".to_string()],
            }],
            groups: Vec::new(),
            layout: ArchitectureLayout {
                kind: "layered-tree".to_string(),
                root_ids: vec!["node".to_string()],
                warnings: Vec::new(),
            },
            legend: default_legend(),
        };
        assert!(validate_graph(&graph));
        let mut broken = graph;
        broken.nodes[0].evidence.clear();
        assert!(!validate_graph(&broken));
    }

    #[test]
    fn run_narrative_state_round_trips_without_pipeline_state() {
        let temp = TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        fs::create_dir_all(temp.path().join("repo")).expect("repo");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "test narrative attach".to_string(),
                cwd: temp.path().join("repo"),
                sandbox: "none".to_string(),
                provider: Some("cli:test".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("run-narrative-test".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        fs::write(state.working_dir.join("src.txt"), "hello").expect("file");
        let projection = build_run_projection(&RunNarrativeInput {
            state: &state,
            spend: &[],
            traces: &[],
            events: &[],
            live_files: vec![LiveFileFact {
                path: "src.txt".to_string(),
                bytes: 5,
                modified_at: None,
            }],
            file_count: 1,
            total_bytes: 5,
            acceptance_summary: "default gate".to_string(),
            provider_activity: &[],
            parent_plan: None,
        });
        assert_eq!(projection.state.scope, NarrativeScope::Run);
        assert_eq!(projection.state.target_id, state.run_id);
        assert_eq!(state.version, 1);
        assert!(validate_graph(&projection.graph));
    }

    #[test]
    fn plain_narrative_attach_renders_ascii_architecture_map() {
        let temp = TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        fs::create_dir_all(temp.path().join("repo")).expect("repo");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "render map".to_string(),
                cwd: temp.path().join("repo"),
                sandbox: "none".to_string(),
                provider: Some("cli:test".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("run-map-test".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        let projection = build_run_projection(&RunNarrativeInput {
            state: &state,
            spend: &[],
            traces: &[],
            events: &[],
            live_files: vec![LiveFileFact {
                path: "crates/deadreckon/src/main.rs".to_string(),
                bytes: 10,
                modified_at: None,
            }],
            file_count: 1,
            total_bytes: 10,
            acceptance_summary: "default gate".to_string(),
            provider_activity: &[],
            parent_plan: None,
        });
        let rendered =
            narrative_plain_lines(&projection, NarrativeVisualMode::Architecture).join("\n");
        assert!(rendered.contains("Visual: architecture"));
        assert!(rendered.contains("-> touches"));
        assert!(rendered.contains("crates/deadreckon/src/main.rs"));
    }

    #[test]
    fn run_evidence_window_pins_acceptance_failure_outside_cap() {
        let temp = TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        fs::create_dir_all(temp.path().join("repo")).expect("repo");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "capture acceptance evidence".to_string(),
                cwd: temp.path().join("repo"),
                sandbox: "none".to_string(),
                provider: Some("cli:test".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("run-acceptance-evidence".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        let progress_path = acceptance_progress_path_for_run_root(&state.run_root);
        fs::create_dir_all(progress_path.parent().expect("progress parent")).expect("proofs");
        let failed = AcceptanceProgressEntry {
            checked_at: Utc::now(),
            status: "failed".to_string(),
            index: 24,
            total: 24,
            result: Some(AcceptanceCheckResult {
                kind: "shell".to_string(),
                passed: false,
                must_pass: true,
                detail: "cargo clippy failed after a long acceptance list".to_string(),
                command: Some("cargo clippy -p deadreckon".to_string()),
                cwd: None,
                duration_ms: Some(123),
                stdout: None,
                stderr: Some("warning promoted to error".to_string()),
            }),
        };
        fs::write(
            &progress_path,
            format!("{}\n", serde_json::to_string(&failed).expect("entry")),
        )
        .expect("progress");

        let projection = build_run_projection(&RunNarrativeInput {
            state: &state,
            spend: &[],
            traces: &[],
            events: &[],
            live_files: Vec::new(),
            file_count: 0,
            total_bytes: 0,
            acceptance_summary: "failed 1 required, 0 passed of 24".to_string(),
            provider_activity: &[],
            parent_plan: None,
        });
        let acceptance_evidence = format!("file:{}", progress_path.display());

        assert!(
            projection
                .snapshot
                .risks
                .iter()
                .any(|claim| claim.evidence.contains(&acceptance_evidence)),
            "{:#?}",
            projection.snapshot.risks
        );
        assert!(
            projection
                .snapshot
                .citations
                .iter()
                .any(|citation| citation.kind == "acceptance_progress"
                    && citation.id == acceptance_evidence),
            "{:#?}",
            projection.snapshot.citations
        );
    }

    #[test]
    fn plan_evidence_window_rolls_up_child_narratives() {
        let temp = TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo");
        let child_state = create_run(
            &paths,
            RunOptions {
                goal: "child finds architecture boundary".to_string(),
                cwd: repo,
                sandbox: "none".to_string(),
                provider: Some("cli:test".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("child-rollup-run".to_string()),
                codebase: None,
            },
        )
        .expect("child run");
        let mut child_projection = build_run_projection(&RunNarrativeInput {
            state: &child_state,
            spend: &[],
            traces: &[],
            events: &[],
            live_files: vec![LiveFileFact {
                path: "crates/deadreckon/src/plan_event_bus.rs".to_string(),
                bytes: 10,
                modified_at: None,
            }],
            file_count: 1,
            total_bytes: 10,
            acceptance_summary: "default gate".to_string(),
            provider_activity: &[],
            parent_plan: None,
        });
        child_projection.snapshot.snapshot_id = "child-nar-rollup".to_string();
        child_projection.state.latest_snapshot_id = "child-nar-rollup".to_string();
        child_projection.snapshot.headline =
            "Child identified the plan event bus boundary.".to_string();
        persist_run_projection(&child_state, &child_projection).expect("persist child narrative");

        let mut task = PlanTask::new(
            0,
            "Inspect plan child feed",
            "Inspect the child feed",
            PlanRole::Child,
            Some("cli:test".to_string()),
        );
        task.child_run_id = Some(child_state.run_id.clone());
        task.status = PlanTaskStatus::Running;
        let other_task = PlanTask::new(
            1,
            "Review child summary",
            "Review the summary",
            PlanRole::Child,
            Some("cli:test".to_string()),
        );
        let plan = Plan::new(
            "roll up child narratives",
            PlanMode::FullPlan,
            vec![task, other_task],
            PlanProviders {
                planner: Some("cli:test".to_string()),
                default_child: Some("cli:test".to_string()),
                coder: None,
                reviewer: None,
                children: BTreeMap::new(),
                ..PlanProviders::default()
            },
            None,
            "0.1.0",
        )
        .expect("plan");

        let projection = build_plan_projection(&PlanNarrativeInput {
            paths: &paths,
            plan: &plan,
            messages: &[],
            plan_events: &[],
            feed_events: &[],
            selected: 0,
        });

        let row = projection.snapshot.agent_table.first().expect("agent row");
        assert!(row.summary.contains("Child identified"), "{row:#?}");
        assert!(
            row.evidence
                .iter()
                .any(|id| id.starts_with("child-narrative:")),
            "{row:#?}"
        );
        assert!(
            projection
                .snapshot
                .citations
                .iter()
                .any(|citation| citation.kind == "child_narrative"),
            "{:#?}",
            projection.snapshot.citations
        );
        assert!(
            projection
                .snapshot
                .source_window
                .files
                .iter()
                .any(|file| file.contains("plan_event_bus.rs")),
            "{:#?}",
            projection.snapshot.source_window.files
        );
        let file_visual =
            graph_ascii_lines(&projection.graph, NarrativeVisualMode::Files).join("\n");
        assert!(file_visual.contains("plan_event_bus.rs"), "{file_visual}");
    }

    #[test]
    fn latest_snapshot_skips_malformed_rows_and_reports_gap() {
        let temp = TempDir::new().expect("temp");
        let projection = sample_projection(&temp);
        let narrative_dir = temp.path().join("narrative");
        fs::create_dir_all(&narrative_dir).expect("dir");
        fs::write(
            narrative_dir.join(NARRATIVE_SNAPSHOTS_JSONL),
            format!(
                "{{not json}}\n{}\n",
                serde_json::to_string(&projection.snapshot).expect("snapshot json")
            ),
        )
        .expect("write");

        let latest = read_latest_snapshot(&narrative_dir);

        assert_eq!(latest.skipped_malformed_rows, 1);
        assert_eq!(
            latest
                .snapshot
                .as_ref()
                .map(|snapshot| &snapshot.snapshot_id),
            Some(&projection.snapshot.snapshot_id)
        );
    }

    #[test]
    fn narrative_redaction_removes_secret_like_values_before_provider_input() {
        let raw = format!(
            "{}[31mtoken=sk-testsecret123456789 user@example.com -----BEGIN PRIVATE KEY-----\nabc\n-----END PRIVATE KEY-----",
            '\u{1b}'
        );
        let report = redact_for_provider(&raw);

        assert!(report.text.contains("<redacted-secret>"));
        assert!(report.text.contains("<redacted-email>"));
        assert!(report.text.contains("<redacted-private-key>"));
        assert!(!report.text.contains("testsecret"));
        assert!(!report.text.contains("\u{1b}"));
        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.contains("terminal control"))
        );
    }

    #[test]
    fn provider_prompt_frontloads_json_only_instruction() {
        let temp = TempDir::new().expect("temp");
        let projection = sample_projection(&temp);

        let prompt = build_provider_prompt(&projection).expect("prompt");

        assert!(
            prompt
                .prompt
                .starts_with("You are a narrative projector over cited evidence"),
            "{}",
            prompt.prompt
        );
        assert!(prompt.prompt.contains("Return exactly one raw JSON object"));
        assert!(prompt.prompt.contains("Do not write Markdown"));
    }

    #[test]
    fn summarizer_rejects_json_without_narrative_fields() {
        let temp = TempDir::new().expect("temp");
        let projection = sample_projection(&temp);
        let content = json!({
            "action": "done",
            "summary": "This is a coding-provider response, not a narrative response."
        })
        .to_string();

        let err = apply_provider_response(&projection, &content, sample_provider())
            .expect_err("schema-less provider output rejected");

        assert!(err.to_string().contains("did not include headline"));
    }

    #[test]
    fn narrative_provider_refusal_hint_is_raw_primary_action() {
        let err = parse_provider_narrative_output("not json")
            .expect_err("invalid provider output rejected");

        match err {
            crate::CliError::Exit { hint, .. } => {
                assert!(
                    !hint.starts_with("try:"),
                    "raw exit hints should not embed try footers: {hint}"
                );
                assert_eq!(
                    hint,
                    "press r later or use --no-narrative-provider for deterministic fallback"
                );
            }
            other => panic!("expected exit error, got {other:?}"),
        }
    }

    #[test]
    fn summarizer_accepts_fenced_json_but_still_validates_claims() {
        let temp = TempDir::new().expect("temp");
        let projection = sample_projection(&temp);
        let evidence = projection.snapshot.citations[0].id.clone();
        let content = format!(
            "```json\n{}\n```",
            json!({
                "headline": "Narrated fenced JSON.",
                "current_work": [
                    {"text": "The provider can wrap JSON while preserving citations.", "evidence": [evidence], "confidence": "high"}
                ]
            })
        );

        let refreshed =
            apply_provider_response(&projection, &content, sample_provider()).expect("refresh");

        assert_eq!(refreshed.state.latest_status, NarrativeStatus::Fresh);
        assert_eq!(refreshed.snapshot.headline, "Narrated fenced JSON.");
    }

    #[test]
    fn claim_validation_rejects_missing_evidence_ids() {
        let temp = TempDir::new().expect("temp");
        let projection = sample_projection(&temp);
        let content = json!({
            "headline": "Narrated run",
            "current_work": [
                {"text": "This claim invents evidence.", "evidence": ["run:missing:event:9"], "confidence": "high"}
            ]
        })
        .to_string();

        let err = apply_provider_response(&projection, &content, sample_provider())
            .expect_err("unknown evidence rejected");

        assert!(err.to_string().contains("unknown evidence"));
    }

    #[test]
    fn summarizer_cannot_invent_architecture_graph_nodes() {
        let temp = TempDir::new().expect("temp");
        let projection = sample_projection(&temp);
        let evidence = projection.snapshot.citations[0].id.clone();
        let content = json!({
            "headline": "Narrated run",
            "current_work": [
                {"text": "The run is explained from evidence.", "evidence": [evidence], "confidence": "high"}
            ],
            "graph_labels": [
                {"target_id": "node:invented", "label": "Invented node"}
            ]
        })
        .to_string();

        let err = apply_provider_response(&projection, &content, sample_provider())
            .expect_err("invented graph node rejected");

        assert!(err.to_string().contains("unknown graph target"));
    }

    #[test]
    fn summarizer_uses_fake_provider_and_writes_cited_snapshot() {
        let temp = TempDir::new().expect("temp");
        let projection = sample_projection(&temp);
        let evidence = projection.snapshot.citations[0].id.clone();
        let first_node = projection.graph.nodes[0].id.clone();
        let content = json!({
            "headline": "Narrated: the run has a cited operator summary.",
            "current_work": [
                {"text": "The deterministic run facts have been rewritten as an operator summary.", "evidence": [evidence], "confidence": "high"}
            ],
            "architecture_notes": [],
            "risks": [],
            "next_likely": [
                {"text": "The operator can inspect raw activity next if needed.", "evidence": [projection.snapshot.citations[0].id], "confidence": "low"}
            ],
            "graph_labels": [
                {"target_id": first_node, "label": "operator run"}
            ]
        })
        .to_string();

        let refreshed =
            apply_provider_response(&projection, &content, sample_provider()).expect("refresh");
        persist_projection(&refreshed, &temp.path().join("narrative")).expect("persist");
        let latest = read_latest_snapshot(&temp.path().join("narrative"));
        let rendered =
            narrative_plain_lines(&refreshed, NarrativeVisualMode::Architecture).join("\n");

        assert_eq!(refreshed.state.latest_status, NarrativeStatus::Fresh);
        assert_eq!(refreshed.state.provider.source, "provider");
        assert!(latest.snapshot.is_some());
        assert!(rendered.contains("freshness: fresh via provider"));
        assert!(rendered.contains("operator summary"));
    }

    #[test]
    fn provider_snapshot_survives_deterministic_redraw_until_coverage_changes() {
        let temp = TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "redraw keeps provider snapshot".to_string(),
                cwd: repo,
                sandbox: "none".to_string(),
                provider: Some("cli:test".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("run-redraw-test".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        let input = RunNarrativeInput {
            state: &state,
            spend: &[],
            traces: &[],
            events: &[],
            live_files: vec![LiveFileFact {
                path: "src/main.rs".to_string(),
                bytes: 10,
                modified_at: None,
            }],
            file_count: 1,
            total_bytes: 10,
            acceptance_summary: "default gate".to_string(),
            provider_activity: &[],
            parent_plan: None,
        };
        let projection = ensure_run_projection(&input).expect("projection");
        let evidence = projection.snapshot.citations[0].id.clone();
        let provider_content = json!({
            "headline": "Narrated provider snapshot survives redraw.",
            "current_work": [
                {"text": "Provider prose is current for the same evidence coverage.", "evidence": [evidence], "confidence": "high"}
            ]
        })
        .to_string();
        let refreshed = apply_provider_response(&projection, &provider_content, sample_provider())
            .expect("provider projection");
        persist_run_projection(&state, &refreshed).expect("persist provider");

        let redraw = ensure_run_projection(&input).expect("redraw projection");

        assert_eq!(redraw.state.latest_status, NarrativeStatus::Fresh);
        assert_eq!(redraw.snapshot.status, NarrativeStatus::Fresh);
        assert_eq!(redraw.state.provider.source, "provider");
        assert_eq!(redraw.state.provider.calls, 1);
        assert_eq!(redraw.state.provider.model.as_deref(), Some("test-model"));
        assert_eq!(redraw.snapshot.headline, refreshed.snapshot.headline);
        assert_eq!(
            redraw.state.latest_snapshot_id,
            refreshed.state.latest_snapshot_id
        );
        let rendered = narrative_plain_lines(&redraw, NarrativeVisualMode::None).join("\n");
        assert!(
            rendered.contains("freshness: fresh via provider"),
            "{rendered}"
        );
    }

    #[test]
    fn provider_snapshot_remains_visible_when_coverage_advances_before_refresh() {
        let temp = TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "redraw keeps stale provider snapshot".to_string(),
                cwd: repo,
                sandbox: "none".to_string(),
                provider: Some("cli:test".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("run-redraw-stale-provider-test".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        let live_files = vec![LiveFileFact {
            path: "src/main.rs".to_string(),
            bytes: 10,
            modified_at: None,
        }];
        let input = RunNarrativeInput {
            state: &state,
            spend: &[],
            traces: &[],
            events: &[],
            live_files: live_files.clone(),
            file_count: 1,
            total_bytes: 10,
            acceptance_summary: "default gate".to_string(),
            provider_activity: &[],
            parent_plan: None,
        };
        let projection = ensure_run_projection(&input).expect("projection");
        let evidence = projection.snapshot.citations[0].id.clone();
        let provider_content = json!({
            "headline": "Provider narrative remains visible while refresh is pending.",
            "current_work": [
                {"text": "Provider prose remains active until the next provider refresh catches up.", "evidence": [evidence], "confidence": "high"}
            ]
        })
        .to_string();
        let refreshed = apply_provider_response(&projection, &provider_content, sample_provider())
            .expect("provider projection");
        persist_run_projection(&state, &refreshed).expect("persist provider");
        let snapshot_path = state
            .run_root
            .join(NARRATIVE_DIR)
            .join(NARRATIVE_SNAPSHOTS_JSONL);
        let snapshot_count_before = fs::read_to_string(&snapshot_path)
            .expect("snapshots before")
            .lines()
            .count();
        let traces = vec![TraceRecord {
            timestamp: Utc::now(),
            run_id: state.run_id.clone(),
            turn: 1,
            event: "provider_activity".to_string(),
            latency_ms: Some(12),
            detail: json!({"seq": 1}),
        }];
        let advanced_input = RunNarrativeInput {
            state: &state,
            spend: &[],
            traces: &traces,
            events: &[],
            live_files,
            file_count: 1,
            total_bytes: 10,
            acceptance_summary: "default gate".to_string(),
            provider_activity: &[],
            parent_plan: None,
        };

        let redraw = ensure_run_projection(&advanced_input).expect("redraw projection");

        assert_eq!(
            redraw.snapshot.headline,
            "Provider narrative remains visible while refresh is pending."
        );
        assert_eq!(
            redraw.state.latest_snapshot_id,
            refreshed.state.latest_snapshot_id
        );
        assert_eq!(redraw.snapshot.snapshot_id, refreshed.snapshot.snapshot_id);
        assert_eq!(redraw.state.latest_status, NarrativeStatus::Stale);
        assert_eq!(redraw.snapshot.status, NarrativeStatus::Fresh);
        assert_eq!(redraw.state.provider.source, "provider");
        assert_eq!(redraw.state.provider.calls, 1);
        assert_eq!(redraw.state.latest_covered.trace_count, Some(1));
        let rendered = narrative_plain_lines(&redraw, NarrativeVisualMode::None).join("\n");
        assert!(
            rendered.contains("freshness: stale via provider"),
            "{rendered}"
        );
        let persisted_state = read_json::<NarrativeState>(
            &state
                .run_root
                .join(NARRATIVE_DIR)
                .join(NARRATIVE_STATE_JSON),
        )
        .expect("persisted state");
        assert_eq!(persisted_state.latest_status, NarrativeStatus::Stale);
        assert_eq!(persisted_state.provider.source, "provider");
        assert_eq!(persisted_state.provider.calls, 1);
        let snapshot_count_after = fs::read_to_string(&snapshot_path)
            .expect("snapshots after")
            .lines()
            .count();
        assert_eq!(snapshot_count_after, snapshot_count_before);
    }

    #[test]
    fn provider_snapshot_with_graph_labels_survives_redraw() {
        let temp = TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "redraw keeps provider graph labels".to_string(),
                cwd: repo,
                sandbox: "none".to_string(),
                provider: Some("cli:test".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("run-redraw-graph-label-test".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        let input = RunNarrativeInput {
            state: &state,
            spend: &[],
            traces: &[],
            events: &[],
            live_files: vec![LiveFileFact {
                path: "src/main.rs".to_string(),
                bytes: 10,
                modified_at: None,
            }],
            file_count: 1,
            total_bytes: 10,
            acceptance_summary: "default gate".to_string(),
            provider_activity: &[],
            parent_plan: None,
        };
        let projection = ensure_run_projection(&input).expect("projection");
        let evidence = projection.snapshot.citations[0].id.clone();
        let node = projection.graph.nodes[0].id.clone();
        let provider_content = json!({
            "headline": "Narrated provider graph labels survive redraw.",
            "current_work": [
                {"text": "Provider prose is current for the same input coverage.", "evidence": [evidence], "confidence": "high"}
            ],
            "graph_labels": [
                {"target_id": node, "label": "provider relabel"}
            ]
        })
        .to_string();
        let refreshed = apply_provider_response(&projection, &provider_content, sample_provider())
            .expect("provider projection");
        persist_run_projection(&state, &refreshed).expect("persist provider");

        let redraw = ensure_run_projection(&input).expect("redraw projection");

        assert_eq!(redraw.state.latest_status, NarrativeStatus::Fresh);
        assert_eq!(redraw.snapshot.headline, refreshed.snapshot.headline);
        assert_eq!(
            redraw.state.latest_snapshot_id,
            refreshed.state.latest_snapshot_id
        );
    }

    #[test]
    fn deterministic_projection_survives_redraw_without_snapshot_churn() {
        let temp = TempDir::new().expect("temp");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "redraw keeps deterministic snapshot".to_string(),
                cwd: repo,
                sandbox: "none".to_string(),
                provider: Some("cli:test".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some("run-deterministic-redraw-test".to_string()),
                codebase: None,
            },
        )
        .expect("run");
        let input = RunNarrativeInput {
            state: &state,
            spend: &[],
            traces: &[],
            events: &[],
            live_files: vec![LiveFileFact {
                path: "src/main.rs".to_string(),
                bytes: 10,
                modified_at: None,
            }],
            file_count: 1,
            total_bytes: 10,
            acceptance_summary: "default gate".to_string(),
            provider_activity: &[],
            parent_plan: None,
        };

        let first = ensure_run_projection(&input).expect("first projection");
        let second = ensure_run_projection(&input).expect("second projection");
        let snapshots = fs::read_to_string(
            state
                .run_root
                .join(NARRATIVE_DIR)
                .join(NARRATIVE_SNAPSHOTS_JSONL),
        )
        .expect("snapshots");

        assert_eq!(second.state.latest_status, NarrativeStatus::Deterministic);
        assert_eq!(
            first.state.latest_snapshot_id,
            second.state.latest_snapshot_id
        );
        assert_eq!(
            snapshots
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count(),
            1
        );
    }

    #[test]
    fn summarizer_respects_min_interval_and_manual_refresh_budget() {
        let temp = TempDir::new().expect("temp");
        let mut projection = sample_projection(&temp);
        projection.state.latest_created_at = Some(Utc::now());
        projection.state.provider.route = Some("cli:test".to_string());

        let automatic = NarrativeRefreshPolicy {
            provider_route: Some("cli:test".to_string()),
            max_spend_usd: Some(10.0),
            manual: false,
            meaningful_delta: false,
            now: Utc::now(),
        };
        assert_eq!(
            provider_refresh_decision(&projection.state, &automatic),
            NarrativeRefreshDecision::Eligible
        );

        projection.state.provider.calls = 1;
        assert_eq!(
            provider_refresh_decision(&projection.state, &automatic),
            NarrativeRefreshDecision::TooSoon
        );

        let manual = NarrativeRefreshPolicy {
            manual: true,
            ..automatic.clone()
        };
        assert_eq!(
            provider_refresh_decision(&projection.state, &manual),
            NarrativeRefreshDecision::Eligible
        );

        let meaningful_delta = NarrativeRefreshPolicy {
            manual: false,
            meaningful_delta: true,
            ..automatic.clone()
        };
        assert_eq!(
            provider_refresh_decision(&projection.state, &meaningful_delta),
            NarrativeRefreshDecision::Eligible
        );

        projection.state.provider.cost_usd = 10.0;
        assert_eq!(
            provider_refresh_decision(&projection.state, &manual),
            NarrativeRefreshDecision::OverBudget
        );

        projection.state.provider.cost_usd = 0.0;
        projection.state.provider.calls = projection.state.cadence.max_provider_calls_per_attach;
        assert_eq!(
            provider_refresh_decision(&projection.state, &manual),
            NarrativeRefreshDecision::CallLimitReached
        );
    }

    #[test]
    fn summarizer_failure_keeps_attach_alive_with_stale_status() {
        let temp = TempDir::new().expect("temp");
        let projection = sample_projection(&temp);

        let stale = projection_with_provider_failure(
            &projection,
            Some("cli:test".to_string()),
            "provider refused structured JSON",
        );

        assert_eq!(stale.state.latest_status, NarrativeStatus::Stale);
        assert_eq!(stale.snapshot.status, NarrativeStatus::Stale);
        assert_ne!(stale.snapshot.snapshot_id, projection.snapshot.snapshot_id);
        assert_eq!(stale.state.latest_snapshot_id, stale.snapshot.snapshot_id);
        assert!(
            stale
                .state
                .last_error
                .as_deref()
                .unwrap()
                .contains("refused")
        );
        assert!(
            narrative_plain_lines(&stale, NarrativeVisualMode::None)
                .join("\n")
                .contains("deterministic facts remain visible")
        );
    }

    #[test]
    fn provider_prompt_redacts_before_leaving_the_process() {
        let temp = TempDir::new().expect("temp");
        let mut projection = sample_projection(&temp);
        projection.snapshot.current_work.push(NarrativeClaim {
            text: "Saw token=ghp_abcdefghijklmnopqrstuvwxyz in provider output.".to_string(),
            evidence: vec![projection.snapshot.citations[0].id.clone()],
            confidence: "high".to_string(),
        });

        let prompt = build_provider_prompt(&projection).expect("prompt");

        assert!(prompt.prompt.contains("<redacted-secret>"));
        assert!(!prompt.prompt.contains("ghp_abcdefghijklmnopqrstuvwxyz"));
        assert!(
            prompt
                .redaction
                .findings
                .iter()
                .any(|finding| finding.contains("secret-like"))
        );
        assert!(!prompt.evidence_ids.is_empty());
        assert!(!prompt.graph_ids.is_empty());
    }

    fn sample_provider() -> NarrativeProviderRefresh {
        NarrativeProviderRefresh {
            route: "cli:test".to_string(),
            model: "test-model".to_string(),
            cost_usd: 0.0,
            subscription_seconds: Some(1.0),
        }
    }

    fn sample_projection(temp: &TempDir) -> NarrativeProjection {
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo).expect("repo");
        let state = create_run(
            &paths,
            RunOptions {
                goal: "sample narrative attach".to_string(),
                cwd: repo,
                sandbox: "none".to_string(),
                provider: Some("cli:test".to_string()),
                skill_name: "default-coding".to_string(),
                max_spend_usd: None,
                max_wall_seconds: None,
                run_id: Some(format!("run-sample-{}", Uuid::new_v4().simple())),
                codebase: None,
            },
        )
        .expect("run");
        build_run_projection(&RunNarrativeInput {
            state: &state,
            spend: &[],
            traces: &[],
            events: &[],
            live_files: vec![LiveFileFact {
                path: "src/main.rs".to_string(),
                bytes: 10,
                modified_at: None,
            }],
            file_count: 1,
            total_bytes: 10,
            acceptance_summary: "default gate".to_string(),
            provider_activity: &[],
            parent_plan: None,
        })
    }
}
