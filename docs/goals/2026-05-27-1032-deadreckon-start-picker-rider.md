# DeadReckon - Start Picker Rider

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-27-1032-deadreckon-start-picker-goal.md`.
It supersedes nothing in prior riders. It narrows the deferred guided-start
work from `/Users/gdc/deadreckon/docs/goals/2026-05-26-1510-deadreckon-guided-experience-rider.md`
into one implementable alpha slice: a selection-first interactive picker for
`deadreckon start`.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime
`/Users/gdc/.deadreckon`.

## Posture

- **Maturity stays alpha.** This is UX polish over the existing launcher, not a
  new runtime.
- **No durable state schema changes.** Do not change `PipelineState`, plan JSON,
  run JSON, chain JSON, or provider registry schemas.
- **No durable launch profiles.** Reusable saved launch shapes remain separate.
- **No LLM mode classifier.** Mode recommendation stays deterministic.
- **No provider-specific setup wizard.** Provider-specific auth/install flows
  stay in registry hints and setup commands unless an existing command already
  supports the action.
- **No alternate-screen TUI.** This is a normal terminal prompt that works over
  SSH and leaves readable transcript text.
- **No `git push`.** Phased local commits only.

## Existing Substrate

Current `start` lives mainly in:

- `/Users/gdc/deadreckon/crates/deadreckon/src/cli.rs`
  - `Commands::Start`
  - `CliStartMode`
  - `StartCommandArgs`
- `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs`
  - `StartLaunchDecision`
  - `start_launch_decision`
  - `resolve_start_setup`
  - `resolve_start_provider`
  - `resolve_start_done_criteria`
  - `resolve_start_source_mode`
  - `start_command`
  - `dispatch_start_command`
- `/Users/gdc/deadreckon/crates/deadreckon/src/setup.rs`
  - provider setup selection and source labels
- `/Users/gdc/deadreckon/crates/deadreckon/src/prompt.rs`
  - current line-oriented prompt wrapper
- `/Users/gdc/deadreckon/crates/deadreckon/src/ui.rs` and `ui_card.rs`
  - styling, streams, rows, headings, and prompt tone

At HEAD, `start` already dispatches to `run_command` and `orchestrate_command`,
but provider selections are not carried into those args; `RunCommandArgs` and
`PlanCommandArgs` already have provider fields that this goal can use.

## Prompt Dependency Policy

Use exactly one prompt crate only if the local wrapper cannot provide a real
selection picker without becoming bespoke UI code.

Preferred: `inquire`, wrapped behind DeadReckon's own abstraction. It provides
select, confirm, input, defaults, and validation in a conventional terminal
flow.

Acceptable fallback: extend `prompt.rs` with numbered selection prompts if the
dependency is rejected during implementation. If falling back, keep the same
`StartPrompter` trait and tests so a later crate swap is contained.

Do not call a prompt crate directly from `start_command` or setup resolvers.

## StartPrompter Contract

Introduce a small local abstraction, preferably near `prompt.rs` or a new
`start_picker.rs` module:

```text
trait StartPrompter {
    fn select_one(&mut self, prompt: SelectPrompt) -> Result<SelectChoice>;
    fn confirm(&mut self, prompt: ConfirmPrompt) -> Result<bool>;
    fn input(&mut self, prompt: InputPrompt) -> Result<String>;
}
```

The exact names can vary, but the shape must preserve:

- prompt title;
- optional help text;
- ordered choices;
- default choice index;
- stable machine id per choice;
- user-facing label;
- secondary detail text;
- cancel handling.

Tests use a fake prompter with scripted answers. PTY tests should cover one
real terminal flow, but most coverage should be deterministic unit or
integration tests with the fake.

## Prompt Eligibility

Prompts are allowed only when all are true:

- stdin is a TTY;
- stdout/stderr policy allows human output;
- `--json` is not set;
- `--plain` is not set;
- `--quiet` is not set;
- `--yes` is not set;
- command execution is not already fully determined by explicit flags.

`--preview` may prompt in a TTY because it is state-free after choices are
resolved. Non-TTY preview must keep today's deterministic behavior.

## Launch Decision Extensions

Extend the ephemeral decision, not durable state. Add fields as needed:

```text
provider_route: Option<String>
planner_provider_route: Option<String>
child_provider_route: Option<String>
coder_provider_route: Option<String>
reviewer_provider_route: Option<String>
provider_selection_source: Config | Detected | Interactive | Explicit | Missing
done_action: Existing | GenerateFromGoal | ManualText | Default | Missing
source_action: Worktree | InitGit | CopyCurrent | Fresh | AllowDirty | Missing
cancelled: bool
```

The names do not matter; the semantics do. These values should feed the
existing `RunCommandArgs` and `PlanCommandArgs` provider fields at dispatch
time, so a user can choose a detected provider in `start` without manually
running `deadreckon config provider` first.

Do not silently persist a selected provider as the default. If the picker offers
"use and save as default", it must call or share the existing config-provider
validation/write path and show the equivalent manual command.

## Picker Flow

### 1. Resolve Existing Facts First

Before asking anything, gather the same facts current `start` gathers:

- configured defaults;
- configured routes;
- detected usable CLI providers;
- done-criteria source;
- git/non-git source state;
- dirty worktree state;
- requested mode flag;
- preview/json/plain/quiet/yes flags.

Obvious cases should still avoid needless questions. Example: configured
provider, project done criteria, clean git repo, and explicit `--mode run`
should proceed to preview/confirmation without extra picker pages.

### 2. Mode Picker

Shown when `--mode auto` is active and prompt eligibility holds.

Choices, in order:

1. recommended path, with reason from current heuristic;
2. single supervised run;
3. coder/reviewer orchestration;
4. full-plan orchestration.

Labels should be plain English. Include the equivalent flag as secondary text,
for example `--mode review`.

Depth expectations:

- selecting review sets `StartSelectedMode::Review` and
  `StartSelectionSource::InteractiveChoice`;
- selecting full-plan sets `StartSelectedMode::FullPlan`;
- cancel refuses before any state change with a `try: deadreckon start "<goal>"`
  line.

### 3. Provider Picker

Shown when no usable configured provider is available, or when the user chooses
an "change provider" advanced option from the final preview.

Choices:

- configured default, if usable;
- detected ready CLI providers, with route ids like `cli:codex`;
- other configured routes;
- type a provider route;
- open setup command / cancel.

For run mode, selected route maps to `RunCommandArgs.provider`.

For review mode, keep alpha simple: use selected route for both coder and
reviewer unless the user explicitly opens advanced role selection. Advanced role
selection can be deferred, but the preview must be honest.

For full-plan mode, keep alpha simple: use selected route as planner and
default child provider unless the user explicitly opens advanced role
selection. Advanced per-child selection can be deferred.

If a typed route is invalid, refuse with provider validation output and
`try: deadreckon providers list --all`.

### 4. Done-Criteria Picker

Shown when project done criteria are missing and prompt eligibility holds.

Choices:

- create from goal using existing `def-done` flow;
- write criteria manually;
- use default gate behavior only if the existing run/orchestrate path already
  supports that safely;
- cancel.

Generation must use the existing provider setup and `def-done` implementation,
not a new prompt-to-provider path. Manual text should flow through the same
acceptance file compiler/writer used by `def-done`.

If implementation discovers that direct `def-done` reuse is too large for this
slice, keep non-mutating recovery behavior and add a clear V1/next-goal note.
Do not invent a second acceptance file format.

### 5. Source Picker

Replace `prompt_start_non_git_mode()` numeric entry with the shared picker.

Non-git choices:

- initialize git, then use worktree mode;
- copy current directory into the run workspace;
- fresh empty workspace;
- cancel.

Dirty git choices:

- stop and show `git stash` recovery;
- continue with `--allow-dirty` semantics;
- cancel.

The final dispatch must preserve current safety behavior. If source flags are
still alpha-limited for orchestrated `start`, the picker must either hide those
choices for orchestration or refuse before state change with a specific message.

### 6. Final Preview And Confirmation

Before dispatch, render the shared launch preview rows:

```text
goal
path
provider
done
workspace
watch
stop
finish
override
```

Then ask one confirmation prompt unless `--yes`, `--preview`, or `--quiet`
policy says otherwise. Cancel exits zero only for preview cancellation; cancel
before an intended launch should refuse with code 1 and a resumable command.

## Output Contracts

- Prompt labels are stable enough to test.
- `--json` never includes prompt text and never blocks.
- `--plain` never starts a picker; it should preserve current script-friendly
  rows and recovery messages.
- `--quiet` never starts a picker; errors still include `try:` lines.
- Non-TTY behavior remains deterministic and never reads stdin for choices.
- All refusal cases include equivalent manual commands.

## Dispatch Contracts

For run:

- pass selected `provider_route` into `RunCommandArgs.provider`;
- leave `model` unset unless a future explicit custom-model prompt is added;
- pass source choices through existing run flags;
- preserve lifecycle footer behavior.

For review:

- pass selected route into `coder_provider` and `reviewer_provider` unless
  advanced role picker is implemented;
- preserve existing `recommend_child_count_for_goal` behavior where relevant;
- preserve repair defaults.

For full-plan:

- pass selected route into `planner_provider` and `provider` unless advanced
  role picker is implemented;
- do not add per-child role selection in this alpha unless it is very small;
- preserve full-plan child-count recommendations.

## Phases

Every phase starts by adding the named depth tests and watching them fail, then
implementation, then focused verification, then a conventional local commit.
Keep commits scoped. Do not run `make verify`, release builds, stress tests, or
full-workspace suites unless the human explicitly asks.

### P1 - Prompt Abstraction

- Add `StartPrompter`, prompt structs, choice structs, and fake prompter.
- Choose `inquire` or local numbered fallback.
- Keep external prompt crate calls isolated.

Depth tests:

- `start_fake_prompter_selects_default_choice`
- `start_fake_prompter_records_prompt_order`
- `start_prompt_eligibility_skips_json_plain_quiet_non_tty_yes`

Focused verification:

```bash
cargo test -p deadreckon prompt
cargo test -p deadreckon start_prompt
```

### P2 - Mode Picker

- Add interactive mode selection after deterministic recommendation.
- Record `InteractiveChoice` source.
- Cancel safely before setup mutation.

Depth tests:

- `start_tty_mode_picker_recommends_run_first_for_focused_goal`
- `start_tty_mode_picker_can_choose_review`
- `start_tty_mode_picker_can_choose_full_plan_preview`
- `start_mode_picker_cancel_refuses_before_state`

Focused verification:

```bash
cargo test -p deadreckon start_mode
cargo test -p deadreckon --test orchestrate start_tty_mode_picker
```

### P3 - Provider Picker

- Build provider choice list from existing setup/registry facts.
- Carry selected routes into launch decision.
- Dispatch selected provider routes into run/plan args.

Depth tests:

- `start_provider_picker_lists_configured_default_first`
- `start_provider_picker_lists_detected_cli_routes`
- `start_detected_provider_can_be_used_ephemerally_without_config_write`
- `start_invalid_typed_provider_refuses_with_providers_list_try_line`

Focused verification:

```bash
cargo test -p deadreckon start_provider
cargo test -p deadreckon --test orchestrate start_detected_provider
```

### P4 - Done-Criteria Picker

- Use existing done-criteria resolver when present.
- For missing criteria, offer create/manual/default/cancel where supported.
- Route create/manual through existing `def-done`/acceptance writer logic.

Depth tests:

- `start_done_picker_uses_existing_project_criteria_without_prompt`
- `start_done_picker_can_create_from_goal`
- `start_done_picker_manual_text_writes_acceptance`
- `start_done_picker_cancel_refuses_with_def_done_try_line`

Focused verification:

```bash
cargo test -p deadreckon start_done
cargo test -p deadreckon --test orchestrate start_missing_done
```

### P5 - Source Picker

- Replace numeric non-git prompt with shared picker.
- Add dirty-worktree picker when prompt eligible.
- Keep current non-TTY refusal text and `try:` lines.

Depth tests:

- `start_non_git_picker_offers_init_copy_fresh_cancel`
- `start_non_git_picker_copy_sets_from_current_dir`
- `start_non_git_picker_fresh_sets_fresh_mode`
- `start_dirty_git_picker_can_choose_allow_dirty`
- `start_source_picker_skips_for_plain_json_quiet`

Focused verification:

```bash
cargo test -p deadreckon --test orchestrate start_non_git
cargo test -p deadreckon --test orchestrate start_dirty_git
```

### P6 - Preview And Confirmation

- Render final preview before dispatch.
- Add final confirmation prompt for intended launches.
- Ensure `--preview` can prompt then stop without state.

Depth tests:

- `start_picker_preview_prints_resolved_human_choices`
- `start_picker_preview_creates_no_run_or_plan_state`
- `start_picker_launch_confirmation_no_refuses_before_state`
- `start_picker_launch_confirmation_yes_dispatches`

Focused verification:

```bash
cargo test -p deadreckon --test orchestrate start_picker_preview
cargo test -p deadreckon --test lifecycle start_picker
```

### P7 - Orchestration Role Honesty

- For review/full-plan, decide whether alpha supports advanced role selection.
- If yes, implement a nested picker for planner/child/coder/reviewer.
- If no, preview explicitly says the selected provider is reused for all roles.

Depth tests:

- `start_review_preview_says_selected_provider_is_coder_and_reviewer`
- `start_full_plan_preview_says_selected_provider_is_planner_and_child`
- `start_orchestration_source_picker_refuses_unsupported_source_flags_before_state`

Focused verification:

```bash
cargo test -p deadreckon --test orchestrate start_review_preview
cargo test -p deadreckon --test orchestrate start_full_plan_preview
```

### P8 - Output And Script Contracts

- Lock JSON/plain/quiet/non-TTY no-prompt behavior.
- Ensure prompt text goes to human streams only.
- Preserve recovery `try:` lines.

Depth tests:

- `start_json_never_emits_picker_labels`
- `start_plain_never_invokes_prompter`
- `start_quiet_never_invokes_prompter`
- `start_non_tty_never_reads_stdin_for_picker`

Focused verification:

```bash
cargo test -p deadreckon --test coherence start_
cargo test -p deadreckon --test orchestrate start_json
```

### P9 - Help And Copy

- Update `start --help`, README, and HOWTO only where useful.
- Do not turn help into a manual; one concise mention of interactive picker is
  enough.
- Keep provider-neutral examples.

Depth tests:

- `start_help_mentions_interactive_picker_and_script_flags`
- `readme_first_use_mentions_picker_without_requiring_provider_choice`
- `howto_start_keeps_power_user_run_and_orchestrate_paths`

Focused verification:

```bash
cargo test -p deadreckon --test coherence start_help
cargo test -p deadreckon --test lifecycle help
```

### P10 - PTY Smokes

- Add a small number of PTY integration tests for the real picker.
- Keep them short and deterministic.
- Avoid provider calls by using fake/smoke providers and temp homes.

Depth tests:

- `pty_start_picker_choose_full_plan_preview`
- `pty_start_picker_choose_detected_provider_preview`
- `pty_start_picker_cancel_exits_before_state`

Focused verification:

```bash
cargo test -p deadreckon --test orchestrate pty_start_picker
```

### P11 - Architecture, Changelog, And Deferrals

- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` section 17.1
  to explain the picker, prompt eligibility, and non-TTY contract.
- Append a `Start picker (alpha)` section to
  `/Users/gdc/deadreckon/CHANGELOG.md`.
- Update `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`:
  - remove or narrow any deferral that this goal closes;
  - keep durable launch profiles, LLM mode classification, and
    provider-specific setup wizards as separate candidates.
- Update README/HOWTO only if command behavior or first-use copy changed.

No depth test is required for docs-only changes unless docs tests already cover
the edited copy.

## Final Smoke Set

Run only focused verification by default:

```bash
cargo test -p deadreckon start_
cargo test -p deadreckon --test coherence start_
cargo test -p deadreckon --test orchestrate start_
cargo test -p deadreckon --test lifecycle start_
```

Optional manual checks:

```bash
deadreckon start "build the app" --preview
deadreckon start "review this codebase" --preview
deadreckon start "split frontend docs and tests into parallel work" --preview
deadreckon start "build the app" --json
```

Do not run `make verify`, release builds, smoke, stress, or full-workspace tests
as part of normal goal execution unless the human explicitly asks.

## Stop Conditions

Stop when:

- TTY users can make the major `start` choices from selections.
- Non-TTY, JSON, plain, quiet, and `--yes` behavior stays deterministic.
- Selected providers flow into run/orchestrate dispatch without requiring a
  config write.
- Preview and recovery text remain provider-neutral and include `try:` lines.
- Focused tests pass.
- AS-BUILT and CHANGELOG record the alpha behavior and remaining deferrals.
- Work is committed locally.
