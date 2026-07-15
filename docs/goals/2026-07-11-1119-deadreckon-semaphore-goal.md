GOAL: Stop driving the CLI agents blind — read their signal flags. Today BOTH workhorse drivers scrape raw stdout as the whole response, report zero tokens (`usage: 0/0` hardcoded in `cli_codex.rs` and `cli_claude_code.rs` — CLI spend rollups are fiction), and start a fresh conversation every turn. Both tools publish structured contracts deadreckon ignores. Codex: `exec --json` streams typed JSONL events (`thread.started` with a thread id, `turn.completed` with usage, `item.*` per tool call), `--output-last-message`, `--output-schema`, `exec resume <thread_id>`. Claude Code: `-p --output-format stream-json` streams JSONL events, the final `result` event carries usage, reported cost, the session id, and the answer itself, and `--resume <session_id>` continues. This slice builds the shared machinery once (capability probe, per-run session file, tolerant-parse doctrine, live flight ingestion) and two thin event mirrors on top: real tokens for both, resume for both, answers from the structured result, schema-constrained output where the binary supports it (codex today; capability-gated elsewhere). Land this slice named Semaphore.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-07-11-1119-deadreckon-semaphore-rider.md` — shared infra, both event mirrors, session file, resume rules, eleven phases, depth tests.
- `crates/deadreckon-providers/src/{cli_codex.rs,cli_claude_code.rs,types.rs,cli_common.rs}` — both one-shot drivers hardcode `usage: 0/0`.
- `crates/deadreckon-runtime/src/turn_loop.rs` (:272); `crates/deadreckon-core/src/flight.rs` (both import schemas — today post-hoc only).
- `/Users/gdc/codex/codex-rs/exec/src/exec_events.rs` + `exec/src/cli.rs`; `claude -p --help` on this machine for the stream-json/resume surface (record real fixtures at implementation time). Prior riders hold; Semaphore takes AS-BUILT §50.

**Posture.** Stable track. No `PipelineState` schema changes — the thread/session id lives in `provider-session.json` in the run root, provider-scoped. Parsing is tolerant: unknown events recorded, never fatal; unparseable output degrades to today's raw-stdout behavior with a caveat trace; capability probing (once per process, from `--help`) disables absent features instead of erroring — old binaries keep working. Resume is per-run; distinct runs never share a conversation. Tokens land; dollars don't move — `SpendEstimate` stays subscription/`$0`; claude's reported cost is trace detail only. No new crates. No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → V1-CANDIDATES.

**The contract, read not scraped.**

- Events: `--json` / `--output-format stream-json`; per-provider mirrors with `#[serde(other)]` tolerance; tool-call items append to the flight ledger live (import stays idempotent).
- Usage: codex `turn.completed.usage`, claude `result.usage` → real `ProviderUsage` for both.
- Resume: persist `thread_id` / `session_id` on turn 1; `exec resume` / `--resume` after; a vanished session retries fresh once with a reset trace.
- Answer: codex `--output-last-message` file; claude `result.result`; raw stdout only in degraded mode.
- Structure: `ProviderRequest.output_schema` → `--output-schema` where probed capable; caveat elsewhere.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `make verify` green → conventional-commit → CHANGELOG. P11 adds AS-BUILT §50.

**Verification.**

- Every rider depth test present and passing; `make verify` green each commit (scripted fake `codex` AND fake `claude` binaries; no live CLIs in CI).
- Both fakes prove: usage flows into spend ledgers; turn 2 resumes the persisted session; malformed JSONL degrades with a caveat.
- `release/preflight-real.sh` (cli:claude-code cli:codex) passes end-to-end with the new drivers.

**Stop when** verification passes, AS-BUILT §50 + V1-CANDIDATES + a `Semaphore (stable)` CHANGELOG section are updated, committed locally.
