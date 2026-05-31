# deadreckon — Composable Seams Rider (swap a worker, keep the gate)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-31-1644-deadreckon-composable-seams-goal.md`.
It supersedes nothing in prior riders (notably
`2026-05-28-1556-deadreckon-tamper-evident-gate-rider.md`,
`2026-05-28-1841-deadreckon-campaign-rider.md`,
`2026-05-28-2032-deadreckon-effortless-rider.md`,
`2026-05-29-1600-deadreckon-decompose-rider.md`) — their invariants still apply.
This rider adds **one uniform seam contract** that makes four compiled-in
governance concerns — `policy`, `model-catalog`, before/after `hooks`, and the
`event-sink` — swappable from `config.toml`; keeps the acceptance gate
deliberately **non-swappable**; and closes the unbounded-history gap on the
direct-API provider path with deterministic, resume-safe compaction.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime `~/.deadreckon/`.

## The bet this lands (read before designing)

The iii harness-decomposition post (Piccolo) argues a harness is ~15 distinct
jobs and bundling them into one framework is the mistake: each job should be an
independently-swappable worker connected by **one** primitive, so "build your
own" means "swap a worker," not "fork a framework." deadreckon already proves it
can do this where it matters most: `cli:*` providers are sandboxed subprocesses
(`crates/deadreckon-providers/src/cli_common.rs::run_cli_with_options`, spawn at
~L79) and `dr-gate` is a separate signing binary
(`crates/deadreckon/src/bin/dr-gate.rs`). Everything else in the governance core
is compiled in: change the policy model, the model catalogue, the per-tool-call
side effects, or where events go, and you fork the Rust.

deadreckon's answer is **not** "decompose everything uniformly." It is:
**decompose at the boundaries that buy trust or reuse; keep one non-swappable
root of trust.** This rider generalizes the existing subprocess seam into a
single `SeamCommand` contract and applies it to exactly four kinds. The
acceptance gate stays out of the seam set on purpose: a swappable trust root is a
forgeable one (the whole point of §35). The slider between a thin harness (no
seams) and a thick one (all four plus compaction) becomes a `config.toml`/flag
change, never a rebuild.

## Posture (decided — do not redesign)

- **Production-release track.** Release-blocking trust behavior, not scaffolding.
- **The gate is not a seam, ever.** `SeamKind` has no gate variant; the config
  rejects `[seams.gate]`; no seam can write `<run-root>/proofs/`, read
  `<run-root>/gate/nonce`, redirect `dr-gate`, or change the inputs to
  `gate.rs::marker_signature`. This is the linchpin invariant, depth-tested in
  P1 and P8.
- **Built-in is the default and the floor.** With no `[seams]` entry for a kind,
  that kind's behavior is **byte-identical to today** (depth-tested per seam).
  The OS sandbox (`ToolSandboxPolicy`) remains the hard floor: the `policy` seam
  may only *narrow* it, never widen it.
- **Files-not-fields.** Seam wiring is a new `[seams]` table in `config.toml`;
  per-run audit is new files `<run-root>/seams.json` and
  `<run-root>/compaction.jsonl`. **No** `PipelineState`, `Plan`,
  `AcceptanceMarker`, `AcceptanceCheckResult`, or `ProviderEntry` field additions.
- **Fail policy is fixed in code per kind, not configurable.** `policy` is
  fail-closed (deny), `catalog` is fail-open (built-in), `hooks` and `event_sink`
  are fail-safe (skip, non-fatal). Changing a fail policy changes the contract.
- **Seam workers run sandboxed.** A seam subprocess is spawned through the
  existing sandbox wrapper with the `gate/` and `proofs/` subtrees denied.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Bus/WebSocket transport, a worker registry, a
  human-approval seam, LLM-backed compaction → `docs/V1-CANDIDATES.md`.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Data model (files, not fields)

### `config.toml` — new `[seams]` table (and `[compaction]`)

```toml
# Each kind is optional. Absent ⇒ built-in default for that kind.
# Allowed kinds: policy | catalog | hooks | event_sink.  gate is NOT a seam.
[seams.policy]
command = ["my-policy-worker", "--rules", "policy.yaml"]  # argv; argv[0] resolved on PATH or absolute
timeout_ms = 5000

[seams.catalog]
command = ["my-catalog-worker"]
timeout_ms = 10000

[seams.hooks]
command = ["my-hook-logger"]
timeout_ms = 2000

[seams.event_sink]
command = ["my-slack-sink"]
timeout_ms = 2000

[compaction]
fraction = 0.75              # compact when est_tokens(history) > fraction * context_window
keep_recent_turns = 6        # most-recent turns kept verbatim
fallback_context_window = 200000   # used only when the catalog reports no context_window
```

Parsed by a **new** `SeamsConfig` (do not extend `ProviderConfigFile`), with
`#[serde(deny_unknown_fields)]` on the seams map so any unknown kind — including
`gate` — is a hard error.

### `<run-root>/seams.json` — per-run resolution audit (written once at run start)

```jsonc
{
  "schema_version": 1,
  "run_id": "…",
  "resolved_at": "RFC3339",
  "no_seams": false,                  // true when --no-seams forced built-ins
  "kinds": {
    "policy":     { "source": "external", "command": "my-policy-worker", "timeout_ms": 5000, "fail_policy": "closed" },
    "catalog":    { "source": "builtin",  "fail_policy": "open" },
    "hooks":      { "source": "external", "command": "my-hook-logger",  "timeout_ms": 2000, "fail_policy": "safe" },
    "event_sink": { "source": "builtin",  "fail_policy": "safe" }
  }
}
```

`command` records argv[0]'s basename only (no args) — audit, not re-exec.

### `<run-root>/compaction.jsonl` — append-only, one row per compaction event

```jsonc
{ "schema_version": 1, "turn": 14, "context_window": 200000, "fraction": 0.75,
  "est_tokens_before": 161240, "est_tokens_after": 38110,
  "kept_recent_turns": 6, "elided_turns": 7, "context_window_source": "catalog" }
```

`context_window_source` ∈ `catalog | seam | fallback`.

## The one primitive (the spec — match it in code)

New module `crates/deadreckon-runtime/src/seam.rs`, re-exported from the runtime
`lib.rs`. Runtime is the home because it already owns subprocess + sandbox
dispatch; `deadreckon-core` stays adapter-free.

```rust
pub enum SeamKind { Policy, Catalog, Hook, EventSink }   // no Gate — by construction

enum FailPolicy { Closed, Open, Safe }                   // fixed per kind, see fail_policy_for

pub enum SeamOutcome {
    Unconfigured,            // no command for this kind ⇒ caller runs built-in
    Ok(serde_json::Value),   // parsed JSON response
    Deny(String),            // policy fail-closed result
    Fallback,                // catalog fail-open ⇒ caller uses built-in
    Skipped(String),         // hook/event_sink fail-safe ⇒ caller continues
}

// One primitive every seam goes through.
pub async fn dispatch_seam(
    kind: SeamKind,
    req: &serde_json::Value,
    seams: &SeamsConfig,
    ctx: &SeamRunCtx,        // run_root, working_dir, sandbox backend
) -> SeamOutcome
```

`dispatch_seam` contract:
1. If `seams` has no command for `kind` → `Unconfigured`.
2. Else spawn argv **sandboxed** (reuse the sandbox path used by
   `cli_common::run_cli_with_options` / `ToolSandboxPolicy::cli_provider`), with
   `<run-root>/gate/` and `<run-root>/proofs/` denied to the child.
3. Write `req` as one JSON line to the child's stdin; read stdout to EOF; enforce
   `timeout_ms`.
4. On success + valid JSON → map to `Ok`/`Deny`/`Fallback` per kind's response
   schema. On timeout / spawn error / non-zero exit / parse error → apply
   `FailPolicy`: `Closed`→`Deny`, `Open`→`Fallback`, `Safe`→`Skipped`.

`SeamsConfig` resolution (also new in `seam.rs`):
`read_seams_config(config_path, no_seams) -> SeamsConfig`. `no_seams == true`
yields an all-empty config (every kind built-in). The config path is the one
already used to build the router (thread it through `RunLoopConfig`; default
`/Users/gdc/.deadreckon/config.toml`). Write `seams.json` immediately after
resolution.

## The four seams (wire shapes + insertion points)

### policy (fail-closed) — `turn_loop.rs` bash dispatch ~L439, write_file dispatch ~L565

- Built-in floor runs **first**: `bash_policy_refusal(state, command, &policy)`
  (policy from `load_tool_policy_from_sandbox_toml(state, "bash")`). If it
  refuses, the tool call is denied and the seam is **not** consulted (a seam can
  never un-refuse the floor).
- If the floor allows, call
  `dispatch_seam(Policy, json!({"function_id":"bash","command":command,"working_dir":wd}))`.
  - Request: `{ "function_id": "bash"|"write_file", "command": "<argv or path>", "working_dir": "<abs>" }`
  - Response: `{ "decision": "allow"|"deny", "reason"?: "<text>" }`
  - `Deny` → refuse this tool call through the **existing** refusal path (emit the
    denial, feed the corrective reason into history, continue the loop). `allow`/
    `Unconfigured` → proceed.

### catalog (fail-open) — resolved at run setup, injected into the router

- Before/at `ProviderRouter` construction, runtime calls
  `dispatch_seam(Catalog, json!({}))`.
  - Response: `{ "models": [ { "id", "context_window"?, "input_per_million"?, "output_per_million"?, "aliases"? } ] }`
  - Build an override keyed by `id`/alias and inject via a new
    `ProviderRouter::with_catalog_override(map)` (or a param on
    `from_config_path_with_model`). Lookups prefer the override, else fall back to
    the built-in `registry/mod.rs::ModelEntry` list (`BUILTIN_DESCRIPTOR_SOURCES`).
  - `Fallback`/malformed/timeout → built-in catalog; the run is never blocked.

### hooks (fail-safe, observe-only) — alongside event emits in `turn_loop.rs`

- After each built-in `ToolCallStarted` (bash ~L428, write_file ~L555) and
  `ToolCallResult` (bash ~L535, write_file ~L617) emit, call
  `dispatch_seam(Hook, <event-json>)`.
  - Request: the serialized `RunEventKind` value.
  - Response: ignored except for logging. The hook **cannot** change the policy
    decision and **cannot** block; `Skipped` on any failure. Built-in telemetry
    (snapshots, provenance, traces, `events.jsonl`) is untouched.

### event-sink (fail-safe, additive) — one broadcast subscriber in runtime

- `events.rs::emit_event` (core) is unchanged; `events.jsonl` + the broadcast
  sender stay the source of truth. When the `event_sink` seam is configured,
  runtime ensures a `broadcast::Sender<RunEvent>` exists and spawns **one**
  forwarder task that subscribes and pipes each event to the sink command
  (`dispatch_seam(EventSink, <event-json>)`), `Skipped` on failure. Attach is
  unaffected (it still reads the file/broadcast).

## Context-window compaction (the gap) — `crates/deadreckon-runtime/src/compaction.rs`

Today `run_turn_loop` accumulates `history: Vec<String>` and sends
`history.join("\n")` (turn_loop.rs:775) to the provider every turn with **no**
bound (`http.rs::payload` passes `request.prompt` as-is). CLI providers self-manage
their own context; the **direct-API (HTTP) path does not**. Close only that gap.

- `estimate_tokens(s) -> usize` = `s.chars().count() / 4` (deterministic).
- `compact_history(history, spec_prefix_len, context_window, cfg) -> (Vec<String>, Option<CompactionRecord>)`:
  - `threshold = (context_window as f64 * cfg.fraction) as usize` tokens. If
    `estimate_tokens(joined) <= threshold` → return history unchanged, `None`.
  - Else retain (a) the **spec prefix** (the leading goal + acceptance segment of
    the prompt — never elided) and (b) the most recent `cfg.keep_recent_turns`
    turns verbatim; replace the middle with one deterministic marker:
    `[seam:compaction] elided <N> earlier turns (~<T> tokens) to fit context window <W>; full history in history.json`.
  - Deterministic: identical inputs ⇒ identical output (no time, no randomness) —
    this is the resume-safety contract.
- Wire at the history-build site (turn_loop.rs ~L154 / before the L775 join)
  **only when the run's provider kind is HTTP/API** (guard on
  `ProviderKind`); CLI subagent prompts are never compacted.
- `context_window` comes from the (possibly seam-overridden) catalog for the
  run's model; if `None`, use `cfg.fallback_context_window` and record
  `context_window_source: "fallback"`. Append a `CompactionRecord` to
  `compaction.jsonl` on every compaction.

## The non-swappable gate (the invariant)

The gate is the one place deadreckon must refuse the article's "swap any layer"
ethos. Concretely, all of the following are guaranteed and depth-tested:

- `SeamKind` has no `Gate` variant; `[seams.gate]` is a hard config error.
- Seam subprocesses are sandboxed with `<run-root>/gate/` and
  `<run-root>/proofs/` denied — a malicious seam cannot read `gate/nonce` or
  write a marker.
- No seam touches the inputs of `gate.rs::marker_signature` (nonce + marker
  fields + check results + tamper file + rollup). A clean run with all seams +
  compaction active produces an identically-bound, valid signature.

## Flag / surface signatures

```
deadreckon run <goal>  [--no-seams]        # force built-ins for every kind this run
deadreckon start <goal> [--no-seams]       # same, through the guided path
deadreckon doctor                          # adds a "seams" section: per-kind builtin/external + timeout
```

Refusal / surfacing cases:

| Case | Behavior |
|---|---|
| `[seams.gate]` present | config refused: `the gate is not swappable`; no run |
| `[seams.<unknown>]` | config refused: unknown seam kind |
| `command` empty / `timeout_ms <= 0` | config refused with the offending kind named |
| `--no-seams` | `seams.json.no_seams = true`; every kind `builtin`; behavior identical to today |

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them fail;
implement; green on `cargo test -p <touched crate>` plus `cargo fmt --check`;
conventional-commit local commit; one-line CHANGELOG entry. Do not run
`make verify` / release / stress / full-workspace suites unless the human asks.

### P1 — Seam primitive + config + per-run audit + gate-guard (RED)

- Add `seam.rs`: `SeamKind` (no `Gate`), `FailPolicy`, `SeamOutcome`,
  `SeamsConfig` with `deny_unknown_fields`, `read_seams_config`, `dispatch_seam`
  (sandboxed spawn, JSON stdio, timeout, per-kind fail policy), and `seams.json`
  writer. No seam wired into behavior yet.

Depth tests (`crates/deadreckon-runtime/src/seam.rs`):
- `seam_kind_has_no_gate_variant`
- `seams_config_rejects_gate_key`
- `dispatch_unconfigured_kind_returns_unconfigured`
- `dispatch_round_trips_json_request_and_response`
- `dispatch_timeout_applies_kind_fail_policy`
- `resolution_writes_seams_json_with_sources_and_fail_policies`

### P2 — policy seam (fail-closed; sandbox stays the floor)

- Wire `dispatch_seam(Policy, …)` after the built-in floor at the bash and
  write_file dispatch points; map `Deny` to the existing refusal path.

Depth tests (`crates/deadreckon-runtime/`):
- `policy_seam_deny_blocks_tool_call_and_records_denial`
- `policy_seam_allow_proceeds`
- `policy_seam_timeout_denies_fail_closed`
- `policy_seam_cannot_widen_sandbox_floor`
- `unconfigured_policy_seam_is_identical_to_today`

### P3 — model-catalog seam (fail-open)

- Resolve at setup; inject override into the router; fall back to built-in.

Depth tests (`crates/deadreckon-providers/` + runtime setup):
- `catalog_seam_overrides_context_window_and_pricing`
- `catalog_seam_malformed_falls_back_to_builtin`
- `unconfigured_catalog_uses_builtin_model_entry_list`

### P4 — hook-fanout seam (observe-only, fail-safe)

- Fan out `ToolCallStarted`/`ToolCallResult` to the hook command; ignore its
  response; never block.

Depth tests:
- `hook_seam_receives_started_and_result_events`
- `hook_seam_failure_is_non_fatal`
- `hook_seam_cannot_alter_dispatch_decision`
- `hook_seam_cannot_write_proofs_or_marker`

### P5 — event-sink seam (additive mirror, fail-safe)

- One broadcast subscriber forwards `RunEvent`s to the sink; `events.jsonl`
  stays the source of truth.

Depth tests:
- `event_sink_receives_mirrored_events`
- `event_sink_failure_keeps_events_jsonl_complete`
- `attach_feed_unchanged_with_event_sink`
- `unconfigured_event_sink_is_identical_to_today`

### P6 — context-window compaction on the API path

- Add `compaction.rs`; wire deterministic elision at history build, HTTP path
  only; record to `compaction.jsonl`.

Depth tests (`crates/deadreckon-runtime/src/compaction.rs` + turn loop):
- `history_over_window_is_compacted_deterministically`
- `goal_and_acceptance_spec_always_retained`
- `cli_provider_path_is_never_compacted`
- `identical_inputs_produce_identical_compaction`
- `unknown_context_window_uses_recorded_fallback`
- `history_under_threshold_is_unchanged`

### P7 — thin↔thick slider + friendliness (§37)

- `--no-seams` on `run`/`start`; `doctor` seam section; preflight/preview line
  listing active external seams; seam-failure error footers.

Depth tests:
- `no_seams_flag_forces_builtin_for_all_kinds`
- `doctor_lists_seam_resolution`
- `preview_lists_active_external_seams`
- `seam_failure_renders_error_footer`

### P8 — trust-boundary consolidation (adversarial)

- Consolidate the gate-protection guarantees as explicit adversarial tests.

Depth tests:
- `no_seam_can_write_or_redirect_the_acceptance_marker`
- `malicious_seam_cannot_read_gate_nonce`
- `seam_worker_cannot_write_proofs_subtree`
- `gate_signature_unchanged_with_all_seams_active`

### P9 — config validation + full-stack integration

- Reject unknown/gate kinds, empty command, non-positive timeout with footers;
  run all four seams + compaction together.

Depth tests:
- `seams_config_rejects_unknown_kind`
- `seams_config_rejects_empty_command_or_bad_timeout`
- `full_stack_seam_run_produces_gated_result_and_seams_json`

### P10 — resume / determinism sweep

- A run resumed after N turns re-resolves seams and re-derives identical
  compaction; audit files survive.

Depth tests:
- `resume_re_resolves_seams_and_keeps_audit`
- `resume_produces_identical_compaction`
- `seams_json_and_compaction_jsonl_survive_resume`

### P11 — AS-BUILT §39 + CHANGELOG + V1-CANDIDATES (doc only; no depth test)

- Insert a new top-level section into
  `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  ```
  ## 39. Composable Seams (swap a worker, keep the gate)

  39.1 The monolith critique and deadreckon's answer
  39.2 The one primitive: SeamCommand (sandboxed JSON-over-stdio, timeout, per-kind fail policy)
  39.3 The four seams: policy / model-catalog / hook-fanout / event-sink (defaults = built-in)
  39.4 The thin↔thick slider: [seams] config and --no-seams
  39.5 The non-swappable gate (why the trust root has no seam)
  39.6 Context-window compaction on the direct-API path
  39.7 Per-run audit: seams.json and compaction.jsonl
  39.8 Sandboxing seam workers; fail-closed policy vs fail-safe observers
  39.9 Limits (no human-approval seam; deterministic compaction only; no bus)
  ```
- Update §22 ("What's Built vs Scaffolding-Thin"): add composable seams +
  API-path compaction to the shipped side; state explicitly that this adds
  capability and does **not** weaken §35 — the gate remains non-swappable.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:
  ```
  ## Composable Seams (production release) — 2026-05-31

  - One uniform seam contract (sandboxed JSON-over-stdio subprocess, per-kind fail
    policy) makes policy, model-catalog, hook-fanout, and event-sink swappable via
    a [seams] config table; unconfigured seams keep built-in behavior and
    --no-seams forces all built-ins.
  - The acceptance gate stays deliberately non-swappable: no seam can write or
    redirect the marker, read gate/nonce, or alter the signature; seam workers run
    sandboxed.
  - Deterministic, resume-safe context-window compaction closes the direct-API
    history gap (recorded in compaction.jsonl); CLI-provider paths are untouched.
  ```
- Log to `docs/V1-CANDIDATES.md` (new `## Composable seams follow-ups` section):
  human-in-the-loop approval seam, LLM-backed compaction summaries, bus/WebSocket
  transport + worker registry, seam versioning/capability negotiation, routing
  built-in telemetry through the hook seam, richer catalog capabilities.

## Integration matrix

| Seam | Wire | Default (unconfigured) | Fail policy | Can affect the gate? |
|---|---|---|---|---|
| policy | per tool-call allow/deny | sandbox.toml floor only | fail-closed (deny) | no |
| catalog | model list | built-in `ModelEntry` | fail-open (built-in) | no |
| hooks | tool events (observe) | none | fail-safe (skip) | no |
| event_sink | `RunEvent` mirror | `events.jsonl` + broadcast | fail-safe (skip) | no |
| compaction | history elision (API path) | none (unbounded today) | deterministic | no (never drops spec) |
| **gate** | — | `dr-gate` (built-in) | — | **n/a — not a seam** |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `seam 'policy' denied bash: <reason>` | `deadreckon show <id>` to review, or adjust the policy worker / re-run with `--no-seams` |
| `seam 'catalog' failed; using built-in catalog` | check `[seams.catalog].command` in config.toml; `deadreckon doctor` |
| `config error: [seams.gate] is not allowed (the gate is not swappable)` | remove `[seams.gate]`; the acceptance gate is the trust root |
| `config error: [seams.<x>] unknown seam kind` | use policy / catalog / hooks / event_sink; see `deadreckon doctor` |
| `seam 'hooks' timed out (non-fatal)` | raise `[seams.hooks].timeout_ms` or remove the hook; the run continued |

(Each footer is exercised by a P7/P9 depth test.)

## Config additions

See the `[seams]` and `[compaction]` blocks under **Data model**. Defaults when a
section is absent: every kind built-in; `fraction = 0.75`,
`keep_recent_turns = 6`, `fallback_context_window = 200000`.

## Out of scope (explicitly V1 candidates)

- **Human-in-the-loop approval seam** (`needs_approval` / pause-and-resume):
  deadreckon is unattended-first with no interactive approval surface; revisit
  with the article's approval-gate worker model.
- **Provider-backed (LLM) compaction summaries**: this milestone elides
  deterministically; semantic summarization needs cost/eval/determinism policy.
- **Persistent bus / WebSocket transport + a long-lived worker registry**: the
  contract is per-call subprocess JSON-over-stdio; a daemon/bus needs
  lifecycle/backpressure design.
- **Seam versioning, capability negotiation, a published seam registry.**
- **Routing built-in telemetry (snapshots/provenance/traces) through the hook
  seam**: today hooks are additive observers; unifying needs the durable-audit
  guarantees preserved.
- **Catalog capabilities beyond `context_window`/pricing** (vision/tools/streaming).
- **A policy seam that can widen the OS sandbox** (deliberately impossible).

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (already in-tree): `serde`/`serde_json` (wire + audit files), `tokio`
(subprocess + timeout, already used by the sandbox), `toml` (config), `chrono`
(timestamps). **No new crates expected.** Tier 2: none. Tier 3: same blocks as
prior riders (no network services, no DB, no bus runtime).

## Engineering invariants (do not violate)

- **No `PipelineState`/`Plan`/`AcceptanceMarker`/`AcceptanceCheckResult`/
  `ProviderEntry` field additions.** Seam config is the `[seams]` table; per-run
  state is `seams.json` / `compaction.jsonl`.
- **The gate is not a seam.** `SeamKind` has no `Gate`; `[seams.gate]` is
  refused; no seam writes `proofs/`, reads `gate/nonce`, or alters
  `marker_signature` inputs. Guarded by P1 and P8.
- **Built-in is the default and the floor.** Unconfigured kind ⇒ behavior
  identical to today (one depth test per seam). The policy seam may only narrow
  the sandbox, never widen it (`policy_seam_cannot_widen_sandbox_floor`).
- **Fail policy is fixed per kind in code** (policy=closed, catalog=open,
  hooks/sink=safe). Changing it changes the contract.
- **Seam workers run sandboxed**; a seam failure never blocks the gate or
  corrupts the audit trail.
- **Compaction determinism is the spec** (resume-safety); changing the algorithm
  changes the contract (`identical_inputs_produce_identical_compaction`).
- **One depth test before each phase implementation.** A phase whose tests were
  never red is suspect; P1 proves the primitive + gate-guard first.
- **No silent expansion.** Anything beyond P1–P11 → `docs/V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing, `cargo fmt --check` clean, and a
  CHANGELOG entry naming the SHA.
- Run focused `cargo test -p <crate>` for touched crates plus `cargo fmt --check`;
  do not run full `make verify` / release / stress suites unless the human asks.
- If a phase reveals a V1-architecture decision (bus transport, approval
  surface), stop and log it in `docs/V1-CANDIDATES.md`; do not expand scope.
- Optional after P11: a short asciinema cast of swapping the policy seam and of
  `--no-seams`, under `/Users/gdc/deadreckon/` demo assets. Skip if it doesn't
  earn it.
