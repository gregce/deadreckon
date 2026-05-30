# Changelog

## Decompose (maintainability refactor) - 2026-05-30

- P1 (`7ef2d5c`): Added a full-binary CLI characterization net for plan creation,
  quiet plan creation, start full-plan preview JSON, chain status, off-TTY attach,
  and canonical `try:` refusal footers, with normalized goldens under
  `crates/deadreckon/tests/goldens/characterization/`.
- P2 (`a6f8d57`): Added shared integration-test helpers under
  `crates/deadreckon/tests/common/` and migrated duplicated tempdir,
  command-construction, stdout/stderr, and success-assertion helpers without
  changing test assertions.
- P3a (`a601ae3`): Lifted `acceptance_integrity_tests` out of `main.rs` into a
  sibling `src` test module without changing test names or widening runtime
  visibility.
- P3b (`9a9892d`): Lifted `acceptance_render_tests` out of `main.rs` into a
  sibling `src` test module while preserving its four render-focused unit test
  names and private-helper access.
- P3c (`098b1cf`): Lifted `campaign_spawn_tests` out of `main.rs` into a
  sibling `src` test module while preserving its campaign orchestration helper
  coverage and private-helper access.
- P3d (`e15eb86`): Lifted `effortless_consistency_tests` out of `main.rs` into a
  sibling `src` test module while preserving its cross-surface consistency
  assertions and private-helper access.
- P3e (`02d8396`): Lifted `flight_cli_tests` out of `main.rs` into a sibling
  `src` test module while preserving CLI flight/log fixture coverage and
  private-helper access.
- P3f (`8e0f276`): Lifted `self_improve_pr_tests` out of `main.rs` into a
  sibling `src` test module while preserving self-improvement PR adapter
  coverage and private-helper access.
- P3g (`bf64b50`): Lifted `tui_tests` out of `main.rs` into a sibling `src`
  test module while preserving attach, plan, narrative, provider-log, and
  guided-start TUI coverage plus private-helper access.
- P4a (`1768d17`): Created the private `commands/` facade, moved the chain
  command family into `src/commands/chain/`, and routed the `main_inner` chain
  branch through `commands::chain` while keeping shared attach infrastructure in
  the crate root.
- P5a (`58cfbd4`): Moved the acceptance and def-done command family into
  `src/commands/acceptance.rs`, preserving the existing `main_inner` dispatch
  and keeping acceptance render helpers in the crate root for the later TUI
  split.
- P5b (`eb55274`): Moved the supervised `run` command body into
  `src/commands/run.rs`, with `main_inner`, `start`, and `try` now calling the
  private command module while shared preview/render helpers remain in the crate
  root.
- P5c (`d72bc9d`): Moved the `init` command body into `src/commands/init.rs`,
  keeping shared completion, config rendering, and provider-detection helpers in
  the crate root for later cleanup phases.
- P5d (`05e96ba`): Moved the campaign command family into
  `src/commands/campaign.rs`, keeping root/start/orchestrate/attach/show/kill
  call sites routed through the private command module.
- P5e (`d1e66f2`): Moved the attach command dispatch and terminal event loops
  into `src/commands/attach.rs`, leaving pure render/state helpers in the crate
  root for the P6 TUI extraction.
- P5f (`5ec29f0`): Moved the merge command entrypoint and CLI repair-strategy
  parsing into `src/commands/merge.rs`, keeping shared merge/repair helpers in
  the crate root for plan dependency composition and the later plan split.
- P5g (`7416c0b`): Moved the orchestrate front-door and interactive
  mode/provider selection helpers into `src/commands/orchestrate.rs`, keeping
  plan creation, fork, and shared render helpers in the crate root for the
  remaining plan split.
- P5h (`9c2a8cf`): Moved the plan/fork command family and child-launch
  orchestration helpers into `src/commands/plan.rs`, leaving plan result docs
  and shared TUI render helpers in the crate root for later phases.

## Effortless (production release) - 2026-05-28

- P1 (`c81b617`): Added the whole-surface friendliness contract table and `docs/FRIENDLINESS-AUDIT.md`, with depth tests proving every canonical top-level verb has one row per six-clause contract item.
- P2 (`bacf76f`): Added `deadreckon try`, a keyless local smoke run that uses the real turn loop and signed `dr-gate` proof, then prints the proof/story/lineage block and one next command.
- P3 (`bbf1e73`): Factored the proof-block renderer and surfaced the signed proof/story/lineage block on completed run exit cards.
- P4 (`e20cf54`): Made `deadreckon start` adopt a single detected subscription CLI inline, keep the provider picker for multiple detected CLIs, and refuse with `deadreckon try`/provider setup recovery when none are available.
- P5 (`663843f`): Added a shared primary-action slot to cards and made exit cards, status, and finish lead with one primary action while demoting secondary lifecycle actions.
- P6 (`85f1d31`): Swept spend and gate verdict rendering so exit cards, status, finish, plan child details, and campaign child summaries show honest subscription spend and per-check gate results.
- P7 (`0bef0f4`): Added opt-in `[notify]` parsing, bounded native/command/webhook channels, redacted notification context, and `notify.jsonl` attempt records.
- P8 (`823945b`): Fired enabled notifications on accepted, paused-at-cap, and failed lifecycle outcomes while disabled configs stay silent.
- P9 (`10dd47b`): Added bounded provider-backed goal-shape recommendations for `start`, preview-scoped classifier records, optional campaign `--n`, and editable campaign preflight controls.
- P10 (`7425883`): Unified the verified-run glossary, changed completed exit cards to the `VERIFIED` verdict, expanded refusal `try:` footer coverage, and added command-notification failure recovery hints.
- P11 (`c37ca2b`): Documented AS-BUILT §37 for the Effortless contract, updated shipped-vs-thin accounting, and logged the palette/localization/template/notifier/classification/onboarding deferrals in V1-CANDIDATES.

## Campaign Orchestration (production release) - 2026-05-28

- P1: Added `deadreckon-core::campaign` module with the nesting `Lineage` record, the `CAMPAIGN_MAX_DEPTH = 2` hard cap, and a `guard` that refuses a campaign at depth >= 1 or a sub-goal that cycles to an ancestor `task_key`/scope.
- P2: Added the file-backed `Campaign`/`SubGoal` model (`campaign.json`) with `build_sub_goals` decomposition validation (exactly-N planner output, non-empty, distinct sub-goals) and `Campaign::new` reusing `validate_task_count` (2..=6).
- P3: Added the sub-orchestrator spawn (`build_sub_orchestrator_command`, lineage env transport + `sub-result.json` sidecar) reusing the plan-child isolation idiom, and wired `orchestrate full-plan` to report its merged result when launched by a campaign.
- P4: Added `run_campaign_fork`, a sequential sub-orchestrator driver that records `campaign-events.jsonl` (`campaign_started`/`sub_launched`/`sub_merged`/`sub_failed`) and marks a failed sub without aborting its siblings.
- P5: Added the tree-budget allocator (`allocate_budget`, even split with remainder-to-first), aggregate-spend exhaustion enforcement that refuses the next sub launch (`tree_budget_exhausted` + `budget_exhausted` event), and the unbounded-budget warning.
- P6: Extracted the shared `mergeable_run_files` enumeration (used by plan merge unchanged) and added `compose_roots`/`compose_result_runs` for independent sub-results; a cross-sub file conflict is reported so the campaign fails rather than silently overwriting.
- P7: Added the gate-verdict roll-up (`CampaignRollup`, `worst_of`, `rollup_permits_completion`, `build_rollup`): any refused or unmerged leaf makes the whole campaign refused (the no-laundering invariant). The roll-up is bound into the result run's marker signature, so editing `campaign-rollup.json` after signing invalidates the marker.
- P8: Added `campaign_can_complete`: a campaign reaches completion only when every sub merged and the roll-up permits it; a refused sub never reaches a clean completed state.
- P9: Added the top-level `deadreckon campaign <goal> --n <2..=6>` verb (peer to run/orchestrate/chain): decomposes via the planner, guards depth/cycles, previews (`--preview`), forks N sub-orchestrators, rolls up verdicts, composes one promoted result run with a `deadreckon-campaign-manifest.json`, and refuses to promote on any refused leaf or cross-sub conflict.
- P10: Surfaced campaigns in `attach <campaign-id>` (sub rows + roll-up + breadcrumb), `show <campaign-id> --why-failed` (refused/caveat subs), and `kill <campaign-id>` (cascades into each sub-plan, then marks the campaign killed) via `resolve_campaign`.
- P11: Documented campaign orchestration in AS-BUILT §36 and logged depth>2, cross-level dependencies/merge-repair, recursive attach, planner-chosen breadth, and richer tree-budget strategies in V1-CANDIDATES.

## Tamper-Evident Gate (production release) - 2026-05-28

- Refuse to sign when a run edits `acceptance.yaml` or a compiled check carries a suppression pattern; downgrade to a surfaced caveat when a run modifies a check-covered test/target file; bind the tamper record into the marker signature.
- Surface per-check verdicts and a tests-modified flag on the exit card, status, and `--why-failed`.
- Render honest subscription spend with `not metered (subscription)` for subscription-only routes and a subscription note for mixed routes.

## Production release posture - 2026-05-28

- Replaced current product docs and generated run-doc front matter that still labeled DeadReckon as alpha with production-release posture language.
- Kept dated alpha changelog entries and old goal briefs as historical records while moving new user-facing wording to compatibility-release terminology.
- Removed live CLI and narrative fallback messages that described current behavior as an alpha slice.

## Plan Doc Consolidation (production release) - 2026-05-28

- Added consolidated orchestration plan docs: `PLAN-NARRATIVE.md`, `PLAN-AS-BUILT.md`, `PLAN-DECISIONS.md`, `PLAN-CHILDREN.md`, and `PLAN-DOCS-MANIFEST.json`.
- Built a plan-doc input collector that reads child run docs, task summaries, worker specs, merge repair evidence, and final result inventory in task-graph order.
- Added provider-backed plan-doc consolidation with bounded input bundles, citation validation, invented-path checks, and deterministic fallback when provider output is unavailable or invalid.
- Materialized plan docs into merged libraries, plan apply worktrees, and exported artifacts without copying child internal logs.
- Rewrote synthetic plan-result apply `RUN-*` docs as wrappers that point to consolidated `PLAN-*` docs instead of showing empty zero-turn run docs.
- Extended `deadreckon doc`/`docs` and `show` so plan ids and plan-result wrapper runs resolve to plan documentation.

## Production command model (alpha) - 2026-05-27

- Reframed default help around the production flow: `start`, `attach`, `status`, `list`, `finish`, setup, and control commands.
- Kept power-user and advanced verbs callable and discoverable through `deadreckon help-all`, per-command help, and completions without crowding the first screen.
- Made `deadreckon start` history-aware for repos with completed promoted runs: TTY users can choose a follow-up, while preview/JSON output shows exact extend, review, and full-plan commands.
- Added done-criteria transparency to interactive `start` when project criteria already exist, with keep/view/check/update/cancel choices before launch.
- Updated README, HOWTO, AS-BUILT, the user-facing matrix, and focused tests without adding runtime schema or durable config.

## Start picker (alpha) - 2026-05-27

- Added selection-first TTY prompts to `deadreckon start` for launch mode, detected/configured provider routes, missing done-criteria action, non-git and dirty source handling, and final launch confirmation.
- Kept scripted surfaces deterministic: non-TTY, `--json`, `--plain`, `--quiet`, and `--yes` never enter the picker and continue to emit structured output or `try:` recovery lines.
- Let interactive users choose a detected CLI provider ephemerally for a launch without writing provider config.
- Routed selected provider routes into existing run/review/full-plan dispatch, with previews showing alpha role reuse for review and full-plan orchestration.
- Documented the picker behavior and remaining V1 deferrals without adding durable launch profiles or runtime state schemas.

## Guided first use (alpha) - 2026-05-26

- Reframed README/HOWTO first-run examples around provider-neutral `deadreckon start`, while keeping direct `run` and `orchestrate` paths documented for power users.
- Added a `start lifecycle` footer after successful guided launches so the new front door ends with exact attach, status, kill, and finish commands for the created run or plan.
- Locked `deadreckon start` JSON, plain, and quiet output behavior with focused coverage for structured recovery, ANSI-free previews, and quiet successful launches.
- Connected confirmed `deadreckon start` launches to the existing run and orchestrate handlers while keeping start previews state-free.
- Added source-mode recovery to `deadreckon start`, including `--fresh`, `--worktree`, `--from`, and `--allow-dirty` parsing plus non-git and dirty-worktree recovery that points to valid guided commands.
- Wired `deadreckon start` into shared provider setup and done-criteria recovery so missing providers, detected-but-unconfigured CLIs, and absent done criteria end with concrete `try:` lines instead of the placeholder launcher error.
- Shared launch preview rows for start, run, and orchestrate so previews name path, provider, done criteria, workspace, watch, stop, and finish actions.
- Added deterministic `start --mode auto` launch-decision heuristics for run, review, and full-plan paths.
- Added the visible `deadreckon start` parser and help surface for the guided front door.
- Clarified DeadReckon's audience as the harness around agent CLIs for unattended, sandboxed, auditable work, and pointed first-use help/docs at `deadreckon start`.
- Documented the guided first-use architecture and V1 deferrals in AS-BUILT and V1 candidates without adding durable launch state.

## TUI Responsiveness (alpha) - 2026-05-26

- Added in-memory attach tick budgets and loop-stage timing for run, plan, and chain attach surfaces, with provider narrative refresh classified as background work for the responsive attach scheduler.
- Moved run narrative attach refresh onto a coalesced background job so manual `r` redraws without awaiting the provider and detach cancels in-flight provider work.
- Routed run attach event and quiet-threshold narrative refreshes through the same background job, preserving failure notices until a later refresh replaces them.
- Moved plan narrative attach refresh onto a plan-keyed background job so manual, event, and quiet-threshold refreshes coalesce while child drill-in and detach cancel in-flight provider work.
- Replaced run attach live-file collection with an attach-specific inventory walker that prunes heavy cache/profile directories before descent and caps displayed files without losing total counts.
- Added attach-owned JSONL tail caches for run spend, trace, and flight activity streams so live run attach parses appended complete rows instead of rereading whole files each tick.
- Added live attach provider-log scan throttling so current flight rows delay fallback root scans, fallback matches are cached by freshness, and matched provider logs invalidate on mtime changes.
- Added run and plan narrative projection caches for attach rendering so redraws reuse covered projections, preserve stale provider snapshots, and avoid appending narrative snapshots from render paths.
- Added incremental chain activity tailing for chain attach, including partial-line tolerance and status hints when event reads fall behind.
- Added focused responsiveness smokes for slow narrative refreshes, large worktrees, and max-size chain timelines without invoking full release or stress suites.
- Documented the TUI responsiveness alpha contract and known limits: no attach daemon, no shared cross-surface broadcaster, and no diagnostic dashboard yet.

## Narrative Attach (alpha) - 2026-05-26

- Added `deadreckon attach --view narrative` for cited run and plan overviews, with `n` to return to raw activity and `v` to cycle architecture, agents, files, evidence, and no-visual modes.
- Added the `Narrated` operator heading for narrative attach projections so the calmer view has a clear product label.
- Defaulted provider-backed narrative refresh to local Claude Code on `sonnet`, while keeping `--narrative-provider` as an explicit route override.
- Added `--no-narrative-provider` for deterministic-only narrative attach when provider refresh is not desired.
- Added file-backed run/plan narrative projections under `narrative/state.json`, `narrative/snapshots.jsonl`, and `narrative/architecture-graph.json` without changing `PipelineState`.
- Added evidence-backed ASCII map rendering for run architecture, plan agents, touched files, and evidence chains, including plain/JSON attach output.
- Added redacted provider refresh on manual `r`: attach builds bounded prompts, validates structured claims and graph labels against known evidence, enforces budget/cadence guards, and falls back to deterministic stale facts when refresh is unavailable or rejected.
- Added TTY narrative-view refresh triggers for meaningful run and plan evidence, including errors, completions, tool milestones, docs checkpoints, child-run discovery, task terminal states, and merge-repair milestones.
- Added quiet-threshold TTY refresh attempts for running runs/plans when no meaningful narrative event arrives for the configured quiet window.
- Added narrative refresh triggers for acceptance running/pass/fail transitions so test evidence can update the operator summary without requiring raw-log watching.
- Added plan narrative roll-up from child run narrative snapshots so plan agent rows can cite the child's latest operator summary before falling back to child run state.
- Added plan file-map roll-up from child narrative graphs so plan-level visuals can show cross-agent touched file evidence.
- Kept plan narrative footer controls visible even when the selected child run is not available yet, preserving the one-key path back to raw activity.
- Added focused run/plan TUI render coverage for narrative panes, citations, agent rows, and visual-map hints.
- Added focused plain/JSON narrative attach coverage, including deterministic non-TTY fallback behavior and the explicit chain unsupported response.
- Added acceptance proof/progress citations to run narrative projections so failed done criteria point at the immutable acceptance artifact instead of only generic run state.
- Added focused run TUI mode coverage for narrative/activity toggling, visual cycling, narrow-terminal footers, and completed-run docs staying separate from narrative attach.
- Added command-level narrative attach smokes for flight-backed run output, file/evidence visuals, plan child refs, two-child plan agent visuals, and completed-run docs separation.
- Added final narrative attach guards for stale provider-refresh fallbacks, attach help copy, provider-neutral examples, and visual-map privacy/no-color documentation.
- Added focused coverage for narrative schemas, malformed snapshot tolerance, redaction, claim validation, graph validation, provider refresh validation, cadence/budget decisions, deterministic fallback, and plain map rendering.

## Self-Improvement Loop (alpha) - 2026-05-26

- Added file-backed learning state under `DEADRECKON_HOME/learning/` for episodes, deterministic signals, provider-backed insights, proposals, redacted bundles, candidates, evals, PR dry-run/open events, and local policy.
- Added `deadreckon learn index`, `deadreckon learn report`, required-reflection `deadreckon learn propose`, and redacted `learn export`/`learn import-bundle` so proposal creation uses a provider only after deterministic redacted evidence exists.
- Added `deadreckon improve self <proposal-id|goal-file>` preview, isolated-worktree candidate execution, evidence scoring, high-risk path classification, PR dry-run body generation, diff redaction checks, and a fake-testable live PR adapter gated behind explicit `--open-pr`.
- Added focused core and CLI coverage for learning paths, schema versioning, episode idempotency, bundle redaction/hash checks, signal extraction, proposal reflection validation, PR risk gating, learning CLI output, public-surface stability, PR dry-run, fake PR adapter behavior, and self-improve preview.

## Provider flight recorder and checkpoint rewind (alpha) - 2026-05-25

- Added durable `flight-manifest.json`, `flight-events.jsonl`, `checkpoints/<id>/`, and `rewind-events.jsonl` files for CLI-backed provider runs, with normalized provider-native events and delta checkpoints.
- Wrapped CLI provider execution in a polling flight recorder sidecar that ingests descriptor logs, watches working-tree changes, captures tool/quiet/exit checkpoints, and marks rerun sessions as superseded.
- Added `deadreckon show <run-id> --flight`, `deadreckon show <run-id> --file <path>`, and preview-first `deadreckon rewind` target resolution with hash-guarded `--apply`.
- Routed attach/TUI provider activity through flight events while keeping descriptor provider-log lines as the live fallback during long CLI subprocesses.

## Provider and done-criteria setup unification (alpha) - 2026-05-24

- Added a shared runtime setup resolver for provider roles and done-criteria sources so `init`, `config provider`, `run`, `extend`, `resume`, `orchestrate`, and doc polish use the same source labels, unknown-provider refusals, credential/install hints, and preview vocabulary.
- Switched run/orchestrate previews from `gate` to user-facing `done criteria` rows while preserving `.deadreckon/acceptance.yaml` as the technical file name and signed `dr-gate` as the enforcement mechanism.
- Updated `--acceptance` help text to describe done-criteria files, kept hidden `acceptance` compatibility wording advanced, and added focused coverage for unknown provider refusal plus run/orchestrate done-criteria preview parity.

## Descriptor import hardening (alpha) - 2026-05-20

- Reworked `deadreckon import` around descriptor-backed provider transcript discovery, concrete session selection, import manifests, and normalized trace/provenance events while preserving Cursor SQLite import.

## Implementation notes (alpha) - 2026-05-18

- Added root `implementation-notes.html` seeding for new runs, with required Design decisions, Deviations, Tradeoffs, and Open questions sections.
- Updated the default run prompt and CLI sub-agent prompt to frame work as `Implement the SPEC` and require the live notes file to stay current while files change.
- Made `RUN-DECISIONS.md` the converged implementation decision ledger by rendering the same four notes sections plus a separate evidence-filtered multi-alternative decision details section.
- Added done-time freshness checks so JSON-action providers and CLI sub-agents cannot complete after documentable implementation changes until `implementation-notes.html` is current.
- Updated `narrator-decisions` and split polish merging so implementation notes can feed the four interpretation sections without turning every note into a multi-alternative decision.
- Pointed lifecycle/doc hints toward `deadreckon doc <run-id> --kind decisions` as the primary inspection path for implementation interpretation.

## Orchestration live UX (alpha) - 2026-05-18

- Added shared orchestration role and dependency summaries across plan creation, orchestrate preflight/start, fork completion, plan attach summaries, and merge completion.
- Added provider role tables with route/model/source/notes rows for planner, default child, child overrides, coder, reviewer, and merge repair roles.
- Added explicit parallelism/dependency summaries that show which children start now, which wait, and which tasks unblock downstream work.
- Replaced terse merge repair plan summaries with structured repair detail covering mode, attempts, provider, conflict paths, sidecar paths, repair run status, latest repair event, and next action.
- Moved plan attach onto a `PlanEventBus` feed that replays `plan-events.jsonl`, tolerates partial/malformed event rows, emits plan snapshots, and multiplexes child and repair run events into the plan activity stream.
- Standardized plan attach footer grammar around detach, focus, child-run entry, back navigation, and `try:` lines.

## Coherence closure (alpha) - 2026-05-17

- Aligned top-level `attach` and `kill` id handling so run, chain, and plan ids all resolve through the normal lifecycle commands, with shared `attaching to <kind> <prefix>` and `killed <kind> <prefix>` banner wording.
- Clarified help for `attach`, `kill`, and `show` to name run, chain, plan, and `plan-id:task-id` support where the commands already accept those ids.
- Aligned provider setup wording so `doctor`, `detect`, and `providers list` use the same `kind=cli|http|local-http|scripted|custom` tokens and normal help says provider route instead of descriptor.
- Added coherence coverage for the updated help, orchestration help vocabulary, top-level chain attach/kill dispatch, provider kind vocabulary, status key/value layout, shared stderr error rendering, raw ANSI ownership, visual identity helpers, and plan-child show help.
- Refreshed README/HOWTO examples to use canonical `run`, `--branch-name`, `--overwrite`, `--max-spend`, `--git-strategy`, `--all-scopes`, and `--escalate` wording.
- Added `docs/PLAN-NARRATIVE.md` for merged plans so orchestration has one plan-level reading path built from child summaries.
- Rendered top help and `help-all` from one command catalog, with tests that catch duplicate rows and catalog entries that drift away from clap commands.
- Clarified the `help-all` discovery policy so documented advanced commands are distinct from compatibility aliases kept inline on canonical rows.
- Standardized `--plain` help across run, orchestration, lifecycle, and inspection commands as "without TUI, spinner, or ANSI affordances."
- Standardized cross-project flag help on "all project scopes" while keeping provider `--all` scoped to provider inventory.
- Renamed visible update override help from `--force` to `--anyway`, keeping `--force` as a hidden alpha alias.
- Aligned branch target wording so `run` names worktree branches with `--branch-name`, `apply`/`finish` target branches with `--into`, and apply output says changes landed `into` the target branch.
- Scoped strategy vocabulary so `merge --strategy` means plan composition, `apply`/`finish --git-strategy` means git apply behavior, and chain help separates `--apply-mode` from per-step `--apply-strategy`.
- Added a `help-all` output/scripting policy and aligned help for `--yes`, `--no-confirm`, `--quiet`, `--plain`, `--json`, and `--no-hints`.
- Added a provider-role glossary to `help-all` and aligned orchestration/doc help around provider routes for planner, child, coder, reviewer, repair, and documentation roles.
- Clarified cleanup help so it names temporary run worktrees/branches as its target and explicitly excludes plan state, promoted library artifacts, and exported directories.
- Made plan merge/result output keep the plan primary, with result run and artifact library labeled as secondary implementation details.
- Moved the CLI style facade into `ui.rs` and added coherence coverage so status tone mapping and public style helpers have one source of truth.
- Added standard JSON envelope fields across representative machine-readable surfaces and exposed `plan --json` for scriptable plan creation.
- Split note, warning, paused, and failed/killed style intents, and routed extended-run terminal outcomes through status tones.
- Rendered run, extend, and resume start summaries through the shared key/value block instead of bespoke provider/docs/state lines.
- Added a `help-all` spend-cap glossary for run, per-child, aggregate chain, and doc polish caps.
- Closed the user-facing matrix as an alpha record and moved larger output-layout, orchestration, provider/done-criteria, and snapshot work to V1 candidates.
- Made integration-test temp roots worktree-relative so coherence verification can run from a detached worktree.

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
