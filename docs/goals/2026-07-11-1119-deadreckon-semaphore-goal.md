GOAL: Stop driving codex blind — read its signal flags. Today `cli:codex` shells one-shot `codex exec`, scrapes raw stdout as the whole response, reports zero tokens (`usage: 0/0`, so spend/turn rollups for codex runs are fiction), and starts a fresh codex conversation every turn, re-sending context in the prompt. Codex publishes a structured contract deadreckon ignores: `exec --json` streams typed JSONL events (`thread.started` with a thread id, `turn.completed` with real token usage, `item.*` per tool call), `--output-last-message` isolates the final answer, `--output-schema` constrains it to a JSON Schema, and `codex exec resume <thread_id>` continues a persisted conversation. This slice upgrades the driver to that contract: parse events, account tokens, persist the thread id per run and resume it turn-over-turn, feed items to the flight recorder live, and expose schema-constrained structured output at the provider boundary. Land this slice named Semaphore.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-07-11-1119-deadreckon-semaphore-rider.md` — event schema, session file, resume rules, eleven phases, depth tests.
- `crates/deadreckon-providers/src/cli_codex.rs` — the current one-shot driver (args, `codex_sandbox_mode`, `usage: 0/0`).
- `crates/deadreckon-providers/src/types.rs` — `ProviderRequest` (:89), `ProviderResponse` (:100), `ProviderUsage` (:70); `cli_common.rs` `run_cli`/`run_cli_with_options`.
- `crates/deadreckon-runtime/src/turn_loop.rs` (:272 builds the request) and `crates/deadreckon-core/src/flight.rs` (the `codex-cli` schema — today post-hoc import only).
- `/Users/gdc/codex/codex-rs/exec/src/exec_events.rs` (`ThreadEvent`, `ThreadItemDetails`), `exec/src/cli.rs` (`--json`, `-o`, `--output-schema`, `resume`). Prior riders hold; Semaphore takes AS-BUILT §50.

**Posture.** Stable track (0.6.0). No `PipelineState` schema changes — the codex thread id lives in a file (`provider-session.json` in the run root), not a field. The JSONL parser is tolerant: unknown event types are recorded, never fatal; if `--json` output is unparseable the driver degrades to today's raw-stdout behavior with a caveat trace. Resume is per-run: a run's turns share one codex thread; distinct runs never share threads. No new crates. No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → V1-CANDIDATES.

**The contract, read not scraped.**

- Events: drive `codex exec --json`, parse `ThreadEvent` lines; `turn.completed.usage` populates `ProviderUsage` (ends the 0/0 era); `item.*` events append to the run's flight ledger as they stream.
- Resume: capture `thread_id` from `thread.started`, persist to `provider-session.json`, subsequent turns invoke `codex exec resume <thread_id>`; a vanished thread falls back to a fresh conversation with a caveat.
- Final answer: `--output-last-message <file>` is the response content — stdout noise never again masquerades as the answer.
- Structure: `ProviderRequest` gains optional `output_schema`; when set the driver passes `--output-schema` so planner/critic/reshape-style callers get schema-enforced JSON from codex itself.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `make verify` green → conventional-commit → CHANGELOG. P11 adds AS-BUILT §50.

**Verification.**

- Every rider depth test present and passing; `make verify` green each commit.
- A scripted fake `codex` binary proves: usage flows from `turn.completed` into spend ledgers; turn 2 invokes `resume` with the persisted thread id; malformed JSONL degrades with a caveat instead of failing the turn.
- `release/preflight-real.sh cli:codex` still passes end-to-end (start → turns → gate → apply → kill/resume) with the new driver.

**Stop when** verification passes, AS-BUILT §50 + V1-CANDIDATES + a `Semaphore (stable)` CHANGELOG section are updated, committed locally.
