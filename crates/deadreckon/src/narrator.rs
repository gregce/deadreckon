//! The live narrator sidecar: an in-process task the run spawns to narrate
//! progress in plain English while the turn loop works.
//!
//! P3 establishes the plumbing — resolve whether narration is on, build a
//! [`RunEventBus`] whose sender feeds `RunLoopConfig.event_sender`, and spawn a
//! cancellable task that drains run events. Continuity, cadence, provider
//! calls, and rendering land in later phases; here the task is a clean drain
//! that stops the instant the run finishes (cancellation) or the bus closes.

use chrono::Utc;
use deadreckon_core::{RunEvent, RunEventBus, SpendRecord};
use deadreckon_providers::{NarratorBackend, ProviderResponse};
use deadreckon_runtime::NarratorConfig;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

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

/// A spawned narrator task plus the token that stops it.
pub(crate) struct NarratorHandle {
    cancel: CancellationToken,
    join: JoinHandle<()>,
}

impl NarratorHandle {
    /// Stop the narrator and wait for it to drain. Called after the run loop
    /// returns so the final state is flushed and the task exits cleanly.
    pub(crate) async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.join.await;
    }
}

/// Build the narration wiring for an (optional) config. When narration is on,
/// returns the broadcast sender to hand to `RunLoopConfig.event_sender` and a
/// handle to the spawned task; when off, returns `(None, None)` so the run is
/// wired exactly as before.
pub(crate) fn build_narration(
    config: Option<NarratorConfig>,
) -> (Option<broadcast::Sender<RunEvent>>, Option<NarratorHandle>) {
    let Some(_config) = config else {
        return (None, None);
    };
    let bus = RunEventBus::new(NARRATOR_BUS_CAPACITY);
    let receiver = bus.subscribe();
    let sender = bus.sender();
    let cancel = CancellationToken::new();
    let join = spawn_narrator(receiver, cancel.clone());
    (Some(sender), Some(NarratorHandle { cancel, join }))
}

const NARRATOR_BUS_CAPACITY: usize = 256;

/// Spawn the narrator loop. P3: drain events until cancelled or the bus closes,
/// tolerating lag (a slow narrator must never block or crash the run).
pub(crate) fn spawn_narrator(
    mut receiver: broadcast::Receiver<RunEvent>,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                received = receiver.recv() => match received {
                    Ok(_event) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                },
            }
        }
    })
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

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

    #[tokio::test]
    async fn run_command_wires_event_bus_when_narration_enabled() {
        // Narration on -> a sender is produced (and a task subscribed to it).
        let config = resolve_narrator_config(true, false, false, None);
        let (sender, handle) = build_narration(config);
        let sender = sender.expect("narration on yields an event sender");
        assert!(
            sender.receiver_count() >= 1,
            "the spawned narrator subscribed to the bus"
        );
        handle.expect("handle present").shutdown().await;

        // Narration off -> wired exactly as before (no sender, no task).
        let (sender_off, handle_off) = build_narration(None);
        assert!(sender_off.is_none());
        assert!(handle_off.is_none());
    }

    #[tokio::test]
    async fn narrator_task_stops_on_run_cancellation() {
        let bus = RunEventBus::new(8);
        let cancel = CancellationToken::new();
        let join = spawn_narrator(bus.subscribe(), cancel.clone());
        cancel.cancel();
        let stopped = tokio::time::timeout(Duration::from_secs(1), join).await;
        assert!(stopped.is_ok(), "narrator task exits promptly on cancel");
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
}
