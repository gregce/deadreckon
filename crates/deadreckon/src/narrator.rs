//! The live narrator sidecar: an in-process task the run spawns to narrate
//! progress in plain English while the turn loop works.
//!
//! P3 establishes the plumbing — resolve whether narration is on, build a
//! [`RunEventBus`] whose sender feeds `RunLoopConfig.event_sender`, and spawn a
//! cancellable task that drains run events. Continuity, cadence, provider
//! calls, and rendering land in later phases; here the task is a clean drain
//! that stops the instant the run finishes (cancellation) or the bus closes.

use deadreckon_core::{RunEvent, RunEventBus};
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
}
