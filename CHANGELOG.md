# Changelog

## Codebase modes (alpha) — 2026-05-11

- P1: Added codebase mode records, fresh-mode metadata, and deterministic mode resolution plumbing without changing `PipelineState`.

## Lifecycle ergonomics

Phase commits: `4481617`, `556897d`, `91ab9a6`.

- Added `deadreckon materialize <run-id> [--dest <path>] [--force] [--include-manifest]` to copy completed library artifacts to user-owned paths with `.deadreckon/parent.json` provenance and library `.materialized-to` reverse markers.
- Added `deadreckon extend <run-id> "<new-goal>"` to create a fresh run from a completed parent artifact, seed the working tree, prepend a parent summary into `history.json`, and record lineage through marker files plus a synthetic trace.
- Added lifecycle hints after completed `run`/`attach`, `--no-hints` suppression, `list` materialization status, and `show` parent-lineage output.
- Kept `PipelineState` unchanged; lifecycle lineage lives in marker files.

## 0.1.0 - Robustness Milestone (alpha)

Implementation commit: `cec49f3`.

- Hardened the run loop with broadcast/file-backed events, per-turn timers, cancellation tokens, wall-clock CLI spend accounting, partial-trace resume, and `resume --from-turn`.
- Hardened sandbox execution with generated Seatbelt/bwrap policy inputs, tmp `$HOME`, network denial, persisted profiles, and adversarial path/network tests.
- Hardened acceptance by moving `dr-gate` to `acceptance.yaml`, signing markers with a run-local nonce, and refusing forged self-attestation.
- Hardened import normalization for Claude Code, Codex, and Cursor histories into deadreckon traces/provenance.
- Hardened multi-run coordination with scope-qualified lock files and same-scope refusal tests.
- Hardened library promotion with post-gate atomic move, manifest writing, and crash recovery.

Still thin: provider pings in `doctor` are intentionally conservative unless explicitly enabled, and the TUI uses durable event replay for cross-process attach because Tokio broadcast is in-process.
