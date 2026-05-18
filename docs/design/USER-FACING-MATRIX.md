# User-Facing Surface Matrix

**Status:** Refreshed against the local working tree on 2026-05-18.
**Scope:** The user-visible CLI, prompts, summaries, TUI labels, JSON/plain modes, help text, docs-facing terminology, and orchestration surfaces in `/Users/gdc/deadreckon`.
**Method:** Source audit of `crates/deadreckon/src/{cli.rs,main.rs,ui.rs,prompt.rs}` and `crates/deadreckon-core/src/{glossary.rs,state.rs,chain.rs,plan.rs}`, plus the current AS-BUILT and goal docs.
**Read this as:** the current coherence backlog. The prior matrix cited commit `455b91a` and listed 108 issues; many of those are now implemented. This refresh keeps the visual affordances that are working and focuses the remaining user-facing drift.

## Fixed Since The Prior Audit

| Area | Current state | Evidence | Notes |
|---|---|---|---|
| Status glossary | A central user-facing glossary exists. Stored `Executing` still serializes as `executing`, but user labels render as `running`. | `crates/deadreckon-core/src/glossary.rs:1` | Applies to run, phase, chain, chain step, plan, and plan task labels. |
| Run/phase display | `RunStatus` and `PhaseStatus` `Display` use the glossary. | `crates/deadreckon-core/src/state.rs:22` | Old `executing`/`running` UI split is mostly closed. |
| Plan status display | `Forked` renders as `running`; `Merged` renders as `completed`. | `crates/deadreckon-core/src/glossary.rs:62` | Good user word, historical stored enum retained. |
| Shared style helper | `ui.rs` owns `Tone`, `Stream`, ANSI rendering, TUI palette, hints, and key-value blocks. | `crates/deadreckon/src/ui.rs:1` | Raw styling is no longer entirely ad hoc. |
| Prompt helper | `prompt::open` and `prompt::confirm` provide a shared confirmation shape. | `crates/deadreckon/src/prompt.rs:1` | The old doc polish default-marker bug is fixed. |
| Error hints | `error_hint` returns actionable strings and uses discovered config paths. | `crates/deadreckon/src/main.rs:147` | Generic provider/core errors now get a fallback hint. |
| `def-done` naming | Top help and command help use `deadreckon def-done`, not `deadreckon done`. | `crates/deadreckon/src/main.rs:845`, `crates/deadreckon/src/cli.rs:189` | The screenshot miss is fixed in the current source. |
| Help discovery | Top help and `help-all` render command rows from one catalog, while command help uses canonical words: `status`, `finish`, `export`, `cleanup`, `run/chain/plan`, and aliases inline. | `crates/deadreckon/src/main.rs:846`, `crates/deadreckon/src/main.rs:1181`, `crates/deadreckon/tests/coherence.rs:77` | Catalog tests verify row uniqueness and that rows point at real clap commands. |
| Advanced command discovery | `help-all` explicitly explains that advanced commands are documented there but hidden from short help, while compatibility aliases stay inline on canonical rows. | `crates/deadreckon/src/main.rs:1116`, `crates/deadreckon/src/main.rs:20873` | `materialize` is not a ghost row; it stays an alias on `export`. |
| Plain output flag | All `--plain` help uses one definition: "Plain output without TUI, spinner, or ANSI affordances." | `crates/deadreckon/src/cli.rs:545`, `crates/deadreckon/tests/coherence.rs:207` | Command-specific behavior is still implemented per command; the user-facing flag definition is shared. |
| Cross-scope flags | Run, chain, history, cleanup, and library cross-project help says "all project scopes"; provider `--all` remains provider inventory. | `crates/deadreckon/src/cli.rs:869`, `crates/deadreckon/tests/coherence.rs:70` | This keeps `--all` for ordinary cross-project listing/search and `--all-scopes` for chain/cleanup compatibility-sensitive surfaces. |
| Force aliases | Visible help uses intent-specific flags: `--escalate`, `--overwrite`, and `--anyway`; old `--force` spellings stay hidden alpha aliases. | `crates/deadreckon/src/cli.rs:917`, `crates/deadreckon/tests/coherence.rs:70` | `update --anyway` replaces the last visible primary-command `--force`. |
| Branch target flags | Worktree runs expose `--branch-name`; apply and finish expose `--into`; apply output says work landed `into` the target branch. | `crates/deadreckon/src/cli.rs:532`, `crates/deadreckon/src/main.rs:12911`, `crates/deadreckon/tests/coherence.rs:60` | Hidden `--branch` aliases remain for one alpha, but help and output use the canonical words. |
| Strategy flag family | `merge --strategy` is plan composition, `apply`/`finish --git-strategy` is git apply behavior, and chain help separates `--apply-mode` from per-step `--apply-strategy`. | `crates/deadreckon/src/cli.rs:735`, `crates/deadreckon/tests/coherence.rs:122` | Hidden `--strategy` aliases for apply/finish remain for one alpha; user-facing help scopes the terms. |
| Prompt-skip flags | `help-all` documents `--yes` as preflight preview acceptance and `--no-confirm` as destructive/follow-up confirmation skipping; command help follows that split. | `crates/deadreckon/src/main.rs:1118`, `crates/deadreckon/src/cli.rs:548`, `crates/deadreckon/tests/coherence.rs:161` | `run` keeps both because it has a preflight plus separate safety confirmations. |
| Output mode flags | `help-all` documents `--quiet`, `--plain`, `--json`, and `--no-hints` precedence; quiet text is shared and JSON remains inspection/list-only in alpha. | `crates/deadreckon/src/main.rs:1118`, `crates/deadreckon/tests/coherence.rs:161` | `--json` wins over styling/hints where present; `DEADRECKON_HINTS=0` is the process-level hint override. |
| Provider role flags | `help-all` documents provider roles for primary run, planner, child, coder, reviewer, documentation, and repair routes; orchestration/doc help uses provider-route wording. | `crates/deadreckon/src/main.rs:1148`, `crates/deadreckon/src/cli.rs:665`, `crates/deadreckon/tests/coherence.rs:241` | Normal user surfaces say provider route/model/kind; descriptor remains registry vocabulary. |
| Copy-out wording | User-facing prompts, docs, help, and refusal text now say `export`; `materialize` remains a hidden compatibility alias/internal marker word. | `crates/deadreckon/src/main.rs:12430`, `crates/deadreckon/src/main.rs:16679`, `crates/deadreckon/tests/coherence.rs:201` | TUI key `m` is preserved while the visible action is `export`. |
| Lifecycle help parity | Top help, attach, show, kill, merge, apply, and cleanup help use run/chain/plan id language, show `finish` first for completed plans, and document plan-child refs. | `crates/deadreckon/src/main.rs:1246`, `crates/deadreckon/src/cli.rs:330`, `crates/deadreckon/tests/coherence.rs:392` | Direct `apply`/`export` remain visible after `finish` for advanced post-completion flows. |
| Cleanup boundary wording | Cleanup help names temporary run worktrees and branches as the target and excludes plan state, promoted library artifacts, and exported directories. | `crates/deadreckon/src/cli.rs:304`, `crates/deadreckon/tests/coherence.rs:456` | This keeps cleanup from sounding like it deletes everything deadreckon knows about. |
| Plan result primacy | Merge/result output names the plan first and labels the synthesized result run and artifact library as secondary implementation details. | `crates/deadreckon/src/main.rs:9586`, `crates/deadreckon/src/main.rs:11849`, `crates/deadreckon/tests/coherence.rs:357` | Plan ids remain the user-facing lifecycle id for `finish`, `apply`, and `export`. |
| Public docs alias sweep | README, HOWTO, and DEVELOPMENT-README no longer teach stale primary aliases or old flag spellings for the current CLI. | `crates/deadreckon/tests/coherence.rs:336` | Historical goal/rider docs may still mention old spellings as history; AS-BUILT calls them hidden alpha aliases where relevant. |
| Plan/orchestrate start output | Orchestrate preflight and started output now print mode, children, providers, source, gate, repair, sandbox, spend, wall, plan, and events. | `crates/deadreckon/src/main.rs:8604`, `crates/deadreckon/src/main.rs:8694` | This is much closer to `run` parity. |
| Plan attach drilldown | `attach <plan-id>` can drill into a child run and return. | `crates/deadreckon/src/main.rs:16757` | Footer copy still needs coherence work. |
| Plan event stream | `plan-events.jsonl` exists and plan attach reads it. | `crates/deadreckon-core/src/plan.rs:13`, `crates/deadreckon/src/main.rs:16768` | It is file-backed polling, not a same-process broadcast bus. |
| Orchestrate finish/apply/export | Completed plans can route through `finish`, `apply`, and `export` by plan id. | `crates/deadreckon/src/main.rs:12232`, `crates/deadreckon/src/main.rs:12499` | Plan id is primary in help; result-run ids are secondary implementation detail. |
| Chain glyph collision | Chain step `Applied` now uses `◉`; `Running` remains `●`. | `crates/deadreckon/src/main.rs:4824` | Preserve glyph set: `○ ● ◐ ✗ ↷ ◉ ↶`. |
| Machine output | JSON exists on key inspect/list surfaces: list, status, show, doctor, detect, providers list, library list, and chain list/show/status. | `crates/deadreckon/src/cli.rs:898`, `crates/deadreckon/src/cli.rs:970`, `crates/deadreckon/src/cli.rs:1287`, `crates/deadreckon/src/cli.rs:1317`, `crates/deadreckon/src/cli.rs:1666`, `crates/deadreckon/src/cli.rs:1723` | Shapes and `try_lines` are not yet uniform. |
| Visual identity | The cyan `deadreckoning` progress label, `* ^ . -` course strip, magenta ids, TUI palette, spend gradient, and step glyphs are present. | `crates/deadreckon/src/main.rs:12044`, `crates/deadreckon/src/ui.rs:25`, `crates/deadreckon/src/main.rs:20384` | Keep these; standardize their construction. |

## Current Command Catalog

Source: `crates/deadreckon/src/cli.rs`.

| Command | Aliases | Visible in top-level clap help | Primary user flow | Coherence note |
|---|---|---:|---|---|
| `init` | `setup` | yes | Configure once. | Good. |
| `config` | `settings` | yes | Read/change defaults. | Uses provider route/model language. |
| `help-all` | `commands` | yes | Discover advanced commands. | Alias is shown inline. |
| `completion` | `completions` | yes | Install/generate completions. | Good. |
| `acceptance` | none | hidden | Compatibility surface for done criteria. | Should remain advanced and defer to `def-done`. |
| `def-done` | none | yes | Write/check done criteria. | Canonical user word. |
| `run` | none | yes | Start one coding run. | Good. |
| `orchestrate` | none | yes | Plan/fork/merge in one command. | Needs final word/flag/style parity with `run`. |
| `plan` | none | yes | Write orchestration plan only. | Good advanced verb. |
| `fork` | none | yes | Start plan children. | Good advanced verb. |
| `merge` | none | yes | Compose plan children. | Help teaches `finish <plan-id>` first, then direct `apply`/`export`. |
| `chain` | none | yes | Serial multi-step goals. | Good, but flag names and footers differ. |
| `doctor` | `check` | yes | Check local setup. | Good. |
| `detect` | none | yes | Probe providers. | Included in custom top help. |
| `providers` | none | yes | List provider routes/models. | Included in custom top help. |
| `update` | none | yes | Self-update. | Included in custom top help. |
| `list` | `runs` | yes | List runs and plans. | Good. |
| `library` | `artifacts` | hidden | Inspect promoted artifacts. | Advanced command appears in `help-all`. |
| `finish` | none | yes | Route completed work. | Best primary verb for most users. |
| `export` | `materialize`, `copy-out` | hidden command alias | Copy a completed artifact to a directory. | `export` is canonical; `materialize` remains compatibility/internal vocabulary. |
| `apply` | `keep` | hidden | Merge work back into source git. | Advanced but central after completion. |
| `abandon` | `discard` | hidden | Remove temporary worktree/branch. | Advanced; common flow prefers `cleanup`. |
| `cleanup` | `prune`, `clean` | yes | Remove stale/completed worktrees. | Good primary cleanup word. |
| `extend` | `follow-up` | yes | Continue from completed output. | Good. |
| `doc` | `docs` | hidden | Print/regenerate run docs. | User sees both "doc" command and "docs" noun. |
| `attach` | `watch` | yes | Open live TUI for run, chain, or plan. | Good. |
| `kill` | `stop` | yes | Cancel run, chain, or plan. | Good. |
| `resume` | `continue` | yes | Resume incomplete run. | Good. |
| `undo` | `restore` | hidden | Restore in-place snapshot. | Advanced. |
| `show` | `inspect` | hidden | Detailed state/provenance/plan failure. | Hidden but examples rely on it. |
| `status` | `next` | yes | Latest run and next action. | `next` is secondary alias only. |
| `import` | none | hidden | Import other tool history. | Advanced. |
| `history` | none | yes | Search traces/provenance. | Included in custom top help. |

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

## Remaining Findings

### Style, Streams, And Text Blocks

| ID | Surface | Current evidence | Required change |
|---|---|---|---|
| S1 | Style helper exists but wrappers remain scattered. | `ui_heading`, `ui_muted`, `ui_id`, `ui_command`, `ui_ok`, `ui_warn`, `ui_error` in `main.rs`. | Move all public style functions into `ui.rs` or one facade. |
| S2 | Status color mapping is too narrow. | `ui_status` only treats `ok`/`polished` as green and otherwise warns. | Map known status labels through one status tone function. |
| S3 | Warning tone is used for non-warning states. | Paused/killed/failed extended run output and provider rows. | Separate `paused`, `failed`, `warning`, and `note` tones. |
| S4 | Key-value blocks are not universal. | `print_kv_block` exists, but `print_run_started`, status locations, and many summaries hand-format rows. | One kv-block helper with optional labels/try-lines. |
| S5 | Hint and try-line formatting is mixed. | `ui::hint`, raw `hint:`, raw `try:`, and styled `ui_command("try:")`. | Centralize `hint`, `try_line`, and `next_action` helpers. |
| S6 | Stream policy is implicit. | Prompts on stdout, progress on stderr, previews on stderr, success on stdout, cancellations mixed. | Document and enforce stdout/stderr rules per output type. |
| S7 | Cards are not scoped by policy. | Run exit/preflight cards exist; list/status use text tables. | Preserve cards only where useful: run/orchestrate preview and completion summaries; do not add cards to list/status/history. |
| S8 | Table headers vary. | Uppercase run/chain list headers vs lowercase kv labels. | Define table style for list-like output and kv style for detail output. |
| S9 | Wait banner has no builder. | `deadreckoning` and course strip built in `cli_wait_status_line`. | Keep the visual, but route through a named helper/test. |
| S10 | ANSI color palette is split between CLI and TUI. | `Tone` plus direct `ratatui::Color` usage. | One palette module that exposes CLI tone and TUI color roles. |

### Lifecycle Flow Consistency

| ID | Surface | Current evidence | Required change |
|---|---|---|---|
| L1 | `run`, `extend`, `resume`, and `orchestrate` start/finish banners differ. | `print_run_started`, `print_orchestrate_started`, extended-run completion lines. | Use one lifecycle summary renderer with object kind = run/plan/chain. |
| L9 | Non-git setup wording differs between run and orchestrate. | Run interactive mode chooser; orchestrate `--init-git` preflight. | Shared source-mode preflight: git, init-git, copy/fresh fallback, done criteria. |
| L10 | Done criteria/gate labels differ by flow. | Run missing criteria prompt; plan preflight `gate`; status `gate`. | User-facing setup says done criteria; detail rows may say gate with explanation. |

### Orchestration-Specific Surfaces

| ID | Surface | Current evidence | Required change |
|---|---|---|---|
| O1 | Plan/fork/merge/orchestrate are not visually a single family. | Separate help constants and summaries. | A shared orchestration prompt/preflight/result builder. |
| O2 | Mode selection needs a clearer interactive model. | Orchestrate has review/full-plan args and provider roles. | Before execution, users should choose mode, child count, planner/coder/reviewer/child providers, repair, caps, source mode. |
| O3 | Provider roles are under-described in output. | `planner`, `default child`, `coder`, `reviewer` rows. | Show a role table with route/model/source for each role. |
| O4 | Parallelism semantics are implicit. | Plan tasks have dependencies, ready/blocked counts. | Preflight should say which children start in parallel vs wait on dependencies. |
| O5 | Merge repair status is terse. | `merge repair {line}`. | Show repair mode, attempts, planner/provider, conflict paths, repair run id, and next action. |
| O6 | Plan attach footer differs from run/chain. | `plan_attach_footer`. | Standard footer grammar and back-navigation hints. |
| O7 | Plan TUI reads disk by polling. | `read_plan_events_lossy` in attach loop. | Acceptable for alpha, but call out V1 broadcast bus in docs. |

### Provider, Done Criteria, And Docs

| ID | Surface | Current evidence | Required change |
|---|---|---|---|
| P2 | Provider setup is not one reusable flow. | `init`, `config provider`, run flags, orchestrate flags, doc polish flags. | Shared provider selection/prompt builder. |
| P3 | Spend cap wording differs by object. | Per-run, per-child, aggregate chain, doc polish. | One cap glossary: run cap, per-child cap, aggregate chain cap, doc polish cap. |
| P5 | Done criteria and acceptance docs can still diverge. | `def-done`, hidden `acceptance`, status/gate labels. | One docs section and help text source for done criteria. |

### Machine-Readable And Test Coverage

| ID | Surface | Current evidence | Required change |
|---|---|---|---|
| J1 | JSON shapes use different next-action conventions. | Status JSON has `try_lines`; other JSON surfaces vary. | Standard optional fields: `kind`, `id`, `status`, `next_actions`, `try_lines`, `paths`. |
| J2 | Plan JSON/plain coverage is incomplete. | Plan summaries are text-heavy; plan command JSON is absent. | Add JSON where the command is inspection-like or preview-like. |
| J4 | Snapshot tests are missing for many user surfaces. | Existing unit tests cover footers and helpers, not the whole command matrix. | Add golden tests for help, summaries, prompts, and JSON no-ANSI/no-hints behavior. |
| J5 | Docs do not describe the user-facing style contract. | AS-BUILT has coherence section but not the full matrix contract. | Update AS-BUILT §17/§18/§26/§30/§32 after implementation. |

## Target Model

The next coherence goal should land these decisions:

1. One glossary for nouns, verbs, statuses, object kinds, provider roles, and done criteria.
2. Keep custom top help and help-all on the shared command catalog, with clap coverage tests for row drift.
3. One flag policy for `--yes`, `--no-confirm`, `--all`, `--all-scopes`, `--plain`, `--quiet`, `--json`, `--no-hints`, provider roles, caps, and strategies.
4. One style/palette helper for headings, ids, commands, statuses, hints, try-lines, warnings, errors, progress, and TUI colors.
5. One lifecycle summary builder for run, plan, chain, finish, apply, export, extend, resume, kill, and cleanup results.
6. One prompt builder for confirmations and interactive preflights, including orchestrate mode/provider/child-count selection.
7. One JSON/plain/quiet contract with tests.
8. A docs sweep that updates README/HOWTO/AS-BUILT/help examples to the canonical words.

Keep the visual identity: the cyan `deadreckoning` banner, `* ^ . -` course strip, magenta ids, spend gauge gradient, and step glyphs `○ ● ◐ ✗ ↷ ◉ ↶`. Coherence means the same concept gets the same word, stream, color, and flag behavior everywhere; it does not mean flattening the CLI into bland output.
