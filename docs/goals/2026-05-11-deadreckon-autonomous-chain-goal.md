GOAL: Extend deadreckon at `/Users/gdc/deadreckon/` from a single-shot harness into an **autonomous chain runner**. Today `run` nails one goal; chaining makes hours-long unattended work safe. The user gives an ordered chain — explicit (`chain "g1" "g2" "g3"`) or provider-decomposed (`chain plan "build a chess app" --n 6`) — and one foreground conductor drives it: spawn a `run`, watch the gate, auto-apply on green, advance N+1 onto N's just-applied HEAD. Headline: **Autonomy**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — substrate; §22 thin.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-deadreckon-autonomous-chain-rider.md` — schemas, signatures, depth tests, hooks.
- `/Users/gdc/Downloads/AS-BUILT-ARCHITECTURE.md` + `/Users/gdc/Downloads/printing-press-agentic-techniques.md` — replan / bounded fix-loops / anti-self-attestation / five-failure framing.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes — chain state in files under `~/.deadreckon/chains/<chain-id>/`. No `git push`. Edits inside `/Users/gdc/deadreckon/`. Conductor is a foreground CLI verb, not a daemon or new binary. V1 → `docs/V1-CANDIDATES.md`.

**Core idea.** A *chain* is an ordered list of step goals with declared branch/apply/stop policy. A *conductor* (`chain run`) acquires a chain lock, spawns step 1 as a normal `deadreckon run`, watches its `state.json` + `events.jsonl`, auto-applies on `RunCompleted{completed}` per policy, derives step 2's base from the just-applied HEAD, continues. Inner runs hold their own task-key locks; conductor holds the outer chain lock. Resume is idempotent — read `chain.json`, find the next non-completed step, re-enter.

**New verbs (rider has full signatures + refusal cases).**

- `chain plan <goal> [--n N]` — provider decomposes; writes draft `chain.json`.
- `chain "g1" "g2" "g3"` / `--from-file <path>` — explicit creation.
- `chain run|attach|status|show|list|pause|resume|kill|undo <id>` — start/inspect/control; kill cascades SIGTERM→SIGKILL(2s); undo `git revert`s applied steps in reverse; `show --why-failed` aggregates over steps; attach is a vertical step-timeline TUI, Tab pages.

**Joy as a verifiable contract** (rider names ten; depth-tested).

- `--preview` prints per-step provider/model/mode/branch/base + aggregate cap + DAG before commit.
- Auto-apply lands a step iff gate passed AND `git rebase` is conflict-free AND files-touched ⊆ allowlist; else preview with `try:`.
- Aggregate `--max-spend` is chain-level; per-step defaults to remaining/remaining_steps; resume inherits, never resets.
- One-command rollback/pause/resume; cascade kill <5s; every error ends `try:`.
- Hooks at `~/.deadreckon/hooks/chain/{pre-step,post-step,on-promote,on-chain-end}` (Claude Code shape) carry policy without prompt-engineering.

**Thinness partly closed.** §22 #9 multi-run coordination (sequential half — parallel stays in orchestrate-rider's scope). Rest of §22 stays honestly.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check` green → conventional-commit → CHANGELOG line. P11 adds §28 Chains to AS-BUILT, updates §22.

**Verification.**

- Commands above green every commit; every rider depth test present + passing.
- Smoke: `chain "tiny hello" "add goodbye" "wire them" && chain run <id>` runs three sequential worktree steps, each gate-passes + auto-applies; `chain show <id>` shows three green dots; aggregate spend ≤ cap.
- Mid-chain `kill <id>` cascades <5s; `pause`/`resume` round-trips; `undo <id>` reverts step 3, 2, 1 with undo trace.
- No edits outside `/Users/gdc/deadreckon/`. No `git push`. No schema changes.

**Stop when** verification passes, AS-BUILT updated, CHANGELOG has "Autonomous chaining (alpha)", committed locally.
