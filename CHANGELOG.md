# Changelog

## Coherence closure (alpha) - 2026-05-17

- Aligned top-level `attach` and `kill` id handling so run, chain, and plan ids all resolve through the normal lifecycle commands, with shared `attaching to <kind> <prefix>` and `killed <kind> <prefix>` banner wording.
- Clarified help for `attach`, `kill`, and `show` to name run, chain, plan, and `plan-id:task-id` support where the commands already accept those ids.
- Aligned provider setup wording so `doctor`, `detect`, and `providers list` use the same `kind=cli|http|local-http|scripted|custom` tokens and normal help says provider route instead of descriptor.
- Added coherence coverage for the updated help, orchestration help vocabulary, top-level chain attach/kill dispatch, provider kind vocabulary, status key/value layout, shared stderr error rendering, raw ANSI ownership, visual identity helpers, and plan-child show help.
- Refreshed README/HOWTO examples to use canonical `run`, `--branch-name`, `--overwrite`, `--max-spend`, `--git-strategy`, `--all-scopes`, and `--escalate` wording.
- Added `docs/PLAN-NARRATIVE.md` for merged plans so orchestration has one plan-level reading path built from child summaries.

## Semantic merge repair (alpha) - 2026-05-16

- Changed orchestration merge to default to DAG-aware composition, so descendant child artifacts can supersede ancestor file edits without a manual `prefer-child` retry.
- Added automatic bounded merge repair for true parallel conflicts: `merge` writes conflict/request/plan/run sidecars under `merge-proofs/`, invokes a repair provider by default, and can prefer a child file, synthesize conflict paths, or run a normal repair child from `merge-working`.
- Added repair controls for advanced/debug flows: `--no-repair`, `--repair-provider`, `--repair-mode auto|prefer|synthesize|child`, `--repair-attempts`, and `--strategy fail-on-conflict|dag-aware|prefer-child`.
- Added plan events for repair planning, repair start, repair child discovery, repaired merges, and repair failure; `show --why-failed`, plain plan summaries, and `history grep --plan` surface the new repair evidence.
- Updated `orchestrate` started/preflight output to say merge repair is automatic and to carry repair through the one-command flow, with `orchestrate ... --no-repair` kept as a debug-only raw conflict path.
- Added orchestration integration coverage for conflict bundles, repair requests, DAG merge precedence, planner prefer/synthesize/child repair, refusal validation, and headless `orchestrate --yes` auto-repair.
- Updated `docs/AS-BUILT-ARCHITECTURE.md` with the semantic merge repair model and sidecar layout.

## Plan observability (alpha) - 2026-05-15

- Added `plan-events.jsonl` as the orchestration-level event timeline for plan, task, child discovery, merge, failure, completion, and kill lifecycle edges.
- Added plan-event surfacing to `attach <plan-id>`, plain plan summaries, `history grep --plan`, and `show <plan-id> --why-failed`.
- Added plan attach drill-down/back context so a user can enter a selected child run's normal attach view and return to the parent plan/task.
- Hardened plan kill bookkeeping so discovered child run ids are preserved even if a child reaches a terminal state before the kill sweep inspects it.
- Hardened plan attach and kill recovery for partial `plan-events.jsonl` lines, missing child run roots, explicit `b`/Backspace back navigation, terminal failed-plan events, and sidecar-recovered child run ids.
- Hardened full-plan planning so build goals ask for implementation/verification child slices instead of research-only packets, and multiplayer/live/networked goals preview network capability correctly.
- Improved interactive `orchestrate` setup with goal-based mode and child-count recommendations, configured-provider guidance, optional child provider overrides, preflight warnings for research-only build plans, and a run-like started banner with attach/show/plan paths.
- Updated `docs/AS-BUILT-ARCHITECTURE.md` with `§32 Plan Observability` and amended `§22`/`§30` to reflect the file-backed plan event stream and remaining broadcast-bus limit.

## Distribution & self-update (alpha) - 2026-05-15

- Added install receipt and update-check cache files under `~/.deadreckon/` with channel detection for npm, Homebrew, shell, cargo, and source installs.
- Added `deadreckon update --check` plus npm/Homebrew/cargo/source channel routing; source installs refuse with a `try: cargo install --path crates/deadreckon` hint.
- Added shell-channel update backups and in-place swap plumbing through axoupdater, with deterministic backup/failure tests and pruning to the latest three backups.
- Added the cached startup stale-version hint, disabled for non-TTYs, source installs, `doctor`, `update`, and `DEADRECKON_UPDATE_CHECK=0`.
- Added cargo-dist release scaffolding for five OS/arch targets, shell/PowerShell installers, glibc 2.28 Linux metadata, and a push-time `dist plan` workflow check.
- Added guarded macOS Developer ID codesign/notarization steps and public release setup docs for the required Apple secrets.
- Added the npm wrapper package, five per-platform optional dependency templates, no-network receipt postinstall, and npm publish workflow wiring.
- Added Homebrew tap publishing for `gdc/homebrew-tap`, including a formula patch that writes the brew install receipt.
- Added first-run update receipt persistence plus shell-update previews, non-TTY `--yes` enforcement, and post-update doctor hints.
- Updated the as-built architecture docs with the distribution/self-update model and remaining operator release steps.

## Overnight UX (alpha) - 2026-05-14

- Added a shared `ui_card` renderer for run preview, run exit summaries, and completed attach footers with `--plain` / `NO_COLOR` behavior.
- Kept read-only inspection surfaces (`list`, `show`, and `status`) as quieter table/report output so they do not repeat the same run metadata inside card wrappers.
- Added `run --prevent-sleep <auto|on|off>` with macOS `caffeinate`, Linux `systemd-inhibit` re-exec/ready-file handling, run-local sleep metadata, and doctor sleep checks.
- Hardened production git invocations behind `deadreckon-core::git` with `GIT_TERMINAL_PROMPT=0` and commit-family GPG signing disabled.
- Added `spend_summary` replay so subscription or estimated turns render approximate spend with `~` without changing the numeric total.

## Orchestration prompt polish (alpha) - 2026-05-14

- Mined Claude Code's coordinator guidance into deadreckon worker specs: self-contained briefs, no sibling transcript peeking, concrete dependency summaries, correction vs fresh-review guidance, and skeptical reviewer posture.
- Planner prompts now ask for execution-order child DAGs with enough context for each child to run without the parent conversation.
- Plan children now run with `--no-docs`; plan-level summaries remain responsible for orchestration docs, avoiding accidental provider-backed narrator work in child runs.
- The coordinator now records each child run id under `plans/<plan-id>/launch/<task-id>/run-id`, so plan kill can map live child PIDs back to run state before marking children killed.
- Added/kept exact orchestration depth coverage for review-mode extension, child PID snapshots, kill cascade, prompt hygiene, and plan lifecycle friendliness.

## Coherence pass (alpha) - 2026-05-14

- Added one glossary for status words; `running` replaces `executing` in user-visible run and phase surfaces while stored enum variants stay unchanged.
- Added one style module and prompt builder; raw ANSI escapes now live in `ui.rs`, and every confirmation prompt uses the same `? question [Y/n]: ` or `? question [y/N]: ` shape.
- Added one key/value block for run and plan summaries, with lowercase keys and aligned colons.
- Standardized alpha flag names with hidden aliases: `--escalate`, `--overwrite`, `--anyway`, `--all-scopes`, `--global`, `--branch-name`, `--into`, `--max-spend`, and `--git-strategy`.
- Preserved the cyan `deadreckoning` banner, course strip, magenta IDs, spend gauge colors, and chain glyphs, with applied steps now using `◉`.
- Aligned attach and kill banners across runs, chains, and plans.
- Aligned `show --why-failed` and `chain show --why-failed` through one failure-summary layout, and added JSON output for list/status/show/doctor/provider/library inspection surfaces.
- Made `export` the visible copy-out word in help, completion prompts, docs, and refusal text while keeping `materialize` as an alpha compatibility alias/internal marker.

## Orchestration milestone (alpha) — 2026-05-13

- Renamed the multi-child orchestration mode from `split` to `full-plan`, added `deadreckon orchestrate review` and `deadreckon orchestrate full-plan` mode subcommands, and require `--yes` after the preflight in headless execution.
- Added file-backed orchestration plans with task DAG validation, provider roles, worker specs, coordinator messages, child summaries, and plan child markers without changing `PipelineState`.
- Added `deadreckon plan`, `fork`, `merge`, and review-mode `orchestrate` so a common coder -> reviewer -> merge flow can complete end to end.
- Added explicit planner/default-child/per-child/coder/reviewer provider resolution and persisted overrides into `plan.json`.
- Added merge conflict detection with `--strategy prefer-child --prefer-child <idx>` and promoted merge artifacts with `deadreckon-plan-manifest.json`.
- Added plan-aware `attach`, `show`, and `kill` so plan IDs participate in the normal lifecycle, including a basic multi-pane plan TUI with child drill-in.
- Added `deadreckon history grep <pattern>` for plan-aware trace/provenance search and `deadreckon show <id> --why-failed` for run or plan failure summaries.
- Review-mode orchestration now launches the reviewer lane as an `extend` of the coder run, preserving parent context and `extended_from_parent` trace lineage.
- Independent full-plan children now start as ready batches, with coordinator PID snapshots for every live child in the batch.
- Plan attach now surfaces child turn/status, spend or token accounting, latest trace activity, acceptance/gate state, capability preview, and final merged gate status in both the TUI and non-TTY summary.
- Headless orchestration flags now apply consistently: `run --plain --quiet` is accepted, `run --quiet` emits no success stdout, `attach --plain` bypasses the TUI, and `plan`/`fork`/`merge` preserve plain output.
- Added provider-backed planning depth coverage: planner prompts are asserted read-only, `--n` outside `2..=6` refuses before saving, one-task provider decompositions are rejected, and explicit planner/default-child/per-child providers are persisted.
- Coordinator launches now refresh each child worker spec with completed dependency summaries, so dependent child prompts include concrete predecessor context instead of only a plan-time dependency id.
- Merge manifests now include an explicit task graph, child summary paths, provider roles, and coordinator message counts for audit without replaying child transcripts.
- Added `show --why-failed` depth coverage for completed runs, failed run RCA traces, and plan blocker messages.
- Added P10 friendliness coverage for `try:` footers, quiet/plain headless output, review-mode provider hints, and plan ready/blocked task counts.
- Verified with focused orchestration tests plus core plan round-trips, clippy on the orchestration target, and `cargo fmt --check`; a broadcast-backed plan event stream remains a future slice.

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
