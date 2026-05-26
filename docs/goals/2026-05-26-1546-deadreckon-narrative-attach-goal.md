GOAL: Add a human-readable narrative attach mode so operators can understand one run or many orchestrated agents without reading raw logs, diffs, or JSONL. Today `attach` is strong for debugging, but multi-agent work can feel like machinery. Land a cited narrative plus visual architecture map that explains current work, system evolution, blockers, risks, and next steps while keeping raw activity one key away. Headline word: **Narrated**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`.
- `/Users/gdc/deadreckon/docs/goals/2026-05-26-1546-deadreckon-narrative-attach-rider.md` - full contract.
- `/Users/gdc/deadreckon/docs/goals/2026-05-25-2238-deadreckon-provider-flight-recorder-rider.md`.
- `/Users/gdc/deadreckon/docs/goals/2026-05-18-2226-deadreckon-orchestration-eventbus-rider.md`.
- Current code: `main.rs`, `plan_event_bus.rs`, core flight, docs polish.

**Posture.** Stays `alpha`. No `PipelineState` schema changes. Narrative and maps are projections over events, flight, traces, docs, and plan state, not sources of truth. Do not write prose into `flight-events.jsonl`, provider activity, or `plan-events.jsonl`. No `git push`. Edits stay inside `/Users/gdc/deadreckon/`. V1 decisions go to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**User contract.**

- `deadreckon attach <id> --view narrative` opens a calmer operator view for a run, plan, or plan child.
- The main pane shows prose; the side pane shows an evidence-backed architecture, agent, file, or evidence map when width allows.
- `--view activity` preserves raw activity; `--view split` may show narrative plus raw evidence or map. Keys: `n` toggles narrative/activity, `v` cycles visuals, `r` refreshes.
- Off-TTY/plain attach prints the latest snapshot with citations and staleness. JSON output is structured.
- Staleness is explicit: if the summarizer is unavailable, behind, over budget, or refused for privacy, the run continues and the UI shows deterministic facts.

**Narrative agent.**

- Add a bounded summarizer sidecar that builds evidence windows, redacts sensitive material, then asks a provider for structured claims only when cadence and budget allow.
- Cadence is event-driven, not constant streaming: refresh on meaningful deltas, blockers, test pass/fail, agent handoff, plan child completion, quiet threshold, or manual `r`.
- Every claim cites immutable evidence: run event, trace id, flight event, checkpoint, file path, plan event, task id, or child run id.
- The architecture map is built deterministically from changed files, symbols, tasks, deps, and citations; LLMs may label nodes but not invent them.

**Visual posture.**

- Reuse existing `ratatui`, `crossterm`, `pulldown-cmark`, `tokio`, `ui.rs`, and `PlanEventBus`.
- Use semantic color, focused borders, compact badges, connectors, progress rails, and tasteful status flourishes from the existing palette.
- Keep ASCII fallback, `NO_COLOR`, and non-color labels. Avoid graph crates unless the rider depth tests prove they are needed.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> focused verification green -> conventional local commit -> CHANGELOG. P11 updates AS-BUILT, docs, and V1-CANDIDATES with limits.

**Verification.**

- Focused tests only by default: schemas, evidence windows, architecture graph, redaction, cadence, fallback, fake provider, run/plan TUI render, plain/json output, citation validation.
- Smokes: fake CLI provider with flight events; fake two-child plan; completed run with `RUN-NARRATIVE.md`; summarizer failure. Each must show a useful narrative.
- Do not run `make verify`, release builds, stress tests, full-workspace tests, or broad smoke suites by default unless the human explicitly asks.

**Stop when** focused verification passes, narrative attach works for live/completed runs and plan attach, raw activity remains accessible, AS-BUILT and CHANGELOG record alpha limits, and the work is committed locally.
