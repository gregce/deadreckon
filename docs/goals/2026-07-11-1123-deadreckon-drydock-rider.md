# deadreckon — Drydock Rider (deterministic tests, observable retries)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-07-11-1123-deadreckon-drydock-goal.md`.
It supersedes nothing in prior riders — their invariants still apply. This
rider adds: the **`MockModel`** wiremock SSE fixture with scripted scenarios,
**observable retry with jittered backoff** in the provider router, **insta**
snapshot conventions, and **paused-time** rewrites of the flakiest timing
tests.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `~/.deadreckon`.
Codex reference `/Users/gdc/codex/codex-rs` (pattern grounding:
`core/tests/common/responses.rs`, `core/src/{util.rs,responses_retry.rs}`).

## Posture (decided — do not redesign)

- **Maturity stays stable** (lands under a `Drydock` CHANGELOG section).
- **Nothing is replaced.** Smoke provider, characterization goldens, and the fake-CLI fixtures stay. MockModel adds the HTTP-provider dimension and scripted multi-turn adversity (rate limits, malformed SSE, mid-stream disconnects) that none of them cover. insta converts inline assertion piles and pins NEW surfaces; goldens keep their update-flag workflow.
- **Retry is an event or it didn't happen.** Every retry appends a `TraceRecord { event: "provider.retry", detail: { attempt, max, delay_ms, reason, retry_after_honored } }`. Silent retry is a bug by definition (depth-tested: N retries ⇒ N trace rows).
- **Retry spec:** `delay = min(cap, base * factor^attempt) * jitter(0.9..1.1)`; base 500ms, factor 2.0, cap 30s, max 4 attempts; a server `Retry-After` (seconds or http-date) replaces the computed delay for that attempt. Only `retryable: true` errors loop (the existing `http.rs` taxonomy: 408/429/5xx + transport). Non-HTTP providers are untouched.
- **Paused-time conversions preserve the theorem.** A converted test must still fail if the behavior regresses; convert only tests whose subject is tokio-time-driven (the attach tick/budget family). Tests gated on real child processes (PTY, smoke runs) keep real time with generous deadlines — that lesson is already paid for.
- **Dev-deps only.** wiremock + insta enter `[dev-dependencies]`; the shipped binary's dependency tree is unchanged (guarded: `cargo tree` check in a test or CI grep).
- **No `PipelineState` schema changes. No `git push`. No V1 invention. Edits stay inside `/Users/gdc/deadreckon`.**

## MockModel (test-support crate module)

Location: `crates/deadreckon-providers/tests/support/mock_model.rs` (shared via
a `#[path]` include or a small `dev-deps` support crate if sharing across
crates demands it — decide at P1, log the choice).

```rust
pub struct MockModel { server: wiremock::MockServer, script: Vec<Scenario> }
pub enum Scenario {
    Respond(CannedResponse),            // SSE stream: text and/or tool_use blocks
    RateLimit { retry_after: Option<Duration> },   // 429 (+ Retry-After header)
    ServerError(u16),                    // 5xx once
    DisconnectMidStream,                 // SSE cut after N events
    MalformedEvent,                      // unparseable SSE frame
}
impl MockModel {
    pub async fn start(script: Vec<Scenario>) -> Self;   // scenarios consumed in order
    pub fn base_url(&self) -> String;                     // plugs into provider config
    pub fn request_count(&self) -> usize;
    pub fn saw_prompt_containing(&self, needle: &str) -> bool;
    pub fn nth_request_body(&self, n: usize) -> serde_json::Value;
}
```

Shapes both wire dialects the HTTP providers speak today (inspect `http.rs`
at HEAD; anthropic-style at minimum, openai-style if the router supports it —
scope to what exists).

## Retry (provider router)

```rust
// deadreckon-providers/src/retry.rs
pub fn backoff_delay(attempt: u32, retry_after: Option<Duration>) -> Duration; // pure, seedable jitter for tests
pub struct RetryPolicy { pub max_attempts: u32 /*4*/, pub base: Duration, pub factor: f64, pub cap: Duration }
```

The router's `complete` wraps HTTP provider calls: on `retryable` error and
attempts remaining → trace `provider.retry` → sleep(delay) → retry. The trace
sink is threaded from the existing `ProviderRequest` context (the same
channel traces use today — verify at HEAD; if traces are runtime-side, emit
via a callback field on `ProviderRequest`, additive and optional).
Cancellation tokens interrupt the sleep. Attach/spine surface retries through
the existing trace-reading attention path — zero new UI plumbing required
beyond a wording test.

## insta conventions

- `cargo insta` documented in `docs/TESTING.md` (new short doc): review flow, `INSTA_UPDATE=auto` ban in CI.
- Snapshot dirs: colocated `snapshots/` per test module (insta default).
- Conversion targets (five noisiest assertion piles, verified at HEAD): the spine band render asserts, course card layout asserts (complement the golden), why-panel render asserts, report markdown asserts, help-overlay asserts. Each conversion keeps one semantic assert (presence of load-bearing fact) alongside the snapshot.

## Paused-time targets

Convert (subject is tokio-time): `input_to_frame_stage_recorded_and_budgeted`
neighbors that sleep, `event_storm_coalesces_frames_within_budget`,
`idle_attach_backs_off_polling`, retry-delay tests (new). Do NOT convert:
PTY/codebase tests, smoke-run tests, anything asserting on real child wall
time. Each conversion commit states the preserved theorem in its message.

## Phases (eleven)

Each phase: named depth test(s) first (red) → implement → `make verify` green
→ conventional-commit → CHANGELOG line naming the SHA.

### P1 — Dev-dep wiring + MockModel skeleton
- wiremock + insta into dev-deps (workspace table), DEPENDENCIES.md entries; MockModel starts and serves one canned SSE response.

Depth tests:
- `mock_model_serves_scripted_sse_response`
- `shipped_binary_tree_has_no_dev_deps`   (guard)

### P2 — Scenario script engine
Depth tests:
- `scenarios_consume_in_order`
- `rate_limit_scenario_sets_retry_after_header`
- `disconnect_mid_stream_cuts_after_n_events`

### P3 — Semantic asserts
Depth tests:
- `saw_prompt_containing_matches_request_bodies`
- `nth_request_body_roundtrips_json`

### P4 — backoff_delay (pure)
Depth tests:
- `backoff_grows_exponentially_with_jitter_bounds`
- `retry_after_overrides_computed_delay`
- `cap_bounds_worst_case_delay`

### P5 — Router retry loop + trace
Depth tests:
- `retryable_error_retries_up_to_max_with_traces`   (N retries ⇒ N provider.retry rows)
- `non_retryable_error_fails_immediately`
- `cancellation_interrupts_retry_sleep`
- `retry_after_header_is_honored_and_marked`

### P6 — Full-loop MockModel integration
- A real turn_loop run against MockModel: tool-call turn → 429+Retry-After → success; spend/ledger/trace assertions end-to-end.

Depth tests:
- `run_completes_against_scripted_mock_model`
- `mock_run_ledgers_record_retry_and_usage`

### P7 — Adversity coverage
Depth tests:
- `mid_stream_disconnect_is_retryable_and_recovers`
- `malformed_sse_event_fails_with_taxonomy_not_panic`

### P8 — insta conventions + first conversions
- docs/TESTING.md; convert the five assertion piles (each keeps one semantic assert).

Depth tests (by existence):
- `spine_band_snapshot_matches`
- `why_panel_snapshot_matches`
- (+ three more per the conversion list)

### P9 — Paused-time conversions
Depth tests (converted, not new names — the commit proves red→green under `start_paused`):
- storm/budget/idle-backoff family on paused time; a new `retry_delays_are_deterministic_under_paused_time`

### P10 — Friendliness/observability polish
- Retry visible in attach attention wording (pinned), `show` renders retry counts per turn; error footer for exhausted retries names the provider and last reason with a `try:` (`deadreckon providers check <name>`).

Depth tests:
- `exhausted_retries_footer_names_provider_and_try_line`
- `attention_wording_for_reconnecting_is_pinned`

### P11 — Architecture doc + CHANGELOG (doc only)
- Insert `## 54. Drydock: Deterministic Testing and Observable Retry` into AS-BUILT (MockModel, retry spec + taxonomy table, insta/paused-time doctrine); cross-reference §47 (budgets) and §50 (fake-codex fixture kinship).
- CHANGELOG:
  ```
  ## Drydock (stable) — deterministic tests, observable retries — <date>
  - a scripted wiremock SSE MockModel drives the full run loop in CI
    (tool calls, rate limits, disconnects); provider retries are jittered,
    Retry-After-aware, bounded, and traced as provider.retry events; insta
    snapshots and paused-time timing tests replace the flakiest asserts.
  ```
- V1-CANDIDATES: proptest/fuzzing of ledger parsers, otel export, MockModel for streaming-delta app-server dialect (Rudder's fixture grows from this).

## Out of scope (explicitly → V1-CANDIDATES)

- Replacing goldens or the smoke provider.
- OpenTelemetry/metrics export (codex `otel` crate pattern).
- Property-based testing / fuzzing.
- Retry for CLI providers (their binaries own retry; only HTTP loops here).
- CI parallelism restructuring.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1: tokio (`test-util` feature for start_paused — verify it's enabled for
dev). Tier 2 (dev-dependencies, log in DEPENDENCIES.md with pins):
`wiremock`, `insta`. Tier 3 (blocked): mockito (wiremock chosen), nextest
(separate decision), any runtime-tree additions.

## Engineering invariants (do not violate)

- **No silent retry** — the N-retries⇒N-traces test is the contract.
- **Dev-deps never reach the shipped binary** — guarded by test.
- **Jitter is seedable in tests** (pure `backoff_delay` takes RNG or is tested on bounds, not exact values).
- **A converted test still proves its original theorem** — stated per commit.
- **MockModel scenarios are consumed exactly once, in order** — leftover scenarios fail the test (catches under-assertion).
- **One depth test before each phase.**

## Process invariants

- Phased local commits only. No `git push`.
- Each phase: depth tests green + CHANGELOG SHA line.
- Before P11: run `make verify` ten times consecutively; record the streak in the P11 commit message (the flake bar).
- V1 discoveries logged, not implemented.
