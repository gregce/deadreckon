# Changelog

## Copilot and Pi providers (alpha) - 2026-05-13

- Added built-in descriptor-backed `cli:copilot` and `cli:pi` providers with subscription auth, detection/install hints, model flags, sandbox read/write roots, and generic CLI routing coverage.
- Added Copilot session-state and Pi session JSONL TUI ingest, including cwd matching, tool/result/thinking rows, and context token telemetry without rewriting provider-owned logs.
- Kept verification focused on provider registry, CLI routing, detect/list UX, provider JSONL parsing, fmt, and crate-local clippy; the long full-suite commands remain out of this goal's default loop.

## Provider CLI ingest (alpha) — 2026-05-13

- Added optional descriptor `[ingest]` metadata and backfilled Codex/Claude Code so TUI provider activity is resolved by registry descriptors instead of provider-id conditionals.
- Added canonical tool-category normalization and schema-keyed provider activity parsers for Codex, Claude Code, Gemini JSON/JSONL, and OpenCode file-mode logs.
- Added descriptor-backed generic CLI launch through `exec_template`, including model flags, prompt delimiters, sandbox placeholders, descriptor sandbox writes, and subscription wall-time spend.
- Added built-in `cli:gemini` and `cli:opencode` descriptors with detection/install hints, `providers list` coverage, registry-order `init --no-confirm`, and stable `cli:` output filenames.
- Kept verification focused on provider/CLI/TUI surfaces; `make verify`, release builds, smoke, stress, and full-workspace tests remain out of this goal's default loop.

## Provider registry (alpha) — 2026-05-13

- P1: Added descriptor TOML, `ProviderDescriptor`, `ProviderRegistry`, override loading from `providers.d`, and shell-like custom command parsing; existing built-in providers now have compiled-in descriptors.
- P2: Existing provider defaults now come from descriptor TOML, `ProviderKind` supports generic descriptor IDs, and CLI sandbox write allowlists are descriptor-backed while preserving current adapter behavior.
- P3: Added descriptor-backed provider probes and `deadreckon detect [<id>]`, including PATH/version checks, credential checks, JSON output, and install `try:` hints.
- P4: Added `deadreckon providers list` with configured-only/default and `--all`, `--models`, and `--full` views backed by the registry.

## Workspace hygiene (alpha) — 2026-05-12

- P1: Captured smoke and public-surface baselines, added invariant tests, and made `make smoke` run fresh/non-interactive for deterministic verification.
- P2: Added warn-only `[workspace.lints]`, `clippy.toml`, per-crate lint inheritance, and a clippy warning snapshot for the P3 cleanup pass.
- P3: Promoted core workspace clippy rules to deny, removed the temporary warning snapshot, and added deny-level lint tests plus a `-D warnings` clippy guard.
- P4: Added `rustfmt.toml` and guard tests for the dedicated format commit and clean `cargo fmt --check`.
- P5: Tuned release/dev profiles and captured a release binary size baseline with slack guard.
- P6: Routed internal crates through `[workspace.dependencies]` and guarded the internal cargo metadata DAG.
- P7: Added library-crate print refusal while keeping the binary crate exempt.
- P8: Added registry-shape guard tests for `deadreckon-core`'s library root; no public surface changed.
- P9: Regrouped provider/runtime/sandbox library roots into registry shape and preserved the public re-export set.
- P10: Added exhaustive retryable/fatal taxonomy methods to core, provider, and sandbox errors while keeping runtime errors on the core taxonomy.
- P11: Updated `docs/AS-BUILT-ARCHITECTURE.md` with §29 Workspace Hygiene and amended §22 to mark the hygiene rider as structural, not a prior thin-item closure.

## Doc depth (alpha) — 2026-05-12

- Per-turn capture extended: full provider response (50 KB cap), per-file diff samples with largest-hunk excerpts, and bash stdout/stderr (10 KB cap each).
- Turn-end documentation is now an explicit run event for both CLI sub-agent turns and JSON-action provider turns; `_incremental.jsonl` is checkpointed before completion polish/acceptance/promotion.
- Templated narrative no longer truncates the title at 40 chars; per-turn outcomes no longer cut at 200 chars; phase prose synthesizes per-turn summaries instead of "deadreckon progressed through turn N".
- Component-table inference uses path rules (`crates/`, `skills/`, `docs/`, manifests, tests, routes, migrations, CI); generic "Project files" rows are not emitted.
- Process topology ASCII is generated only when at least three top-level directories changed.
- Provider-backed doc polish now defaults to four repo skills: `narrator-overview`, `narrator-phases`, `narrator-as-built`, and `narrator-decisions`, each with a 16K output budget and per-subcall status/cost recorded in `polish.json` schema v2.
- `deadreckon run` and `deadreckon doc --polish` expose doc-provider selection (`--doc-provider`) with flag/config/subscription/run-provider resolution, preview output, preflight `--budget-cap` refusal, and post-polish subcall summaries.

## Lifecycle help polish — 2026-05-12

- Added `deadreckon finish` / `done` as a completion intent command that routes completed worktree runs to `apply`, fresh/copy runs to `export`, and in-place runs to review guidance.
- Added lifecycle-oriented `--help` text to every top-level verb, including real `chain` subcommand examples and focused `deadreckon chain help <topic>` output.
- Expanded friendly aliases across the lifecycle: `setup`, `settings`, `check`, `runs`, `artifacts`, `keep`, `clean`, `follow-up`, `docs`, `watch`, `stop`, `continue`, `restore`, and `inspect`.

## Autonomous chaining (alpha) — 2026-05-11

- Added the chain data model foundation: `chain.json`, `chain-events.jsonl`, chain path helpers, chain lock task-key convention, and `RunPromoted` events after promotion.
- Added the first user-facing chain flow: `chain "..."`, `--from-file`, `--from-stdin`, `--draft`, preview/confirm, `chain run`, `chain list/status/show/attach`, and a foreground conductor that runs sequential steps through existing run/apply paths.
- Added provider-backed `chain plan` / `chain expand`, including JSON-array validation, duplicate/single-step refusal, and planner spend recording under the chain directory.
- Added chain policy depth: branch-policy stack/base behavior, aggregate per-step spend allocation, and chain hooks for `pre-step`, `post-step`, `on-promote`, and `on-chain-end` with hook events.
- Added chain-step context markers to inner runs and surfaced them in single-run `show` / non-TTY attach summaries.
- Added lifecycle depth for `latest`/`last`, `resume`, `extend`, `redo`, `undo`, pause refusals, and cascade `chain kill` that terminates the live inner run and conductor.
- Added the multi-step `chain attach` TUI with policy header, step timeline, chain activity stream, pause/kill/redo/extend controls, and single-run `attach` chain drill-out via `c`.
- Added policy gate coverage for allowlist refusal, manual apply pause, merge branch policy, on-fail stop/skip, and configurable circuit breaker thresholds.
- Completed the rider depth-test matrix under exact test names and tightened resume-after-manual-pause, quiet auto-apply, bounded undo, TTY auto-attach, preview diff, and aggregate wall-clock behavior.
- Updated `docs/AS-BUILT-ARCHITECTURE.md` with §28 Chains and refreshed §17/§22 chain accounting.

## Hardening v2 (alpha) — 2026-05-11

- Added `docs/AUDIT-2026-05-11.md` mapping the original 25 unmet needs to current evidence and the P2-P10 closure plan.
- Replaced TUI polling-only attach with event-backed attach: same-process broadcast plus cross-process `events.jsonl` replay.
- Hardened cross-process cancellation with durable cancel markers, provider abort coverage, and kill-storm tests.
- Hardened partial-trace resume and `resume --from-turn` so trace, spend, and snapshot tails are truncated together.
- Added durable `sandbox.toml` per run, per-tool sandbox policy, and refusal provenance for disallowed filesystem/network actions.
- Expanded `acceptance.yaml` support with required/optional checks, file/content/build/shell checks, and signed per-check proof results.
- Made `doctor` more actionable across providers, sandboxes, OS, permissions, disk, and opt-in provider pings.
- Added `deadreckon library list|search|show` for promoted artifacts, including goal/date filters and promoted-doc grep.
- Hardened Claude Code/Codex/Cursor import normalization with source metadata, deterministic imported run IDs, stable Cursor ordering, malformed JSONL errors, and committed show-output golden tests.
- Polished CLI help/status/completion UX, including command groups, run health/library/disk status blocks, and `DEADRECKON_HINTS=0`.
- Updated `docs/AS-BUILT-ARCHITECTURE.md` and `docs/AUDIT-2026-05-11.md` with the Hardening v2 closure evidence.

## UX consolidation — 2026-05-11

- Added an in-TUI Markdown docs view for completed runs. Press `d` in `attach` to toggle a styled `RUN-NARRATIVE.md` rendering instead of dropping to plain terminal output.
- Made `deadreckon apply` idempotent when a run branch has already landed on the target branch; it now reports `already applied` and can still perform `--cleanup` instead of failing on an empty commit.
- Added explicit provider/model affordances: `run --model`, `extend --model`, model-aware run previews, and `deadreckon config provider|model` shortcuts.
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
