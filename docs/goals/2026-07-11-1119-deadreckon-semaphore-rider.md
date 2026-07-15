# deadreckon — Semaphore Rider (read the CLI agents' signal flags)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-07-11-1119-deadreckon-semaphore-goal.md`.
It supersedes nothing in prior riders — their invariants still apply. This
rider adds: **shared contract machinery** (capability probe, per-run session
file, tolerant-parse doctrine, live flight ingestion) and **two thin event
mirrors** over it — codex `exec --json` and claude `-p --output-format
stream-json` — delivering **real token accounting**, **per-run conversation
resume**, **structured final answers**, and **schema-constrained output**
(where the binary supports it) for both `cli:codex` and `cli:claude-code`.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `~/.deadreckon`.
Codex reference checkout `/Users/gdc/codex/codex-rs` (read-only grounding).
Claude Code has no local source checkout: its contract is grounded by probing
the installed binary (`claude -p --help`) and by fixtures recorded from real
invocations at implementation time. For BOTH providers the wire contract is
what the installed binary emits, feature-detected at runtime — never a
compile-time dependency, never an assumed version.

## Posture (decided — do not redesign)

- **Maturity stays stable** (lands under a `Semaphore` CHANGELOG section).
- **No `PipelineState` schema changes.** The conversation id is a file: `<run_root>/provider-session.json`. Files, not fields.
- **Tolerant parsing is the law.** Unknown `type` tags are preserved as raw JSON in the flight ledger and never abort a turn. Output that fails to parse as JSONL at all degrades to the current raw-stdout behavior and appends a `provider.contract.degraded` trace. Both drivers must work against binaries that predate their structured flags.
- **Feature detection, not version pinning.** Probe once per process per binary (`codex exec --help`; `claude -p --help`); cache the capability set; absent capabilities disable the corresponding behavior with a caveat, never an error.
- **Conversation scope is the run.** One deadreckon run ⇒ at most one codex thread / one claude session. Plan children, campaign subs, and separate runs never share conversations.
- **Tokens land; dollars don't move.** `ProviderUsage` becomes real for both providers. `SpendEstimate` stays `subscription: true, cost_usd: 0.0`; claude's reported `total_cost_usd` is recorded in the turn trace detail as informational — billing semantics are out of scope.
- **No new crates.** serde_json is already in tree.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Anything past P1–P11 → `docs/V1-CANDIDATES.md`.
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Data model (files, not fields)

### provider-session.json (per run root; written by the provider layer)

```json
{
  "schema": 1,
  "provider": "cli:codex",              // or "cli:claude-code"
  "conversation_id": "0197c8a2-…",      // codex thread_id | claude session_id
  "created_at": "2026-07-11T18:00:00Z",
  "last_turn_at": "2026-07-11T18:04:12Z",
  "resume_failures": 0                    // consecutive failed resumes; >=1 forces fresh next turn
}
```

Written atomically. Absent file = first turn. Provider-scoped: a run whose
provider changes mid-life (rescue) ignores a session recorded by a different
provider name.

### Codex event mirror (`codex_events.rs`; wire per exec_events.rs)

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

### Claude event mirror (`claude_events.rs`; wire per recorded fixtures)

```rust
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ClaudeStreamEvent {
    System { #[serde(default)] subtype: Option<String>,
             #[serde(default)] session_id: Option<String> },   // subtype "init" carries session_id
    Assistant { message: serde_json::Value },                   // content blocks incl. tool_use
    User { message: serde_json::Value },                        // tool results
    Result { #[serde(default)] subtype: Option<String>,
             #[serde(default)] result: Option<String>,          // the final answer text
             #[serde(default)] usage: Option<serde_json::Value>,
             #[serde(default)] total_cost_usd: Option<f64>,
             #[serde(default)] session_id: Option<String>,
             #[serde(default)] is_error: Option<bool> },
    #[serde(other)] Unknown,
}
```

The exact field set is confirmed against fixtures recorded from the REAL
`claude` binary during P3 (`claude -p --output-format stream-json --verbose
"trivial prompt"` in a scratch dir); the mirror uses `#[serde(default)]`
everywhere so field drift degrades, never breaks. Item payloads stay
`serde_json::Value` for the flight ledger; only session id, usage, cost,
result text, and is_error are read structurally.

## Turn algorithm (shared; per-provider table below)

```
capabilities = probe(binary)                       // cached OnceLock per binary path
session = read provider-session.json (same provider only)
args = today's base args (sandbox/approval unchanged)
+ structured-stream flag if capable
+ answer-isolation flag if capable (codex only)
+ --output-schema if request.output_schema && capable (codex only today)
+ resume invocation if session present, capable, resume_failures == 0
run via run_cli (pid_file, cancellation, sandbox unchanged)
parse each stdout line via the provider's mirror:
    conversation id event  -> persist provider-session.json
    tool/item events       -> append flight row live (provider's import schema)
    usage-bearing terminal -> ProviderUsage (+ claude: cost into trace detail)
    failure event          -> provider error with the message
content = answer source (table) else degraded raw stdout
resume exits nonzero with a session-not-found signature:
    increment resume_failures, retry ONCE fresh, trace `provider.session.reset`
```

| | cli:codex | cli:claude-code |
|---|---|---|
| stream flag | `--json` | `--output-format stream-json` (with `--verbose` if the binary requires it for stream output) |
| conversation id | `thread.started.thread_id` | `system(init).session_id` / `result.session_id` |
| resume | `exec resume <id>` | `--resume <id>` |
| usage | `turn.completed.usage` | `result.usage` |
| answer | `--output-last-message <file>` | `result.result` |
| failure | `turn.failed` / `error` | `result.is_error == true` |
| output schema | `--output-schema <file>` | not offered → capability caveat |

`ProviderRequest` gains two optional fields (internal struct, not persisted
state): `session_dir: Option<PathBuf>` (threaded from `turn_loop.rs:272`) and
`output_schema: Option<serde_json::Value>`. Other providers ignore both.

**Build the machinery descriptor-aware.** The shared pieces (capability
probe, session file, tolerant-parse dispatch, live flight append, degraded
fallback) must be parameterized by a `ProviderContract` value — for codex and
claude that value is constructed in code from the table above; a follow-up
slice (Pennant) will construct it from descriptor TOML for the generic fleet
(cli:pi, cli:copilot, cli:gemini, cli:opencode). Do not implement descriptor
parsing here; do keep the machinery free of codex/claude-specific branching
outside the two mirror modules, so Pennant is data plus fixtures, not a
refactor.

## Phases (eleven)

Each phase: named depth test(s) **first** (watch them fail) → implement →
`make verify` green → conventional-commit → one-line CHANGELOG entry naming
the SHA. Tests use scripted fake `codex` AND fake `claude` binaries (shell
fixtures replaying canned JSONL); no live CLIs in CI.

### P1 — Capability probes (both binaries)
- `CodexCapabilities { json, output_last_message, output_schema, resume }`; `ClaudeCapabilities { stream_json, resume }`; one probe fn per driver, cached.

Depth tests:
- `codex_probe_detects_json_and_resume_flags`
- `claude_probe_detects_stream_json_and_resume`
- `absent_flags_disable_features_not_error`

### P2 — Codex event mirror
Depth tests:
- `codex_events_parse_started_completed_and_items`
- `codex_unknown_event_parses_as_unknown_not_error`
- `garbage_line_is_skipped_and_counted`

### P3 — Claude event mirror (fixtures from the real binary)
- Record real stream-json fixtures into the test tree; mirror per the struct above.

Depth tests:
- `claude_events_parse_init_assistant_and_result`
- `claude_result_carries_usage_cost_and_session`
- `claude_unknown_event_parses_as_unknown_not_error`

### P4 — provider-session.json (shared)
Depth tests:
- `session_file_roundtrips_and_scopes_by_provider`
- `resume_failure_marks_session_for_fresh_conversation`

### P5 — Codex driver: stream + usage + answer
- `--json`, usage from `turn.completed`, content from `--output-last-message`; degraded path + `provider.contract.degraded` trace.

Depth tests:
- `codex_turn_reports_real_token_usage`
- `codex_response_content_is_last_message_not_stdout_noise`
- `codex_unparseable_stdout_degrades_with_caveat`

### P6 — Claude driver: stream + usage + answer
- `--output-format stream-json`, usage/cost/answer from `result`; `is_error` maps to provider error; degraded path shared.

Depth tests:
- `claude_turn_reports_real_token_usage`
- `claude_reported_cost_lands_in_trace_detail_not_spend`
- `claude_response_content_is_result_text`
- `claude_is_error_result_maps_to_provider_error`

### P7 — Resume (both)
Depth tests:
- `codex_second_turn_resumes_persisted_thread`
- `claude_second_turn_resumes_persisted_session`
- `distinct_runs_never_share_a_conversation`
- `vanished_conversation_retries_fresh_once_with_reset_trace`

### P8 — Live flight ingestion (both)
- Tool/item events append flight rows during the turn (schemas `codex-cli` / `claude-code`); post-hoc import dedupes rows already ingested live.

Depth tests:
- `codex_items_stream_into_flight_ledger_during_turn`
- `claude_tool_use_streams_into_flight_ledger_during_turn`
- `post_hoc_import_dedupes_live_ingested_items`

### P9 — Structured output + turn-loop threading
- `output_schema` → codex `--output-schema`; claude (and incapable codex) caveat + proceed unconstrained. `turn_loop.rs:272` threads `session_dir`; spend ledgers carry real usage for both providers.

Depth tests:
- `output_schema_passes_schema_file_to_codex`
- `schema_incapable_provider_caveats_and_proceeds`
- `spend_ledger_records_cli_tokens_per_turn_both_providers`

### P10 — Friendliness pass
- `show`/`report` render token counts for both CLI providers; degraded-contract caveat reaches the attention/why surface; refusal table below.

Depth tests:
- `show_renders_cli_token_usage_for_both_providers`
- `degraded_contract_caveat_reaches_attention_surface`

### P11 — Architecture doc + CHANGELOG (doc only; no depth test)
- Insert `## 50. Semaphore: The CLI Agent Wire Contracts` into `docs/AS-BUILT-ARCHITECTURE.md` (probe, both mirrors, session file, resume rules, degraded mode, the per-provider table); cross-reference §33 (flight) and §46 (planner gains schema-constrained codex output).
- Append CHANGELOG:
  ```
  ## Semaphore (stable) — read the CLI agents' signal flags — <date>
  - cli:codex and cli:claude-code read their structured contracts: real token
    usage, per-run conversation resume, live flight ingestion, answers from
    the structured result, schema-constrained output where supported;
    unparseable contracts degrade with a caveat instead of failing the turn.
  ```
- Log deferrals in `docs/V1-CANDIDATES.md`.

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| resume failed twice (conversation gone) | `try: deadreckon show <run> --raw provider-session` |
| binary lacks structured output flags | caveat only: "…predates structured output; upgrade for token accounting" |
| output-schema requested, incapable binary | caveat + proceed unconstrained; `try: codex --version` |

## Out of scope (explicitly → V1-CANDIDATES)

- The app-server route, steering, interrupts, approvals (Rudder's slice; Rudder's session-file extension keys still apply on top of this schema).
- Metered pricing / billing semantics for subscription CLIs (claude's reported cost stays informational trace detail).
- Descriptor-declared contracts for the generic fleet — cli:pi and cli:copilot (whose descriptors already request JSON output nothing parses), cli:gemini, cli:opencode — are the Pennant slice (`2026-07-15-1658-deadreckon-pennant-{goal,rider}.md`); Semaphore only keeps the machinery contract-shaped for it.
- Bare `cli:generic` entries with no descriptor contract (no standard exists; capability probe reports "no contract detected").
- Parsing `reasoning`/thinking items into narrative (flight records them verbatim).
- Claude `--json-schema`-style structured output if/when the binary grows it — the capability probe is forward-ready; wiring it is a follow-up.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1: serde/serde_json (in tree). Tier 2: none. Tier 3 (blocked): depending
on codex-rs crates or any Anthropic SDK — both wire contracts are mirrored,
never linked.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes**; the session is a file.
- **A parse failure never fails a turn.** Degraded mode is a caveat, not an error — depth-tested per provider.
- **Resume never crosses runs or providers.** (`distinct_runs_never_share_a_conversation`, provider-scoped session file.)
- **Both mirrors are additive-tolerant** (`#[serde(other)]`, defaults) — pinned by the unknown-event tests.
- **Claude fixtures come from the real binary**, recorded at P3 and checked in; hand-written fixtures are not acceptable grounding.
- **preflight-real must stay green** for BOTH routes before the slice closes (`release/preflight-real.sh` covers cli:claude-code and cli:codex by default).
- **One depth test before each phase.** A phase whose tests were never red is suspect.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing and a CHANGELOG entry naming the SHA.
- Fake-binary fixtures live with provider tests; CI never invokes real codex or claude.
- If a phase reveals a V1 decision (e.g. the mirrors want to become a shared provider-contract crate), log it in V1-CANDIDATES; do not expand scope.
