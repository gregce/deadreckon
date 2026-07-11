GOAL: Put the harness in drydock — test the agent loop against a scripted model, pin surfaces with real snapshot tooling, make time and retries deterministic and observable. Today deadreckon's tests lean on the smoke provider (one canned happy path), hand-stabilized goldens, wall-clock timing tests that flake under load (the PTY timeout, the storm budgets), and provider retries that are invisible and unprincipled (`http.rs` marks `retryable` but no backoff loop or Retry-After honor exists). codex-rs shows the kit: a wiremock SSE mock model driving the full agent loop deterministically, insta snapshots everywhere, `#[tokio::test(start_paused)]` for timing, and retries surfaced as events with jittered backoff honoring `Retry-After`. This slice lands all four: a `MockModel` fixture with scripted multi-turn behaviors, insta for new snapshots, paused-time rewrites of the flakiest tests, and observable retry in the provider router. Land this slice named Drydock.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-07-11-1123-deadreckon-drydock-rider.md` — mock-model API, retry spec, paused-time targets, eleven phases, depth tests.
- `crates/deadreckon-providers/src/{http.rs,router.rs}` — the `retryable` flags with no loop; `crates/deadreckon-runtime/src/turn_loop.rs` (where a retrying router call lands).
- `crates/deadreckon/src/tui_tests.rs` (storm/budget tests to convert), `crates/deadreckon/tests/codebase.rs` (the PTY-timeout lesson), `tests/characterization.rs` (goldens that stay — insta complements, does not replace them).
- `/Users/gdc/codex/codex-rs/core/tests/common/responses.rs` (`ResponseMock`), `core/src/util.rs` (`backoff`), `core/src/responses_retry.rs` (observable retry). Prior riders hold; Drydock takes §54.

**Posture.** Stable track. The smoke provider and characterization goldens are NOT replaced — MockModel covers the HTTP provider path and multi-turn scripted behaviors goldens can't (tool-call sequences, mid-stream errors, rate limits); insta is for NEW snapshots and converted inline `assert!(text.contains…)` piles, never a bulk golden migration. Retry becomes: jittered exponential backoff (factor^attempt ± 10%), `Retry-After` honored when present, bounded attempts, and every retry appended as a trace/event (`provider.retry` with attempt/max/delay/reason) so the spine's "anything wrong?" can see it — never silent. Paused-time conversions must preserve what each test proves. New dev-deps are Tier 2 (wiremock, insta) — dev-dependencies only, never in the shipped binary. No `PipelineState` schema changes. No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → V1-CANDIDATES.

**The kit.**

- `MockModel` — wiremock-based OpenAI/Anthropic-shaped SSE server with a scripted scenario API (`respond_with`, `then_rate_limit(retry_after)`, `then_error(500)`) and semantic asserts (`saw_prompt_containing`, `request_count`).
- Retry: one shared `backoff(attempt)` + router loop, capped, observable; `http.rs` `retryable` flags finally consumed.
- insta: snapshot module conventions + `cargo insta` workflow documented; the five noisiest assertion piles converted.
- Paused time: the attach input/storm budget tests and PTY-adjacent waits rewritten on `start_paused` where the subject is tokio-time-driven.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `make verify` green → conventional-commit → CHANGELOG. P11 adds AS-BUILT §54.

**Verification.**

- Every rider depth test present and passing; `make verify` green each commit.
- A full run loop completes against MockModel with a scripted tool-call turn, a 429+Retry-After, then success — retries visible in the trace ledger with correct delays (paused time).
- Ten consecutive `make verify` runs green (the flake bar this slice exists to raise).

**Stop when** verification passes, AS-BUILT §54 + V1-CANDIDATES + a `Drydock (stable)` CHANGELOG section are updated, committed locally.
