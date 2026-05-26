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
    load_run, plan_status_label, plan_task_status_label, run_status_label,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
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

pub(crate) fn ensure_run_projection(
    input: &RunNarrativeInput<'_>,
) -> crate::Result<NarrativeProjection> {
    let narrative_dir = input.state.run_root.join(NARRATIVE_DIR);
    let projection = build_run_projection(input);
    persist_projection(&projection, &narrative_dir)?;
    Ok(projection)
}

pub(crate) fn ensure_plan_projection(
    input: &PlanNarrativeInput<'_>,
) -> crate::Result<NarrativeProjection> {
    let plan_dir = input.paths.plan_dir(&input.plan.plan_id);
    let projection = build_plan_projection(input);
    persist_projection(&projection, &plan_dir.join(NARRATIVE_DIR))?;
    Ok(projection)
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
    let mut current_work = vec![claim(
        format!(
            "[{}] turn {} in {} ({})",
            status, state.turn, phase, input.acceptance_summary
        ),
        vec![state_evidence.clone()],
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
            vec![state_evidence.clone()],
            "high",
        ));
    }
    risks.push(claim(
        "Provider-backed narration has not run in this alpha slice; deterministic fallback facts are shown."
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
    let graph_hash = stable_hash(&graph);
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
        .map(|task| {
            let evidence = vec![format!("task:{}", task.task_id)];
            NarrativeAgentRow {
                task_id: task.task_id.clone(),
                role: format!("{:?}", task.role).to_ascii_lowercase(),
                provider: task.provider.clone(),
                status: plan_task_status_label(task.status).to_string(),
                summary: task_summary(input.paths, task),
                evidence,
            }
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
        "Provider-backed narration has not run in this alpha slice; deterministic fallback facts are shown."
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
    let source_window = NarrativeSourceWindow {
        plan_events: seq_window(input.plan_events.len() as u64),
        files: plan
            .tasks
            .iter()
            .filter_map(|task| task.summary_path.as_ref())
            .map(|path| path.display().to_string())
            .collect(),
        ..NarrativeSourceWindow::default()
    };
    let graph = build_plan_graph(plan, &source_window);
    debug_assert!(validate_graph(&graph));
    let graph_hash = stable_hash(&graph);
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

pub(crate) fn narrative_plain_lines(
    projection: &NarrativeProjection,
    visual: NarrativeVisualMode,
) -> Vec<String> {
    let snapshot = &projection.snapshot;
    let mut lines = vec![
        format!(
            "narrative {}  status {:?}  visual {}",
            snapshot.snapshot_id,
            projection.state.latest_status,
            visual.label()
        ),
        format!(
            "freshness: deterministic fallback  covered: {}",
            coverage_label(&projection.state.latest_covered)
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

fn persist_projection(projection: &NarrativeProjection, narrative_dir: &Path) -> crate::Result<()> {
    let latest = read_json::<NarrativeState>(&narrative_dir.join(NARRATIVE_STATE_JSON));
    if latest
        .as_ref()
        .is_some_and(|state| state.latest_snapshot_id == projection.snapshot.snapshot_id)
    {
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

fn build_plan_graph(plan: &Plan, source_window: &NarrativeSourceWindow) -> ArchitectureGraph {
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
        groups: vec![ArchitectureGroup {
            id: "group:tasks".to_string(),
            label: "Plan tasks".to_string(),
            node_ids: plan
                .tasks
                .iter()
                .map(|task| format!("task:{}", task.task_id))
                .collect(),
            evidence: vec![plan_evidence],
        }],
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
        hint: "try: rerun attach with a valid run or plan id".to_string(),
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
        hint: "try: rerun attach with a valid run or plan id".to_string(),
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

fn one_line(value: &str, max_chars: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
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
    use deadreckon_core::{RunOptions, create_run};
    use tempfile::TempDir;

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
}
