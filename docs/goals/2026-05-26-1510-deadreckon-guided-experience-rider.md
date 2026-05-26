# deadreckon - Guided Experience Rider (Obvious First Use)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-26-1510-deadreckon-guided-experience-goal.md`.
It supersedes nothing in prior riders, especially setup, orchestration,
coherence, event bus, provider flight recorder, and self-improvement. Their
invariants still apply. This rider adds a product-positioning contract and a
guided launcher that makes "run a goal" and "orchestrate a goal" obvious from a
cold start.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided - do not redesign)

- **Maturity stays `alpha`.** The goal improves clarity and approachability; it
  does not claim stable CLI compatibility.
- **No `PipelineState` schema changes.** A launch decision is an ephemeral
  command decision. If a helper needs test fixtures, keep them in tests or
  non-durable structs.
- **Do not hide the model.** `run`, `orchestrate`, `plan`, `fork`, and `merge`
  remain real. The new guided path explains and routes to them.
- **Do not pre-select a specific provider brand in docs or examples.** Use the
  configured default in normal examples; mention provider CLIs as supported
  categories only.
- **Do not weaken safety.** Existing provider setup, sandbox, done criteria,
  gate, worktree, budget, attach, repair, flight, import, and learning
  contracts remain.
- **No V1 invention.** Durable profiles, telemetry-driven personalization,
  cloud onboarding, or provider-specific setup wizards beyond this slice go to
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.
- **No `git push`.** Phased local commits only.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Product Identity Contract

DeadReckon must answer "who is this for?" in the first screen a serious user
sees:

> DeadReckon is for people who already use agent CLIs and need those agents to
> run longer, safer, and more accountably than a raw terminal session allows.

Supporting copy may vary by surface, but every public first-use surface should
communicate these four ideas:

- **Bring your own agent.** DeadReckon supervises Claude Code, Codex, Copilot
  CLI, Pi, Cursor CLI, direct APIs, and compatible providers instead of
  replacing them.
- **Say what done means.** DeadReckon turns plain-English done criteria into a
  gate the agent cannot self-attest.
- **Walk away without losing the run.** State, traces, provider logs, docs,
  checkpoints, and lifecycle commands survive the terminal.
- **Finish deliberately.** The result is a promoted artifact that can be
  inspected, exported, applied, extended, or abandoned.

Avoid grand claims. Do not call it a general autonomous engineer, IDE,
chatbot, deployment system, observability platform, or model-training product.

## Guided Front Door

Add a new visible command:

```text
deadreckon start "<goal>"
    [--mode auto|run|review|full-plan]
    [--preview]
    [--yes]
    [--plain]
    [--quiet]
    [--json]
    [shared run/orchestrate overrides that already exist where practical]
```

`start` is the default recommendation for new users. Existing commands keep
their jobs:

- `run` - one supervised coding run.
- `orchestrate` - one-command multi-agent wrapper.
- `plan` / `fork` / `merge` - advanced orchestration primitives.

## Rust Library Posture

Use Rust libraries to make the start sequence reliable, but keep the user flow
owned by DeadReckon's own setup contracts.

### Already in the workspace

- **`clap`** remains the command surface owner. Use `ValueEnum` for
  `--mode auto|run|review|full-plan`, shared `Args` structs for common
  output/preflight flags, and catalog tests for help drift.
- **`crossterm` + `ratatui`** remain attach/TUI owners. Do not make `start`
  an alternate-screen TUI; the start flow should be a normal terminal wizard
  that works over SSH, in CI, and when pasted into a shell.
- **Existing `ui.rs`, `ui_card.rs`, and `prompt.rs`** remain the first place to
  add shared rows, try-lines, confirmations, and prompt wrappers.
- **Existing `setup.rs`** remains the resolver for provider and done-criteria
  choices. Start should compose these selectors instead of inventing parallel
  provider or acceptance logic.
- **`which` and the provider registry** remain the detection layer for local
  CLI providers. Do not add shell-specific probing crates unless registry
  detection proves insufficient.

### Optional prompt dependency

If the hand-rolled prompt wrapper becomes awkward, add exactly one prompt
library after a focused spike:

- Prefer **`inquire`** when the implementation needs typed text, select,
  multi-select, confirm, validators, defaults, and completion-like behavior in
  a conventional terminal prompt.
- Consider **`dialoguer`** only if DeadReckon wants a smaller prompt surface
  focused on confirm/input/select and is comfortable pairing it with the
  console-rs ecosystem.
- Avoid **`cliclack`** for this slice unless the team deliberately wants its
  opinionated visual style; DeadReckon already has a visual identity and output
  contract.
- Avoid **`requestty`** for this slice unless conditional multi-question flows
  become hard to express; the start sequence should remain a small deterministic
  decision tree, not an Inquirer-style form engine.

Any prompt dependency must be wrapped behind a small local abstraction:

```text
trait StartPrompter {
    fn confirm(&mut self, prompt: ConfirmPrompt) -> Result<bool>;
    fn select_one(&mut self, prompt: SelectPrompt) -> Result<String>;
    fn input(&mut self, prompt: InputPrompt) -> Result<String>;
}
```

Tests use a fake prompter; command handlers never call the external crate
directly. Non-TTY, `--json`, `--plain`, and `--quiet` paths must bypass the
interactive prompter and return deterministic decisions with `try_lines`.

### Selection-first interaction

Interactive `deadreckon start` should be selection-first, not typing-first. The
user should almost never have to remember an internal provider id, orchestration
mode, or setup flag during first use.

TTY prompts should present concrete choices:

- **Provider route.** Show configured default first, then detected ready CLI
  providers, then other configured routes. Include a typed "other route" escape
  hatch for advanced users.
- **Mode.** Show the recommended mode first with its reason, then single run,
  review, and full-plan. The labels should be plain English; the flag names can
  appear as secondary detail.
- **Done criteria.** Show existing project criteria when present, generate from
  the goal, write manually, or use default gate behavior. If generation needs a
  provider, say which provider will be used before calling it.
- **Source mode.** Outside git, offer init git, copy mode, fresh mode, or
  cancel. In a dirty git repo, offer the existing allow-dirty/autostash-safe
  choices rather than silently carrying dirty files.
- **Model.** Do not ask by default. Offer a "custom model" prompt only from an
  advanced option or when the user explicitly requested model selection.

Non-TTY behavior is the opposite: never prompt, never block waiting for input,
and never silently mutate config. Return a deterministic refusal or preview with
`try_lines` that correspond to the same choices the TTY menu would have offered.

Typed input remains available, but only as an escape hatch:

- custom provider route;
- custom model;
- custom done-criteria sentence;
- custom export/source path if a future phase adds it.

Selection labels are user-facing strings and need tests. Internal ids are shown
only when they help the user understand a command they can paste, for example
`cli:codex` in `deadreckon config provider cli:codex`.

### Do-directly policy

The start sequence may perform a step directly only when all of these are true:

- the user is in a TTY or passed an explicit `--yes`/mode flag;
- the mutation is already supported by an existing command path;
- the preview showed the mutation and its rollback or follow-up command;
- JSON/plain/quiet behavior remains deterministic;
- refusal text still includes the equivalent manual `try:` command.

Examples:

- Configure a detected provider directly only from an interactive selection or
  explicit flag; otherwise print `deadreckon config provider <route>`.
- Generate done criteria directly only through the existing `def-done` flow;
  otherwise print `deadreckon def-done "..."`
- Initialize git directly only after interactive confirmation or an explicit
  flag; otherwise print `git init` / `deadreckon start --from .` /
  `deadreckon start --fresh` recovery lines.

### Launch decision

Implement a small `LaunchDecision` helper near existing CLI setup/preflight
helpers. It must be deterministic, testable, and free of durable state.

Fields:

```text
goal: String
selected_mode: Run | Review | FullPlan
selection_source: ExplicitFlag | Heuristic | InteractiveChoice | Default
reason: String
provider_source: Configured | Detected | Missing
done_criteria_source: Project | Generated | Default | Missing
source_mode: Worktree | InitGit | Copy | Fresh | InPlace | Missing
requires_confirmation: bool
try_lines: Vec<String>
```

If the real implementation already has equivalent structs, reuse them instead
of introducing this exact type. The contract matters more than the name.

### Mode selection

`--mode run`, `--mode review`, and `--mode full-plan` are explicit. `--mode
auto` is the default:

- If the user asks for review, second-pass cleanup, critique, hardening, or
  validation, recommend review orchestration.
- If the goal clearly names multiple independent workstreams, many modules, or
  "parallel", recommend full-plan orchestration.
- Otherwise recommend a normal run.
- Under TTY, ambiguous auto mode may ask a single prompt. Non-TTY auto mode
  must pick the conservative normal run unless an explicit mode is supplied.

Never call a provider just to choose the mode in this alpha slice. LLM-based
goal classification belongs in V1 unless the implementation can reuse an
already-required provider call without extra cost or privacy exposure.

### Setup resolution order

The start path resolves setup in this order:

1. **Provider.** Use configured default. If absent, run the same detection logic
   exposed by `detect` and show the most likely next command. Do not silently
   write config in non-interactive mode.
2. **Done criteria.** Use project criteria when present. If missing under TTY,
   offer to create from the goal using the existing `def-done` flow. In
   non-TTY, show `try: deadreckon def-done "..."; deadreckon start "..."`
   unless an existing default can run safely.
3. **Source mode.** In git, prefer worktree. Outside git, TTY may offer init
   git, copy, fresh, or cancel; non-TTY refuses with `try:` lines unless flags
   choose a mode.
4. **Budget and wall clock.** Use configured defaults. High budgets keep the
   existing confirmation policy.
5. **Execution.** Dispatch to the existing run/orchestrate handlers after the
   launch decision is complete.

## Output Contract

Every preview for `start`, `run`, and `orchestrate` should share a builder that
can render the same facts:

```text
goal:          <short goal summary>
path:          run | review orchestration | full-plan orchestration
provider:      <provider or missing>
done:          <criteria source or missing>
workspace:     <worktree/copy/fresh/in-place/init-git>
watch:         deadreckon attach <id-or-after-start>
stop:          deadreckon kill <id-or-after-start>
finish:        deadreckon finish <id-or-after-start>
override:      <one command to force a different path, when relevant>
```

After a successful start, print a lifecycle footer with exact ids:

```text
watch:  deadreckon attach <id>
status: deadreckon status <id>
stop:   deadreckon kill <id>
finish: deadreckon finish <id>
```

For JSON output, include `kind`, `goal`, `selected_mode`, `reason`,
`provider`, `done_criteria`, `source_mode`, `will_start`, `next_actions`, and
`try_lines`. No ANSI, no prose-only hints.

For quiet output, success prints nothing unless existing command policy already
requires an id for scriptability. Refusals still print errors to stderr with
`try:` lines.

## Documentation Contract

Update these files:

- `/Users/gdc/deadreckon/README.md`
- `/Users/gdc/deadreckon/HOWTO.md`
- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`
- `/Users/gdc/deadreckon/CHANGELOG.md`
- `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` if deferring anything

README first viewport must answer:

- Who is it for?
- Why not just run the provider CLI directly?
- What is the first command?
- How do I watch and finish the work?

HOWTO must keep both the new-user path and the power-user paths:

```bash
deadreckon start "build the app"
deadreckon run "build the app"
deadreckon orchestrate "build and review the app"
```

Do not remove accurate lower-level docs. Reorder and label them so they feel
like "advanced control" rather than prerequisites.

## Refusal And Recovery Cases

Every case below needs either a depth test or a golden output assertion:

| Case | Required behavior |
|---|---|
| No provider configured, no CLI detected | Refuse with `deadreckon init` and `deadreckon detect` try-lines. |
| Provider CLI detected but not configured | Suggest `deadreckon config provider <route>` or interactive `deadreckon init`. |
| No done criteria | TTY offers creation; non-TTY gives `def-done` try-line. |
| Non-git directory without mode | TTY offers init/copy/fresh; non-TTY gives `--from .`, `--fresh`, and `git init` try-lines. |
| Dirty git worktree | Reuse existing allow-dirty/autostash guidance; do not silently include dirty files. |
| High spend or wall-clock | Preserve existing confirmation/escalation behavior. |
| Auto recommends orchestration | Preview explains why and shows `--mode run` override. |
| Auto recommends run | Preview shows `--mode review` / `--mode full-plan` override examples. |
| JSON mode | Includes structured `try_lines` and no ANSI. |
| Quiet/plain | Honors current stream/styling rules. |

## Phases

Each phase follows the same loop: write depth tests first and watch them fail;
implement; run focused verification for touched crates; commit locally with a
conventional message; add a one-line changelog note when the phase ships user
behavior. Do not run `make verify`, release builds, stress tests, broad smoke
suites, or full-workspace tests by default.

### P1 - Audience copy contract

- Centralize short audience/product copy in a helper or constants module so top
  help, README assertions, and docs tests cannot drift.
- Update top-level help/about copy to name the audience and the first command.
- Keep copy concise enough that `deadreckon --help` remains scannable.

Depth tests:

- `top_help_names_audience_and_start_path`
- `public_docs_first_screen_explains_harness_of_harnesses`
- `audience_copy_does_not_call_deadreckon_a_provider_replacement`

### P2 - Start command parser

- Add visible `start` command.
- Add `--mode auto|run|review|full-plan`, `--preview`, `--yes`, and output mode
  flags consistent with existing commands.
- Keep aliases conservative. Do not add cute aliases in this slice.

Depth tests:

- `start_command_is_visible_in_top_help_and_help_all`
- `start_mode_values_parse_and_reject_unknown_modes`
- `start_help_points_to_run_or_orchestrate_for_power_users`

### P3 - Launch decision helper

- Build deterministic mode selection and setup-resolution helpers.
- Reuse existing provider, setup, done-criteria, and source-mode code instead
  of duplicating ad hoc checks.
- Keep heuristic strings testable and stable.

Depth tests:

- `start_auto_defaults_to_run_for_simple_goal`
- `start_auto_recommends_review_for_review_goal`
- `start_auto_recommends_full_plan_for_parallel_goal`
- `non_tty_ambiguous_auto_chooses_run`

### P4 - Shared preview/result renderer

- Factor a shared launch preview builder used by `start`, and where practical
  by `run` and `orchestrate`.
- Include path, provider, done criteria, workspace, watch, stop, finish, and
  override lines.
- Keep cards scoped to transition surfaces per the matrix.

Depth tests:

- `start_preview_names_path_provider_done_workspace_and_finish`
- `run_preview_uses_same_done_and_workspace_labels`
- `orchestrate_preview_uses_same_watch_stop_finish_labels`

### P5 - Provider and done-criteria recovery

- Ensure missing provider and missing done-criteria paths end with actionable
  `try:` lines.
- TTY can offer guided creation; non-TTY must not silently mutate config.
- No provider-specific docs examples as the default path.

Depth tests:

- `start_missing_provider_refuses_with_init_and_detect_try_lines`
- `start_detected_unconfigured_provider_suggests_config_provider`
- `start_missing_done_criteria_suggests_def_done_in_non_tty`

### P6 - Source mode recovery

- Make non-git and dirty-repo guidance identical across start/run/orchestrate
  where the underlying behavior is the same.
- Do not silently initialize git or copy source in non-TTY mode.

Depth tests:

- `start_non_git_non_tty_refuses_with_source_mode_try_lines`
- `start_non_git_tty_can_choose_init_git_copy_or_fresh`
- `start_dirty_git_reuses_allow_dirty_guidance`

### P7 - Dispatch integration

- Dispatch `start --mode run` through the existing run path.
- Dispatch review/full-plan modes through the existing orchestrate path.
- `--preview` must not create run, plan, library, or provider-flight state.
- No extra provider call for auto mode.

Depth tests:

- `start_run_preview_creates_no_state`
- `start_review_preview_creates_no_plan_state`
- `start_dispatches_explicit_review_to_orchestrate`
- `start_dispatches_explicit_run_to_run`

### P8 - JSON/plain/quiet contract

- Add structured JSON for start decisions and refusals.
- Ensure plain output has no ANSI.
- Ensure quiet mode follows existing scriptability rules.

Depth tests:

- `start_preview_json_has_next_actions_and_try_lines_without_ansi`
- `start_plain_preview_strips_ansi`
- `start_quiet_success_obeys_existing_quiet_policy`

### P9 - Lifecycle footer and TUI handoff

- After start creates a run or plan, print exact attach/status/kill/finish
  commands.
- Plan ids and run ids remain user-facing; result-run ids stay secondary.
- Attach TUI behavior does not change except any copied footer grammar should
  use existing helpers.

Depth tests:

- `start_run_success_prints_attach_status_kill_finish`
- `start_orchestrate_success_prints_plan_lifecycle_commands`
- `start_footer_uses_plan_id_for_orchestrated_goal`

### P10 - Docs and first-run examples

- Rewrite README first-use flow around `deadreckon start`.
- Keep `run` and `orchestrate` documented as direct commands for users who
  already know which path they want.
- HOWTO should present "new user", "normal single run", and "multi-agent"
  sections in that order.

Depth tests:

- `readme_first_screen_mentions_start_watch_finish`
- `howto_new_user_path_does_not_require_provider_flags`
- `docs_still_document_run_and_orchestrate_directly`

### P11 - Architecture doc, changelog, and deferrals

- Add an AS-BUILT section or subsection for guided first use.
- Update the CLI surface section to include `start`.
- Move any deferred durable profiles, LLM mode classification, personalized
  onboarding, or provider-specific wizards to V1-CANDIDATES.
- Append a CHANGELOG section:

```markdown
## Guided first use (alpha) - 2026-05-26

- Added `deadreckon start` as a guided front door for run vs orchestration.
- Shared launch previews and lifecycle footers across start/run/orchestrate.
- Clarified DeadReckon's audience as the harness around agent CLIs for
  unattended, sandboxed, auditable work.
```

## Focused Verification Menu

Use the narrowest command that covers the phase. Examples:

```bash
cargo test -p deadreckon coherence::top_help_names_audience_and_start_path
cargo test -p deadreckon --test coherence start_
cargo test -p deadreckon --test orchestrate start_
cargo test -p deadreckon --test lifecycle start_
cargo clippy -p deadreckon -- -D warnings
cargo fmt --check
```

If test names land in different files, keep the intent and update the commands.
Run full `make verify` only when the human explicitly asks or when the
implementation changes shared runtime behavior broadly enough that focused
verification is no longer credible.
