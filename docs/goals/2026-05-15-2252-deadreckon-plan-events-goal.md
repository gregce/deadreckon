GOAL: Make orchestration plans observable like runs and chains. Today `run` writes `events.jsonl`, chains write `chain-events.jsonl`, and plan children are normal runs, but the plan object itself only has `plan.json`, `messages.jsonl`, child run pointers, and summaries. Land a first-class plan event stream plus attach navigation parity so a user can attach to an orchestrator plan, drill into a child run's normal run view, and return to the exact plan detail context. Headline word: **Observable**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - §§14, 18, 30: run events, attach data source, orchestration limits.
- `/Users/gdc/deadreckon/docs/goals/2026-05-15-2252-deadreckon-plan-events-rider.md` - schemas, navigation contract, depth tests.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-1444-deadreckon-orchestrate-rider.md` - plan model, coordinator, child runs.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` - invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes. Plan observability state lives in files under `~/.deadreckon/plans/<plan-id>/`. Child runs keep their existing `events.jsonl`; chain behavior is unchanged. No `git push`. Edits inside `/Users/gdc/deadreckon/`. V1 decisions -> `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**Core idea.**

- Add `plan-events.jsonl` as the orchestration-level timeline: plan created/started, task ready/started/run-discovered/completed/blocked/failed/killed, merge started/conflicted/completed, plan completed/failed/killed.
- Keep `messages.jsonl` as typed coordinator communication; do not overload it as the only activity feed.
- Teach `attach <plan-id>` to tail plan events and still inspect child run files for detail.
- Formalize navigation: plan overview -> selected child run detail -> back to the same plan selection/scroll/detail state.
- Preserve the existing child run view. A child remains a normal `deadreckon run` or `deadreckon extend`; drill-down reuses run attach rendering instead of inventing a second run UI.

**User experience.**

- `deadreckon attach <plan-id>` shows live plan activity from `plan-events.jsonl`.
- `Enter` on a child task opens that child's run detail.
- `Esc` / back returns to the same plan view, same selected task, same scroll offsets.
- Non-TTY / `--plain` plan summaries print explicit child commands: `deadreckon attach <child-run-id>`, `deadreckon show <child-run-id>`, and `deadreckon attach <plan-id>`.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> `cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check` green when practical, focused verification for tight phases -> conventional-commit -> CHANGELOG. P11 adds AS-BUILT §32 and updates §22/§30.

**Verification.**

- Every rider depth test present and passing.
- Plan smoke: `plan "tiny hello rust" --mode review && fork <plan-id> && attach <plan-id> --plain` shows plan events and child attach commands.
- TUI smoke/test: plan attach opens a child run view and returns to the same plan selection.
- No edits outside `/Users/gdc/deadreckon/`. No `git push`. No `PipelineState` schema changes.

**Stop when** verification passes, AS-BUILT updated, CHANGELOG has a "Plan observability (alpha)" section, and work is committed locally.
