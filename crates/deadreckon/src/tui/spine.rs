#![allow(dead_code)]

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use deadreckon_core::campaign::{Campaign, CampaignEvent, CampaignStatus, SubGoalStatus};
use deadreckon_core::glossary::run_status_label;
use deadreckon_core::{
    Chain, ChainEvent, ChainStatus, ChainStepStatus, DeadreckonPaths, PipelineState, Plan,
    PlanEvent, PlanStatus, PlanTaskStatus, RUN_EVENTS_JSONL, RunStatus,
};
use deadreckon_protocol::{RunEvent, RunEventKind};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::{commands, one_line, read_jsonl};

pub(crate) const SPINE_STALE_AFTER_SECONDS: u64 = 30;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum SpineSurface {
    Run,
    Plan,
    Chain,
    Campaign,
}

impl SpineSurface {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Plan => "plan",
            Self::Chain => "chain",
            Self::Campaign => "campaign",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) enum SpineQuestion {
    Alive,
    Doing,
    OnTrack,
    Wrong,
    Next,
}

impl SpineQuestion {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Alive => "alive",
            Self::Doing => "doing",
            Self::OnTrack => "on_track",
            Self::Wrong => "wrong",
            Self::Next => "next",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SpineContractCell {
    pub(crate) surface: SpineSurface,
    pub(crate) question: SpineQuestion,
    pub(crate) source: &'static str,
}

pub(crate) const SPINE_SURFACES: [SpineSurface; 4] = [
    SpineSurface::Run,
    SpineSurface::Plan,
    SpineSurface::Chain,
    SpineSurface::Campaign,
];

pub(crate) const SPINE_QUESTIONS: [SpineQuestion; 5] = [
    SpineQuestion::Alive,
    SpineQuestion::Doing,
    SpineQuestion::OnTrack,
    SpineQuestion::Wrong,
    SpineQuestion::Next,
];

pub(crate) const SPINE_CONTRACT_TABLE: &[SpineContractCell] = &[
    SpineContractCell {
        surface: SpineSurface::Run,
        question: SpineQuestion::Alive,
        source: "PipelineState.status plus newest events.jsonl timestamp, falling back to state.updated_at",
    },
    SpineContractCell {
        surface: SpineSurface::Run,
        question: SpineQuestion::Doing,
        source: "PipelineState.status, turn, and active phase from state.json",
    },
    SpineContractCell {
        surface: SpineSurface::Run,
        question: SpineQuestion::OnTrack,
        source: "state spend and turn counters, with spend ceiling from launch-plan.json before state.max_spend_usd",
    },
    SpineContractCell {
        surface: SpineSurface::Run,
        question: SpineQuestion::Wrong,
        source: "failure_reason, pause_reason, Error events, quiet-event age, reshape-proposal.json, and pending steer-inbox.jsonl entries",
    },
    SpineContractCell {
        surface: SpineSurface::Run,
        question: SpineQuestion::Next,
        source: "reshape-proposal.json when valid, otherwise PipelineState.status mapped to attach/resume/finish",
    },
    SpineContractCell {
        surface: SpineSurface::Plan,
        question: SpineQuestion::Alive,
        source: "Plan.status plus newest plan-events.jsonl timestamp, falling back to plan timestamps",
    },
    SpineContractCell {
        surface: SpineSurface::Plan,
        question: SpineQuestion::Doing,
        source: "Plan.status and child task status counts from plan.json",
    },
    SpineContractCell {
        surface: SpineSurface::Plan,
        question: SpineQuestion::OnTrack,
        source: "child task completion counts and plan-dir launch-plan.json spend ceiling",
    },
    SpineContractCell {
        surface: SpineSurface::Plan,
        question: SpineQuestion::Wrong,
        source: "failed or killed child tasks, Plan.status, and terminal plan events",
    },
    SpineContractCell {
        surface: SpineSurface::Plan,
        question: SpineQuestion::Next,
        source: "Plan.status and child completion state mapped to fork/attach/merge/finish/show",
    },
    SpineContractCell {
        surface: SpineSurface::Chain,
        question: SpineQuestion::Alive,
        source: "Chain.status plus newest chain-events.jsonl timestamp, falling back to chain timestamps",
    },
    SpineContractCell {
        surface: SpineSurface::Chain,
        question: SpineQuestion::Doing,
        source: "Chain.status and step status counts from chain.json",
    },
    SpineContractCell {
        surface: SpineSurface::Chain,
        question: SpineQuestion::OnTrack,
        source: "completed or applied step counts, chain spend counters, and chain max_spend_usd",
    },
    SpineContractCell {
        surface: SpineSurface::Chain,
        question: SpineQuestion::Wrong,
        source: "paused_reason, failure_reason, killed status, failed steps, and stale running age",
    },
    SpineContractCell {
        surface: SpineSurface::Chain,
        question: SpineQuestion::Next,
        source: "Chain.status mapped to chain resume/attach/show with pause-specific remediation",
    },
    SpineContractCell {
        surface: SpineSurface::Campaign,
        question: SpineQuestion::Alive,
        source: "Campaign.status plus newest campaign-events.jsonl timestamp, falling back to campaign timestamps",
    },
    SpineContractCell {
        surface: SpineSurface::Campaign,
        question: SpineQuestion::Doing,
        source: "Campaign.status and sub-goal status counts from campaign.json",
    },
    SpineContractCell {
        surface: SpineSurface::Campaign,
        question: SpineQuestion::OnTrack,
        source: "merged sub-goal counts and campaign tree_budget_usd/tree_wall_seconds fields",
    },
    SpineContractCell {
        surface: SpineSurface::Campaign,
        question: SpineQuestion::Wrong,
        source: "failed or killed sub-goals plus Campaign.status",
    },
    SpineContractCell {
        surface: SpineSurface::Campaign,
        question: SpineQuestion::Next,
        source: "Campaign.status and merged_run_id mapped to attach/apply/show",
    },
];

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SpineSnapshot {
    pub(crate) alive: Aliveness,
    pub(crate) doing: String,
    pub(crate) on_track: OnTrack,
    pub(crate) wrong: Vec<Attention>,
    pub(crate) next: PrimaryAction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Aliveness {
    Live { last_event_age_seconds: u64 },
    Stale { age_seconds: u64 },
    Dead { reason: String },
    Done,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct OnTrack {
    pub(crate) gate_passed: Option<u32>,
    pub(crate) gate_total: Option<u32>,
    pub(crate) spend_used_usd: f64,
    pub(crate) spend_ceiling_usd: Option<f64>,
    pub(crate) wall_seconds_used: f64,
    pub(crate) wall_seconds_ceiling: Option<f64>,
    pub(crate) turns_used: Option<u32>,
    pub(crate) turns_max: Option<u32>,
}

impl Default for OnTrack {
    fn default() -> Self {
        Self {
            gate_passed: None,
            gate_total: None,
            spend_used_usd: 0.0,
            spend_ceiling_usd: None,
            wall_seconds_used: 0.0,
            wall_seconds_ceiling: None,
            turns_used: None,
            turns_max: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Attention {
    pub(crate) kind: AttentionKind,
    pub(crate) summary: String,
    pub(crate) evidence_path: Option<PathBuf>,
}

impl Attention {
    fn new(
        kind: AttentionKind,
        summary: impl Into<String>,
        evidence_path: Option<PathBuf>,
    ) -> Self {
        Self {
            kind,
            summary: summary.into(),
            evidence_path,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AttentionKind {
    GateFailure,
    TamperCaveat,
    ProviderError,
    Stall,
    PausedAtCap,
    ReshapeProposed,
    SteerPending,
    Failure,
    Killed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PrimaryAction {
    pub(crate) command: String,
}

impl PrimaryAction {
    pub(crate) fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into().trim().to_string(),
        }
    }
}

pub(crate) fn spine_for_run(state: &PipelineState, now: DateTime<Utc>) -> SpineSnapshot {
    let events = read_jsonl::<RunEvent>(&state.run_root.join(RUN_EVENTS_JSONL)).unwrap_or_default();
    spine_for_run_with_events(state, &events, now)
}

pub(crate) fn spine_for_run_with_events(
    state: &PipelineState,
    events: &[RunEvent],
    now: DateTime<Utc>,
) -> SpineSnapshot {
    let alive = run_aliveness(state, events, now);
    let mut wrong = run_attention(state, events, &alive);
    let reshape_path = state.run_root.join("reshape-proposal.json");
    let has_reshape_proposal = valid_launch_plan(&reshape_path);

    if has_reshape_proposal {
        wrong.push(Attention::new(
            AttentionKind::ReshapeProposed,
            "reshape proposal waiting for explicit operator acceptance",
            Some(reshape_path),
        ));
    }

    if let Ok(pending) = deadreckon_core::steer_inbox::pending_steers(&state.run_root)
        && !pending.is_empty()
    {
        let count = pending.len();
        let noun = if count == 1 { "steer" } else { "steers" };
        wrong.push(Attention::new(
            AttentionKind::SteerPending,
            format!("{count} pending {noun} waiting for delivery"),
            Some(deadreckon_core::steer_inbox::steer_inbox_path(
                &state.run_root,
            )),
        ));
    }

    SpineSnapshot {
        alive,
        doing: run_doing(state),
        on_track: run_on_track(state),
        wrong,
        next: if has_reshape_proposal {
            PrimaryAction::new(format!("deadreckon reshape {}", state.run_id))
        } else {
            run_primary_action(state)
        },
    }
}

pub(crate) fn spine_for_plan(
    paths: &DeadreckonPaths,
    plan: &Plan,
    now: DateTime<Utc>,
) -> SpineSnapshot {
    let events = deadreckon_core::read_plan_events(paths, &plan.plan_id).unwrap_or_default();
    spine_for_plan_with_events(paths, plan, &events, now)
}

pub(crate) fn spine_for_plan_with_events(
    paths: &DeadreckonPaths,
    plan: &Plan,
    events: &[PlanEvent],
    now: DateTime<Utc>,
) -> SpineSnapshot {
    SpineSnapshot {
        alive: plan_aliveness(plan, events, now),
        doing: plan_doing(plan),
        on_track: plan_on_track(paths, plan),
        wrong: plan_attention(plan),
        next: plan_primary_action(plan),
    }
}

pub(crate) fn spine_for_chain(
    paths: &DeadreckonPaths,
    chain: &Chain,
    now: DateTime<Utc>,
) -> SpineSnapshot {
    let events = read_jsonl::<ChainEvent>(&paths.chain_events(&chain.chain_id)).unwrap_or_default();
    spine_for_chain_with_events(chain, &events, now)
}

pub(crate) fn spine_for_chain_with_events(
    chain: &Chain,
    events: &[ChainEvent],
    now: DateTime<Utc>,
) -> SpineSnapshot {
    SpineSnapshot {
        alive: chain_aliveness(chain, events, now),
        doing: chain_doing(chain),
        on_track: chain_on_track(chain),
        wrong: chain_attention(chain, events, now),
        next: chain_primary_action(chain),
    }
}

pub(crate) fn spine_for_campaign(
    paths: &DeadreckonPaths,
    campaign: &Campaign,
    now: DateTime<Utc>,
) -> SpineSnapshot {
    let plan_dir = paths.plan_dir(&campaign.campaign_id);
    let events =
        read_jsonl::<CampaignEvent>(&deadreckon_core::campaign::campaign_events_path(&plan_dir))
            .unwrap_or_default();
    spine_for_campaign_with_events(campaign, &events, now)
}

pub(crate) fn spine_for_campaign_with_events(
    campaign: &Campaign,
    events: &[CampaignEvent],
    now: DateTime<Utc>,
) -> SpineSnapshot {
    SpineSnapshot {
        alive: campaign_aliveness(campaign, events, now),
        doing: campaign_doing(campaign),
        on_track: campaign_on_track(campaign),
        wrong: campaign_attention(campaign),
        next: campaign_primary_action(campaign),
    }
}

pub(crate) fn spine_contract_markdown_rows() -> Vec<String> {
    SPINE_CONTRACT_TABLE
        .iter()
        .map(|cell| {
            format!(
                "| {} | {} | {} |",
                cell.surface.label(),
                cell.question.label(),
                cell.source
            )
        })
        .collect()
}

pub(crate) fn spine_plain_lines(snapshot: &SpineSnapshot) -> Vec<String> {
    vec![
        format!("alive: {}", aliveness_text(&snapshot.alive)),
        format!("doing: {}", snapshot.doing),
        format!("on track: {}", on_track_text(&snapshot.on_track)),
        format!("wrong: {}", attention_text(&snapshot.wrong)),
        format!("next: {}", snapshot.next.command),
    ]
}

pub(crate) fn render_spine_band(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    snapshot: &SpineSnapshot,
) {
    let lines = spine_band_lines(snapshot, area.width.saturating_sub(4) as usize);
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title("status spine").borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn spine_band_lines(snapshot: &SpineSnapshot, width: usize) -> Vec<Line<'static>> {
    let first = format!(
        "alive: {} | doing: {} | on track: {}",
        aliveness_text(&snapshot.alive),
        snapshot.doing,
        on_track_text(&snapshot.on_track)
    );
    let second = format!(
        "wrong: {} | next: {}",
        attention_text(&snapshot.wrong),
        snapshot.next.command
    );
    vec![
        Line::from(vec![Span::styled(
            one_line(&first, width),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]),
        Line::from(one_line(&second, width)),
    ]
}

fn aliveness_text(alive: &Aliveness) -> String {
    match alive {
        Aliveness::Live {
            last_event_age_seconds,
        } => format!("live {last_event_age_seconds}s"),
        Aliveness::Stale { age_seconds } => format!("stale {age_seconds}s"),
        Aliveness::Dead { reason } => format!("dead - {reason}"),
        Aliveness::Done => "done".to_string(),
    }
}

fn on_track_text(on_track: &OnTrack) -> String {
    let gate = match (on_track.gate_passed, on_track.gate_total) {
        (Some(passed), Some(total)) => format!("gate {passed}/{total}"),
        _ => "gate -".to_string(),
    };
    let spend = match on_track.spend_ceiling_usd {
        Some(ceiling) => format!("${:.2}/${:.2}", on_track.spend_used_usd, ceiling),
        None if on_track.spend_used_usd > 0.0 => {
            format!("${:.2}/unbounded", on_track.spend_used_usd)
        }
        None => "spend unbounded".to_string(),
    };
    let turns = on_track
        .turns_used
        .map(|turns| format!("turn {turns}"))
        .unwrap_or_else(|| "turn -".to_string());
    format!("{gate} - {spend} - {turns}")
}

fn attention_text(wrong: &[Attention]) -> String {
    match wrong {
        [] => "none".to_string(),
        [first] => format!("1 attention - {}", first.summary),
        [first, rest @ ..] => format!("{} attention - {}", rest.len() + 1, first.summary),
    }
}

fn run_aliveness(state: &PipelineState, events: &[RunEvent], now: DateTime<Utc>) -> Aliveness {
    match state.status {
        RunStatus::Completed => Aliveness::Done,
        RunStatus::Failed => Aliveness::Dead {
            reason: state
                .failure_reason
                .clone()
                .unwrap_or_else(|| "run failed".to_string()),
        },
        RunStatus::Killed => Aliveness::Dead {
            reason: state
                .failure_reason
                .clone()
                .unwrap_or_else(|| "run killed".to_string()),
        },
        RunStatus::Pending | RunStatus::Planned | RunStatus::Executing => {
            aliveness_from_timestamp(newest_run_event_ts(events).unwrap_or(state.updated_at), now)
        }
    }
}

fn run_doing(state: &PipelineState) -> String {
    let phase = state
        .active_phase()
        .map(|phase| phase.name.as_str())
        .unwrap_or("no active phase");
    format!(
        "{} - turn {} - {}",
        run_status_label(state.status),
        state.turn,
        phase
    )
}

fn run_on_track(state: &PipelineState) -> OnTrack {
    OnTrack {
        spend_used_usd: state.total_spend_usd,
        spend_ceiling_usd: launch_plan_spend_ceiling(&state.run_root).or(state.max_spend_usd),
        wall_seconds_used: state.total_wall_seconds,
        wall_seconds_ceiling: launch_plan_wall_ceiling(&state.run_root).or(state.max_wall_seconds),
        turns_used: Some(state.turn),
        ..OnTrack::default()
    }
}

fn run_attention(state: &PipelineState, events: &[RunEvent], alive: &Aliveness) -> Vec<Attention> {
    let mut wrong = Vec::new();
    let events_path = state.run_root.join(RUN_EVENTS_JSONL);

    if let Some(reason) = state.pause_reason.as_deref() {
        wrong.push(Attention::new(
            AttentionKind::PausedAtCap,
            reason.to_string(),
            Some(state.state_path()),
        ));
    }

    if matches!(state.status, RunStatus::Failed) {
        wrong.push(Attention::new(
            AttentionKind::Failure,
            state
                .failure_reason
                .clone()
                .unwrap_or_else(|| "run failed".to_string()),
            Some(state.state_path()),
        ));
    }

    if matches!(state.status, RunStatus::Killed) {
        wrong.push(Attention::new(
            AttentionKind::Killed,
            state
                .failure_reason
                .clone()
                .unwrap_or_else(|| "run killed".to_string()),
            Some(state.state_path()),
        ));
    }

    if let Some(message) = newest_run_error(events) {
        wrong.push(Attention::new(
            AttentionKind::ProviderError,
            message,
            Some(events_path.clone()),
        ));
    }

    if matches!(state.status, RunStatus::Executing)
        && let Aliveness::Stale { age_seconds } = alive
    {
        wrong.push(Attention::new(
            AttentionKind::Stall,
            format!("no run event for {age_seconds}s"),
            Some(events_path),
        ));
    }

    wrong
}

fn run_primary_action(state: &PipelineState) -> PrimaryAction {
    let id = &state.run_id;
    match state.status {
        RunStatus::Pending | RunStatus::Planned | RunStatus::Executing => {
            PrimaryAction::new(format!("deadreckon attach {id}"))
        }
        RunStatus::Failed | RunStatus::Killed => {
            PrimaryAction::new(format!("deadreckon resume {id}"))
        }
        RunStatus::Completed => PrimaryAction::new(format!("deadreckon finish {id}")),
    }
}

fn plan_aliveness(plan: &Plan, events: &[PlanEvent], now: DateTime<Utc>) -> Aliveness {
    match plan.status {
        PlanStatus::Merged => Aliveness::Done,
        PlanStatus::Failed => Aliveness::Dead {
            reason: newest_plan_failure(events).unwrap_or_else(|| "plan failed".to_string()),
        },
        PlanStatus::Pending | PlanStatus::Forked => {
            let timestamp = events
                .iter()
                .map(|event| event.timestamp)
                .max()
                .or(plan.forked_at)
                .unwrap_or(plan.created_at);
            aliveness_from_timestamp(timestamp, now)
        }
    }
}

fn plan_doing(plan: &Plan) -> String {
    let completed = plan_completed_tasks(plan);
    let total = plan.tasks.len();
    match plan.status {
        PlanStatus::Pending => format!("plan pending - {total} children ready to fork"),
        PlanStatus::Forked if completed == total => {
            format!("plan running - {completed}/{total} children complete; ready to merge")
        }
        PlanStatus::Forked => format!("plan running - {completed}/{total} children complete"),
        PlanStatus::Merged => plan
            .merged_run_id
            .as_deref()
            .map(|run_id| format!("plan merged - result run {run_id}"))
            .unwrap_or_else(|| "plan merged".to_string()),
        PlanStatus::Failed => format!("plan failed - {completed}/{total} children complete"),
    }
}

fn plan_on_track(paths: &DeadreckonPaths, plan: &Plan) -> OnTrack {
    OnTrack {
        gate_passed: Some(plan_completed_tasks(plan) as u32),
        gate_total: Some(plan.tasks.len() as u32),
        spend_ceiling_usd: launch_plan_spend_ceiling(&paths.plan_dir(&plan.plan_id)),
        wall_seconds_ceiling: launch_plan_wall_ceiling(&paths.plan_dir(&plan.plan_id)),
        ..OnTrack::default()
    }
}

fn plan_attention(plan: &Plan) -> Vec<Attention> {
    let mut wrong = Vec::new();
    for task in &plan.tasks {
        match task.status {
            PlanTaskStatus::Failed => wrong.push(Attention::new(
                AttentionKind::Failure,
                format!("child {} failed", task.task_id),
                task.summary_path.clone(),
            )),
            PlanTaskStatus::Killed => wrong.push(Attention::new(
                AttentionKind::Killed,
                format!("child {} was killed", task.task_id),
                task.summary_path.clone(),
            )),
            PlanTaskStatus::Pending
            | PlanTaskStatus::Running
            | PlanTaskStatus::Completed
            | PlanTaskStatus::Skipped => {}
        }
    }
    if plan.status == PlanStatus::Failed && wrong.is_empty() {
        wrong.push(Attention::new(AttentionKind::Failure, "plan failed", None));
    }
    wrong
}

fn plan_primary_action(plan: &Plan) -> PrimaryAction {
    let id = &plan.plan_id;
    match plan.status {
        PlanStatus::Pending => PrimaryAction::new(format!("deadreckon fork {id}")),
        PlanStatus::Forked if plan_completed_tasks(plan) == plan.tasks.len() => {
            PrimaryAction::new(format!("deadreckon merge {id}"))
        }
        PlanStatus::Forked => PrimaryAction::new(format!("deadreckon attach {id}")),
        PlanStatus::Merged => PrimaryAction::new(format!("deadreckon finish {id}")),
        PlanStatus::Failed => PrimaryAction::new(format!("deadreckon show {id} --why-failed")),
    }
}

fn chain_aliveness(chain: &Chain, events: &[ChainEvent], now: DateTime<Utc>) -> Aliveness {
    match chain.status {
        ChainStatus::Completed | ChainStatus::Undone => Aliveness::Done,
        ChainStatus::Failed => Aliveness::Dead {
            reason: chain
                .failure_reason
                .clone()
                .unwrap_or_else(|| "chain failed".to_string()),
        },
        ChainStatus::Killed => Aliveness::Dead {
            reason: chain
                .failure_reason
                .clone()
                .unwrap_or_else(|| "chain killed".to_string()),
        },
        ChainStatus::Pending | ChainStatus::Running | ChainStatus::Paused => {
            let timestamp = events
                .iter()
                .map(|event| event.timestamp)
                .max()
                .or(chain.started_at)
                .unwrap_or(chain.created_at);
            aliveness_from_timestamp(timestamp, now)
        }
    }
}

fn chain_doing(chain: &Chain) -> String {
    let completed = chain_completed_steps(chain);
    let total = chain.steps.len();
    match chain.status {
        ChainStatus::Pending => format!("chain pending - {total} steps"),
        ChainStatus::Running => format!("chain running - {completed}/{total} steps complete"),
        ChainStatus::Paused => chain
            .paused_reason
            .as_deref()
            .map(|reason| format!("chain paused - {reason}"))
            .unwrap_or_else(|| "chain paused".to_string()),
        ChainStatus::Completed => format!("chain completed - {completed}/{total} steps complete"),
        ChainStatus::Failed => format!("chain failed - {completed}/{total} steps complete"),
        ChainStatus::Killed => format!("chain killed - {completed}/{total} steps complete"),
        ChainStatus::Undone => format!("chain undone - {completed}/{total} steps complete"),
    }
}

fn chain_on_track(chain: &Chain) -> OnTrack {
    OnTrack {
        gate_passed: Some(chain_completed_steps(chain) as u32),
        gate_total: Some(chain.steps.len() as u32),
        spend_used_usd: chain.total_spend_usd,
        spend_ceiling_usd: chain.max_spend_usd,
        wall_seconds_used: chain.total_wall_seconds,
        wall_seconds_ceiling: chain.max_wall_seconds,
        ..OnTrack::default()
    }
}

fn chain_attention(chain: &Chain, events: &[ChainEvent], now: DateTime<Utc>) -> Vec<Attention> {
    let mut wrong = Vec::new();
    if let Some(reason) = chain.paused_reason.as_deref() {
        wrong.push(Attention::new(AttentionKind::PausedAtCap, reason, None));
    }
    if let Some(reason) = chain.failure_reason.as_deref() {
        wrong.push(Attention::new(AttentionKind::Failure, reason, None));
    }
    for step in &chain.steps {
        if step.status == ChainStepStatus::Failed {
            wrong.push(Attention::new(
                AttentionKind::Failure,
                step.fail_reason
                    .clone()
                    .unwrap_or_else(|| format!("step {} failed", step.index)),
                None,
            ));
        }
    }
    if matches!(chain.status, ChainStatus::Running) {
        let timestamp = events
            .iter()
            .map(|event| event.timestamp)
            .max()
            .or(chain.started_at)
            .unwrap_or(chain.created_at);
        let age = age_seconds(now, timestamp);
        if age > SPINE_STALE_AFTER_SECONDS {
            wrong.push(Attention::new(
                AttentionKind::Stall,
                format!("no chain event for {age}s"),
                None,
            ));
        }
    }
    wrong
}

fn chain_primary_action(chain: &Chain) -> PrimaryAction {
    let id = &chain.chain_id;
    match chain.status {
        ChainStatus::Failed | ChainStatus::Killed => {
            PrimaryAction::new(format!("deadreckon chain show {id} --why-failed"))
        }
        ChainStatus::Paused if chain_pause_reason_starts(chain, "apply_refused_dirty_target") => {
            PrimaryAction::new(format!("git -C {} status --short", chain.cwd.display()))
        }
        ChainStatus::Paused
            if chain_pause_reason_starts(chain, "apply_refused_by_hook_on_promote") =>
        {
            PrimaryAction::new("deadreckon chain hooks list")
        }
        ChainStatus::Paused
            if chain_pause_reason_starts(chain, "apply_refused_outside_allowlist") =>
        {
            PrimaryAction::new(format!("deadreckon chain resume {id} --apply-mode preview"))
        }
        ChainStatus::Paused | ChainStatus::Pending => {
            PrimaryAction::new(format!("deadreckon chain resume {id}"))
        }
        ChainStatus::Running => PrimaryAction::new(format!("deadreckon chain attach {id}")),
        ChainStatus::Completed | ChainStatus::Undone => {
            PrimaryAction::new(format!("deadreckon chain show {id}"))
        }
    }
}

fn chain_pause_reason_starts(chain: &Chain, prefix: &str) -> bool {
    chain
        .paused_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with(prefix))
}

fn campaign_aliveness(
    campaign: &Campaign,
    events: &[CampaignEvent],
    now: DateTime<Utc>,
) -> Aliveness {
    match campaign.status {
        CampaignStatus::Merged => Aliveness::Done,
        CampaignStatus::Failed => Aliveness::Dead {
            reason: "campaign failed".to_string(),
        },
        CampaignStatus::Killed => Aliveness::Dead {
            reason: "campaign killed".to_string(),
        },
        CampaignStatus::Pending | CampaignStatus::Forked => {
            let timestamp = events
                .iter()
                .map(|event| event.ts)
                .max()
                .or(campaign.forked_at)
                .unwrap_or(campaign.created_at);
            aliveness_from_timestamp(timestamp, now)
        }
    }
}

fn campaign_doing(campaign: &Campaign) -> String {
    let merged = campaign_merged_sub_goals(campaign);
    let total = campaign.sub_goals.len();
    let status = campaign_status_label(campaign.status);
    format!("campaign {status} - {merged}/{total} sub-goals merged")
}

fn campaign_on_track(campaign: &Campaign) -> OnTrack {
    OnTrack {
        gate_passed: Some(campaign_merged_sub_goals(campaign) as u32),
        gate_total: Some(campaign.sub_goals.len() as u32),
        spend_ceiling_usd: campaign.tree_budget_usd,
        wall_seconds_ceiling: campaign.tree_wall_seconds,
        ..OnTrack::default()
    }
}

fn campaign_attention(campaign: &Campaign) -> Vec<Attention> {
    let mut wrong = Vec::new();
    for sub in &campaign.sub_goals {
        match sub.status {
            SubGoalStatus::Failed => wrong.push(Attention::new(
                AttentionKind::Failure,
                format!("sub-goal {} failed", sub.sub_id),
                None,
            )),
            SubGoalStatus::Killed => wrong.push(Attention::new(
                AttentionKind::Killed,
                format!("sub-goal {} was killed", sub.sub_id),
                None,
            )),
            SubGoalStatus::Pending | SubGoalStatus::Running | SubGoalStatus::Merged => {}
        }
    }
    if campaign.status == CampaignStatus::Failed && wrong.is_empty() {
        wrong.push(Attention::new(
            AttentionKind::Failure,
            "campaign failed",
            None,
        ));
    }
    if campaign.status == CampaignStatus::Killed && wrong.is_empty() {
        wrong.push(Attention::new(
            AttentionKind::Killed,
            "campaign killed",
            None,
        ));
    }
    wrong
}

fn campaign_primary_action(campaign: &Campaign) -> PrimaryAction {
    let id = &campaign.campaign_id;
    if let Some(result_run_id) = campaign.merged_run_id.as_deref() {
        return PrimaryAction::new(format!("deadreckon apply {result_run_id}"));
    }
    match campaign.status {
        CampaignStatus::Pending | CampaignStatus::Forked => {
            PrimaryAction::new(format!("deadreckon attach {id}"))
        }
        CampaignStatus::Merged => PrimaryAction::new(format!("deadreckon show {id}")),
        CampaignStatus::Failed | CampaignStatus::Killed => {
            PrimaryAction::new(format!("deadreckon show {id} --why-failed"))
        }
    }
}

fn aliveness_from_timestamp(timestamp: DateTime<Utc>, now: DateTime<Utc>) -> Aliveness {
    let age = age_seconds(now, timestamp);
    if age <= SPINE_STALE_AFTER_SECONDS {
        Aliveness::Live {
            last_event_age_seconds: age,
        }
    } else {
        Aliveness::Stale { age_seconds: age }
    }
}

fn age_seconds(now: DateTime<Utc>, timestamp: DateTime<Utc>) -> u64 {
    now.signed_duration_since(timestamp).num_seconds().max(0) as u64
}

fn newest_run_event_ts(events: &[RunEvent]) -> Option<DateTime<Utc>> {
    events.iter().map(|event| event.timestamp).max()
}

fn newest_run_error(events: &[RunEvent]) -> Option<String> {
    events.iter().rev().find_map(|event| match &event.event {
        RunEventKind::Error { message, .. } => Some(message.clone()),
        RunEventKind::TurnStarted { .. }
        | RunEventKind::ToolCallStarted { .. }
        | RunEventKind::ToolCallResult { .. }
        | RunEventKind::TokenUsageDelta { .. }
        | RunEventKind::SpendDelta { .. }
        | RunEventKind::DocsCheckpoint { .. }
        | RunEventKind::RunCompleted { .. }
        | RunEventKind::RunPromoted { .. } => None,
    })
}

fn newest_plan_failure(events: &[PlanEvent]) -> Option<String> {
    events.iter().rev().find_map(|event| match &event.event {
        deadreckon_core::PlanEventKind::PlanFailed { reason }
        | deadreckon_core::PlanEventKind::TaskFailed { reason, .. }
        | deadreckon_core::PlanEventKind::TaskBlocked { reason, .. }
        | deadreckon_core::PlanEventKind::TaskBudgetExhausted { reason, .. }
        | deadreckon_core::PlanEventKind::RootBudgetExhausted { reason, .. }
        | deadreckon_core::PlanEventKind::MergeRepairFailed { reason } => Some(reason.clone()),
        deadreckon_core::PlanEventKind::CircuitBreakerTripped {
            consecutive_failures,
            threshold,
        } => Some(format!(
            "circuit breaker: {consecutive_failures} nodes failed in a row (threshold {threshold})"
        )),
        // A retry is the plan healing itself, not a failure state to surface.
        deadreckon_core::PlanEventKind::TaskRetrying { .. }
        | deadreckon_core::PlanEventKind::TaskApplied { .. }
        | deadreckon_core::PlanEventKind::TaskSkipped { .. }
        | deadreckon_core::PlanEventKind::PlanCreated { .. }
        | deadreckon_core::PlanEventKind::PlanStarted
        | deadreckon_core::PlanEventKind::TaskReady { .. }
        | deadreckon_core::PlanEventKind::TaskStarted { .. }
        | deadreckon_core::PlanEventKind::TaskRunDiscovered { .. }
        | deadreckon_core::PlanEventKind::TaskCompleted { .. }
        | deadreckon_core::PlanEventKind::TaskKilled { .. }
        | deadreckon_core::PlanEventKind::MergeStarted
        | deadreckon_core::PlanEventKind::MergeConflict { .. }
        | deadreckon_core::PlanEventKind::MergeRepairPlanned { .. }
        | deadreckon_core::PlanEventKind::MergeRepairStarted { .. }
        | deadreckon_core::PlanEventKind::MergeRepairRunDiscovered { .. }
        | deadreckon_core::PlanEventKind::MergeRepaired { .. }
        | deadreckon_core::PlanEventKind::MergeCompleted { .. }
        | deadreckon_core::PlanEventKind::PlanCompleted
        | deadreckon_core::PlanEventKind::PlanKilled => None,
    })
}

fn plan_completed_tasks(plan: &Plan) -> usize {
    plan.tasks
        .iter()
        .filter(|task| task.status == PlanTaskStatus::Completed)
        .count()
}

fn chain_completed_steps(chain: &Chain) -> usize {
    chain
        .steps
        .iter()
        .filter(|step| {
            matches!(
                step.status,
                ChainStepStatus::Completed | ChainStepStatus::Applied | ChainStepStatus::Undone
            )
        })
        .count()
}

fn campaign_merged_sub_goals(campaign: &Campaign) -> usize {
    campaign
        .sub_goals
        .iter()
        .filter(|sub| sub.status == SubGoalStatus::Merged)
        .count()
}

fn launch_plan_spend_ceiling(root: &Path) -> Option<f64> {
    commands::course::load_launch_plan(&commands::course::launch_plan_path(root))
        .ok()
        .and_then(|plan| plan.budget.ceiling_usd)
}

fn launch_plan_wall_ceiling(root: &Path) -> Option<f64> {
    commands::course::load_launch_plan(&commands::course::launch_plan_path(root))
        .ok()
        .and_then(|plan| plan.budget.wall_seconds)
        .map(|seconds| seconds as f64)
}

fn valid_launch_plan(path: &Path) -> bool {
    path.is_file() && commands::course::load_launch_plan(path).is_ok()
}

fn campaign_status_label(status: CampaignStatus) -> &'static str {
    match status {
        CampaignStatus::Pending => "pending",
        CampaignStatus::Forked => "running",
        CampaignStatus::Merged => "completed",
        CampaignStatus::Failed => "failed",
        CampaignStatus::Killed => "killed",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use chrono::Duration as ChronoDuration;
    use deadreckon_core::state::{RunOptions, append_json_line, create_run};
    use deadreckon_protocol::{RunEvent, RunEventKind};
    use tempfile::TempDir;

    use super::*;
    use crate::commands::course::{CourseShape, save_launch_plan, trivial_operator_plan};

    fn run_fixture() -> (TempDir, PipelineState) {
        let temp = TempDir::new().expect("tempdir");
        let paths = DeadreckonPaths::from_home(temp.path().join("home"));
        let state = create_run(
            &paths,
            RunOptions {
                goal: "test the helm spine".to_string(),
                cwd: std::env::current_dir().expect("cwd"),
                sandbox: "danger-full-access".to_string(),
                provider: None,
                skill_name: "deadreckon".to_string(),
                max_spend_usd: Some(3.0),
                max_wall_seconds: Some(600.0),
                run_id: Some("spinerun000000000000000000000000".to_string()),
                codebase: None,
            },
        )
        .expect("create run");
        (temp, state)
    }

    fn append_run_event(state: &PipelineState, timestamp: DateTime<Utc>) {
        append_json_line(
            &state.run_root.join(RUN_EVENTS_JSONL),
            &RunEvent {
                timestamp,
                run_id: state.run_id.clone(),
                event: RunEventKind::TurnStarted { turn: state.turn },
            },
        )
        .expect("append run event");
    }

    #[test]
    fn spine_contract_table_has_no_placeholder_cells() {
        let mut cells = BTreeSet::new();
        for cell in SPINE_CONTRACT_TABLE {
            assert!(
                !cell.source.trim().is_empty(),
                "empty source for {:?}/{:?}",
                cell.surface,
                cell.question
            );
            let source = cell.source.to_ascii_lowercase();
            for placeholder in ["todo", "tbd", "placeholder", "unknown"] {
                assert!(
                    !source.contains(placeholder),
                    "placeholder source for {:?}/{:?}: {}",
                    cell.surface,
                    cell.question,
                    cell.source
                );
            }
            assert!(
                cells.insert((cell.surface, cell.question)),
                "duplicate source for {:?}/{:?}",
                cell.surface,
                cell.question
            );
        }

        for surface in SPINE_SURFACES {
            for question in SPINE_QUESTIONS {
                assert!(
                    cells.contains(&(surface, question)),
                    "missing source for {surface:?}/{question:?}"
                );
            }
        }
        assert_eq!(cells.len(), SPINE_SURFACES.len() * SPINE_QUESTIONS.len());
    }

    #[test]
    fn spine_contract_doc_rows_are_generated_from_contract_table() {
        let rows = spine_contract_markdown_rows();
        assert_eq!(rows.len(), SPINE_CONTRACT_TABLE.len());
        for (row, cell) in rows.iter().zip(SPINE_CONTRACT_TABLE) {
            assert!(row.contains(cell.surface.label()), "{row}");
            assert!(row.contains(cell.question.label()), "{row}");
            assert!(row.contains(cell.source), "{row}");
        }
    }

    #[test]
    fn run_spine_reports_aliveness_from_event_age() {
        let (_temp, mut state) = run_fixture();
        let now = Utc::now();
        state.status = RunStatus::Executing;
        state.turn = 6;
        append_run_event(&state, now - ChronoDuration::seconds(7));

        let spine = spine_for_run(&state, now);

        assert_eq!(
            spine.alive,
            Aliveness::Live {
                last_event_age_seconds: 7
            }
        );
    }

    #[test]
    fn spine_next_action_is_exactly_one_command() {
        let (_temp, mut state) = run_fixture();
        state.status = RunStatus::Failed;
        state.failure_reason = Some("provider exited".to_string());

        let command = spine_for_run(&state, Utc::now()).next.command;

        assert_eq!(command, format!("deadreckon resume {}", state.run_id));
        assert!(!command.contains('\n'), "{command}");
        assert!(!command.contains(" && "), "{command}");
        assert!(!command.contains(" ; "), "{command}");
        assert!(!command.contains(" | "), "{command}");
    }

    #[test]
    fn pending_reshape_proposal_surfaces_as_attention_and_reshape_next() {
        let (_temp, mut state) = run_fixture();
        let now = Utc::now();
        state.status = RunStatus::Planned;
        append_run_event(&state, now - ChronoDuration::seconds(3));
        let proposal = trivial_operator_plan("split this run", CourseShape::Plan, "reshape");
        save_launch_plan(&state.run_root.join("reshape-proposal.json"), &proposal)
            .expect("save reshape proposal");

        let spine = spine_for_run(&state, now);

        assert_eq!(
            spine.next.command,
            format!("deadreckon reshape {}", state.run_id)
        );
        assert!(spine.wrong.iter().any(|attention| {
            attention.kind == AttentionKind::ReshapeProposed
                && attention
                    .evidence_path
                    .as_deref()
                    .is_some_and(|path| path.ends_with("reshape-proposal.json"))
        }));
    }

    #[test]
    fn reshape_proposal_does_not_mark_run_paused_or_dead() {
        let (_temp, mut state) = run_fixture();
        let now = Utc::now();
        state.status = RunStatus::Planned;
        append_run_event(&state, now - ChronoDuration::seconds(3));
        let proposal = trivial_operator_plan("split this run", CourseShape::Plan, "reshape");
        save_launch_plan(&state.run_root.join("reshape-proposal.json"), &proposal)
            .expect("save reshape proposal");

        let spine = spine_for_run(&state, now);

        assert_eq!(
            spine.alive,
            Aliveness::Live {
                last_event_age_seconds: 3
            }
        );
        assert!(!matches!(spine.alive, Aliveness::Dead { .. }));
        assert!(
            !spine
                .wrong
                .iter()
                .any(|attention| attention.kind == AttentionKind::PausedAtCap)
        );
    }

    #[test]
    fn run_spine_reads_spend_ceiling_from_launch_plan_before_state() {
        let (_temp, mut state) = run_fixture();
        state.total_spend_usd = 1.25;
        let mut plan = trivial_operator_plan(&state.goal, CourseShape::Single, "run");
        plan.budget.ceiling_usd = Some(12.5);
        save_launch_plan(&commands::course::launch_plan_path(&state.run_root), &plan)
            .expect("save launch plan");

        let spine = spine_for_run(&state, Utc::now());

        assert_eq!(spine.on_track.spend_used_usd, 1.25);
        assert_eq!(spine.on_track.spend_ceiling_usd, Some(12.5));
    }
}
