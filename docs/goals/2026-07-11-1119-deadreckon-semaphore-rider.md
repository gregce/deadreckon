# deadreckon — Semaphore Rider (read codex's signal flags)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-07-11-1119-deadreckon-semaphore-goal.md`.
It supersedes nothing in prior riders — their invariants still apply. This
rider adds: a **typed `codex exec --json` event reader**, **real token
accounting** for `cli:codex`, **per-run thread resume** via a durable
`provider-session.json`, **live flight ingestion** of item events, and
**schema-constrained structured output** at the provider boundary.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `~/.deadreckon`.
Codex reference checkout `/Users/gdc/codex/codex-rs` (read-only grounding; the
wire contract is what the installed `codex` binary emits, feature-detected at
runtime, never a compile-time dependency on that checkout).

## Posture (decided — do not redesign)

- **Maturity stays stable** (0.6.0 shipped; lands under a `Semaphore` CHANGELOG section).
- **No `PipelineState` schema changes.** The codex thread id is a file: `<run_root>/provider-session.json`. Files, not fields.
- **Tolerant parsing is the law.** Unknown `type` tags are preserved as raw JSON in the flight ledger and never abort a turn. A stdout that fails to parse as JSONL at all degrades to the current raw-stdout behavior and appends a `provider.contract.degraded` trace. The driver must work against codex versions that predate `--json`.
- **Feature detection, not version pinning.** Probe once per process (`codex exec --help` contains `--json` / `--output-schema` / `resume`); cache the capability set; absent capabilities disable the corresponding behavior with a caveat, never an error.
- **Thread scope is the run.** One deadreckon run ⇒ at most one codex thread. Plan children, campaign subs, and separate runs never share threads. `--ephemeral` is NOT passed — rollout files are codex's own ledger and the flight importer may still read them.
- **No new crates.** serde_json is already in tree.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Anything past P1–P11 → `docs/V1-CANDIDATES.md`.
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Data model (files, not fields)

### provider-session.json (per run root; written by the provider layer)

```json
{
  "schema": 1,
  "provider": "cli:codex",
  "thread_id": "0197c8a2-…",          // from thread.started
  "created_at": "2026-07-11T18:00:00Z",
  "last_turn_at": "2026-07-11T18:04:12Z",
  "resume_failures": 0                  // consecutive failed resumes; >=1 forces fresh thread next turn
}
```

Written atomically. Absent file = first turn (fresh conversation). The file is
provider-scoped: a run whose provider changes mid-life (rescue) ignores a
session recorded by a different provider name.

### CodexThreadEvent (parser types — mirror, do not import)

Mirror the wire shape of `codex-rs/exec/src/exec_events.rs` in
`crates/deadreckon-providers/src/codex_events.rs`:

```rust
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum CodexThreadEvent {
    #[serde(rename = "thread.started")] ThreadStarted { thread_id: String },
    #[serde(rename = "turn.started")]   TurnStarted {},
    #[serde(rename = "turn.completed")] TurnCompleted { usage: CodexUsage },
    #[serde(rename = "turn.failed")]    TurnFailed { #[serde(default)] error: Option<serde_json::Value> },
    #[serde(rename = "item.started")]   ItemStarted { item: serde_json::Value },
    #[serde(rename = "item.updated")]   ItemUpdated { item: serde_json::Value },
    #[serde(rename = "item.completed")] ItemCompleted { item: serde_json::Value },
    #[serde(rename = "error")]          Error { message: String },
    #[serde(other)]                     Unknown,
}
pub(crate) struct CodexUsage { input_tokens: u64, cached_input_tokens: u64, output_tokens: u64 }
```

Item payloads stay `serde_json::Value` — the flight ledger records them
verbatim; only `agent_message` text and `command_execution` status are read
structurally. Every field addition upstream must be non-breaking by
construction (`#[serde(default)]`, `Unknown` catch-all).

## Turn algorithm (the driver, per request)

```
capabilities = probe_codex_capabilities(binary)        // cached OnceLock
session = read provider-session.json (same provider only)
args = base sandbox/approval args (unchanged from today)
if capabilities.json { args += ["--json"] }
if capabilities.output_last_message { args += ["-o", <run scratch file>] }
if request.output_schema && capabilities.output_schema { args += ["--output-schema", <schema file>] }
if session.thread_id && capabilities.resume && session.resume_failures == 0 {
    subcommand = ["exec", "resume", session.thread_id]
} else { subcommand = ["exec"] }
run via run_cli (pid_file, cancellation, sandbox unchanged)
for each stdout line: parse CodexThreadEvent
    thread.started  -> persist provider-session.json
    item.*          -> append flight row (schema "codex-cli", live source)
    turn.completed  -> usage -> ProviderUsage
    turn.failed / error -> provider error with the message
content = output-last-message file if present else assembled agent_message items else raw stdout (degraded)
resume invocation that exits nonzero with a thread-not-found signature:
    increment resume_failures, retry ONCE as fresh exec, trace `provider.session.reset`
```

`ProviderRequest` gains two optional fields (internal struct, not persisted
state): `session_dir: Option<PathBuf>` (the run root, threaded from
`turn_loop.rs:272`) and `output_schema: Option<serde_json::Value>`. All other
providers ignore both.

## Spend truth

`ProviderUsage` for cli:codex reports real `input_tokens`/`output_tokens` from
`turn.completed`. `SpendEstimate.subscription` stays `true` and `cost_usd`
stays `0.0` — Semaphore fixes token *truth*, not pricing; metered pricing for
subscription CLIs is out of scope. Status/rollup surfaces that render
`0 tokens` today start showing real counts with zero changes of their own.

## Phases (eleven)

Each phase: named depth test(s) **first** (watch them fail) → implement →
`make verify` green → conventional-commit → one-line CHANGELOG entry naming
the SHA. Tests use a scripted fake `codex` binary (shell script fixture that
replays canned JSONL; the pattern exists in provider tests today) — no live
codex in CI.

### P1 — Capability probe
- `probe_codex_capabilities` parses `codex exec --help` once per process; struct `CodexCapabilities { json, output_last_message, output_schema, resume }`.

Depth tests (`crates/deadreckon-providers/src/codex_events.rs` + `tests/`):
- `capability_probe_detects_json_and_resume_flags`
- `capability_probe_absent_flags_disable_features_not_error`

### P2 — Event parser
- `codex_events.rs` with the enum above; line-by-line tolerant parse helper `parse_thread_events(&str) -> Vec<CodexThreadEvent>`.

Depth tests:
- `thread_event_parses_started_completed_and_items`
- `unknown_event_type_parses_as_unknown_not_error`
- `garbage_line_is_skipped_and_counted`

### P3 — provider-session.json
- Read/write helpers with atomic write; provider-name scoping; `resume_failures` bookkeeping.

Depth tests:
- `session_file_roundtrips_and_scopes_by_provider`
- `resume_failure_marks_session_for_fresh_thread`

### P4 — Driver emits --json and parses usage
- `cli_codex.rs` switches to `--json` when capable; `ProviderUsage` from `turn.completed`; degraded path preserves old behavior + `provider.contract.degraded` trace.

Depth tests:
- `codex_turn_reports_real_token_usage_from_turn_completed`
- `unparseable_stdout_degrades_to_raw_content_with_caveat`

### P5 — Final answer via --output-last-message
- Content comes from the `-o` file when present; assembled `agent_message` items as fallback; raw stdout only in degraded mode.

Depth tests:
- `response_content_is_last_message_not_stdout_noise`
- `missing_last_message_file_falls_back_to_agent_message_items`

### P6 — Thread resume
- Turn 1 persists `thread_id`; turn 2+ invokes `exec resume <id>`; thread-not-found retries once fresh and resets the session.

Depth tests:
- `second_turn_resumes_persisted_thread_id`
- `distinct_runs_never_share_a_thread`
- `vanished_thread_retries_fresh_once_with_session_reset_trace`

### P7 — Live flight ingestion
- `item.*` events append flight rows (schema `codex-cli`) during the turn; the post-hoc importer dedupes rows already ingested live (idempotent by item id).

Depth tests:
- `item_events_stream_into_flight_ledger_during_turn`
- `post_hoc_import_dedupes_live_ingested_items`

### P8 — Structured output at the provider boundary
- `ProviderRequest.output_schema` threads to `--output-schema`; the response content is the schema-conforming JSON string; callers opt in (no caller migration in this slice beyond one: the launch planner's codex route, proving the plumbing).

Depth tests:
- `output_schema_request_passes_schema_file_to_codex`
- `schema_incapable_codex_omits_flag_with_caveat`

### P9 — Turn-loop threading + spend surfaces
- `turn_loop.rs:272` populates `session_dir`; spend ledger rows for cli:codex carry the real usage; `status`/rollup token counts verified against a fixture run.

Depth tests:
- `turn_loop_threads_session_dir_for_run_root`
- `spend_ledger_records_codex_tokens_per_turn`

### P10 — Friendliness pass
- `deadreckon show`/`report` render token counts for codex runs; degraded-contract caveat surfaces in the run's why/attention path; refusals carry `try:` lines (see table).

Depth tests:
- `show_renders_codex_token_usage`
- `degraded_contract_caveat_reaches_attention_surface`

### P11 — Architecture doc + CHANGELOG (doc only; no depth test)
- Insert `## 50. Semaphore: The Codex Wire Contract` into `docs/AS-BUILT-ARCHITECTURE.md` (capability probe, event mirror, session file, resume rules, degraded mode); cross-reference §33 (flight recorder) and §46 (launch planning: output_schema availability).
- Append CHANGELOG:
  ```
  ## Semaphore (stable) — read codex's signal flags — <date>
  - cli:codex drives `exec --json`: real token usage, per-run thread resume,
    live flight ingestion, final answers from --output-last-message, and
    schema-constrained structured output; unparseable contracts degrade to
    the old behavior with a caveat instead of failing the turn.
  ```
- Log deferrals in `docs/V1-CANDIDATES.md`.

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| resume failed twice (thread gone) | `try: deadreckon show <run> --raw provider-session` |
| codex binary lacks --json | caveat only (no refusal): "codex predates --json; upgrade codex for token accounting" |
| output-schema requested, incapable binary | caveat + proceed unconstrained; `try: codex --version` |

## Out of scope (explicitly → V1-CANDIDATES)

- The app-server route, steering, interrupts, approvals (Rudder's slice).
- Metered pricing for subscription CLIs (tokens land; dollars stay 0).
- Equivalent `--json` upgrades for cli:claude-code / cli:gemini (follow-up slices; the capability-probe pattern generalizes).
- Parsing `reasoning` items into narrative (flight records them verbatim).
- Passing `--ephemeral` or managing codex rollout retention.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1: serde/serde_json (in tree). Tier 2: none. Tier 3 (blocked): depending
on codex-rs crates directly — the wire contract is mirrored, never linked.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes**; the session is a file.
- **A parse failure never fails a turn.** Degraded mode is a caveat, not an error.
- **Resume never crosses runs.** Depth-tested (`distinct_runs_never_share_a_thread`).
- **The event mirror is additive-tolerant** (`#[serde(other)]`, defaults) — pin with the unknown-event test.
- **preflight-real must stay green**: the release proof (`release/preflight-real.sh cli:codex`) runs the real binary through the new driver before the slice closes.
- **One depth test before each phase.** A phase whose tests were never red is suspect.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing and a CHANGELOG entry naming the SHA.
- The fake-codex fixture lives with provider tests; CI never invokes real codex.
- If a phase reveals a V1 decision (e.g. the session file wants to become shared provider infrastructure), log it in V1-CANDIDATES; do not expand scope.
