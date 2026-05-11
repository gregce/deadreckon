GOAL: Extend deadreckon at `/Users/gdc/deadreckon/` from a single-agent harness into a multi-agent orchestrator. Decompose one goal into parallel sub-goals, drive each on the existing turn-loop substrate, merge their results through the gate. Land the ergonomics that make the multi-agent view usable; close the §22 thinness it exposes.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — substrate to extend (turn loop §9, gate §13, promotion §8, locks §7, scopes §23); §22 names what's thin.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-deadreckon-orchestrate-rider.md` — schemas, signatures, phases, named depth tests.
- `/Users/gdc/stoa/docs/research/2026-05-10-unmet-needs/REPORT.md` — primary needs #2, #6; incidental #1 #5 #7 #8.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants still hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes — plan + child lineage are files under the working tree (per `usability-rider`). No `git push`. Edits stay inside `/Users/gdc/deadreckon/`. If a phase reveals a V1-architecture decision, log it in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` and continue.

**Core idea.** A *plan* = one goal decomposed into N sub-goals. Each becomes a *child run* under the normal turn loop, in its own sub-scope, with its own working dir, lock, sandbox, provider, gate. A *coordinator* owns the plan, supervises children, exposes a multi-pane attach view, and on completion atomically merges children's `library/` outputs into a merge working dir that goes through the gate again before promotion. A child is a normal `deadreckon run` plus a `parent_plan_id` in its `.deadreckon/parent.json`.

**New verbs (rider has full signatures + refusal cases).**

- `plan <goal>` — provider decomposes into N sub-goals; writes `plan.json`.
- `fork <plan-id>` — spawns one child per sub-goal; prints attach/merge/kill hints.
- `attach <plan-id>` — multi-pane TUI; Enter drills, Esc returns, Ctrl-D detaches.
- `merge <plan-id>` — composes children's `library/`, gates the merged result, promotes.
- `kill <plan-id>` — cascades SIGTERM → SIGKILL(2s) to every child + coordinator.
- `history grep <pattern>` — over `library/**/traces.jsonl`; `--plan` scopes to one plan.
- `show <id> --why-failed` — RCA over failure-adjacent traces. Works for runs and plans.

**Ergonomics the multi-agent view requires.**

- TUI streaming via `RunEventBus` (closes thin #1; polling cannot drive N panes).
- Cross-process cancellation that's actually crisp (closes thin #3).
- Every error message ends with `try: <command>`.
- `--quiet` / `--plain` on `run`/`fork`/`attach`/`merge` for headless CI.
- Post-action hints (`fork` → attach/merge/kill; `merge` → materialize).

**Thinness closed.** §22 #1, #3, #9, #10. The rest stay in §22 honestly — explicitly out of scope.

**Phases.** Eleven (P1–P11) in the rider. Each phase: depth test first → implementation → `cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check` green → conventional-commit local commit → one-line `CHANGELOG.md` entry. P11 adds `## 18. Plans & Multi-Agent Orchestration` to AS-BUILT and updates §22.

**Verification.**

- Commands above green on every commit; every rider-named depth test present and passing (grep-count enforced).
- Multi-agent smoke: `plan "tiny hello rust in two files" --n 2 && fork <plan-id> && attach <plan-id>` shows two panes; children complete; `merge <plan-id>` produces a merged library entry; `materialize <merged-id> --dest ./hello` writes both files; mid-run `kill <plan-id>` cascade-kills in <5s.
- Single-agent smoke: `run "tiny hello rust" --plain --quiet` unchanged.
- `history grep hello --plan <plan-id>` finds child traces; `show <plan-id> --why-failed` meaningful on both success and failure.
- AS-BUILT updated. No edits outside `/Users/gdc/deadreckon/`. No `git push`.

**Stop when** verification passes, AS-BUILT updated, CHANGELOG has an "Orchestration milestone (alpha)" section, committed locally, no invariant violated.
