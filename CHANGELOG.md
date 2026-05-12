# Changelog

## UX consolidation — 2026-05-11

- Added an in-TUI Markdown docs view for completed runs. Press `d` in `attach` to toggle a styled `RUN-NARRATIVE.md` rendering instead of dropping to plain terminal output.
- Made `deadreckon apply` idempotent when a run branch has already landed on the target branch; it now reports `already applied` and can still perform `--cleanup` instead of failing on an empty commit.
- Made `deadreckon list` project-scoped by default, with `--all` for global history and `--full` for script-friendly full values.
- Added `latest` / `last` run-id aliases for user-facing run commands, resolved to the latest run in the current project.
- Added `deadreckon status` with `next` as an alias; running `deadreckon` with no subcommand now shows the current project's latest run and next action.
- Added `deadreckon cleanup` with `prune` as an alias for cleaned, stale, or completed worktree cleanup.
- Added friendlier command aliases: `export` for `materialize` and `discard` for `abandon`.
- Improved root and subcommand help text, terminal output formatting, TUI layout, completion action footer, and scoped workflow hints.

## Apply/list usability — 2026-05-11

- Made run-id arguments accept unique prefixes so compact `deadreckon list` IDs can be reused directly.
- Made `deadreckon list` compact by default with `--full` for scripts and exact full values.
- Added `deadreckon apply --autostash` for dirty checkouts and `--cleanup` to remove the run worktree/branch after a successful apply.

## Self-documenting runs (alpha) — 2026-05-11

- Added run-start doc scaffolding under `working/.deadreckon/docs/` with stoa-shaped `RUN-NARRATIVE.md`, `RUN-AS-BUILT.md`, `RUN-DECISIONS.md`, `_incremental.jsonl`, and `polish.json`.
- Added deterministic per-turn narrative chunks, phase coalescing, decision detection, trace/snapshot citations, worktree commit SHA capture, and optional `AS-BUILT-DELTA.md`.
- Added the `run-narrator` skill, provider-backed end-of-run polish with JSON retry, SHA-256 idempotency, diff coverage retry, and nonfatal polish failure statuses.
- Added `deadreckon doc <run-id>`, `list` DOCS status, doc-aware completion actions, extend-parent narrative updates, and generated `apply` commit bodies from run docs.
- Added 48 rider-named depth tests in `crates/deadreckon/tests/self_documenting.rs`.

## Codebase modes (alpha) — 2026-05-11

- P1: Added codebase mode records, fresh-mode metadata, and deterministic mode resolution plumbing without changing `PipelineState`.
- Added codebase-aware `run` defaults: clean git repos now run in an isolated `git worktree` on a `dr/...` branch, while the old empty-working-dir behavior remains behind `--fresh`.
- Added explicit copy (`--from`), worktree (`--worktree`, `--base`, `--branch`, `--allow-dirty`), and in-place (`--in-place --i-know-its-a-lot`) modes with single-screen preview and `--preview` / `--yes` scripting paths.
- Added worktree lifecycle verbs: `deadreckon apply <run-id>` with squash/merge/cherry-pick strategies and `deadreckon abandon <run-id>` with branch/worktree cleanup.
- Integrated codebase modes into `list`, `show`, `materialize`, `extend`, `undo`, run completion prompts, and TUI completion actions. Worktree runs now hint apply/abandon; copy/fresh runs continue to hint materialize/extend.
- Added worktree-aware `extend`: child worktree runs branch from the parent `dr/...` branch and record `parent_branch` in `codebase.json`; in-place parents refuse with a `run --in-place` hint.
- Added depth coverage for every rider-named codebase test, including dirty/refusal preflight, preview and non-git prompt UX, worktree/copy/in-place modes, apply conflict handling, abandon force cleanup, lifecycle hints, and extend integration.

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
