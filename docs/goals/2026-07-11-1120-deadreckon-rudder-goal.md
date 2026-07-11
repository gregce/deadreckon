GOAL: Give the operator a rudder — steer a running codex child instead of watch-or-kill. Today deadreckon's only mid-run verbs are attach (observe) and kill (abandon); the one thing operators ask for while watching a run drift is the one thing the harness cannot do. Codex ships the mechanism: `codex app-server` is a long-lived JSON-RPC daemon (the designed embedding API — exec itself is a client of it) exposing `thread/start|resume|fork`, `turn/start`, **`turn/steer`** (inject operator input into the active turn, guarded by an `expected_turn_id` precondition), `turn/interrupt`, streaming delta notifications, and server→client approval requests (`item/commandExecution/requestApproval`) that the embedder answers programmatically. This slice lands a `cli:codex-server` provider route over a supervised app-server connection, a durable steer inbox with a `deadreckon steer` verb, Helm's `:steer` command, and approval requests answered by deadreckon's own capability policy — closing the network=Deny-vs-npm-install incoherence class at the source. Land this slice named Rudder.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-07-11-1120-deadreckon-rudder-rider.md` — connection model, steer inbox, approval mapping, eleven phases, depth tests.
- `/Users/gdc/deadreckon/docs/goals/2026-07-11-1119-deadreckon-semaphore-rider.md` — Semaphore lands first; Rudder reuses its session file and event-tolerance doctrine.
- `crates/deadreckon-providers/src/{cli_codex.rs,router.rs,types.rs}`; `crates/deadreckon-sandbox` (capability posture to answer approvals from).
- `/Users/gdc/codex/codex-rs/app-server-protocol/src/protocol/common.rs`, `v2/turn.rs`, `app-server/tests/suite/v2/turn_steer.rs`.
- `docs/AS-BUILT-ARCHITECTURE.md` §47 (Helm command mode), §50 (Semaphore). Prior riders hold; Rudder takes §51.

**Posture.** Stable track. `cli:codex-server` is a NEW opt-in route; `cli:codex` (exec) remains the default and the fallback — a dead server degrades to the exec path mid-run with a caveat, never a failed turn. One app-server process per deadreckon run (supervised child over stdio; the shared unix-socket daemon is V1). Steering is durable-first: `steer-inbox.jsonl` in the run root is the source of truth; the TUI and CLI both write it; delivery to codex is at-least-once with delivered markers. Approvals map from deadreckon's existing capability posture — no new policy language. No `PipelineState` schema changes. No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → V1-CANDIDATES.

**The verbs.**

- `deadreckon steer <run-id> "<text>"` — append to the steer inbox; refuses (with `try:`) when the run is not live or its provider route cannot steer.
- Helm `:steer <text>` on a run surface — same inbox, same code path.
- `deadreckon kill` on a server-routed run maps to `turn/interrupt` before process kill (graceful, then hard).

**Approvals.** Run codex under its own sandbox with `approval_policy: on-request`; deadreckon answers `requestApproval` from the run's capability posture (network allowlist, install/deploy flags) and records every decision as a trace — replacing the blanket `danger-full-access` inversion.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `make verify` green → conventional-commit → CHANGELOG. P11 adds AS-BUILT §51.

**Verification.**

- Every rider depth test present and passing; `make verify` green each commit (fake app-server fixture; no live codex in CI).
- Steer round-trip: inbox line → `turn/steer` with correct `expected_turn_id` → delivered marker → visible in attach.
- Approval round-trip: a denied-capability command yields a recorded deny; an allowed one proceeds.
- Server death mid-turn degrades to the exec route with a caveat trace; the turn completes.

**Stop when** verification passes, AS-BUILT §51 + V1-CANDIDATES + a `Rudder (stable)` CHANGELOG section are updated, committed locally.
