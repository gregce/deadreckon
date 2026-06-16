GOAL: Make a `dr run` narrate itself in plain English as it works, so an operator can glance at what the model is doing instead of reading tool calls, edits, and JSONL. Today narration exists only as the attach-time Narrative view: deterministic-first, the optional model pass only relabels claims and overwrites with no memory, and the running process emits nothing — a piped run is silent. Promote live narration to a first-class, continuity-carrying, model-driven sidecar that runs DURING run/orchestrate/campaign, writes one rolling story, and renders it everywhere. Land this slice named Live Narrator.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-06-15-1433-deadreckon-live-narrator-rider.md` — phases, schemas, depth tests, citations.
- `/Users/gdc/deadreckon/crates/deadreckon/src/narrative.rs` — projection, render, prompt, cadence.
- `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/turn_loop.rs` — `RunLoopConfig`, `append_turn_doc_checkpoint`, the EventSink/`complete_run_docs` precedents.
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/{events.rs,docs.rs}` — `RunEventBus`, `TurnRecord`.
- `/Users/gdc/deadreckon/crates/deadreckon-providers/src/{router.rs,auth_probe.rs}` — `selected_route_info`, auth detection.
- `/Users/gdc/deadreckon/docs/{AS-BUILT-ARCHITECTURE.md,V1-CANDIDATES.md}`. Prior narrative-attach invariants hold.

**Posture.** Stable track (0.1.1 shipped). No `PipelineState`/`RunLoopConfig` schema breakage — additive fields/config only; rolling state lives in files under `<run>/narrative/`. No `git push`. Edits inside `/Users/gdc/deadreckon`. The deterministic projection stays the floor: narration MUST stay useful with no provider call; the narrator is a projection and never mutates `flight/plan-events.jsonl`. Major decisions → V1-CANDIDATES.

**One rolling story, written live, rendered everywhere.** A narrator tokio task spawned by the run subscribes to the `RunEventBus`, reacts to per-turn `DocsCheckpoint`, reads the rich `TurnRecord`, and amends `<run>/narrative/snapshots.jsonl`. Attach renders what the run already wrote; interactive and headless converge on one document.

**Continuity.** Each beat feeds the model its PRIOR narrative + the NEW windowed turn(s) + a carried rolling summary, and AMENDS/EXTENDS — never regenerates (cost O(turns)). Every beat cites a real turn id; the narrator may add genuine beats, not only relabel.

**Backend, auto and subscription-first.** Builds its OWN cheap-model router. Preference (rider): claude-code/haiku → codex/mini → anthropic/haiku → openai/4o-mini → deterministic floor. Its own budget cap; spend recorded to `spend.jsonl` with a narrator label, never racing the loop's totals.

**Cadence.** Time-gated + coalesced: a minimum gap, bursts coalesced, a per-run beat cap; a long turn still gets a beat via the quiet timer.

**Surfaces.** Foreground of run/orchestrate/campaign: ON by default, a CALM bounded block of a few lines max, not a scrolling stream. Piped/headless: opt-in `--narrate` → append-only turn-stamped beats to STDERR, stdout clean; fix the silent-piped-run TTY gap.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → fmt+clippy+focused test green → conventional-commit → CHANGELOG line. P11 adds AS-BUILT §44.

**Verification.**

- Every rider depth test present and passing; `cargo test --workspace --locked` green.
- A multi-turn run writes `<run>/narrative/snapshots.jsonl` whose successive beats EXTEND rather than replace, each citing a real turn id; with no provider, the run still narrates via the deterministic floor.
- `dr run --narrate` piped to a file yields append-only turn-stamped beats on stderr; stdout stays clean. `cargo fmt --check` and `git diff --check` clean. No `git push`. No schema breakage.

**Stop when** verification passes, AS-BUILT §44 / V1-CANDIDATES / a `0.2.0 — Live Narrator` CHANGELOG section are updated, and all phases are committed locally.
