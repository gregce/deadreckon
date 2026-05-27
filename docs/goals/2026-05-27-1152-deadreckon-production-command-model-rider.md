# DeadReckon - Production Command Model Rider

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-27-1152-deadreckon-production-command-model-goal.md`.
It supersedes nothing in prior riders. It turns the coherence and guided-start
work into a production-facing command model: a small default help surface, a
friendly path to every advanced verb, and docs that stop making internal
topology feel like the product.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime
`/Users/gdc/.deadreckon`.

## Posture

- **Maturity target is production-facing UX, not 1.0 schema stability.** The
  command surface should feel ready for real local production use of supervised
  agent CLI work. Do not claim SemVer stability or remove honest 0.x caveats.
- **No durable state schema changes.** Do not change `PipelineState`, run JSON,
  plan JSON, chain JSON, provider registry TOML schemas, learning schemas, or
  config schemas.
- **No command deletion.** Power-user and compatibility commands remain
  callable. This goal changes presentation, guidance, and documentation.
- **No broken completion.** Shell completion may still expose the full clap
  tree. Friendly discovery is a feature, not a leak.
- **No hidden docs drift.** README, HOWTO, AS-BUILT, command help, and
  `docs/design/USER-FACING-MATRIX.md` must agree on the primary model.
- **No `git push`.** Phased local commits only.
- **Focused verification by default.** Do not run `make verify`, release builds,
  stress tests, or full-workspace tests unless the human explicitly asks.

## Existing Substrate

At HEAD, the command model is already centralized enough to finish this without
large plumbing:

- `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs`
  - `COMMAND_HELP_CATALOG`
  - `TopHelpGroup`
  - `HelpAllGroup`
  - `print_top_help`
  - `print_help_all`
  - command catalog coherence tests near the bottom of the file
- `/Users/gdc/deadreckon/crates/deadreckon/src/cli.rs`
  - `Commands`
  - clap command visibility, aliases, `after_help` text, and subcommand help
- `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md`
  - current visible/hidden/advanced command matrix
- `/Users/gdc/deadreckon/README.md`
  - public first-contact framing
- `/Users/gdc/deadreckon/HOWTO.md`
  - first serious run and power-user examples
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`
  - section 17 CLI surface and section 26 coherence pass

Recent riders that still govern this work:

- `/Users/gdc/deadreckon/docs/goals/2026-05-13-1900-deadreckon-coherence-rider.md`
- `/Users/gdc/deadreckon/docs/goals/2026-05-17-1403-deadreckon-coherence-closure-rider.md`
- `/Users/gdc/deadreckon/docs/goals/2026-05-26-1510-deadreckon-guided-experience-rider.md`
- `/Users/gdc/deadreckon/docs/goals/2026-05-27-1032-deadreckon-start-picker-rider.md`

## Production Command Model

The first-screen model is:

| Verb | User job | First-screen role |
|---|---|---|
| `start` | Begin supervised agent work from a goal | Primary begin verb |
| `attach` | Understand what is happening now | Primary watch verb |
| `status` | See the latest item and next action | Primary orient verb |
| `list` | Find runs and plans in the current project | Primary orient verb |
| `finish` | Keep/apply/export completed work | Primary keep verb |
| `doctor` | Diagnose setup and environment | Primary repair verb |
| `kill` | Stop live work | Primary control verb |
| `resume` | Recover interrupted work | Primary control verb |
| `cleanup` | Remove temporary worktrees/branches | Primary tidy verb |

Setup support stays close to the model:

| Verb | Role |
|---|---|
| `init` | Configure DeadReckon and install completion |
| `def-done` | Write/check done criteria |
| `help-all` | Discover every command |
| `<command> --help` | Deep help for one command |

These are not removed; they are advanced/power-user surfaces:

| Family | Verbs |
|---|---|
| Direct launch | `run`, `orchestrate`, `chain` |
| Orchestration internals | `plan`, `fork`, `merge` |
| Result internals | `apply`, `export`, `doc`, `library`, `show` |
| Setup internals | `config`, `detect`, `providers`, `completion` |
| Recovery internals | `extend`, `undo`, `abandon` |
| Inspection/learning/import | `history`, `import`, `learn`, `improve` |
| Compatibility | `acceptance` and hidden aliases |
| Update channel | `update` |

`update` may remain easy to find from `help-all` and install docs, but it should
not compete with the core production flow unless release/install documentation
is the page being read.

## History-Aware `start` Contract

When `deadreckon start` runs in a project scope with existing DeadReckon history,
it must not assume "brand new run" is the only useful production path. It should
make these paths available without requiring the user to remember lower-level
verbs:

| Situation | Offered path | Existing command machinery |
|---|---|---|
| Completed run exists and user wants a follow-up | Extend a completed run with the new goal | `extend <run-id> "<goal>"` |
| Existing project needs another independent pass | Start fresh work in the current source | `run` or `orchestrate` through `start` |
| User wants independent review/fix of prior work | Start review orchestration | `orchestrate review` through `start --mode review` |
| User wants a broader multi-agent pass | Start full-plan orchestration | `orchestrate full-plan` through `start --mode full-plan` |
| User is unsure which prior item matters | Point to scoped history | `list`, then `status`/`show` as needed |

Rules:

- Detect current project runs/plans with existing `DeadreckonPaths`,
  `list_runs`, and plan listing helpers. Do not add durable state.
- Only offer extend for terminal completed/promoted runs that the existing
  `extend` command can actually consume.
- Do not silently pick a prior run in non-TTY mode. Non-TTY recovery should show
  concrete commands such as `deadreckon extend latest "goal"` and
  `deadreckon start "goal" --mode review --yes`.
- TTY `start` may add a "continue from prior run" choice before final launch
  confirmation. The final preview must name the chosen base run/plan id, source
  mode, provider roles, done criteria, and watch/finish commands.
- Existing lower-level verbs remain available and should be named in advanced
  help, but the production path should make follow-up work discoverable from
  `start`.

## Done-Criteria Transparency Contract

Every place that prompts about done criteria, including `start`, direct `run`,
orchestration setup, and any recovery path touched by this goal, must let the
user understand and act before launch.

Minimum prompt facts:

- source label: project file, generated file, explicit path, default gate, or
  missing;
- path when a file exists;
- short human summary of the criteria or checks;
- whether it has just been checked/evaluated and the result;
- exact command to inspect or update it manually.

Minimum prompt actions when interactive:

- **keep** the current criteria;
- **view** the current criteria/check summary;
- **check/evaluate** the criteria against the current working tree when the
  existing check machinery supports it;
- **update** through the existing `def-done` flow or manual editor/text path;
- **cancel** before any run/plan state is created.

Rules:

- Do not introduce a second done-criteria file format.
- Do not ask "use default gate?" without saying exactly what the default gate
  will check.
- Do not hide `dr-gate` behind brand copy when the user is evaluating criteria;
  it is fine to explain that `dr-gate` enforces the criteria after launch.
- Non-TTY refusals should include `try: deadreckon def-done ...` and, when a
  file exists, a concrete view/check command.
- If existing command coverage is too uneven to make every old prompt comply in
  one slice, update all prompts touched by `start` and normal launch paths, then
  record the remaining prompt locations in `docs/V1-CANDIDATES.md`.

## Default Help Shape

`deadreckon --help` should be short enough to scan in one terminal screen and
should avoid making every implementation verb a peer. Target shape:

```text
Usage:
  deadreckon [command]

Production flow:
  deadreckon start "build the app"
  deadreckon attach latest
  deadreckon status
  deadreckon list
  deadreckon finish latest

Start, watch, keep:
  start    begin supervised agent work
  attach   watch and understand a run, chain, or plan
  status   see the latest item and next action
  list     find runs and plans
  finish   apply or export completed work

Setup and health:
  init      configure deadreckon
  doctor    check provider, sandbox, and local setup
  def-done  write/check done criteria in English

Control:
  kill      stop a run, chain, or plan
  resume    resume interrupted work
  cleanup   remove stale or completed worktrees

Find more:
  help-all         show every command, including advanced commands
  <command> --help detailed help for one command
```

Exact wording can vary, but the invariants cannot:

- `list` is in the first-screen model.
- `run`, `orchestrate`, `chain`, `plan`, `fork`, `merge`, `apply`, `export`,
  `doc`, `show`, `history`, `import`, `learn`, and `improve` are not first-screen
  peers.
- The path to advanced verbs is obvious and nonjudgmental.
- The audience sentence still says DeadReckon supervises provider CLIs rather
  than replacing them.

## `help-all` Shape

`deadreckon help-all` becomes the friendly command map. It should:

- state that the default help is the production model;
- list every callable command family;
- label direct `run`/`orchestrate`/`chain` as power-user launch paths;
- label `plan`/`fork`/`merge` as orchestration building blocks;
- label `apply`/`export` as direct result operations usually reached through
  `finish`;
- keep compatibility aliases inline on canonical rows;
- retain output/scripting policy, provider roles, and spend glossary;
- point back to the primary model at the top.

Do not hide advanced commands in a way that forces users to read source or use
shell completion to discover them.

## Command-Specific Help Rules

Every advanced command that remains callable should explain its relationship to
the primary model.

Examples:

- `run --help`: "Power-user one-run launcher. Most users can start with
  `deadreckon start "goal"`."
- `orchestrate --help`: "Power-user multi-agent launcher. `start --mode review`
  and `start --mode full-plan` route here."
- `chain --help`: "Serial multi-step power tool. Start with `start` unless you
  already know the step sequence."
- `apply --help`: "Direct git apply operation. `finish <id>` is the normal
  completed-work path."
- `export --help`: "Direct artifact copy operation. `finish <id>` is the normal
  completed-work path."
- `doc --help`: "Direct run-doc reader/regenerator. `attach --view narrative`
  and `finish` surface the common reading paths."
- `show --help`: "Raw inspection. `status` and `list` are the normal orienting
  commands."

Do not bury the direct commands under shaming language. Power users should feel
they found the deeper map, not a deprecated corner.

## Documentation Rules

README/HOWTO first contact should use:

```bash
deadreckon start "build the app"
deadreckon attach latest
deadreckon status
deadreckon list
deadreckon finish latest
```

Docs may still teach direct `run`, `orchestrate`, `chain`, `apply`, `export`,
and `doc` in advanced sections. Those sections must say why a user would reach
for the direct verb.

Replace broad "alpha software" first-screen warnings with precise status
language. Acceptable framing:

- "Production-use preview for local supervised agent CLI work."
- "0.x CLI: power-user commands and aliases may still change."
- "Core lifecycle is implemented and tested; broad release/stress verification
  remains operator-controlled."

Do not claim cloud reliability, team policy management, deployment safety, or
provider neutrality beyond what is implemented.

## Phases

Each phase: write the named depth tests first and watch them fail; implement;
run the focused commands listed for the phase; commit locally with a
conventional message; append one concise CHANGELOG bullet where relevant.

### P1 - Command taxonomy spec in code

- Add or refine command-catalog metadata so each command has an audience/family:
  primary, setup-support, advanced, compatibility, or pseudo.
- Keep this static and local; no config schema, runtime state, or new file
  format.
- Make the taxonomy reusable by top help, help-all, and tests.

Depth tests:

- `command_help_catalog_assigns_every_real_command_to_one_audience`
- `production_command_model_contains_start_attach_status_list_finish_doctor_kill_resume_cleanup`
- `advanced_command_model_keeps_run_orchestrate_chain_plan_fork_merge_apply_export_doc_show_history_import_learn_improve`

Focused verification:

```bash
cargo test -p deadreckon command_help_catalog_
cargo test -p deadreckon production_command_model
git diff --check
```

### P2 - Simplify default help

- Rework `print_top_help` and catalog grouping so `deadreckon --help` teaches
  the production model.
- Include `list` in the production flow and command rows.
- Remove first-screen rows for advanced implementation verbs while preserving
  their command handlers and clap parsing.
- Keep the audience copy concise.

Depth tests:

- `top_help_shows_production_flow_with_list`
- `top_help_hides_advanced_launch_and_result_verbs`
- `top_help_points_to_help_all_and_command_help_for_more`
- `top_help_names_harness_of_harnesses_audience_without_alpha_first`

Focused verification:

```bash
cargo test -p deadreckon --test coherence top_help
./target/debug/deadreckon --help
git diff --check
```

### P3 - Make `help-all` the friendly full map

- Reframe `help-all` as "the full command map", not an afterthought.
- Keep all commands listed.
- Group advanced commands by job rather than implementation chronology.
- Preserve output/scripting policy, provider-role glossary, spend-cap glossary,
  and alias notes.

Depth tests:

- `help_all_lists_every_clap_command_after_default_help_simplification`
- `help_all_labels_power_user_launch_paths`
- `help_all_labels_finish_as_normal_apply_export_path`
- `help_all_keeps_output_provider_and_spend_glossaries`

Focused verification:

```bash
cargo test -p deadreckon --test coherence help_all
./target/debug/deadreckon help-all
git diff --check
```

### P4 - Align command-specific help

- Update `after_help` constants and command descriptions for advanced verbs so
  they explain their relation to the production model.
- Do not make direct commands sound deprecated.
- Ensure `start`, `attach`, `status`, `list`, `finish`, `doctor`, `kill`,
  `resume`, and `cleanup` have first-class help text.

Depth tests:

- `run_help_points_to_start_without_hiding_direct_run`
- `orchestrate_help_points_to_start_modes_without_hiding_roles`
- `chain_help_explains_serial_power_user_path`
- `apply_export_doc_show_help_point_to_finish_status_or_attach`
- `primary_command_help_has_no_advanced_only_jargon_first`

Focused verification:

```bash
cargo test -p deadreckon --test coherence command_help
cargo test -p deadreckon --test lifecycle help
git diff --check
```

### P5 - History-aware `start`

- Extend the guided `start` decision model so existing project history can
  produce an extend/new-pass choice without adding durable state.
- TTY mode may ask whether the user wants a new run, a follow-up from a
  completed run, a review pass, or a full-plan pass.
- Non-TTY mode must stay deterministic and print exact lower-level commands
  instead of guessing a prior run.
- Final previews must name the selected prior run/plan when one is used.

Depth tests:

- `start_in_repo_with_completed_history_offers_extend_choice`
- `start_extend_choice_dispatches_existing_extend_path`
- `start_existing_history_offers_review_and_full_plan_new_passes`
- `start_non_tty_history_does_not_guess_prior_run_and_prints_try_lines`
- `start_history_preview_names_base_run_and_next_actions`

Focused verification:

```bash
cargo test -p deadreckon --test orchestrate start_
cargo test -p deadreckon --test lifecycle extend
git diff --check
```

### P6 - Done-criteria transparency and update prompts

- Update `start` and normal launch prompt paths so done-criteria decisions
  always expose source, path, summary, check/evaluation status, and manual
  commands.
- Add interactive actions for view, check/evaluate, update, keep, and cancel
  wherever the existing substrate supports them.
- Reuse `def-done` and existing acceptance check machinery; do not create a
  second criteria writer or schema.
- Make non-TTY recovery show view/check/update commands when criteria exist or
  are missing.

Depth tests:

- `start_done_prompt_shows_current_criteria_path_and_summary`
- `start_done_prompt_can_check_existing_criteria_before_launch`
- `start_done_prompt_can_update_existing_criteria_before_launch`
- `run_and_orchestrate_done_prompts_do_not_accept_opaque_default_gate`
- `non_tty_done_recovery_prints_view_check_update_try_lines`

Focused verification:

```bash
cargo test -p deadreckon --test orchestrate start_missing_done
cargo test -p deadreckon --test lifecycle acceptance done
git diff --check
```

### P7 - Contextual hints prefer the production model

- Audit post-action hints, refusal footers, and `try:` lines touched by normal
  flows.
- Prefer `finish` over direct `apply`/`export` unless the user is already in an
  advanced direct command.
- Prefer `start` for new work, `attach` for watching, `status`/`list` for
  orientation, `doctor` for setup health, `kill` for stopping, `resume` for
  interruption recovery, and `cleanup` for temporary worktrees.
- Keep direct advanced hints where the command context is explicitly advanced.

Depth tests:

- `post_start_footer_uses_primary_model_with_list_status_attach_finish`
- `completed_run_hints_prefer_finish_before_apply_export`
- `missing_provider_recovery_prefers_doctor_init_then_help_all`
- `existing_history_hints_offer_start_extend_and_list`
- `done_criteria_hints_offer_view_check_update`
- `advanced_command_context_may_still_hint_direct_verb`

Focused verification:

```bash
cargo test -p deadreckon --test orchestrate start_
cargo test -p deadreckon --test cards_exit_summary
cargo test -p deadreckon --test lifecycle run_completion
git diff --check
```

### P8 - README and HOWTO first-contact rewrite

- Make README first-contact copy production-facing without overclaiming 1.0
  stability.
- Make HOWTO's "New User Path" teach `start`, `attach`, `status`, `list`, and
  `finish`.
- Add a short "When this repo already has DeadReckon history" section showing
  `start` follow-up, `list`, and `extend latest`.
- Add a done-criteria transparency section: view, check/evaluate, update, and
  what default gate means.
- Move direct `run`, `orchestrate`, `chain`, `apply`, `export`, and `doc`
  examples into clearly labeled power-user or advanced sections.
- Preserve provider/done/sandbox explanations.

Depth tests:

- `readme_first_screen_teaches_production_flow_with_list`
- `howto_new_user_path_uses_start_attach_status_list_finish`
- `howto_existing_history_section_teaches_start_extend_orchestrate`
- `docs_explain_done_criteria_view_check_update`
- `docs_do_not_make_alpha_warning_the_first_user_action`
- `docs_still_teach_direct_run_orchestrate_chain_as_power_paths`

Focused verification:

```bash
cargo test -p deadreckon --test coherence readme howto docs_
git diff --check
```

### P9 - User-facing matrix and AS-BUILT command section

- Update `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md` to record
  the production model and advanced-map policy.
- Update AS-BUILT section 17 and coherence section 26 so future agents know the
  command model is intentional.
- Record the history-aware `start` and done-criteria transparency contracts.
- Do not delete history about alpha coherence; add the new production command
  model as the next state.

Depth tests:

- `user_facing_matrix_records_production_primary_model`
- `as_built_documents_default_help_and_help_all_split`
- `as_built_lists_list_as_primary_orienting_verb`
- `as_built_documents_history_aware_start`
- `as_built_documents_done_criteria_transparency`

Focused verification:

```bash
cargo test -p deadreckon --test coherence user_facing_matrix as_built
git diff --check
```

### P10 - Completion, parity, and stale-reference guardrails

- Verify shell completion generation still exposes callable commands.
- Ensure default-help simplification does not remove clap subcommands or break
  visible aliases.
- Add tests that compare the catalog, clap tree, and help-all output after the
  new audience model.
- Help changes must not change machine-readable behavior.
- Keep `--json` inspection surfaces documented in `help-all`, not top help.
- Ensure plain output examples remain ANSI-free.
- Ensure `--quiet` still suppresses chatter but not requested data or errors.
- Search docs and source copy for stale first-screen examples that still teach
  direct `run`, `orchestrate`, `apply`, or `export` as the normal first path.
- Keep advanced examples, but label them.
- Add a conservative docs test for the most important public docs instead of a
  brittle whole-repo wording ban.
- Add any broader command-renaming or release-policy questions to
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

Depth tests:

- `completion_scripts_still_include_advanced_verbs`
- `default_help_simplification_does_not_remove_clap_commands`
- `help_all_and_completion_agree_on_callable_command_names`
- `help_plain_output_keeps_no_ansi_contract`
- `help_all_json_policy_survives_production_model`
- `quiet_policy_still_suppresses_hints_not_errors`
- `primary_model_hints_respect_no_hints`
- `public_docs_first_path_uses_production_model`
- `advanced_docs_examples_are_labeled_power_user_or_advanced`
- `v1_candidates_records_out_of_scope_command_release_policy`

Focused verification:

```bash
cargo test -p deadreckon --test lifecycle completion_scripts
cargo test -p deadreckon --test coherence command_help_catalog plain json quiet hints public_docs_first_path
cargo test -p deadreckon --test cards_friendliness
rg -n 'deadreckon (run|orchestrate|apply|export)' README.md HOWTO.md docs
git diff --check
```

### P11 - Architecture, changelog, and final review

- Add an AS-BUILT subsection, likely under section 26 or a new section after
  local self-improvement, describing:
  - production-facing default help;
  - primary command model;
  - history-aware `start` for follow-up and new passes;
  - done-criteria view/check/update prompts;
  - `help-all` as the full command map;
  - no command deletion and no schema changes.
- Append a `Production command model (alpha)` or similar section to
  `/Users/gdc/deadreckon/CHANGELOG.md`.
- Ensure `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md` points to
  this goal and rider.
- Commit the docs closeout separately if earlier phases changed code.

Depth tests:

- `changelog_records_production_command_model`
- `as_built_production_command_model_section_matches_help_contract`
- `goal_rider_references_are_current`

Focused verification:

```bash
cargo test -p deadreckon --test coherence production_command_model
cargo fmt --check
git diff --check
```

## Integration Matrix

| Surface | Primary model visible | Advanced verbs findable | List included | Notes |
|---|---:|---:|---:|---|
| `deadreckon --help` | yes | via `help-all` and `<command> --help` | yes | One-screen target |
| `deadreckon help-all` | yes | yes | yes | Full command map |
| command-specific help | contextual | yes | where relevant | Power-user framing |
| README | yes | yes, later sections | yes | First-contact doc |
| HOWTO | yes | yes, later sections | yes | Operational doc |
| AS-BUILT | yes | yes | yes | Architecture truth |
| USER-FACING-MATRIX | yes | yes | yes | Design inventory |
| shell completion | not relevant | yes | yes | Do not remove callable commands |
| post-action hints | yes | context-dependent | yes | Prefer primary verbs |
| TTY `start` in repo with history | yes | yes | yes | Offer extend/new pass |
| non-TTY `start` in repo with history | yes | via concrete commands | yes | Do not guess prior run |
| done-criteria prompts | yes | yes | not relevant | View/check/update before launch |

## Error-Footer Canonical Pairs

| Situation | Preferred `try:` |
|---|---|
| User needs first launch | `deadreckon start "goal"` |
| User has prior history and wants a follow-up | `deadreckon start "follow-up goal"` or `deadreckon extend latest "follow-up goal"` |
| User needs setup health | `deadreckon doctor` |
| User needs provider setup | `deadreckon init` or `deadreckon config provider` only after `doctor`/setup context |
| User needs done criteria | `deadreckon def-done "what done means"` |
| User needs to inspect done criteria | `deadreckon def-done show` then `deadreckon def-done check` |
| User needs to watch work | `deadreckon attach <id>` |
| User needs orientation | `deadreckon status` or `deadreckon list` |
| Completed work needs keeping | `deadreckon finish <id>` |
| Live work needs stopping | `deadreckon kill <id>` |
| Interrupted work needs recovery | `deadreckon resume <id>` |
| Temporary work needs removal | `deadreckon cleanup --completed` |
| User asks for deeper command map | `deadreckon help-all` |

## Out of scope

- Removing or renaming callable top-level commands.
- Durable saved launch profiles.
- Automatic prior-run selection in non-TTY `start`.
- Changing `extend` into a multi-agent primitive; orchestration follow-up starts
  a new review/full-plan pass unless a separate future design says otherwise.
- A 1.0 release declaration, SemVer policy, or installer-channel promotion.
- Full output-layout facade beyond what help/hints need.
- New TUI screens for command discovery.
- Provider-specific onboarding wizards.
- Broad command-family renames or alias removals.
- Any change that requires migration of existing run, plan, chain, learning, or
  provider state.

## Dependencies

Tier 1: none expected. This should be static Rust/docs work over existing
helpers.

Tier 2: none expected.

Tier 3: any dependency used solely to render help, command docs, or snapshots is
blocked unless the human explicitly approves it. Existing clap/ratatui/ui
helpers are enough.

## Engineering invariants

- No durable schema changes.
- No command deletion.
- No broken shell completion.
- Top help is the production model; `help-all` is the full map.
- `list` is a first-screen orienting verb.
- `start` in a repo with history can lead to new work, an extend follow-up, or
  a new review/full-plan pass.
- Every done-criteria prompt touched by this goal shows what criteria will be
  enforced and offers view/check/update/cancel paths.
- `finish` is the normal completed-work verb; direct `apply`/`export` remain
  advanced.
- `start` is the normal begin verb; direct `run`/`orchestrate`/`chain` remain
  power-user paths.
- Depth tests come before implementation in every phase.
- Do not turn copy edits into broad refactors.
- Do not add brittle whole-repo word bans that make normal docs maintenance
  painful.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with focused tests and `git diff --check`.
- Run `cargo fmt` before any commit touching Rust.
- Do not run `make verify`, release builds, stress tests, or full-workspace
  tests by default. If the human asks for them, run them and fix issues.
- Keep commit messages conventional, for example:
  - `test: lock production command taxonomy`
  - `feat(cli): simplify default command help`
  - `docs: document production command model`
- Stop and update `docs/V1-CANDIDATES.md` for release-policy or command-removal
  decisions that exceed this UX slice.
