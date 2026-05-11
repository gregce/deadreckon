# deadreckon Primary Flow Gap Analysis

Audit date: 2026-05-11. Scope: `/Users/gdc/deadreckon/` after commit `bac0337`.

| phase | audited hole | evidence | decision / fix direction |
|---:|---|---|---|
| 0 | Default `deadreckon run` is a hardcoded smoke fallback, not a provider-driven agent loop. | `crates/deadreckon/src/main.rs:603` defines `coding_turn_script()`; `crates/deadreckon/src/main.rs:212-219` runs that fixed shell script. | Replace with core turn loop that calls `ProviderRouter::complete`, parses actions, executes sandboxed tools, and only keeps a labeled `--smoke` path. |
| 0 | Provider router is not used for completions in the run loop. | `crates/deadreckon/src/main.rs:166-168` builds a router but only calls `estimate_for_route`; `ProviderRouter::complete` exists in `crates/deadreckon-providers/src/lib.rs:329`. | Run loop must call `complete()` each turn and record LLM request/response traces. |
| 0 | No model-to-tool protocol exists. | `crates/deadreckon-providers/src/lib.rs:65-75` has raw text response fields only; no action/tool-call schema in core. | Add JSON action protocol: `bash`, `write_file`, `done`; feed tool results back as conversation history. |
| 0 | No CLI sub-agent providers exist. | `crates/deadreckon-providers/src/` only has `lib.rs`; no `cli_claude_code.rs` or `cli_codex.rs`; Tier B grep would fail. | Add `cli:claude-code` and `cli:codex` providers implementing `Provider`. |
| 0 | `kill` does not interrupt live work. | `crates/deadreckon/src/main.rs:337-346` only releases lock and marks failed; no child PID or in-flight HTTP cancellation. | Persist process info and use PID signaling for child work; run loop polls kill marker/cancel status. |
| 0 | `resume` does not continue a conversation. | `crates/deadreckon/src/main.rs:350-371` only clears failure/pause and marks planned. | Reconstruct history from JSONL and continue from the next turn. |
| 0 | State cannot represent killed runs or child supervision. | `crates/deadreckon-core/src/state.rs:15-21` has no `Killed`; `PipelineState` has no `child_pids` or `killed_at`. | Extend state schema with `Killed`, `child_pids`, `killed_at`, and history helpers. |
| 0 | Anti-self-attestation gate is absent. | No `dr-gate` binary in `crates/deadreckon/src/bin/`; no `proofs/turn-acceptance.json` validator. | Add separate `dr-gate` binary and validation before completed status. |
| 0 | Provenance IDs are not tied to trace IDs. | `crates/deadreckon/src/main.rs:255` generates a UUID for provenance after the sandbox trace at `:227`; no matching trace field is written. | Each tool trace must carry `tool_call_id`; provenance must reuse that ID. |
| 0 | Demo cast is hand-authored smoke output. | `demo.cast:7-15` contains placeholder run ID `000000...`; `DESIGN.md:72` says it records smoke path. | Replace with an actual recorded or generated cast from real command output after Tier A/B pass. |
| 0 | Keyless tests do not prove agentic behavior. | Existing tests cover state, locks, provider parsing, sandbox wrapper; no `tests/agentic-loop/` or mock-server recorder. | Add mock OpenAI-compatible provider fixtures and integration tests for ≥3 requests/turns/traces/spend lines. |
| 0 | Original UX commitments are incomplete. | V0 rider requires `init`, `config get/set`, actionable `doctor`, default max spend; current `Commands` enum lacks `Init` and `Config` and `run` has no default cap. | Track as original-rider invariant debt; implement if needed for final completion audit. |

Conflicts logged:

- `asciinema` is not installed in the current environment. The final demo must still keep `demo.cast` as asciicast v2 JSON; if live `asciinema rec` is unavailable, generate the cast from captured real command output and record that limitation here.
- Existing untracked `.cursorindexingignore` is outside the task scope and was not created or modified by deadreckon work.
