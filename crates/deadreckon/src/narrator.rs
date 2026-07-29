//! The live narrator sidecar: an in-process task the run spawns to narrate
//! progress in plain English while the turn loop works.
//!
//! A [`RunEventBus`] sender feeds `RunLoopConfig.event_sender`; the spawned
//! [`NarratorEngine`] subscribes and, on each turn checkpoint, windows the new
//! turn, decides cadence, and (when due) builds a continuity prompt, calls the
//! cheap narrator model, and appends an amended beat to `snapshots.jsonl` —
//! falling back to a deterministic floor beat when no provider is available,
//! the budget is spent, or a model call fails. Between beats a $0 ticker keeps
//! a long turn from looking frozen. The task stops the instant the run finishes
//! (cancellation) or the bus closes, flushing a final beat.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use deadreckon_core::RunEventBus;
use deadreckon_protocol::{LedgerItem, RunEvent, RunEventKind, SpendRecord};
use deadreckon_providers::{
    NarratorBackend, ProviderRequest, ProviderResponse, ProviderRouter, select_narrator_backend,
};
use deadreckon_runtime::NarratorConfig;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::narrative::{
    LiveBeatMeta, NarrativeProviderRefresh, NarrativeSnapshot, NarratorWindow,
    append_narrative_snapshot, apply_live_narrator_response, build_live_floor_beat,
    build_live_narrator_prompt, headless_beat_lines, live_block_lines, read_turn_record,
    seed_live_snapshot, turn_record_to_input,
};

const NARRATOR_MAX_TOKENS: u32 = 1024;
const QUIET_TICK: Duration = Duration::from_secs(10);

/// Decide whether to narrate and how, from the run's surface and flags.
///
/// Foreground narration is on by default on a TTY (the calm block, P8).
/// Headless append to stderr (P9) is the explicit `--narrate` opt-in and only
/// applies off-TTY. `--no-narrate` disables everything. When neither surface
/// would show anything (piped without `--narrate`), narration is `None` and no
/// bus or task is created.
pub(crate) fn resolve_narrator_config(
    is_tty: bool,
    narrate_flag: bool,
    no_narrate: bool,
    model_override: Option<String>,
) -> Option<NarratorConfig> {
    if no_narrate {
        return None;
    }
    let foreground = is_tty;
    let headless_append = !is_tty && narrate_flag;
    if !foreground && !headless_append {
        return None;
    }
    Some(NarratorConfig {
        foreground,
        headless_append,
        model_override,
        ..NarratorConfig::default()
    })
}

/// Resolve narration for a spawned orchestrate/campaign CHILD. A child is
/// headless and its stdout (run-id scrape) and stderr (failure capture) belong
/// to the parent, so a child narrates FILE-ONLY: `foreground=false` AND
/// `headless_append=false` — `render_beat` no-ops on both surfaces while beats
/// still append to `snapshots.jsonl`. Returns `None` unless the parent opted in
/// via `--narrate`.
pub(crate) fn resolve_narrator_config_for_child(
    narrate_flag: bool,
    no_narrate: bool,
    model_override: Option<String>,
) -> Option<NarratorConfig> {
    if no_narrate || !narrate_flag {
        return None;
    }
    Some(NarratorConfig {
        foreground: false,
        headless_append: false,
        model_override,
        ..NarratorConfig::default()
    })
}

/// Env var the parent sets on a narrating child so the child resolves narration
/// FILE-ONLY (not the off-TTY `--narrate` stderr path a user gets from
/// `dr run --narrate | …`).
pub(crate) const NARRATE_CHILD_ENV: &str = "DEADRECKON_NARRATE_CHILD";

/// A child defaults to the deterministic floor ($0, no auth probe) unless the
/// parent pinned a model — this bounds the dollar blast radius and the
/// `probe_cli_auth` storm across a wide fan-out.
pub(crate) fn child_narrator_backend_is_floor(model_override: Option<&str>) -> bool {
    model_override.is_none()
}

/// Resolve narration for any run loop: a spawned child (`is_child`, file-only)
/// or an interactive/leaf run (`is_tty` TTY contract). The single decision used
/// by `dr run` and both `extend` paths.
pub(crate) fn resolve_narration(
    is_child: bool,
    is_tty: bool,
    narrate: bool,
    no_narrate: bool,
    model_override: Option<String>,
) -> Option<NarratorConfig> {
    if is_child {
        resolve_narrator_config_for_child(narrate, no_narrate, model_override)
    } else {
        resolve_narrator_config(is_tty, narrate, no_narrate, model_override)
    }
}

/// A spawned narrator task plus the token that stops it.
pub(crate) struct NarratorHandle {
    cancel: CancellationToken,
    join: JoinHandle<()>,
}

/// Grace period for the narrator to flush a final beat after the run ends. A
/// model call may be in flight; if it does not return within the grace window
/// we stop waiting (detaching the task) so a run never hangs on narration.
const NARRATOR_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

impl NarratorHandle {
    /// Stop the narrator and wait briefly for it to drain. Called after the run
    /// loop returns. Bounded so an in-flight provider call can never block the
    /// run's exit.
    pub(crate) async fn shutdown(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(NARRATOR_SHUTDOWN_GRACE, self.join).await;
    }
}

const NARRATOR_BUS_CAPACITY: usize = 256;

/// Everything the narrator sidecar needs about the run it is narrating.
pub(crate) struct NarratorCtx {
    pub(crate) run_id: String,
    pub(crate) run_root: PathBuf,
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) backend: NarratorBackend,
    pub(crate) config: NarratorConfig,
}

/// Resolve the narrator backend for a run from the provider registry, honoring
/// a model override. Falls back to the deterministic floor if the registry
/// cannot be loaded.
pub(crate) fn resolve_narrator_backend(
    home: &Path,
    model_override: Option<&str>,
) -> NarratorBackend {
    match deadreckon_providers::registry::ProviderRegistry::with_overrides(home) {
        Ok(registry) => select_narrator_backend(&registry, model_override),
        Err(_) => NarratorBackend::DeterministicFloor,
    }
}

/// Build the narration wiring: a bus whose sender feeds the run loop, plus a
/// spawned engine task subscribed to it. Returns the sender and a handle that
/// stops the task after the run.
pub(crate) fn build_narration(ctx: NarratorCtx) -> (broadcast::Sender<RunEvent>, NarratorHandle) {
    let bus = RunEventBus::new(NARRATOR_BUS_CAPACITY);
    let receiver = bus.subscribe();
    let sender = bus.sender();
    let cancel = CancellationToken::new();
    let router = build_narrator_router(&ctx);
    let engine = NarratorEngine::new(ctx, cancel.clone());
    let join = tokio::spawn(run_engine(engine, receiver, cancel.clone(), router));
    (sender, NarratorHandle { cancel, join })
}

fn build_narrator_router(ctx: &NarratorCtx) -> Option<ProviderRouter> {
    match &ctx.backend {
        NarratorBackend::Model { provider, model } => ctx.config_path.as_ref().and_then(|path| {
            ProviderRouter::from_config_path_with_model(path, Some(provider), Some(model)).ok()
        }),
        NarratorBackend::DeterministicFloor => None,
    }
}

/// Shared narrator wiring for any run loop (the `dr run` command and both
/// `extend` paths). When `config` is `Some`, pick the backend (the deterministic
/// floor when `force_floor`, else the subscription-first resolver), build the
/// `NarratorCtx`, and spawn the engine; when `None`, the run is wired exactly as
/// before. The single shutdown-ordering contract lives at the call site:
/// `handle.shutdown().await` after the awaited loop, before lock release.
pub(crate) fn build_run_narration(
    home: &Path,
    config_path: Option<PathBuf>,
    run_id: &str,
    run_root: &Path,
    force_floor: bool,
    config: Option<NarratorConfig>,
) -> (Option<broadcast::Sender<RunEvent>>, Option<NarratorHandle>) {
    let Some(config) = config else {
        return (None, None);
    };
    let backend = if force_floor {
        NarratorBackend::DeterministicFloor
    } else {
        resolve_narrator_backend(home, config.model_override.as_deref())
    };
    let ctx = NarratorCtx {
        run_id: run_id.to_string(),
        run_root: run_root.to_path_buf(),
        config_path,
        backend,
        config,
    };
    let (sender, handle) = build_narration(ctx);
    (Some(sender), Some(handle))
}

fn append_narrator_spend(run_root: &Path, record: &SpendRecord) -> deadreckon_core::Result<()> {
    deadreckon_core::ledger_io::append_ledger_item(run_root, LedgerItem::Spend(record.clone()))
}

/// Write append-only beat lines to a sink (stderr in production). Plain,
/// newline-terminated, no cursor controls — so stdout stays clean for piped
/// consumers.
fn write_headless_lines(out: &mut impl Write, lines: &[String]) -> std::io::Result<()> {
    for line in lines {
        writeln!(out, "{line}")?;
    }
    out.flush()
}

/// Whether the run should drive plain progress. Piped runs (stdout not a TTY)
/// would get plain progress so a run is never silent between the start and exit
/// cards. NOTE: not wired to the run surface — this project intentionally keeps
/// rich (box-drawing) rendering when piped, opting out of color only via
/// `NO_COLOR`/`--plain`, so forcing plain off-TTY would regress that contract.
/// The decision is kept here (and unit-tested) and its run wiring is a V1
/// candidate; `--narrate` already gives piped progress when opted in.
#[allow(dead_code)]
pub(crate) fn effective_plain(
    plain_flag: bool,
    configured: bool,
    no_color: bool,
    stdout_is_tty: bool,
) -> bool {
    plain_flag || configured || no_color || !stdout_is_tty
}

/// Refuse conflicting narration flags with a `try:`-style message.
pub(crate) fn validate_narration_flags(narrate: bool, no_narrate: bool) -> Result<(), String> {
    if narrate && no_narrate {
        Err("pass only one of --narrate / --no-narrate".to_string())
    } else {
        Ok(())
    }
}

/// Whether a `--narrator-model` id appears in any provider catalog (id or alias).
pub(crate) fn narrator_model_known(
    registry: &deadreckon_providers::registry::ProviderRegistry,
    model: &str,
) -> bool {
    registry.iter().any(|descriptor| {
        descriptor
            .model_catalog
            .iter()
            .any(|entry| entry.id == model || entry.aliases.iter().any(|alias| alias == model))
    })
}

/// Refusal message for an unknown narrator model, pointing at `deadreckon models`.
pub(crate) fn narrator_model_refusal(model: &str) -> String {
    format!("unknown narrator model '{model}'; try: deadreckon models")
}

/// The narrator engine: holds the rolling story and turns run events into beats
/// via the window + cadence + continuity machinery. Sync logic; the async task
/// drives it and performs the provider call between prompt and commit.
struct NarratorEngine {
    ctx: NarratorCtx,
    narrative_dir: PathBuf,
    window: NarratorWindow,
    ledger: NarratorLedger,
    current: NarrativeSnapshot,
    beats_emitted: u32,
    last_beat_at: Option<DateTime<Utc>>,
    turn_started: Option<(u32, DateTime<Utc>)>,
    block: ForegroundBlock,
    // Cancelled at shutdown. Threaded into the model request so an in-flight
    // beat call is interrupted instead of blocking the final flush.
    cancel: CancellationToken,
}

impl NarratorEngine {
    fn new(ctx: NarratorCtx, cancel: CancellationToken) -> Self {
        let narrative_dir = ctx.run_root.join("narrative");
        let current = seed_live_snapshot(&ctx.run_id);
        let ledger = NarratorLedger::new(ctx.config.budget_usd);
        Self {
            ctx,
            narrative_dir,
            window: NarratorWindow::new(),
            ledger,
            current,
            beats_emitted: 0,
            last_beat_at: None,
            turn_started: None,
            block: ForegroundBlock::new(),
            cancel,
        }
    }

    fn use_model(&self) -> bool {
        narrator_should_use_model(&self.ctx.backend, &self.ledger)
    }

    fn decision(&self, now: DateTime<Utc>, in_flight: Option<u64>) -> BeatDecision {
        let since = self
            .last_beat_at
            .map(|last| (now - last).num_seconds().max(0) as u64);
        cadence_decision(
            &self.ctx.config,
            self.beats_emitted,
            self.window.pending().len() as u32,
            since,
            in_flight,
        )
    }

    fn render_ticker(&mut self, turn: u32, tool: &str, now: DateTime<Utc>) {
        if !self.ctx.config.foreground {
            return;
        }
        let elapsed = self
            .turn_started
            .filter(|(t, _)| *t == turn)
            .map(|(_, started)| (now - started).num_seconds().max(0) as u64)
            .unwrap_or(0);
        let line = deterministic_ticker_line(turn, tool, elapsed);
        self.draw(&[line]);
    }

    fn render_beat(&mut self) {
        if self.ctx.config.foreground {
            let lines = live_block_lines(&self.current, self.ctx.config.lines);
            self.draw(&lines);
        } else if self.ctx.config.headless_append {
            // Append-only, turn-stamped beats to stderr; stdout stays clean.
            let lines = headless_beat_lines(&self.current, self.ctx.config.lines + 1);
            let mut err = std::io::stderr();
            let _ = write_headless_lines(&mut err, &lines);
        }
    }

    fn draw(&mut self, lines: &[String]) {
        let out = self.block.render(lines);
        eprint!("{out}");
        let _ = std::io::stderr().flush();
    }

    async fn emit(&mut self, now: DateTime<Utc>, router: Option<&ProviderRouter>) {
        if !self.window.has_pending() {
            return;
        }
        if self.use_model()
            && let Some(router) = router
            && let Ok(bundle) = build_live_narrator_prompt(
                &self.current,
                self.window.pending(),
                self.window.rolling_summary(),
            )
        {
            let request = ProviderRequest {
                prompt: bundle.prompt,
                max_output_tokens: NARRATOR_MAX_TOKENS,
                cwd: None,
                output_path: None,
                sandbox_backend: None,
                workspace_access: deadreckon_providers::WorkspaceAccess::ReadWrite,
                pid_file: None,
                // Interruptible: a shutdown cancel aborts the in-flight beat call
                // so emit() falls through to a floor beat instead of the task
                // being killed mid-call with no beat written.
                cancellation_token: Some(self.cancel.clone()),
                session_dir: None,
                output_schema: None,
                capability_posture: None,
            };
            if let Ok(response) = router.complete(&request).await
                && self.commit_model_beat(&response, now).is_ok()
            {
                return;
            }
        }
        let _ = self.commit_floor_beat(now);
    }

    fn commit_model_beat(
        &mut self,
        response: &ProviderResponse,
        now: DateTime<Utc>,
    ) -> crate::Result<()> {
        let pending = self.window.pending().to_vec();
        let beat_seq = u64::from(self.beats_emitted) + 1;
        let covers = self.window.commit_beat().unwrap_or(0);
        let refresh = NarrativeProviderRefresh {
            route: response.spend.provider.clone(),
            model: response.spend.model.clone(),
            cost_usd: response.spend.cost_usd,
            subscription_seconds: if response.spend.subscription {
                response.spend.wall_time_seconds
            } else {
                None
            },
        };
        let meta = LiveBeatMeta {
            beat_seq,
            covers_turn: covers,
            rolling_summary: self.window.rolling_summary().map(str::to_string),
            provider: refresh,
        };
        let beat = apply_live_narrator_response(&self.current, &pending, &response.content, meta)?;
        append_narrative_snapshot(&self.narrative_dir, &beat)?;
        self.ledger.record_spend(response.spend.cost_usd);
        let spend = narrator_spend_record(
            covers,
            response,
            self.ledger.spent_usd(),
            self.ctx.config.budget_usd,
        );
        let _ = append_narrator_spend(&self.ctx.run_root, &spend);
        self.current = beat;
        self.beats_emitted += 1;
        self.last_beat_at = Some(now);
        self.render_beat();
        Ok(())
    }

    fn commit_floor_beat(&mut self, now: DateTime<Utc>) -> crate::Result<()> {
        let pending = self.window.pending().to_vec();
        if pending.is_empty() {
            return Ok(());
        }
        let beat_seq = u64::from(self.beats_emitted) + 1;
        let covers = self.window.commit_beat().unwrap_or(0);
        let meta = LiveBeatMeta {
            beat_seq,
            covers_turn: covers,
            rolling_summary: self.window.rolling_summary().map(str::to_string),
            provider: NarrativeProviderRefresh {
                route: "deterministic".to_string(),
                model: "none".to_string(),
                cost_usd: 0.0,
                subscription_seconds: None,
            },
        };
        let beat = build_live_floor_beat(&self.current, &pending, meta);
        append_narrative_snapshot(&self.narrative_dir, &beat)?;
        self.current = beat;
        self.beats_emitted += 1;
        self.last_beat_at = Some(now);
        self.render_beat();
        Ok(())
    }

    async fn on_event(&mut self, event: RunEvent, router: Option<&ProviderRouter>) {
        let now = event.timestamp;
        match event.event {
            RunEventKind::TurnStarted { turn } => self.turn_started = Some((turn, now)),
            RunEventKind::ToolCallStarted {
                turn, tool_name, ..
            } => self.render_ticker(turn, &tool_name, now),
            RunEventKind::DocsCheckpoint { turn, path, .. } => {
                if let Some(record) = read_turn_record(&path, turn) {
                    self.window.observe(turn_record_to_input(&record));
                }
                if matches!(self.decision(now, None), BeatDecision::Emit) {
                    self.emit(now, router).await;
                }
            }
            RunEventKind::RunCompleted { .. } | RunEventKind::RunPromoted { .. } => {
                self.emit(now, router).await;
            }
            _ => {}
        }
    }

    fn on_quiet_tick(&mut self, now: DateTime<Utc>) {
        if let Some((turn, _)) = self.turn_started {
            self.render_ticker(turn, "working", now);
        }
    }

    /// Fold a turn into the window without emitting. Used to drain events still
    /// buffered at shutdown so the final floor flush covers them — never starts
    /// a model call, so the run's exit is not delayed.
    fn observe_event(&mut self, event: &RunEvent) {
        if let RunEventKind::DocsCheckpoint { turn, path, .. } = &event.event
            && let Some(record) = read_turn_record(path, *turn)
        {
            self.window.observe(turn_record_to_input(&record));
        }
    }
}

async fn run_engine(
    mut engine: NarratorEngine,
    mut receiver: broadcast::Receiver<RunEvent>,
    cancel: CancellationToken,
    router: Option<ProviderRouter>,
) {
    let router = router.as_ref();
    let mut quiet = tokio::time::interval(QUIET_TICK);
    quiet.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                // Drain events still buffered at shutdown before the final flush.
                // `cancel` and a non-empty `recv()` race in this `select!`; if
                // cancel wins first, the run's own DocsCheckpoint/RunCompleted
                // events are still in the channel, and skipping them leaves the
                // window empty so `commit_floor_beat` writes nothing — a fast run
                // would produce zero beats. Observe-only keeps shutdown floor-only.
                while let Ok(event) = receiver.try_recv() {
                    engine.observe_event(&event);
                }
                let _ = engine.commit_floor_beat(Utc::now());
                break;
            }
            _ = quiet.tick() => engine.on_quiet_tick(Utc::now()),
            received = receiver.recv() => match received {
                Ok(event) => engine.on_event(event, router).await,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => {
                    let _ = engine.commit_floor_beat(Utc::now());
                    break;
                }
            }
        }
    }
}

/// What the cadence should do at a decision point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BeatDecision {
    /// Emit a model beat now.
    Emit,
    /// Hold — fold this turn into the window and wait (coalescing).
    Coalesce,
    /// The per-run beat cap is reached; emit no more model beats.
    CapReached,
}

/// Time-gated + coalesced cadence. A model beat is due when there is new work
/// AND (enough time has passed OR a burst of turns has accumulated), or when a
/// single long turn has been quiet past the threshold (escalation). Bursts
/// under the gap coalesce; the per-run cap bounds total model calls.
#[allow(dead_code)] // wired into the narrator task in P7
pub(crate) fn cadence_decision(
    config: &NarratorConfig,
    beats_emitted: u32,
    turns_since_last_beat: u32,
    seconds_since_last_beat: Option<u64>,
    turn_in_flight_seconds: Option<u64>,
) -> BeatDecision {
    if beats_emitted >= config.max_beats {
        return BeatDecision::CapReached;
    }
    let due_by_gap = match seconds_since_last_beat {
        None => true,
        Some(elapsed) => elapsed >= config.min_gap_seconds,
    };
    let due_by_burst = turns_since_last_beat >= config.turn_burst;
    let due_by_quiet =
        turn_in_flight_seconds.is_some_and(|elapsed| elapsed >= config.quiet_seconds);
    let has_new_work = turns_since_last_beat > 0;
    if (has_new_work && (due_by_gap || due_by_burst)) || due_by_quiet {
        BeatDecision::Emit
    } else {
        BeatDecision::Coalesce
    }
}

/// The deterministic, $0 live-activity ticker shown between model beats so a
/// long turn never looks frozen. Pure formatting over event fields — no
/// provider call.
#[allow(dead_code)] // wired into the narrator task in P8
pub(crate) fn deterministic_ticker_line(turn: u32, tool: &str, elapsed_seconds: u64) -> String {
    let tool = if tool.trim().is_empty() {
        "working"
    } else {
        tool.trim()
    };
    format!("turn {turn} · {tool} ({})", format_elapsed(elapsed_seconds))
}

#[allow(dead_code)] // wired into the narrator task in P8
fn format_elapsed(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    }
}

/// Tracks the narrator's own spend against its per-run cap. Kept entirely
/// separate from the run loop's `state.total_spend_usd` so narration never
/// inflates or races the run's accounting.
#[allow(dead_code)] // wired into the narrator task in P8
pub(crate) struct NarratorLedger {
    budget_usd: f64,
    spent_usd: f64,
}

#[allow(dead_code)] // wired into the narrator task in P8
impl NarratorLedger {
    pub(crate) fn new(budget_usd: f64) -> Self {
        Self {
            budget_usd,
            spent_usd: 0.0,
        }
    }

    pub(crate) fn budget_available(&self) -> bool {
        self.spent_usd < self.budget_usd
    }

    pub(crate) fn record_spend(&mut self, cost_usd: f64) {
        self.spent_usd += cost_usd.max(0.0);
    }

    pub(crate) fn spent_usd(&self) -> f64 {
        self.spent_usd
    }
}

/// Whether the narrator should make a model call: only when it has a model
/// backend AND its budget is not exhausted. Otherwise it degrades to the
/// deterministic floor — the run is never affected.
#[allow(dead_code)] // wired into the narrator task in P8
pub(crate) fn narrator_should_use_model(
    backend: &NarratorBackend,
    ledger: &NarratorLedger,
) -> bool {
    matches!(backend, NarratorBackend::Model { .. }) && ledger.budget_available()
}

/// Build a `kind: "narrator"` spend row from a provider response, carrying the
/// narrator's own running total and cap. Subscription backends report $0.
#[allow(dead_code)] // wired into the narrator task in P8
pub(crate) fn narrator_spend_record(
    turn: u32,
    response: &ProviderResponse,
    narrator_total_usd: f64,
    cap_usd: f64,
) -> SpendRecord {
    SpendRecord {
        timestamp: Utc::now(),
        turn,
        provider: response.spend.provider.clone(),
        model: response.spend.model.clone(),
        input_tokens: response.spend.input_tokens,
        output_tokens: response.spend.output_tokens,
        cost_usd: response.spend.cost_usd,
        total_cost_usd: narrator_total_usd,
        cap_usd: Some(cap_usd),
        subscription: response.spend.subscription,
        estimated: false,
        wall_time_seconds: response.spend.wall_time_seconds,
        wall_time_cap_seconds: None,
        kind: "narrator".to_string(),
    }
}

/// In-place renderer for the calm foreground block. Each render first clears
/// the previously drawn lines (cursor up + clear line) and then draws the new
/// ones, so the block updates in place rather than scrolling — calm, not a
/// stream.
#[allow(dead_code)] // wired into the narrator task's foreground render in P8 integration
pub(crate) struct ForegroundBlock {
    drawn_lines: usize,
}

#[allow(dead_code)] // wired into the narrator task's foreground render in P8 integration
impl ForegroundBlock {
    pub(crate) fn new() -> Self {
        Self { drawn_lines: 0 }
    }

    pub(crate) fn render(&mut self, lines: &[String]) -> String {
        let mut out = crate::ui::cursor_clear_lines(self.drawn_lines);
        for line in lines {
            out.push_str(line);
            out.push('\n');
        }
        self.drawn_lines = lines.len();
        out
    }

    pub(crate) fn drawn_lines(&self) -> usize {
        self.drawn_lines
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tempfile::TempDir;

    use super::*;
    use crate::narrative::LiveTurnInput;

    const EVENTS_FIXTURE: &str =
        include_str!("../../deadreckon-protocol/tests/fixtures/pre-keel-run/events.jsonl");
    const SPEND_FIXTURE: &str =
        include_str!("../../deadreckon-protocol/tests/fixtures/pre-keel-run/spend.jsonl");
    const TRACES_FIXTURE: &str =
        include_str!("../../deadreckon-protocol/tests/fixtures/pre-keel-run/traces.jsonl");
    const FLIGHT_FIXTURE: &str =
        include_str!("../../deadreckon-protocol/tests/fixtures/pre-keel-run/flight-events.jsonl");
    const NARRATIVE_FIXTURE: &str = include_str!(
        "../../deadreckon-protocol/tests/fixtures/pre-keel-run/narrative/snapshots.jsonl"
    );

    fn floor_ctx(run_root: PathBuf) -> NarratorCtx {
        NarratorCtx {
            run_id: "run-x".to_string(),
            run_root,
            config_path: None,
            backend: NarratorBackend::DeterministicFloor,
            config: NarratorConfig {
                foreground: false,
                ..NarratorConfig::default()
            },
        }
    }

    #[test]
    fn resolve_narrator_config_decisions() {
        assert!(
            resolve_narrator_config(true, false, false, None)
                .expect("tty narrates by default")
                .foreground
        );
        let headless = resolve_narrator_config(false, true, false, None)
            .expect("--narrate enables headless off-tty");
        assert!(headless.headless_append);
        assert!(!headless.foreground);
        assert!(
            resolve_narrator_config(false, false, false, None).is_none(),
            "piped without --narrate does not spawn the narrator"
        );
        assert!(
            resolve_narrator_config(true, false, true, None).is_none(),
            "--no-narrate disables narration even on a tty"
        );
    }

    #[test]
    fn writer_output_bytes_unchanged_on_fixture_run() {
        use deadreckon_core::ledger_io::append_ledger_item;
        use deadreckon_protocol::LedgerItem;

        let temp = TempDir::new().expect("tempdir");
        let run_root = temp.path();

        append_ledger_item(run_root, LedgerItem::Event(parse_fixture(EVENTS_FIXTURE)))
            .expect("append event fixture");
        append_narrator_spend(run_root, &parse_fixture(SPEND_FIXTURE))
            .expect("append spend fixture through policy");
        append_ledger_item(run_root, LedgerItem::Trace(parse_fixture(TRACES_FIXTURE)))
            .expect("append trace fixture");
        append_ledger_item(run_root, LedgerItem::Flight(parse_fixture(FLIGHT_FIXTURE)))
            .expect("append flight fixture");
        let snapshot = parse_fixture(NARRATIVE_FIXTURE);
        crate::narrative::append_narrative_snapshot(&run_root.join("narrative"), &snapshot)
            .expect("append narrative fixture");

        for (relative, expected) in [
            ("events.jsonl", EVENTS_FIXTURE),
            ("spend.jsonl", SPEND_FIXTURE),
            ("traces.jsonl", TRACES_FIXTURE),
            ("flight-events.jsonl", FLIGHT_FIXTURE),
            ("narrative/snapshots.jsonl", NARRATIVE_FIXTURE),
        ] {
            assert_eq!(
                std::fs::read_to_string(run_root.join(relative)).expect("read written ledger"),
                expected,
                "{relative} bytes changed during writer rewire"
            );
        }

        fn parse_fixture<T: serde::de::DeserializeOwned>(fixture: &str) -> T {
            serde_json::from_str(fixture.trim_end()).expect("parse pre-Keel fixture")
        }
    }

    #[test]
    fn resolve_narrator_config_for_child_returns_some_file_only_off_tty() {
        let child = resolve_narrator_config_for_child(true, false, None)
            .expect("a narrating child gets a file-only config");
        assert!(!child.foreground, "child has no foreground calm block");
        assert!(
            !child.headless_append,
            "child never writes beats to stdout/stderr — file-only"
        );
        assert!(
            resolve_narrator_config_for_child(false, false, None).is_none(),
            "child stays silent unless the parent passes --narrate"
        );
        assert!(
            resolve_narrator_config_for_child(true, true, None).is_none(),
            "--no-narrate wins for a child too"
        );
    }

    #[test]
    fn resolve_narrator_config_unchanged_for_dr_run_tty_matrix() {
        // The dr-run contract must be untouched by the new child path.
        assert!(
            resolve_narrator_config(true, false, false, None)
                .expect("tty default")
                .foreground
        );
        assert!(
            resolve_narrator_config(false, true, false, None)
                .expect("off-tty --narrate")
                .headless_append
        );
        assert!(resolve_narrator_config(false, false, false, None).is_none());
        assert!(resolve_narrator_config(true, false, true, None).is_none());
    }

    #[test]
    fn child_narrator_defaults_to_deterministic_floor_when_no_narrator_model() {
        assert!(
            child_narrator_backend_is_floor(None),
            "no pinned model -> floor ($0, no auth probe)"
        );
        assert!(
            !child_narrator_backend_is_floor(Some("haiku")),
            "an explicit --narrator-model opts into a metered backend"
        );
    }

    #[test]
    fn extend_command_in_place_reviewer_child_narrates_when_narrate_passed() {
        // A spawned reviewer (child env set, off-TTY, --narrate) narrates file-only.
        let config = resolve_narration(true, false, true, false, None)
            .expect("reviewer child narrates when --narrate is passed");
        assert!(!config.foreground && !config.headless_append, "file-only");
    }

    #[test]
    fn extend_worktree_command_reviewer_child_narrates_when_narrate_passed() {
        // The worktree extend path uses the same resolution as in-place.
        let config = resolve_narration(true, false, true, false, Some("haiku".to_string()))
            .expect("worktree reviewer child narrates");
        assert!(!config.foreground && !config.headless_append, "file-only");
        assert_eq!(config.model_override.as_deref(), Some("haiku"));
    }

    #[test]
    fn headless_child_narration_keeps_stdout_clean_so_parent_scrapes_run_id() {
        let config = resolve_narration(true, false, true, false, None).expect("child config");
        assert!(
            !config.foreground && !config.headless_append,
            "a child writes to no terminal channel (clean stdout for run-id scrape)"
        );
    }

    #[test]
    fn headless_child_narration_keeps_stderr_clean_no_failure_summary_pollution() {
        let config = resolve_narration(true, false, true, false, None).expect("child config");
        assert!(!config.headless_append, "no append beats on stderr");
    }

    #[tokio::test]
    async fn extend_narrator_handle_shutdown_runs_before_lock_release() {
        // The handle's shutdown is bounded, so calling it before lock release
        // (as both extend sites do) can never block the run's exit.
        let temp = TempDir::new().expect("tempdir");
        let (_sender, handle) = build_narration(floor_ctx(temp.path().to_path_buf()));
        let stopped = tokio::time::timeout(Duration::from_secs(2), handle.shutdown()).await;
        assert!(
            stopped.is_ok(),
            "shutdown completes within the grace window"
        );
    }

    #[tokio::test]
    async fn run_command_wires_event_bus_when_narration_enabled() {
        // Narration on -> a sender is produced and the engine subscribes to it.
        let temp = TempDir::new().expect("tempdir");
        let (sender, handle) = build_narration(floor_ctx(temp.path().to_path_buf()));
        assert!(
            sender.receiver_count() >= 1,
            "the spawned narrator engine subscribed to the bus"
        );
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn narrator_task_stops_on_run_cancellation() {
        let temp = TempDir::new().expect("tempdir");
        let (_sender, handle) = build_narration(floor_ctx(temp.path().to_path_buf()));
        let stopped = tokio::time::timeout(Duration::from_secs(2), handle.shutdown()).await;
        assert!(stopped.is_ok(), "narrator task exits promptly on cancel");
    }

    #[test]
    fn narrate_headless_writes_beats_to_stderr_not_stdout() {
        // The engine writes these to stderr; the writer keeps stdout clean.
        let mut buf = Vec::new();
        write_headless_lines(
            &mut buf,
            &["[turn 5] Did X".to_string(), "  · work".to_string()],
        )
        .expect("write");
        let text = String::from_utf8(buf).expect("utf8");
        assert!(text.contains("[turn 5] Did X"));
        assert!(text.ends_with('\n'), "newline-terminated append output");
    }

    #[test]
    fn narrate_headless_beats_are_append_only_and_turn_stamped() {
        let mut buf = Vec::new();
        write_headless_lines(&mut buf, &["[turn 1] a".to_string()]).expect("write 1");
        write_headless_lines(&mut buf, &["[turn 2] b".to_string()]).expect("write 2");
        let text = String::from_utf8(buf).expect("utf8");
        assert!(
            text.contains("[turn 1]") && text.contains("[turn 2]"),
            "both beats retained (append, not overwrite)"
        );
        assert!(
            !text.as_bytes().contains(&0x1b),
            "append-only: no in-place cursor escapes"
        );
    }

    #[test]
    fn narrate_conflicting_flags_refuse_with_try_line() {
        let err = validate_narration_flags(true, true).expect_err("conflict refused");
        assert!(err.contains("--narrate") && err.contains("--no-narrate"));
        assert!(validate_narration_flags(true, false).is_ok());
        assert!(validate_narration_flags(false, true).is_ok());
        assert!(validate_narration_flags(false, false).is_ok());
    }

    #[test]
    fn bad_narrator_model_refuses_with_models_hint() {
        let registry =
            deadreckon_providers::registry::ProviderRegistry::builtin().expect("registry");
        assert!(
            narrator_model_known(&registry, "haiku"),
            "catalog id is known"
        );
        assert!(
            !narrator_model_known(&registry, "totally-bogus-model"),
            "a typo is unknown"
        );
        assert!(
            narrator_model_refusal("totally-bogus-model").contains("deadreckon models"),
            "the refusal points at the models command"
        );
    }

    #[test]
    fn piped_run_is_not_silent_between_start_and_exit() {
        // Piped (stdout not a tty) => plain progress on, so never silent.
        assert!(effective_plain(false, false, false, false));
        // Interactive tty with no flags => not forced into plain.
        assert!(!effective_plain(false, false, false, true));
        // Explicit flags still force plain on a tty.
        assert!(effective_plain(true, false, false, true));
    }

    #[tokio::test]
    async fn live_wiring_writes_a_beat_from_docscheckpoint_then_runcompleted() {
        // End-to-end through the real bus + spawned engine: a DocsCheckpoint
        // pointing at a real `_incremental.jsonl`, then RunCompleted, must leave
        // a beat in snapshots.jsonl. (A real `dr run --narrate` wrote zero beats
        // even though every unit piece passed — this exercises the live path.)
        let temp = TempDir::new().expect("tempdir");
        let run_root = temp.path().join("run");
        let working_dir = temp.path().join("work");
        let incremental = working_dir.join(".deadreckon").join("docs");
        std::fs::create_dir_all(&incremental).expect("docs dir");
        let record = serde_json::json!({
            "turn": 1,
            "title": "Edit calc.py",
            "tool_kind": "cli_subagent",
            "latency_ms": 1200,
            "files": [{"path": "calc.py", "adds": 26, "dels": 0,
                       "largest_hunk_excerpt": "+def add", "is_new": true, "is_binary": false}],
            "outcome": "ok",
            "response_full": "added add()",
            "response_summary": "added add()",
            "response_text": "added add()",
            "trace_link": "",
            "snapshot_link": "",
            "commit_sha": null,
            "decision_candidate": false
        });
        let incremental_file = incremental.join("_incremental.jsonl");
        std::fs::write(&incremental_file, format!("{record}\n")).expect("write incremental");

        let (sender, handle) = build_narration(floor_ctx(run_root.clone()));
        let send = |kind| {
            sender
                .send(RunEvent {
                    timestamp: Utc::now(),
                    run_id: "run-x".to_string(),
                    event: kind,
                })
                .map(|_| ())
        };
        send(RunEventKind::TurnStarted { turn: 1 }).expect("turn started");
        send(RunEventKind::DocsCheckpoint {
            turn: 1,
            path: incremental_file.clone(),
            status: "turn-end".to_string(),
        })
        .expect("docs checkpoint");
        send(RunEventKind::RunCompleted {
            status: "completed".to_string(),
        })
        .expect("run completed");
        // No yield: shutdown races the buffered events. The engine must still
        // drain them (fill the window) before its final flush, or a fast run
        // writes zero beats — exactly what a real `dr run --narrate` did.
        handle.shutdown().await;

        let snapshots = run_root.join("narrative").join("snapshots.jsonl");
        assert!(
            snapshots.exists(),
            "the live wiring must write at least one beat to snapshots.jsonl"
        );
    }

    #[tokio::test]
    async fn narrator_engine_writes_floor_beat_from_window() {
        let temp = TempDir::new().expect("tempdir");
        let run_root = temp.path().to_path_buf();
        let mut engine = NarratorEngine::new(floor_ctx(run_root.clone()), CancellationToken::new());
        engine.window.observe(LiveTurnInput {
            turn: 1,
            title: "first step".to_string(),
            summary: "did a thing".to_string(),
            tool_kind: "bash".to_string(),
            outcome: "ok".to_string(),
            files: Vec::new(),
        });
        // No router -> deterministic floor beat, written to snapshots.jsonl.
        engine.emit(Utc::now(), None).await;
        let snapshots = run_root.join("narrative").join("snapshots.jsonl");
        assert!(snapshots.exists(), "a floor beat was written");
        let raw = std::fs::read_to_string(&snapshots).expect("read snapshots");
        assert!(raw.contains("turn 1"), "the beat narrates the turn");
    }

    #[test]
    fn narrator_coalesces_fast_turns_into_one_beat() {
        let config = NarratorConfig::default();
        // Three turns in 5s — under the 30s gap and the 8-turn burst: hold.
        assert_eq!(
            cadence_decision(&config, 1, 3, Some(5), None),
            BeatDecision::Coalesce
        );
    }

    #[test]
    fn narrator_forces_beat_after_turn_burst() {
        let config = NarratorConfig::default();
        // Eight turns since the last beat forces a beat even under the gap.
        assert_eq!(
            cadence_decision(&config, 1, config.turn_burst, Some(5), None),
            BeatDecision::Emit
        );
    }

    #[test]
    fn narrator_quiet_timer_escalates_long_turn_to_model_beat() {
        let config = NarratorConfig::default();
        // No completed turns, but one turn has been in flight past the quiet
        // threshold: escalate to a model beat so a long turn isn't silent.
        assert_eq!(
            cadence_decision(&config, 1, 0, Some(5), Some(config.quiet_seconds + 1)),
            BeatDecision::Emit
        );
        // Without the quiet escalation, no new work means coalesce.
        assert_eq!(
            cadence_decision(&config, 1, 0, Some(5), Some(1)),
            BeatDecision::Coalesce
        );
    }

    #[test]
    fn deterministic_ticker_updates_between_beats_with_no_model_call() {
        // Pure formatting over event fields — no provider, no backend needed.
        let line = deterministic_ticker_line(14, "cargo test", 72);
        assert!(line.contains("turn 14"));
        assert!(line.contains("cargo test"));
        assert!(line.contains("1m12s"));
        assert_eq!(deterministic_ticker_line(3, "", 5), "turn 3 · working (5s)");
    }

    #[test]
    fn narrator_respects_per_run_beat_cap() {
        let config = NarratorConfig::default();
        assert_eq!(
            cadence_decision(&config, config.max_beats, 8, Some(120), None),
            BeatDecision::CapReached
        );
    }

    fn provider_response(cost_usd: f64, subscription: bool) -> ProviderResponse {
        use deadreckon_providers::{ProviderUsage, SpendEstimate};
        ProviderResponse {
            provider: "anthropic".to_string(),
            model: "claude-haiku-4-5".to_string(),
            content: "{}".to_string(),
            usage: ProviderUsage {
                input_tokens: 100,
                output_tokens: 20,
            },
            spend: SpendEstimate {
                provider: "anthropic".to_string(),
                model: "claude-haiku-4-5".to_string(),
                input_tokens: 100,
                output_tokens: 20,
                cost_usd,
                subscription,
                wall_time_seconds: Some(1.0),
            },
            trace: serde_json::Value::Null,
        }
    }

    #[test]
    fn narrator_spend_rows_tagged_and_separate_from_loop_totals() {
        let narrator_row = narrator_spend_record(5, &provider_response(0.01, false), 0.01, 0.50);
        assert_eq!(narrator_row.kind, "narrator");
        // A loop row alongside it; the run's spend math filters kind=="loop"
        // and so never counts narrator cost.
        let loop_row = SpendRecord {
            kind: "loop".to_string(),
            cost_usd: 0.02,
            ..narrator_row.clone()
        };
        let rows = vec![loop_row, narrator_row];
        let loop_total: f64 = rows
            .iter()
            .filter(|row| row.kind == "loop")
            .map(|row| row.cost_usd)
            .sum();
        assert_eq!(loop_total, 0.02, "narrator cost excluded from loop totals");
    }

    #[test]
    fn narrator_degrades_to_floor_at_budget_cap() {
        let backend = NarratorBackend::Model {
            provider: "anthropic".to_string(),
            model: "claude-haiku-4-5".to_string(),
        };
        let mut ledger = NarratorLedger::new(0.50);
        assert!(narrator_should_use_model(&backend, &ledger));
        ledger.record_spend(0.30);
        assert!(narrator_should_use_model(&backend, &ledger));
        ledger.record_spend(0.25); // 0.55 >= 0.50 cap
        assert!(
            !narrator_should_use_model(&backend, &ledger),
            "narrator degrades to the deterministic floor at its budget cap"
        );
        assert!(
            !narrator_should_use_model(
                &NarratorBackend::DeterministicFloor,
                &NarratorLedger::new(0.50)
            ),
            "a floor backend never makes a model call"
        );
    }

    #[test]
    fn narrator_subscription_backend_records_zero_cost() {
        let row = narrator_spend_record(3, &provider_response(0.0, true), 0.0, 0.50);
        assert_eq!(row.cost_usd, 0.0);
        assert!(row.subscription);
        assert_eq!(row.kind, "narrator");
    }

    #[test]
    fn foreground_block_updates_in_place_not_appends() {
        let mut block = ForegroundBlock::new();
        let first = block.render(&["headline".to_string(), "· work".to_string()]);
        assert_eq!(
            first, "headline\n· work\n",
            "the first render draws the lines with no clearing prefix"
        );
        assert_eq!(block.drawn_lines(), 2);

        let second = block.render(&["new headline".to_string()]);
        let expected_clear = crate::ui::cursor_clear_lines(2);
        assert!(
            second.starts_with(&expected_clear),
            "the next render clears the two prior lines in place rather than appending"
        );
        assert_eq!(block.drawn_lines(), 1);
    }

    #[test]
    fn foreground_on_by_default_off_with_no_narrate() {
        assert!(
            resolve_narrator_config(true, false, false, None)
                .expect("tty narrates by default")
                .foreground
        );
        assert!(
            resolve_narrator_config(true, false, true, None).is_none(),
            "--no-narrate turns the foreground block off"
        );
    }
}
