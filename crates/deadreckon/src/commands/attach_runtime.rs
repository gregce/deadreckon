use super::super::*;
use super::campaign::CampaignAttachState;
use crossterm::event::EventStream;
use futures_util::StreamExt;
#[cfg(test)]
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};

static TUI_SUSPEND_DEPTH: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AttachTickBudget {
    pub(crate) target_frame_ms: u64,
    pub(crate) max_sync_io_ms: u64,
    pub(crate) slow_warning_ms: u64,
    pub(crate) max_input_to_frame_ms: u64,
    pub(crate) idle_initial_ms: u64,
    pub(crate) idle_max_ms: u64,
}

impl Default for AttachTickBudget {
    fn default() -> Self {
        Self {
            target_frame_ms: 500,
            max_sync_io_ms: 80,
            slow_warning_ms: 180,
            max_input_to_frame_ms: 32,
            idle_initial_ms: 16,
            idle_max_ms: 250,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachSurface {
    Run,
    Plan,
    Chain,
    Campaign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachLoopStage {
    LoadState,
    ReadJsonl,
    EventFeed,
    LiveCollect,
    PlanMessages,
    ProviderNarrativeRefresh,
    Draw,
    InputPoll,
    InputToFrame,
}

impl AttachLoopStage {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::LoadState => "load state",
            Self::ReadJsonl => "read jsonl",
            Self::EventFeed => "event feed",
            Self::LiveCollect => "live collect",
            Self::PlanMessages => "plan messages",
            Self::ProviderNarrativeRefresh => "narrative provider refresh",
            Self::Draw => "draw",
            Self::InputPoll => "input poll",
            Self::InputToFrame => "input to frame",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachWorkMode {
    UiSync,
    Background,
}

pub(crate) fn attach_loop_stage_work(
    surface: AttachSurface,
    stage: AttachLoopStage,
) -> AttachWorkMode {
    match (surface, stage) {
        (AttachSurface::Run | AttachSurface::Plan, AttachLoopStage::ProviderNarrativeRefresh) => {
            AttachWorkMode::Background
        }
        _ => AttachWorkMode::UiSync,
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AttachStageTiming {
    pub(crate) stage: AttachLoopStage,
    pub(crate) elapsed: Duration,
    pub(crate) work: AttachWorkMode,
}

#[derive(Debug)]
pub(crate) struct AttachTickTiming {
    surface: AttachSurface,
    budget: AttachTickBudget,
    started_at: Instant,
    stages: Vec<AttachStageTiming>,
}

impl AttachTickTiming {
    pub(crate) fn new(surface: AttachSurface, budget: AttachTickBudget) -> Self {
        Self {
            surface,
            budget,
            started_at: Instant::now(),
            stages: Vec::new(),
        }
    }

    pub(crate) fn record(&mut self, stage: AttachLoopStage, elapsed: Duration) {
        self.stages.push(AttachStageTiming {
            stage,
            elapsed,
            work: attach_loop_stage_work(self.surface, stage),
        });
    }

    pub(crate) fn record_since(&mut self, stage: AttachLoopStage, started_at: Instant) {
        self.record(stage, started_at.elapsed());
    }

    pub(crate) fn total_elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub(crate) fn frame_exceeded(&self) -> bool {
        self.total_elapsed() > Duration::from_millis(self.budget.target_frame_ms)
    }

    pub(crate) fn input_to_frame_exceeded(&self) -> bool {
        let max_input = Duration::from_millis(self.budget.max_input_to_frame_ms);
        self.stages
            .iter()
            .any(|stage| stage.stage == AttachLoopStage::InputToFrame && stage.elapsed > max_input)
    }

    pub(crate) fn slow_sync_stages(&self) -> Vec<&AttachStageTiming> {
        let max_sync = Duration::from_millis(self.budget.max_sync_io_ms);
        self.stages
            .iter()
            .filter(|stage| stage.work == AttachWorkMode::UiSync && stage.elapsed > max_sync)
            .collect()
    }

    pub(crate) fn slow_warning_stages(&self) -> Vec<&AttachStageTiming> {
        let slow = Duration::from_millis(self.budget.slow_warning_ms);
        self.stages
            .iter()
            .filter(|stage| stage.elapsed > slow)
            .collect()
    }

    pub(crate) fn slow_stage_labels(&self) -> Vec<&'static str> {
        self.slow_warning_stages()
            .into_iter()
            .map(|stage| stage.stage.label())
            .collect()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AttachIdleBackoff {
    initial: Duration,
    max: Duration,
    current: Duration,
}

impl AttachIdleBackoff {
    pub(crate) fn new(initial: Duration, max: Duration) -> Self {
        let initial = initial.min(max);
        Self {
            initial,
            max,
            current: initial,
        }
    }

    pub(crate) fn from_budget(budget: AttachTickBudget) -> Self {
        Self::new(
            Duration::from_millis(budget.idle_initial_ms),
            Duration::from_millis(budget.idle_max_ms),
        )
    }

    pub(crate) fn current_delay(&self) -> Duration {
        self.current
    }

    pub(crate) fn record_idle(&mut self) {
        self.current = (self.current * 2).min(self.max);
    }

    pub(crate) fn reset(&mut self) {
        self.current = self.initial;
    }
}

pub(crate) struct AttachInputEvents {
    stream: EventStream,
    idle: AttachIdleBackoff,
}

impl AttachInputEvents {
    pub(crate) fn new(budget: AttachTickBudget) -> Self {
        Self {
            stream: EventStream::new(),
            idle: AttachIdleBackoff::from_budget(budget),
        }
    }

    pub(crate) async fn next_event(&mut self) -> Result<Option<Event>> {
        tokio::select! {
            event = self.stream.next() => {
                self.idle.reset();
                match event {
                    Some(Ok(event)) => Ok(Some(event)),
                    Some(Err(error)) => Err(error.into()),
                    None => Ok(None),
                }
            }
            _ = tokio::time::sleep(self.idle.current_delay()) => {
                self.idle.record_idle();
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachWakeReason {
    Input,
    Ledger,
    FallbackTick,
}

#[cfg(test)]
pub(crate) async fn wait_for_attach_wake_for_test(
    input_rx: &mut tokio::sync::mpsc::Receiver<()>,
    ledger_rx: &mut tokio::sync::mpsc::Receiver<()>,
    idle: &mut AttachIdleBackoff,
    fallback_tick: Duration,
) -> AttachWakeReason {
    tokio::select! {
        Some(_) = input_rx.recv() => {
            idle.reset();
            AttachWakeReason::Input
        }
        Some(_) = ledger_rx.recv() => {
            idle.reset();
            AttachWakeReason::Ledger
        }
        _ = tokio::time::sleep(idle.current_delay().min(fallback_tick)) => {
            idle.record_idle();
            AttachWakeReason::FallbackTick
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct AttachStormReplayConfig {
    pub(crate) surface: AttachSurface,
    pub(crate) budget: AttachTickBudget,
    pub(crate) frame_interval: Duration,
    pub(crate) tail_row_limit: usize,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AttachStormReplayResult {
    pub(crate) events_seen: usize,
    pub(crate) frames_drawn: usize,
    pub(crate) max_input_to_frame: Duration,
    pub(crate) input_to_frame_exceeded: bool,
    pub(crate) max_tail_rows: usize,
    pub(crate) retained_tail_rows: usize,
}

#[cfg(test)]
#[derive(Debug, serde::Deserialize)]
struct AttachStormReplayEvent {
    at_ms: u64,
    kind: String,
}

#[cfg(test)]
pub(crate) fn replay_attach_event_storm_for_test(
    jsonl: &str,
    config: AttachStormReplayConfig,
) -> Result<AttachStormReplayResult> {
    let frame_ms = u64::try_from(config.frame_interval.as_millis())
        .unwrap_or(u64::MAX)
        .max(1);
    let mut events_seen = 0_usize;
    let mut frames_drawn = 0_usize;
    let mut next_frame_due_ms: Option<u64> = None;
    let mut pending_input_at_ms: Option<u64> = None;
    let mut max_input_to_frame = Duration::ZERO;
    let mut input_to_frame_exceeded = false;
    let mut retained_tail_rows = VecDeque::new();
    let mut max_tail_rows = 0_usize;

    for line in jsonl.lines().filter(|line| !line.trim().is_empty()) {
        let row: serde_json::Value = serde_json::from_str(line)?;
        let event: AttachStormReplayEvent = serde_json::from_value(row.clone())?;
        events_seen = events_seen.saturating_add(1);
        retained_tail_rows.push_back(row);
        while retained_tail_rows.len() > config.tail_row_limit {
            retained_tail_rows.pop_front();
        }
        max_tail_rows = max_tail_rows.max(retained_tail_rows.len());

        if event.kind == "input" && pending_input_at_ms.is_none() {
            pending_input_at_ms = Some(event.at_ms);
        }
        let due = next_frame_due_ms.get_or_insert(event.at_ms.saturating_add(frame_ms));
        while event.at_ms >= *due {
            record_storm_frame(
                *due,
                &mut pending_input_at_ms,
                &mut max_input_to_frame,
                &mut input_to_frame_exceeded,
                &mut frames_drawn,
                config,
            );
            *due = (*due).saturating_add(frame_ms);
        }
    }

    if events_seen > 0
        && let Some(due) = next_frame_due_ms
    {
        record_storm_frame(
            due,
            &mut pending_input_at_ms,
            &mut max_input_to_frame,
            &mut input_to_frame_exceeded,
            &mut frames_drawn,
            config,
        );
    }

    Ok(AttachStormReplayResult {
        events_seen,
        frames_drawn,
        max_input_to_frame,
        input_to_frame_exceeded,
        max_tail_rows,
        retained_tail_rows: retained_tail_rows.len(),
    })
}

#[cfg(test)]
fn record_storm_frame(
    frame_at_ms: u64,
    pending_input_at_ms: &mut Option<u64>,
    max_input_to_frame: &mut Duration,
    input_to_frame_exceeded: &mut bool,
    frames_drawn: &mut usize,
    config: AttachStormReplayConfig,
) {
    *frames_drawn = frames_drawn.saturating_add(1);
    if let Some(input_at_ms) = pending_input_at_ms.take() {
        let elapsed = Duration::from_millis(frame_at_ms.saturating_sub(input_at_ms));
        *max_input_to_frame = (*max_input_to_frame).max(elapsed);
        let mut tick = AttachTickTiming::new(config.surface, config.budget);
        tick.record(AttachLoopStage::InputToFrame, elapsed);
        if tick.input_to_frame_exceeded() {
            *input_to_frame_exceeded = true;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CampaignAttachKeyAction {
    None,
    Refresh,
    Back,
    Quit,
    DrillInto { sub_id: String, plan_id: String },
}

pub(crate) fn handle_campaign_key(
    state: &mut CampaignAttachState,
    key: KeyEvent,
) -> CampaignAttachKeyAction {
    match key.code {
        KeyCode::Enter => state
            .selected_sub_plan()
            .map(|(sub_id, plan_id)| CampaignAttachKeyAction::DrillInto { sub_id, plan_id })
            .unwrap_or(CampaignAttachKeyAction::None),
        KeyCode::Char('r') if key.modifiers.is_empty() => CampaignAttachKeyAction::Refresh,
        KeyCode::Char('b') if key.modifiers.is_empty() => CampaignAttachKeyAction::Back,
        KeyCode::Backspace | KeyCode::Esc => CampaignAttachKeyAction::Back,
        KeyCode::Char('q') if key.modifiers.is_empty() => CampaignAttachKeyAction::Quit,
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            CampaignAttachKeyAction::Quit
        }
        _ => {
            // Navigation keys move the selection; any other key is a no-op.
            let count = state.campaign.sub_goals.len();
            let mut nav = CampaignNav {
                selected: &mut state.selected,
                count,
            };
            crate::tui::navigation::dispatch_navigation(&mut nav, key);
            CampaignAttachKeyAction::None
        }
    }
}

pub(crate) fn note_tui_suspended() -> usize {
    TUI_SUSPEND_DEPTH.fetch_add(1, Ordering::SeqCst) + 1
}

pub(crate) fn note_tui_resumed() -> usize {
    loop {
        let current = TUI_SUSPEND_DEPTH.load(Ordering::SeqCst);
        if current == 0 {
            return 0;
        }
        if TUI_SUSPEND_DEPTH
            .compare_exchange(current, current - 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return current - 1;
        }
    }
}

#[cfg(test)]
pub(crate) fn reset_tui_suspend_depth() {
    TUI_SUSPEND_DEPTH.store(0, Ordering::SeqCst);
}

#[cfg(test)]
pub(crate) fn tui_suspend_depth() -> usize {
    TUI_SUSPEND_DEPTH.load(Ordering::SeqCst)
}

#[derive(Debug, Clone)]
pub(crate) struct AttachNarrativeRefreshState {
    pub(crate) kind: NarrativeRefreshKind,
    pub(crate) started_at: DateTime<Utc>,
    pub(crate) token: CancellationToken,
    pub(crate) coalesced_requests: usize,
    pub(crate) completed: bool,
}

impl AttachNarrativeRefreshState {
    pub(crate) fn new(
        kind: NarrativeRefreshKind,
        started_at: DateTime<Utc>,
        token: CancellationToken,
    ) -> Self {
        Self {
            kind,
            started_at,
            token,
            coalesced_requests: 0,
            completed: false,
        }
    }

    pub(crate) fn start_notice(&self) -> String {
        format!(
            "{}: refresh running in background; q detaches immediately",
            self.kind.label()
        )
    }

    pub(crate) fn coalesce(&mut self, kind: NarrativeRefreshKind, now: DateTime<Utc>) -> String {
        self.coalesced_requests = self.coalesced_requests.saturating_add(1);
        format!(
            "{}: refresh already running in background ({}s); coalesced {}",
            self.kind.label(),
            (now - self.started_at).num_seconds().max(0),
            kind.label()
        )
    }

    pub(crate) fn completion_notice_once(&mut self, notice: String) -> Option<String> {
        if self.completed {
            return None;
        }
        self.completed = true;
        Some(notice)
    }

    pub(crate) fn cancel(&self) {
        self.token.cancel();
    }
}

pub(crate) struct AttachRunNarrativeRefreshJob {
    pub(crate) state: AttachNarrativeRefreshState,
    pub(crate) handle: tokio::task::JoinHandle<String>,
}

pub(crate) struct AttachRunNarrativeRefreshRequest {
    pub(crate) paths: DeadreckonPaths,
    state: deadreckon_core::PipelineState,
    spend: Vec<SpendRecord>,
    traces: Vec<TraceRecord>,
    events: Vec<RunEvent>,
    live: AttachLive,
    tui_state: AttachTuiState,
    pub(crate) config: NarrativeAttachConfig,
    kind: NarrativeRefreshKind,
}

pub(crate) struct AttachPlanNarrativeRefreshJob {
    pub(crate) plan_id: String,
    pub(crate) state: AttachNarrativeRefreshState,
    pub(crate) handle: tokio::task::JoinHandle<String>,
}

pub(crate) struct AttachPlanNarrativeRefreshRequest {
    pub(crate) paths: DeadreckonPaths,
    pub(crate) plan: Plan,
    pub(crate) messages: Vec<PlanMessage>,
    pub(crate) plan_events: Vec<PlanEvent>,
    pub(crate) feed_events: Vec<PlanFeedEvent>,
    pub(crate) selected: usize,
    pub(crate) config: NarrativeAttachConfig,
    kind: NarrativeRefreshKind,
}

pub(crate) struct PlanNarrativeRefreshInput<'a> {
    pub(crate) paths: &'a DeadreckonPaths,
    pub(crate) plan: &'a Plan,
    pub(crate) messages: &'a [PlanMessage],
    pub(crate) plan_events: &'a [PlanEvent],
    pub(crate) feed_events: &'a [PlanFeedEvent],
    pub(crate) selected: usize,
    pub(crate) config: &'a NarrativeAttachConfig,
}

pub(crate) fn start_or_coalesce_run_narrative_refresh_job(
    job: &mut Option<AttachRunNarrativeRefreshJob>,
    request: AttachRunNarrativeRefreshRequest,
    now: DateTime<Utc>,
) -> String {
    if let Some(active) = job.as_mut() {
        return active.state.coalesce(request.kind, now);
    }

    let token = CancellationToken::new();
    let refresh_state = AttachNarrativeRefreshState::new(request.kind, now, token.clone());
    let notice = refresh_state.start_notice();
    let handle = tokio::spawn(async move {
        let AttachRunNarrativeRefreshRequest {
            paths,
            state,
            spend,
            traces,
            events,
            live,
            tui_state,
            config,
            kind,
        } = request;
        let run_view = deadreckon_core::RunView::from_state(&state).ok();
        refresh_run_narrative_with_provider_for_kind_with_token(
            &paths,
            &RunNarrativeRenderInput {
                state: &state,
                run_view: run_view.as_ref(),
                spend: &spend,
                traces: &traces,
                events: &events,
                live: &live,
                tui_state: &tui_state,
            },
            &config,
            kind,
            Some(token),
        )
        .await
        .unwrap_or_else(|err| {
            format!(
                "provider refresh failed: {}",
                one_line(&err.to_string(), 120)
            )
        })
    });
    *job = Some(AttachRunNarrativeRefreshJob {
        state: refresh_state,
        handle,
    });
    notice
}

pub(crate) fn start_or_coalesce_plan_narrative_refresh_job(
    job: &mut Option<AttachPlanNarrativeRefreshJob>,
    request: AttachPlanNarrativeRefreshRequest,
    now: DateTime<Utc>,
) -> String {
    if let Some(active) = job.as_mut()
        && active.plan_id == request.plan.plan_id
    {
        return active.state.coalesce(request.kind, now);
    }
    cancel_plan_narrative_refresh_job(job);

    let token = CancellationToken::new();
    let plan_id = request.plan.plan_id.clone();
    let refresh_state = AttachNarrativeRefreshState::new(request.kind, now, token.clone());
    let notice = refresh_state.start_notice();
    let handle = tokio::spawn(async move {
        let AttachPlanNarrativeRefreshRequest {
            paths,
            plan,
            messages,
            plan_events,
            feed_events,
            selected,
            config,
            kind,
        } = request;
        refresh_plan_narrative_with_provider_for_kind(PlanNarrativeProviderRefresh {
            paths: &paths,
            plan: &plan,
            messages: &messages,
            plan_events: &plan_events,
            feed_events: &feed_events,
            selected,
            config: &config,
            kind,
            cancellation_token: Some(token),
        })
        .await
        .unwrap_or_else(|err| {
            format!(
                "provider refresh failed: {}",
                one_line(&err.to_string(), 120)
            )
        })
    });
    *job = Some(AttachPlanNarrativeRefreshJob {
        plan_id,
        state: refresh_state,
        handle,
    });
    notice
}

pub(crate) async fn poll_run_narrative_refresh_job(
    job: &mut Option<AttachRunNarrativeRefreshJob>,
) -> Option<String> {
    if !job
        .as_ref()
        .is_some_and(|active| active.handle.is_finished())
    {
        return None;
    }
    let AttachRunNarrativeRefreshJob { mut state, handle } = job.take()?;
    match handle.await {
        Ok(notice) => state.completion_notice_once(notice),
        Err(err) if err.is_cancelled() => None,
        Err(err) => Some(format!(
            "provider refresh failed: {}",
            one_line(&err.to_string(), 120)
        )),
    }
}

pub(crate) async fn poll_plan_narrative_refresh_job(
    job: &mut Option<AttachPlanNarrativeRefreshJob>,
) -> Option<String> {
    if !job
        .as_ref()
        .is_some_and(|active| active.handle.is_finished())
    {
        return None;
    }
    let AttachPlanNarrativeRefreshJob {
        mut state, handle, ..
    } = job.take()?;
    match handle.await {
        Ok(notice) => state.completion_notice_once(notice),
        Err(err) if err.is_cancelled() => None,
        Err(err) => Some(format!(
            "provider refresh failed: {}",
            one_line(&err.to_string(), 120)
        )),
    }
}

pub(crate) fn cancel_run_narrative_refresh_job(
    job: &mut Option<AttachRunNarrativeRefreshJob>,
) -> bool {
    let Some(active) = job.take() else {
        return false;
    };
    active.state.cancel();
    active.handle.abort();
    true
}

pub(crate) fn cancel_plan_narrative_refresh_job(
    job: &mut Option<AttachPlanNarrativeRefreshJob>,
) -> bool {
    let Some(active) = job.take() else {
        return false;
    };
    active.state.cancel();
    active.handle.abort();
    true
}

pub(crate) fn run_narrative_refresh_request(
    paths: &DeadreckonPaths,
    input: &RunNarrativeRenderInput<'_>,
    config: &NarrativeAttachConfig,
    kind: NarrativeRefreshKind,
) -> AttachRunNarrativeRefreshRequest {
    AttachRunNarrativeRefreshRequest {
        paths: paths.clone(),
        state: input.state.clone(),
        spend: input.spend.to_vec(),
        traces: input.traces.to_vec(),
        events: input.events.to_vec(),
        live: input.live.clone(),
        tui_state: input.tui_state.clone(),
        config: config.clone(),
        kind,
    }
}

pub(crate) fn plan_narrative_refresh_request(
    input: &PlanNarrativeRefreshInput<'_>,
    kind: NarrativeRefreshKind,
) -> AttachPlanNarrativeRefreshRequest {
    AttachPlanNarrativeRefreshRequest {
        paths: input.paths.clone(),
        plan: input.plan.clone(),
        messages: input.messages.to_vec(),
        plan_events: input.plan_events.to_vec(),
        feed_events: input.feed_events.to_vec(),
        selected: input.selected,
        config: input.config.clone(),
        kind,
    }
}

const CAMPAIGN_LIST_PAGE: usize = 10;

/// Drives the campaign attach sub-goal selection through the shared navigation
/// core ([`crate::tui::navigation`]). Arrows/`jk`/`Tab` move one; `PgUp`/`PgDn`
/// page; `Home`/`End`/`g`/`G` jump to the first/last sub-goal — parity with run.
struct CampaignNav<'a> {
    selected: &'a mut usize,
    count: usize,
}

impl crate::tui::navigation::NavigableSurface for CampaignNav<'_> {
    fn focus_next(&mut self) {
        self.scroll_lines(1);
    }

    fn focus_previous(&mut self) {
        self.scroll_lines(-1);
    }

    fn scroll_lines(&mut self, delta: isize) {
        let max = self.count.saturating_sub(1) as isize;
        *self.selected = (*self.selected as isize + delta).clamp(0, max) as usize;
    }

    fn scroll_page(&mut self, direction: isize) {
        self.scroll_lines(direction.signum() * CAMPAIGN_LIST_PAGE as isize);
    }

    fn scroll_to_start(&mut self) {
        *self.selected = 0;
    }

    fn scroll_to_end(&mut self) {
        *self.selected = self.count.saturating_sub(1);
    }
}

#[cfg(test)]
mod campaign_nav_tests {
    use super::{CAMPAIGN_LIST_PAGE, CampaignNav};
    use crate::tui::navigation::dispatch_navigation;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn nav(selected: &mut usize, count: usize) -> CampaignNav<'_> {
        CampaignNav { selected, count }
    }

    #[test]
    fn campaign_attach_supports_paging_keys() {
        let count = 30;
        let mut selected = 0;
        dispatch_navigation(&mut nav(&mut selected, count), key(KeyCode::PageDown));
        assert_eq!(selected, CAMPAIGN_LIST_PAGE, "PgDn pages forward");
        dispatch_navigation(&mut nav(&mut selected, count), key(KeyCode::PageUp));
        assert_eq!(selected, 0, "PgUp pages back");
        for code in [KeyCode::End, KeyCode::Char('G')] {
            let mut selected = 3;
            dispatch_navigation(&mut nav(&mut selected, count), key(code));
            assert_eq!(selected, count - 1, "{code:?} jumps to last");
        }
        for code in [KeyCode::Home, KeyCode::Char('g')] {
            let mut selected = 7;
            dispatch_navigation(&mut nav(&mut selected, count), key(code));
            assert_eq!(selected, 0, "{code:?} jumps to first");
        }
    }

    #[test]
    fn campaign_attach_navigation_matches_run_reference() {
        let count = 10;
        for code in [KeyCode::Down, KeyCode::Char('j'), KeyCode::Tab] {
            let mut selected = 4;
            assert!(dispatch_navigation(
                &mut nav(&mut selected, count),
                key(code)
            ));
            assert_eq!(selected, 5, "{code:?} should advance one");
        }
        for code in [KeyCode::Up, KeyCode::Char('k'), KeyCode::BackTab] {
            let mut selected = 4;
            assert!(dispatch_navigation(
                &mut nav(&mut selected, count),
                key(code)
            ));
            assert_eq!(selected, 3, "{code:?} should retreat one");
        }
        let mut selected = 0;
        dispatch_navigation(&mut nav(&mut selected, count), key(KeyCode::Up));
        assert_eq!(selected, 0, "clamps at top");
        let mut selected = count - 1;
        dispatch_navigation(&mut nav(&mut selected, count), key(KeyCode::Down));
        assert_eq!(selected, count - 1, "clamps at bottom");
    }
}
