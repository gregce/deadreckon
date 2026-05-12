GOAL: Extend deadreckon at `/Users/gdc/deadreckon/` from a single-shot harness into an **autonomous chain runner**. Today `run` nails one goal; chaining makes hours-long unattended work safe. The shape mirrors `run`: `chain "g1" "g2" "g3"` (explicit) or `chain plan "build a chess app" --n 6` (provider-decomposed) validates → previews → confirms → runs end-to-end in one command. A conductor spawns each step's `run`, watches the gate, auto-applies on green, advances N+1 onto N's HEAD. Headline: **Autonomy**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` (§22 thin).
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-deadreckon-autonomous-chain-rider.md` — schemas, verbs, depth tests, hooks, TUI.
- `/Users/gdc/Downloads/{AS-BUILT-ARCHITECTURE,printing-press-agentic-techniques}.md`.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes — chain state in files under `~/.deadreckon/chains/<id>/`. No `git push`. Edits inside `/Users/gdc/deadreckon/`. Conductor is a CLI verb. V1 → `docs/V1-CANDIDATES.md`.

**Core idea.** A *chain* is an ordered list of step goals with branch/apply/stop policy. A *conductor* (entered by `chain "..."` or `chain run <id>`) acquires a chain lock, spawns step 1 as a normal `deadreckon run`, watches its `state.json` + `events.jsonl`, auto-applies on `RunCompleted{completed}` per policy, derives step 2's base from the just-applied HEAD, continues. Inner runs hold task-key locks; conductor holds the outer chain lock. Resume is idempotent.

**New verbs (rider has full signatures + refusal cases).**

- `chain "g1" "g2" "g3"` / `--from-file` / `--from-stdin` — create + preview + confirm + run + auto-attach; `--draft` writes only.
- `chain plan <goal>` (alias `expand`) — provider decomposes; same one-shot flow.
- `chain` no-args → status; `chain run` no-args → resume latest paused; `latest` accepted as id.
- `chain {run|attach|status|show|list|pause|resume|kill|undo|extend|redo} <id>` — `extend` appends; `redo --step N [--extend "..."]`; kill cascades SIGTERM→SIGKILL(2s); undo `git revert`s in reverse; attach is a step-timeline TUI.

**Joy as a verifiable contract** (rider names ten; depth-tested).

- One-command create+run mirrors `run`; `latest` + bare-verb defaults erase id-ceremony.
- `chain attach` is a step-timeline TUI; single-run `attach` surfaces "step N/M of chain <id>" via `chain-step.json`.
- Auto-apply lands iff gate passed AND rebase conflict-free AND files ⊆ allowlist; else preview with `try:`.
- Aggregate `--max-spend` is chain-level; per-step defaults to remaining/remaining_steps; resume inherits, never resets.
- One-command rollback/pause/resume/redo/extend; cascade kill <5s; every error ends `try:`.
- Hooks at `~/.deadreckon/hooks/chain/{pre-step,post-step,on-promote,on-chain-end}` carry policy without prompt-engineering.

**Thinness partly closed.** §22 #9 (sequential half — parallel stays orchestrate-rider). Rest of §22 stays honestly.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `cargo build --release && cargo test --workspace && cargo clippy -- -D warnings && cargo fmt --check` green → commit → CHANGELOG line. P11 adds §28 Chains to AS-BUILT, updates §22.

**Verification.**

- Commands green every commit; every rider depth test present + passing.
- Smoke: `chain --yes "tiny hello" "add goodbye" "wire them"` runs three sequential worktree steps in one command; each gate-passes + auto-applies; `chain show latest` shows three green dots; aggregate spend ≤ cap.
- `kill latest` cascades <5s; `pause`/`resume latest` round-trips; `redo latest --step 2 --extend "make idempotent"` re-runs step 2; `undo latest` reverts in reverse.
- No edits outside `/Users/gdc/deadreckon/`. No `git push`. No schema changes.

**Stop when** verification passes, AS-BUILT updated, CHANGELOG has "Autonomous chaining (alpha)", committed.
