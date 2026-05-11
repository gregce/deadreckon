# deadreckon Primary Flow Gap Analysis

Audit date: 2026-05-11. Scope: `/Users/gdc/deadreckon/`.

## Primary-Flow Findings

| finding | status | evidence |
|---|---|---|
| Default run was a fixed local script instead of a provider-driven loop. | Resolved. | `crates/deadreckon-core/src/turn_loop.rs` calls `ProviderRouter::complete`; Tier B grep confirms no `coding_turn_script`, `hardcoded_smoke`, or `fn smoke_turn` remains under `crates/`. |
| Provider router existed but was unused by `run`. | Resolved. | `crates/deadreckon/src/main.rs` builds a router for normal runs, and `run_turn_loop` records `llm.complete` traces per turn. |
| No model-to-tool protocol existed. | Resolved. | `turn_loop.rs` accepts JSON `bash`, `write_file`, and `done` actions and feeds tool results into `history.json`. |
| CLI sub-agent providers were missing. | Resolved. | `crates/deadreckon-providers/src/cli_claude_code.rs`, `cli_codex.rs`, and `cli_common.rs` implement `cli:claude-code` and `cli:codex`; tests use fake binaries and verify sandbox wrapping. |
| `kill` could not interrupt live work. | Resolved for the harness process and sandbox child pids. | `state.json` persists `child_pids`; sandbox/provider pid files live under `child-pids/`; `deadreckon kill` signals both sources and marks status `killed`. |
| `resume` did not continue execution. | Resolved. | `deadreckon resume` reloads state/history and re-enters `run_turn_loop`; integration tests verify history survives. |
| Anti-self-attestation gate was absent. | Resolved. | `crates/deadreckon/src/bin/dr-gate.rs` writes `proofs/turn-acceptance.json`; core validates `run_id`, producer, and pass status before completion. |
| Provenance IDs were not tied to traces. | Resolved. | Tool traces carry `tool_call_id`; provenance records reuse those ids; integration tests assert every provenance id appears in traces. |
| Demo cast was hand-authored placeholder output. | Resolved. | `demo.cast` now contains asciicast v2 JSON generated from real release-binary `doctor/run --provider cli:codex/list/attach/show/undo` output using `DEADRECKON_HOME=/Users/gdc/deadreckon/.deadreckon-smoke`. |
| Keyless tests did not prove agentic behavior. | Resolved. | `cargo test --workspace` includes mock OpenAI-compatible provider tests, three-turn integration, kill, resume-history, acceptance-marker, and fake CLI-provider tests. |

## Logged Decisions

- API keys are not required for Tier A/B verification. The keyless paths are the OpenAI-compatible mock provider in tests and the explicit `--smoke` scripted provider for local release-binary checks.
- Tier C was executed with live `cli:codex` using `codex exec --ephemeral` and deadreckon's outer `sandbox-exec` wrapper. The successful run id is `59c57e4565704135a9982789d0754803`.
- `asciinema` is not installed in the current environment. `demo.cast` is therefore generated from captured real command output and kept in asciicast v2 format.
- Existing untracked `.cursorindexingignore` is outside the task scope and was not created or modified by deadreckon work.
