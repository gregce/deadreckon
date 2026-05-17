# User-Facing Surface Matrix

**Status:** Refreshed against the local working tree on 2026-05-17.
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
| Help discovery | Top help, help-all, and command help now prefer canonical words: `status`, `finish`, `export`, `cleanup`, `run/chain/plan`, and aliases inline. | `crates/deadreckon/src/main.rs:834`, `crates/deadreckon/src/main.rs:917`, `crates/deadreckon/tests/coherence.rs:77` | The remaining gap is structural: the rows are tested but not generated from one command table. |
| Copy-out wording | User-facing prompts, docs, help, and refusal text now say `export`; `materialize` remains a hidden compatibility alias/internal marker word. | `crates/deadreckon/src/main.rs:12430`, `crates/deadreckon/src/main.rs:16679`, `crates/deadreckon/tests/coherence.rs:201` | TUI key `m` is preserved while the visible action is `export`. |
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

### Help And Command Discovery

| ID | Surface | Current evidence | Required change |
|---|---|---|---|
| H9 | Hidden commands are discoverable through examples. | `apply`, `materialize`, `abandon`, `doc`, `show` are hidden but heavily referenced. | Define "advanced but documented" vs "hidden compatibility alias"; do not leave ghost commands. |
| H12 | Help is tested but not generated from one command table. | `crates/deadreckon/tests/coherence.rs` covers top help, help-all, and key command help, but `TOP_LEVEL_HELP`, `print_top_help`, and `print_help_all` still duplicate rows. | Add a command metadata table or stronger snapshot tests so custom help and clap help cannot drift. |

### Flags And Option Semantics

| ID | Surface | Current evidence | Required change |
|---|---|---|---|
| F1 | Prompt-skipping flags differ. | `--yes` on run/orchestrate/chain/update; `--no-confirm` on finish/apply/doc. | Define one semantic model: preflight confirmation vs destructive confirmation vs all prompts. |
| F2 | `--force` carries different meanings. | High-spend/in-place, overwrite, cleanup escalation, and aliases. | Use command-specific visible flags; keep `--force` only as hidden compatibility where needed. |
| F3 | `--all` vs `--all-scopes` is inconsistent. | `providers list --all`; `cleanup --all-scopes` alias `--all`; `history --all`; `list --all`. | Define `--all` per object vs `--all-scopes` for project scope, then update help. |
| F4 | Branch naming flags vary. | `--branch`, hidden `--branch-name`, apply commit/branch outputs. | One visible branch flag and one output label. |
| F5 | Strategy flags vary. | `merge --strategy`, `finish/apply --git-strategy`, `chain --apply-strategy`. | Split terms deliberately: merge strategy, git apply strategy, chain apply mode. Document them as a family. |
| F6 | Plain-output help differs. | `--plain` says "without TUI, spinner, or ANSI" in some places and only "without ANSI" in others. | One definition for `--plain`; command-specific effects can be second sentence. |
| F7 | Quiet behavior is not global. | `--quiet` exists on run/orchestrate/plan/fork/merge/chain, not most completed-run actions. | Define which commands can suppress success stdout and why. |
| F8 | JSON availability is partial. | JSON exists on many inspect commands, but not plan/fork/merge/orchestrate preflight. | Decide which stateful commands emit JSON and ensure no hints/ANSI leak into JSON. |
| F9 | Hints use `--no-hints` unevenly. | Run/plan/orchestrate/update/attach surfaces vary. | One hint policy helper and one env/flag precedence rule. |
| F10 | Provider flags are role-specific but not presented as a system. | `--provider`, `--planner-provider`, `--coder-provider`, `--reviewer-provider`, `--child-provider`, `--doc-provider`. | Add a provider-role glossary and shared provider prompt/preflight builder. |

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
| L2 | Plan completion still exposes result run mapping. | `print_merge_finished`, `print_plan_summary`. | Show plan id as primary; result run id as secondary detail. |
| L3 | `finish` is the primary post-completion action but output still points to direct actions first in places. | `print_lifecycle_hints`, merge help. | Always show `finish <id>` first unless the command is already a direct action. |
| L4 | Latest-id language is run-only in top help. | `print_top_help` note. | Say "run and plan ids accept unique prefixes" where both are accepted; clarify `latest` scope. |
| L5 | Plan child refs exist but are under-explained. | `deadreckon attach <plan-id>:task-0` output. | Add help and status rows for child refs, child run ids, and back-to-plan navigation. |
| L6 | `show` handles plans but its help is run-centric. | `SHOW_HELP`, `print_plan_summary`. | Say `show <run-id|plan-id>` and document plan failure/repair detail. |
| L7 | `kill` can target plans, but help says run. | `KILL_HELP`, kill output. | Say run or plan; output child cascade count consistently. |
| L8 | Cleanup language is worktree-centric. | `CLEANUP_HELP`, cleanup output. | Clarify what cleans run worktrees, plan state, libraries, and materialized exports. |
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
| O8 | Plan artifacts and apply/export outputs mix "library", "result run", and "plan". | `print_merge_finished`, `print_plan_summary`. | Treat plan as primary object and artifact/run as implementation details. |
| O9 | Child run detail is available, but returning context is not described in help. | Attach drilldown code. | Add discoverable help and tests for plan -> child -> back. |

### Provider, Done Criteria, And Docs

| ID | Surface | Current evidence | Required change |
|---|---|---|---|
| P1 | Provider nouns drift. | `providers`, `detect`, `config`, and run/orchestrate help mix provider/route/descriptor. | One provider glossary with progressive disclosure. |
| P2 | Provider setup is not one reusable flow. | `init`, `config provider`, run flags, orchestrate flags, doc polish flags. | Shared provider selection/prompt builder. |
| P3 | Spend cap wording differs by object. | Per-run, per-child, aggregate chain, doc polish. | One cap glossary: run cap, per-child cap, aggregate chain cap, doc polish cap. |
| P4 | Doc polish has its own provider/cap/prompt vocabulary. | `DOC_HELP`, `doc_command`. | Fold into provider/cap/prompt policy. |
| P5 | Done criteria and acceptance docs can still diverge. | `def-done`, hidden `acceptance`, status/gate labels. | One docs section and help text source for done criteria. |
| P6 | Existing docs likely contain stale alias examples. | README/HOWTO/AS-BUILT/goals predate parts of the coherence pass. | Sweep docs after code changes and update examples to canonical words. |

### Machine-Readable And Test Coverage

| ID | Surface | Current evidence | Required change |
|---|---|---|---|
| J1 | JSON shapes use different next-action conventions. | Status JSON has `try_lines`; other JSON surfaces vary. | Standard optional fields: `kind`, `id`, `status`, `next_actions`, `try_lines`, `paths`. |
| J2 | Plan JSON/plain coverage is incomplete. | Plan summaries are text-heavy; plan command JSON is absent. | Add JSON where the command is inspection-like or preview-like. |
| J3 | `--plain`, `--quiet`, and `--json` precedence is undocumented. | Flags are distributed by command. | Define precedence: JSON wins over styling/hints; quiet suppresses success chatter but not requested data/errors. |
| J4 | Snapshot tests are missing for many user surfaces. | Existing unit tests cover footers and helpers, not the whole command matrix. | Add golden tests for help, summaries, prompts, and JSON no-ANSI/no-hints behavior. |
| J5 | Docs do not describe the user-facing style contract. | AS-BUILT has coherence section but not the full matrix contract. | Update AS-BUILT §17/§18/§26/§30/§32 after implementation. |

## Target Model

The next coherence goal should land these decisions:

1. One glossary for nouns, verbs, statuses, object kinds, provider roles, and done criteria.
2. One command/help table that feeds custom top help, help-all, and clap after-help examples.
3. One flag policy for `--yes`, `--no-confirm`, `--all`, `--all-scopes`, `--plain`, `--quiet`, `--json`, `--no-hints`, provider roles, caps, and strategies.
4. One style/palette helper for headings, ids, commands, statuses, hints, try-lines, warnings, errors, progress, and TUI colors.
5. One lifecycle summary builder for run, plan, chain, finish, apply, export, extend, resume, kill, and cleanup results.
6. One prompt builder for confirmations and interactive preflights, including orchestrate mode/provider/child-count selection.
7. One JSON/plain/quiet contract with tests.
8. A docs sweep that updates README/HOWTO/AS-BUILT/help examples to the canonical words.

Keep the visual identity: the cyan `deadreckoning` banner, `* ^ . -` course strip, magenta ids, spend gauge gradient, and step glyphs `○ ● ◐ ✗ ↷ ◉ ↶`. Coherence means the same concept gets the same word, stream, color, and flag behavior everywhere; it does not mean flattening the CLI into bland output.
