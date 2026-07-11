# deadreckon — Rudder Rider (steer the running child)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-07-11-1120-deadreckon-rudder-goal.md`.
It supersedes nothing in prior riders — their invariants still apply, and the
Semaphore rider (`2026-07-11-1119-…-semaphore-rider.md`) is a prerequisite:
Rudder reuses its `provider-session.json`, capability-probe pattern, and
tolerant-parse doctrine. This rider adds: a **`cli:codex-server` provider
route** over a supervised `codex app-server` child, a **durable steer inbox**
+ `deadreckon steer` verb + Helm `:steer`, **interrupt-before-kill**, and
**capability-mapped approval answering**.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `~/.deadreckon`.
Codex reference `/Users/gdc/codex/codex-rs` (grounding only; wire mirrored,
never linked).

## Posture (decided — do not redesign)

- **Maturity stays stable** (lands under a `Rudder` CHANGELOG section).
- **Opt-in route.** `cli:codex-server` is selected explicitly (provider flag or config); `cli:codex` stays the default. Course's planner may not auto-select it in this slice.
- **One server per run, supervised child, stdio transport.** deadreckon spawns `codex app-server` as a child of the run (pid recorded like any supervised pid) and speaks the JSON-RPC framing over stdin/stdout. `unix://`/`ws://`, the shared `app-server-daemon`, and cross-run server reuse are V1.
- **Degrade, never strand.** If the server dies or a method errors structurally, the run's next turn falls back to the exec route (Semaphore driver) with a `provider.route.degraded` trace. Steering becomes unavailable (inbox lines stay pending, surfaced as attention) but the run continues.
- **Steering is durable-first.** `steer-inbox.jsonl` is the source of truth; delivery is at-least-once with `delivered_turn_id` markers; codex's `expected_turn_id` precondition makes duplicate delivery harmless (stale precondition ⇒ retry next turn).
- **Approvals map from existing posture.** The run's capability preview (network allowlist, deploy, install — the same facts the Course card prints) answers `requestApproval`. No new policy schema. Undecidable requests deny-by-default with a recorded reason.
- **No `PipelineState` schema changes.** New durable state is files in the run root.
- **No new crates** expected (JSON-RPC framing is newline-delimited JSON — serde_json suffices; note the codex protocol omits the `"jsonrpc"` field).
- **No `git push`.** Phased local commits. **No V1 invention** — log and continue.

## Data model (files, not fields)

### steer-inbox.jsonl (run root; append-only)

```json
{"ts":"2026-07-11T18:00:00Z","source":"cli","text":"prefer sqlite over postgres","status":"pending"}
{"ts":"…","source":"tui","text":"stop refactoring, ship the fix","status":"delivered","delivered_turn_id":"turn_3","delivered_at":"…"}
```

Status transitions append corrected rows (ledger style, last-writer-wins by
`ts`+`text` identity) — never in-place edits. `source ∈ {cli, tui}`.

### approval decisions (traces, not a new file)

Every answered `requestApproval` appends a `TraceRecord` with
`event: "provider.approval"` and detail `{kind, command|path, decision:
allow|deny, reason, capability}`. The why panel and verdict read traces
already; no new reader needed.

### provider-session.json (Semaphore's file, extended)

Adds optional `"route": "cli:codex-server"`, `"server_pid": 12345`,
`"active_turn_id": "turn_3"`. Additive keys only — Semaphore's schema 1
readers must keep working (`#[serde(default)]`).

## Connection model (mirror types in `codex_app_server.rs`)

Mirror only the methods used — a thin, hand-rolled client, not a bindings
crate:

```
requests  → thread/start, thread/resume, turn/start, turn/steer, turn/interrupt
responses ← matching ids
notifications ← turn/started, turn/completed, item/started, item/completed,
                item/agentMessage/delta (ignored beyond liveness),
                thread/tokenUsage/updated
server requests ← item/commandExecution/requestApproval,
                  item/fileChange/requestApproval (answered from capability map)
```

Turn algorithm per `ProviderRequest`: ensure server child (spawn if absent,
handshake `initialize`), ensure thread (start/resume via session file), send
`turn/start` with the prompt, then drive a select loop: deliver pending inbox
lines via `turn/steer{expected_turn_id: active}` as soon as a turn id is
known; answer approval requests from the capability map; accumulate usage
from `thread/tokenUsage/updated`; return on `turn/completed`/`turn/failed`.
Cancellation token ⇒ `turn/interrupt`, await completion ≤5s, then kill child.

## Approval mapping (deterministic)

```
commandExecution: parse argv[0] + a conservative network-intent heuristic
  (curl/wget/npm/pip/cargo/git-remote verbs) →
  network Deny  + network-intent        → deny "network denied by run capabilities"
  network Allowlist + host extractable  → allow iff host matches; else deny
  install intent (npm i -g, brew, apt)  → allow iff capabilities.install
  otherwise                             → allow (workspace-write sandbox still applies)
fileChange: allow iff all paths are under the run working dir or --add-dir roots; else deny
```

The map is a pure function `fn answer_approval(&CapabilityPosture, &ApprovalRequest) -> Decision`
— depth-tested exhaustively; the deny reason string is part of the spec.

## Verb signatures

```
deadreckon steer <run-id|latest> "<text>"
    # appends to steer-inbox.jsonl; prints delivery expectation
```

Refusals:

| Case | `try:` |
|---|---|
| run not live | `try: deadreckon extend <run-id> "<text>"` |
| route cannot steer (exec route) | `try: deadreckon config provider cli:codex-server` |
| empty text | `try: deadreckon steer <run-id> "one concrete instruction"` |

Helm: `:steer <text>` on run surfaces appends to the same inbox (command-mode
verb table entry; run surface only). `deadreckon kill` on a server-routed live
run: interrupt → grace ≤5s → existing kill path.

## Phases (eleven)

Each phase: named depth test(s) first (red) → implement → `make verify` green
→ conventional-commit → CHANGELOG line naming the SHA. CI uses a scripted
fake app-server (a fixture binary speaking canned JSON-RPC over stdio); no
live codex.

### P1 — JSON-RPC client core
- `codex_app_server.rs`: framing, id correlation, notification stream, server-request dispatch; typed mirrors for the used subset.

Depth tests:
- `rpc_client_correlates_responses_by_id`
- `rpc_client_routes_server_requests_to_handler`
- `unknown_notification_is_recorded_not_fatal`

### P2 — Server child supervision
- Spawn `codex app-server`, initialize handshake, pid into the run's supervised pids, kill on drop.

Depth tests:
- `server_child_pid_is_supervised_and_killed_on_drop`
- `handshake_failure_degrades_route_with_trace`

### P3 — Thread lifecycle + session file extension
- thread/start on first turn, thread/resume after; additive session keys.

Depth tests:
- `server_route_persists_thread_and_resumes_it`
- `semaphore_session_schema_still_readable`

### P4 — turn/start + completion + usage
- Full turn round-trip; usage from tokenUsage notifications; content from the final agent message item.

Depth tests:
- `server_turn_completes_with_real_usage`
- `turn_failed_surfaces_provider_error`

### P5 — Steer inbox (durable layer)
- `steer_inbox.rs` in core: append/read/mark-delivered; ledger-style status rows.

Depth tests:
- `steer_inbox_appends_and_marks_delivered`
- `pending_lines_survive_process_restart`

### P6 — `deadreckon steer` verb
- CLI verb + refusal table above; lifecycle hint names attach.

Depth tests:
- `steer_verb_appends_pending_line`
- `steer_refuses_dead_run_with_extend_try`
- `steer_refuses_exec_route_with_config_try`

### P7 — Delivery via turn/steer
- The turn loop delivers pending lines with `expected_turn_id`; stale precondition retries next turn; delivered markers written.

Depth tests:
- `pending_steer_delivers_with_expected_turn_id`
- `stale_turn_precondition_retries_not_drops`
- `duplicate_delivery_is_harmless`

### P8 — Approval answering
- `answer_approval` pure map + wiring; every decision traced.

Depth tests:
- `network_deny_capability_denies_curl_command`
- `allowlisted_host_command_is_approved`
- `file_change_outside_workspace_is_denied`
- `every_approval_decision_appends_trace`

### P9 — Interrupt-before-kill + degrade path
- kill maps to interrupt→grace→kill; server death mid-turn falls back to exec route next turn; pending steers surface as attention.

Depth tests:
- `kill_sends_interrupt_before_process_kill`
- `server_death_degrades_to_exec_route_with_caveat`
- `undelivered_steers_surface_as_attention`

### P10 — Helm `:steer` + friendliness
- Command-mode verb (run surfaces only); footer/help entries; `--plain` attach prints pending/delivered steer lines; spine attention for pending steers.

Depth tests:
- `colon_steer_appends_to_inbox_from_attach`
- `steer_verb_absent_from_non_run_surfaces`
- `plain_attach_lists_steer_inbox_state`

### P11 — Architecture doc + CHANGELOG (doc only)
- Insert `## 51. Rudder: Steering the Running Child` into AS-BUILT (connection model, inbox, approval map, degrade rules); cross-reference §47 (command mode gains `:steer`) and §50.
- CHANGELOG:
  ```
  ## Rudder (stable) — steer the running child — <date>
  - cli:codex-server drives codex over its app-server: operator steering
    (deadreckon steer / :steer) with a durable at-least-once inbox,
    interrupt-before-kill, and capability-answered approvals replacing the
    danger-full-access inversion; server loss degrades to the exec route.
  ```
- V1-CANDIDATES: shared daemon/unix socket, cross-run server reuse, thread/fork↔rewind mapping, steering for cli:claude-code.

## Out of scope (explicitly → V1-CANDIDATES)

- `thread/fork`/`thread/rollback` mapped onto deadreckon rewind (design note only).
- Shared app-server daemon, `unix://`/`ws://` transports, remote control.
- Auto-selecting the server route in Course's planner.
- Steering non-codex providers.
- Guardian/auto-approval-review integration.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1: serde/serde_json, tokio (in tree). Tier 2: none expected. Tier 3
(blocked): linking codex-rs crates; `jsonrpc`/`tower` frameworks (the framing
is 40 lines, not a dependency).

## Engineering invariants (do not violate)

- **A server failure never fails a run** — degrade is depth-tested.
- **Steer text is operator input, recorded before delivery** (durable-first; the inbox is the truth, delivery is best-effort).
- **Approval decisions are pure + traced** — no decision without a trace row.
- **The exec route remains untouched and default** — Rudder adds, never rewires.
- **Additive session-file keys only** (Semaphore readers keep working).
- **One depth test before each phase.**

## Process invariants

- Phased local commits only. No `git push`.
- Each phase: depth tests green + CHANGELOG SHA line.
- Fake app-server fixture lives with provider tests; optionally verify once against real codex locally before P11 (operator step, like preflight-real).
- V1 discoveries logged, not implemented.
