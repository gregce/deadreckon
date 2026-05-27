# User-Facing Surface Matrix

**Status:** Refreshed against the local working tree on 2026-05-27.
**Scope:** The user-visible CLI, prompts, summaries, TUI labels, JSON/plain modes, help text, docs-facing terminology, and orchestration surfaces in `/Users/gdc/deadreckon`.
**Method:** Source audit of `crates/deadreckon/src/{cli.rs,main.rs,ui.rs,prompt.rs}` and `crates/deadreckon-core/src/{glossary.rs,state.rs,chain.rs,plan.rs}`, plus the current AS-BUILT and goal docs.
**Read this as:** the alpha coherence closure record. The prior matrix cited commit `455b91a` and listed 108 issues; the closure fixes the accidental user-facing drift and leaves larger template/palette/orchestration redesigns as explicit V1 deferrals.

## Fixed Since The Prior Audit

| Area | Current state | Evidence | Notes |
|---|---|---|---|
| Status glossary | A central user-facing glossary exists. Stored `Executing` still serializes as `executing`, but user labels render as `running`. | `crates/deadreckon-core/src/glossary.rs:1` | Applies to run, phase, chain, chain step, plan, and plan task labels. |
| Run/phase display | `RunStatus` and `PhaseStatus` `Display` use the glossary. | `crates/deadreckon-core/src/state.rs:22` | Old `executing`/`running` UI split is mostly closed. |
| Plan status display | `Forked` renders as `running`; `Merged` renders as `completed`. | `crates/deadreckon-core/src/glossary.rs:62` | Good user word, historical stored enum retained. |
| Shared style helper | `ui.rs` owns `Tone`, `Stream`, ANSI rendering, the `ui_*` facade, status tone mapping, TUI palette, hints, and key-value blocks. | `crates/deadreckon/src/ui.rs:1`, `crates/deadreckon/tests/coherence.rs:41` | Raw styling and public CLI style wrappers now have one owner. |
| Status tone semantics | Status labels route through `ui::status_tone`; `failed`/`killed`, `paused`, `warning`, and `note` are separate style intents. | `crates/deadreckon/src/ui.rs:132`, `crates/deadreckon/tests/coherence.rs:73` | Extended-run and done-criteria failure lines no longer use the warning facade. |
| Card policy | Cards are scoped to run preflights, run exit summaries, and completed attach footers; list/status/history/show stay table or report shaped. | `docs/AS-BUILT-ARCHITECTURE.md:1732`, `crates/deadreckon/src/ui_card.rs:1` | This preserves richer transition summaries without making inspection surfaces noisy. |
| Wait banner builder | CLI wait output and TUI footer progress share the named `deadreckoning` course strip helpers with golden coverage for `* ^ . -`. | `crates/deadreckon/src/main.rs:12347`, `crates/deadreckon/src/main.rs:21729`, `crates/deadreckon/src/main.rs:22386` | Keep the visual identity, but construction is named and tested. |
| Prompt helper | `prompt::open` and `prompt::confirm` provide a shared confirmation shape. | `crates/deadreckon/src/prompt.rs:1` | The old doc polish default-marker bug is fixed. |
| Error hints | `error_hint` returns actionable strings and uses discovered config paths. | `crates/deadreckon/src/main.rs:147` | Generic provider/core errors now get a fallback hint. |
| `def-done` naming | Top help and command help use `deadreckon def-done`, not `deadreckon done`. | `crates/deadreckon/src/main.rs:845`, `crates/deadreckon/src/cli.rs:189` | The screenshot miss is fixed in the current source. |
| Help discovery | Top help and `help-all` render command rows from one catalog. Default help now teaches the production model (`start`, `attach`, `status`, `list`, `finish`, setup, and control), while `help-all` is the full map. | `crates/deadreckon/src/main.rs`, `crates/deadreckon/tests/coherence.rs` | Catalog tests verify row uniqueness, command existence, audience classification, and the primary-vs-advanced split. |
| Advanced command discovery | `help-all` explicitly explains that advanced commands remain callable and discoverable outside short help, while compatibility aliases stay inline on canonical rows. | `crates/deadreckon/src/main.rs`, `crates/deadreckon/tests/coherence.rs` | `run`, `orchestrate`, `chain`, `plan`, `fork`, `merge`, `extend`, `apply`, `export`, `doc`, `show`, `history`, `learn`, and `improve` stay available without dominating the first screen. |
| History-aware `start` | Repos with completed promoted history surface follow-up and new pass options. TTY users can choose a prior run to extend; previews/JSON include exact extend/review/full-plan commands. | `crates/deadreckon/src/main.rs`, `crates/deadreckon/tests/orchestrate.rs` | This adds no runtime schema and skips in-place or unpromoted runs. |
| Done-criteria prompt transparency | Existing done criteria are no longer an opaque TTY carry-forward in `start`; users can keep, view, check, update, or cancel before launch. | `crates/deadreckon/src/main.rs`, `crates/deadreckon/src/cli.rs` | Direct `def-done show/check/<text>` remains the explicit command surface. |
| Plain output flag | All `--plain` help uses one definition: "Plain output without TUI, spinner, or ANSI affordances." | `crates/deadreckon/src/cli.rs:545`, `crates/deadreckon/tests/coherence.rs:207` | Command-specific behavior is still implemented per command; the user-facing flag definition is shared. |
| Cross-scope flags | Run, chain, history, cleanup, and library cross-project help says "all project scopes"; provider `--all` remains provider inventory. | `crates/deadreckon/src/cli.rs:869`, `crates/deadreckon/tests/coherence.rs:70` | This keeps `--all` for ordinary cross-project listing/search and `--all-scopes` for chain/cleanup compatibility-sensitive surfaces. |
| Force aliases | Visible help uses intent-specific flags: `--escalate`, `--overwrite`, and `--anyway`; old `--force` spellings stay hidden alpha aliases. | `crates/deadreckon/src/cli.rs:917`, `crates/deadreckon/tests/coherence.rs:70` | `update --anyway` replaces the last visible primary-command `--force`. |
| Branch target flags | Worktree runs expose `--branch-name`; apply and finish expose `--into`; apply output says work landed `into` the target branch. | `crates/deadreckon/src/cli.rs:532`, `crates/deadreckon/src/main.rs:12911`, `crates/deadreckon/tests/coherence.rs:60` | Hidden `--branch` aliases remain for one alpha, but help and output use the canonical words. |
| Strategy flag family | `merge --strategy` is plan composition, `apply`/`finish --git-strategy` is git apply behavior, and chain help separates `--apply-mode` from per-step `--apply-strategy`. | `crates/deadreckon/src/cli.rs:735`, `crates/deadreckon/tests/coherence.rs:122` | Hidden `--strategy` aliases for apply/finish remain for one alpha; user-facing help scopes the terms. |
| Prompt-skip flags | `help-all` documents `--yes` as preflight preview acceptance and `--no-confirm` as destructive/follow-up confirmation skipping; command help follows that split. | `crates/deadreckon/src/main.rs:1118`, `crates/deadreckon/src/cli.rs:548`, `crates/deadreckon/tests/coherence.rs:161` | `run` keeps both because it has a preflight plus separate safety confirmations. |
| Output mode flags | `help-all` documents `--quiet`, `--plain`, `--json`, and `--no-hints` precedence; quiet text is shared and JSON remains inspection/list-only in alpha. | `crates/deadreckon/src/main.rs:1118`, `crates/deadreckon/tests/coherence.rs:161` | `--json` wins over styling/hints where present; `DEADRECKON_HINTS=0` is the process-level hint override. |
| Provider role flags | `help-all` documents provider roles for primary run, planner, child, coder, reviewer, documentation, and repair routes; orchestration/doc help uses provider-route wording. | `crates/deadreckon/src/main.rs:1148`, `crates/deadreckon/src/cli.rs:665`, `crates/deadreckon/tests/coherence.rs:241` | Normal user surfaces say provider route/model/kind; descriptor remains registry vocabulary. |
| Spend cap glossary | `help-all` defines run cap, per-child cap, aggregate chain cap, and doc polish cap in one place. | `crates/deadreckon/src/main.rs:1158`, `crates/deadreckon/tests/coherence.rs:370` | This keeps `--max-spend` scoped by object instead of relying on local help wording alone. |
| Copy-out wording | User-facing prompts, docs, help, and refusal text now say `export`; `materialize` remains a hidden compatibility alias/internal marker word. | `crates/deadreckon/src/main.rs:12430`, `crates/deadreckon/src/main.rs:16679`, `crates/deadreckon/tests/coherence.rs:201` | TUI key `m` is preserved while the visible action is `export`. |
| Lifecycle help parity | Top help, attach, show, kill, merge, apply, and cleanup help use run/chain/plan id language, show `finish` first for completed plans, and document plan-child refs. | `crates/deadreckon/src/main.rs:1246`, `crates/deadreckon/src/cli.rs:330`, `crates/deadreckon/tests/coherence.rs:392` | Direct `apply`/`export` remain visible after `finish` for advanced post-completion flows. |
| Cleanup boundary wording | Cleanup help names temporary run worktrees and branches as the target and excludes plan state, promoted library artifacts, and exported directories. | `crates/deadreckon/src/cli.rs:304`, `crates/deadreckon/tests/coherence.rs:456` | This keeps cleanup from sounding like it deletes everything deadreckon knows about. |
| Plan result primacy | Merge/result output names the plan first and labels the synthesized result run and artifact library as secondary implementation details. | `crates/deadreckon/src/main.rs:9586`, `crates/deadreckon/src/main.rs:11849`, `crates/deadreckon/tests/coherence.rs:357` | Plan ids remain the user-facing lifecycle id for `finish`, `apply`, and `export`. |
| Public docs alias sweep | README, HOWTO, and DEVELOPMENT-README no longer teach stale primary aliases or old flag spellings for the current CLI. | `crates/deadreckon/tests/coherence.rs:336` | Historical goal/rider docs may still mention old spellings as history; AS-BUILT calls them hidden alpha aliases where relevant. |
| Plan/orchestrate start output | Plan, fork, merge, and orchestrate now share provider-role, dependency, parallelism, and repair summary helpers. | `crates/deadreckon/src/main.rs`, `crates/deadreckon/src/plan_event_bus.rs` | This closes the V1 live-UX slice without introducing a full output-layout facade. |
| Run start key/value layout | Fresh, extended, and resumed run start banners share one padded key/value block for provider, model, docs, state, and attach. | `crates/deadreckon/src/main.rs:16988`, `crates/deadreckon/tests/coherence.rs:93` | This closes the most visible run/extend/resume start-summary mismatch without changing durable state. |
| Plan attach drilldown | `attach <plan-id>` can drill into a child run and return with standardized detach/focus/back/try footer grammar. | `crates/deadreckon/src/main.rs`, `crates/deadreckon/src/plan_event_bus.rs` | Child detail remains the normal run attach renderer. |
| Plan event stream | `plan-events.jsonl` exists, and plan attach now consumes a `PlanEventBus` feed that owns replay/tail, snapshots, and child/repair run multiplexing. | `crates/deadreckon-core/src/plan.rs:13`, `crates/deadreckon/src/plan_event_bus.rs` | Same-process broadcaster wiring for embedded attach remains future hardening. |
| Orchestrate finish/apply/export | Completed plans can route through `finish`, `apply`, and `export` by plan id. | `crates/deadreckon/src/main.rs:12232`, `crates/deadreckon/src/main.rs:12499` | Plan id is primary in help; result-run ids are secondary implementation detail. |
| Chain glyph collision | Chain step `Applied` now uses `◉`; `Running` remains `●`. | `crates/deadreckon/src/main.rs:4824` | Preserve glyph set: `○ ● ◐ ✗ ↷ ◉ ↶`. |
| Machine output | JSON exists on key inspect/list surfaces plus `plan --json`; representative JSON responses include `kind`, `id`, `status`, `next_actions`, `try_lines`, and `paths` without ANSI or hints. | `crates/deadreckon/src/cli.rs:695`, `crates/deadreckon/src/main.rs:1664`, `crates/deadreckon/src/main.rs:8954`, `crates/deadreckon/tests/coherence.rs:1050` | Existing payload keys such as `runs`, `run`, `plan`, `providers`, and `chains` remain for compatibility. |
| Style contract docs | AS-BUILT §26 documents glossary, style facade, key/value layout, flag/output policy, provider roles, JSON parity, card policy, and V1 limits. | `docs/AS-BUILT-ARCHITECTURE.md:1670`, `docs/V1-CANDIDATES.md:1` | The docs now distinguish alpha closure from broader V1 refinements. |
| Visual identity | The cyan `deadreckoning` progress label, `* ^ . -` course strip, magenta ids, TUI palette, spend gradient, and step glyphs are present. | `crates/deadreckon/src/main.rs:12044`, `crates/deadreckon/src/ui.rs:25`, `crates/deadreckon/src/main.rs:20384` | Keep these; standardize their construction. |

## Current Command Catalog

Source: parser definitions in `crates/deadreckon/src/cli.rs`, default/help-all catalog in `crates/deadreckon/src/main.rs`.

| Command | Aliases | Visible in default help | Primary user flow | Coherence note |
|---|---|---:|---|---|
| `start` | none | yes | Begin supervised agent work. | Primary front door for run/follow-up/review/full-plan. |
| `attach` | `watch` | yes | Watch and understand a run, chain, or plan. | Primary observation path. |
| `status` | `next` | yes | Latest run and next action. | `next` is secondary alias only. |
| `list` | `runs` | yes | Find runs and plans. | Primary discovery path. |
| `finish` | none | yes | Route completed work. | Best primary verb for most users. |
| `doctor` | `check` | yes | Check local setup. | Primary setup health command. |
| `init` | `setup` | yes | Configure once. | Setup-support command. |
| `def-done` | none | yes | Write/check done criteria. | Setup-support command and canonical user word. |
| `kill` | `stop` | yes | Stop run, chain, or plan. | Primary control command. |
| `resume` | `continue` | yes | Resume incomplete run. | Primary recovery command. |
| `cleanup` | `prune`, `clean` | yes | Remove stale/completed worktrees. | Primary cleanup word. |
| `help-all` | `commands` | yes | Discover advanced commands. | Alias is shown inline. |
| `config` | `settings` | no | Read/change defaults. | Advanced setup; uses provider route/model language. |
| `completion` | `completions` | no | Install/generate completions. | Advanced setup, still in `help-all`. |
| `acceptance` | none | hidden | Compatibility surface for done criteria. | Should remain advanced and defer to `def-done`. |
| `run` | none | no | Start one coding run directly. | Power-user launch path. |
| `orchestrate` | none | no | Plan/fork/merge in one command. | Power-user launch path; shared summaries in place. |
| `chain` | none | no | Serial multi-step goals. | Power-user launch path; full footer/table templating is deferred. |
| `plan` | none | no | Write orchestration plan only. | Advanced building block. |
| `fork` | none | no | Start plan children. | Advanced building block. |
| `merge` | none | no | Compose plan children. | Advanced building block; help teaches `finish <plan-id>` first. |
| `extend` | `follow-up` | no | Continue from completed output. | Direct command remains available; `start` can offer it from history. |
| `export` | `materialize`, `copy-out` | no | Copy a completed artifact to a directory. | `export` is canonical; `materialize` remains compatibility/internal vocabulary. |
| `apply` | `keep` | no | Merge work back into source git. | Advanced but central after completion. |
| `abandon` | `discard` | hidden | Remove temporary worktree/branch. | Advanced; common flow prefers `cleanup`. |
| `doc` | `docs` | no | Print/regenerate run docs. | Advanced results command. |
| `library` | `artifacts` | no | Inspect promoted artifacts. | Advanced results command. |
| `show` | `inspect` | no | Detailed state/provenance/plan failure. | Advanced inspection command. |
| `import` | none | no | Import other tool history. | Advanced. |
| `history` | none | no | Search traces/provenance. | Advanced history command. |
| `learn` | none | no | Index local evidence and propose improvements. | Advanced learning command. |
| `improve` | none | no | Run evidence-backed self-improvement candidates. | Advanced learning command. |
| `detect` | none | no | Probe providers. | Advanced setup command. |
| `providers` | none | no | List provider routes/models. | Advanced setup command. |
| `update` | none | no | Self-update. | Advanced install/update command. |
| `undo` | `restore` | hidden | Restore in-place snapshot. | Advanced. |

## Current Vocabulary

| Concept | Stored/schema word | User-facing word today | Keep/change |
|---|---|---|---|
| Active run/phase | `executing` | `running` | Keep user word `running`; keep schema for alpha compatibility. |
| Active plan | `forked` | `running` | Keep user word `running`; consider a schema migration later only for V1. |
| Completed plan | `merged` | `completed` | Keep user word `completed`. |
| Multi-agent object | plan / orchestration / job | mixed | Use `plan` as noun, `orchestrate` as action. Avoid `job` unless referring generically. |
| Plan unit | task / child / worker | mixed | Use `child` for the user, `task-*` for stable ids, `worker spec` for files only. |
| Done contract | done criteria / acceptance / gate / final gate | mixed | Use `done criteria` for users; use `gate` only in technical status rows. |
| Provider selection | provider / route / descriptor / model | mixed | Use `provider` and `model` in normal output; reserve `route`/`descriptor` for advanced provider docs. |
| Copy output | materialize marker / export command | `export` | Prefer `finish` first; direct copy action says `export`; `materialize` stays hidden compatibility/internal marker text. |
| Throw away temp work | abandon / discard / cleanup | mixed | Prefer `cleanup` for common flow, one direct verb for advanced flow. |
| Latest id | run id only / run or plan id | mixed | Say "run and plan ids accept prefixes" wherever both are accepted. |

## Explicit Alpha Deferrals

These are intentionally not alpha-closure blockers. Rows marked fixed below were closed after the initial matrix; remaining rows are captured in `docs/V1-CANDIDATES.md` and should be tackled as follow-up design/implementation work, not as hidden drift in the current CLI.

### Style, Streams, And Text Blocks

| ID | Surface | Current evidence | Deferred change |
|---|---|---|---|
| S4 | Key-value blocks are not universal. | `print_kv_block` exists, but some status detail rows and summaries still hand-format rows. | One kv-block helper with optional labels/try-lines. |
| S5 | Hint and try-line formatting is mixed. | `ui::hint`, raw `hint:`, raw `try:`, and styled `ui_command("try:")`. | Centralize `hint`, `try_line`, and `next_action` helpers. |
| S6 | Stream policy is implicit. | Prompts on stdout, progress on stderr, previews on stderr, success on stdout, cancellations mixed. | Document and enforce stdout/stderr rules per output type. |
| S8 | Table headers vary. | Uppercase run/chain list headers vs lowercase kv labels. | Define table style for list-like output and kv style for detail output. |
| S10 | ANSI color palette is split between CLI and TUI. | `Tone` plus direct `ratatui::Color` usage. | One palette module that exposes CLI tone and TUI color roles. |

### Lifecycle Flow Consistency

| ID | Surface | Current evidence | Deferred change |
|---|---|---|---|
| L1 | `run`, `extend`, `resume`, and `orchestrate` start/finish banners differ. | `print_run_started`, `print_orchestrate_started`, extended-run completion lines. | Use one lifecycle summary renderer with object kind = run/plan/chain. |
| L9 | Non-git setup wording differs between run and orchestrate. | Run interactive mode chooser; orchestrate `--init-git` preflight. | Shared source-mode preflight: git, init-git, copy/fresh fallback, done criteria. |
| L10 | Done criteria/gate labels differ by flow. | Run missing criteria prompt; plan preflight `gate`; status `gate`. | User-facing setup says done criteria; detail rows may say gate with explanation. |

### Orchestration-Specific Surfaces

| ID | Surface | Current evidence | Deferred change |
|---|---|---|---|
| O1 | Plan/fork/merge/orchestrate are not visually a single family. | Fixed in the live-UX slice: shared orchestration role/dependency summaries now appear on plan creation, orchestrate preflight/start, fork completion, plan summaries, and merge completion. | Further polish belongs to the full output-layout facade, not this matrix item. |
| O2 | Mode selection needs a clearer interactive model. | Partially improved by the shared role/dependency/cap/repair preflight; full interactive setup still lives across existing flags and prompts. | Defer the reusable mode/provider/done-criteria setup flow. |
| O3 | Provider roles are under-described in output. | Fixed: role table rows show role, route, model, source, and notes for planner, default child, child overrides, coder, reviewer, and repair. | Keep model as `-` when no model metadata is persisted. |
| O4 | Parallelism semantics are implicit. | Fixed: orchestration summaries show `starts now`, `waits`, `waits_for`, and `unblocks` for child dependencies. | None for this surface. |
| O5 | Merge repair status is terse. | Fixed: plan and merge summaries show repair mode, attempts, provider, conflicts, sidecar paths, repair run, latest repair event, and next action. | Persisting exact historical attempt count remains a possible sidecar enhancement. |
| O6 | Plan attach footer differs from run/chain. | Fixed: `plan_attach_footer` now uses detach/focus/child-run/back/try grammar aligned with the other attach views. | None for this surface. |
| O7 | Plan TUI reads disk by polling. | Fixed at the TUI ownership layer: plan attach consumes `PlanEventBus` rather than calling `read_plan_events_lossy` in its redraw loop. The feed owns JSONL replay/tail, snapshots, and child/repair run multiplexing. | Future hardening may wire long-lived same-process plan writers into the bus broadcaster. |

### Provider, Done Criteria, And Docs

| ID | Surface | Current evidence | Deferred change |
|---|---|---|---|
| P2 | Provider setup is one reusable runtime flow. | Fixed: `setup.rs` backs `init`, `config provider`, run/extend/resume setup, orchestration roles, provider selection display, and doc polish source labels. | Deeper interactive setup polish can move to the output-layout/prompt-builder V1 slice. |
| P5 | Done criteria and hidden acceptance compatibility share one setup model. | Fixed: `DoneCriteriaSelection` backs explicit paths, project files, generated criteria, and default `dr-gate`; run/orchestrate previews say `done criteria`. | Golden snapshots can still harden the exact copy once the layout settles. |

### Machine-Readable And Test Coverage

| ID | Surface | Current evidence | Deferred change |
|---|---|---|---|
| J4 | Snapshot tests are missing for many user surfaces. | Existing unit tests cover footers and helpers, not the whole command matrix. | Add golden tests for help, summaries, prompts, and JSON no-ANSI/no-hints behavior. |

## V1 Target Model

The alpha closure landed the immediate user-facing vocabulary, help, flag, provider-role, provider/done-criteria setup, JSON, and docs coherence work. A V1 coherence/design pass can still deepen these decisions:

1. One glossary for nouns, verbs, statuses, object kinds, provider roles, and done criteria.
2. Keep custom top help and help-all on the shared command catalog, with clap coverage tests for row drift.
3. One flag policy for `--yes`, `--no-confirm`, `--all`, `--all-scopes`, `--plain`, `--quiet`, `--json`, `--no-hints`, provider roles, caps, and strategies.
4. One style/palette helper for headings, ids, commands, statuses, hints, try-lines, warnings, errors, progress, and TUI colors.
5. One lifecycle summary builder for run, plan, chain, finish, apply, export, extend, resume, kill, and cleanup results.
6. One prompt builder for confirmations and interactive preflights, including orchestrate mode/provider/child-count selection.
7. One JSON/plain/quiet contract with tests.
8. A docs sweep that updates README/HOWTO/AS-BUILT/help examples to the canonical words.

Keep the visual identity: the cyan `deadreckoning` banner, `* ^ . -` course strip, magenta ids, spend gauge gradient, and step glyphs `○ ● ◐ ✗ ↷ ◉ ↶`. Coherence means the same concept gets the same word, stream, color, and flag behavior everywhere; it does not mean flattening the CLI into bland output.
