# DeadReckon - Verdict Surface Rider (one verdict, one command, one explanation)

This rider holds the prescriptive constraints for the goal at `/Users/gdc/deadreckon/docs/goals/2026-06-01-1417-deadreckon-verdict-surface-goal.md`. It supersedes nothing in prior riders. The invariants from the production command model, navigable, effortless, campaign, plan-doc-consolidation, composable-seams, and seam-conformance-kit riders still apply. This rider adds one narrow production-release UX slice: all terminal failure and completion surfaces converge on the same outcome grammar.

**All paths absolute.** Source root is `/Users/gdc/deadreckon`. Runtime state is normally under `/Users/gdc/.deadreckon`. Test fixtures must stay inside the repo or tempdirs owned by tests.

## Posture (decided - do not redesign)

- **Maturity stays production-release track for 0.1.0 readiness.** The goal is a cohesive UX contract, not a new orchestration architecture.
- **No durable state schema changes.** Do not add required fields to existing run, plan, chain, campaign, provider, acceptance, or library state files.
- **JSON changes are additive only.** Existing consumers must keep working.
- **No command removals, broad renames, or verb migrations.** The next command may be clearer, but the command set is stable.
- **No full V1 output-layout facade.** `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` already reserves the larger facade, template engine, localization, theming, and command-matrix goldens. This rider may introduce a small internal helper that can later be absorbed by that facade.
- **No rich merge UI or AST/semantic merge engine.** Merge repair behavior can be explained and recommended, but not redesigned here.
- **No `git push`.** Local commits only when the phase is green.
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Existing pieces to reuse

- `/Users/gdc/deadreckon/crates/deadreckon/src/ui_card.rs`
  - `Card`, `CardSection`, `HintLine`, `TitleGlyph`, and `render_card`.
  - Prefer extending or wrapping this over inventing a second visual card grammar.
- `/Users/gdc/deadreckon/crates/deadreckon/src/cards/exit_summary.rs`
  - `ExitSummaryInput`, `OutcomeKind`, `build_exit_summary_card`, and `exit_summary_primary_action`.
  - This is the closest shipped precedent for a verdict surface.
- `/Users/gdc/deadreckon/crates/deadreckon/src/friendliness_contract.rs`
  - The clause `OneVerdictOnePrimaryAction` is the explicit contract to burn down.
- `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs`
  - Existing helpers include `print_kv_block`, `try_footer`, `render_why_failed`, `show_*_why_failed`, `print_plan_summary`, `print_run_summary`, `next_action_label`, `print_action_block`, and `lifecycle_actions`.
- `/Users/gdc/deadreckon/crates/deadreckon/src/commands/plan.rs`
  - Existing plan next-action logic and dependency summaries.
- `/Users/gdc/deadreckon/crates/deadreckon/src/commands/start.rs`
  - Start preview and guided-start recovery/history rendering.
- `/Users/gdc/deadreckon/crates/deadreckon/src/commands/campaign.rs`
  - Campaign failure, repair, and apply surfaces.
- `/Users/gdc/deadreckon/crates/deadreckon/src/commands/chain/mod.rs`
  - Chain status, failure, and next-action surfaces.

## Data model (files, not fields)

No new durable file is expected. This should be an internal Rust rendering contract, probably a small module such as `/Users/gdc/deadreckon/crates/deadreckon/src/verdict_surface.rs` or a focused extension of `/Users/gdc/deadreckon/crates/deadreckon/src/ui_card.rs`.

Suggested internal shape, adjust to match the codebase:

```rust
pub(crate) struct VerdictSurface {
    pub(crate) subject: VerdictSubject,
    pub(crate) kind: VerdictKind,
    pub(crate) verdict: String,
    pub(crate) recommended: HintLine,
    pub(crate) explanation: ExplanationPanel,
    pub(crate) facts: Vec<(String, String)>,
    pub(crate) secondary_actions: Vec<HintLine>,
}

pub(crate) enum VerdictKind {
    Completed,
    Verified,
    Failed,
    Blocked,
    Killed,
    Paused,
    Preview,
    NeedsInput,
    Noop,
}

pub(crate) struct ExplanationPanel {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) evidence: Vec<(String, String)>,
}
```

The exact names can differ. The required invariant cannot: one `recommended` action and one `explanation` block per failure/completion surface.

## Output contract

### Human output

Every in-scope terminal surface should read in this order:

1. **Verdict.** A short label in the title or first line, for example `failed plan 4b4fdc93`, `completed run d0179589`, `blocked campaign 16fd75c0`, or `preview start`.
2. **Explanation panel.** One bounded panel headed consistently, for example `Explanation`, with:
   - what happened;
   - why the verdict was chosen;
   - evidence: state path, run id, plan id, failure reason, gate result, repairability, or provider event.
3. **Recommended command.** Exactly one primary command, using existing command syntax.
4. **Secondary actions.** Optional and subordinate. They must never be visually confused with the primary command.

Do not print a scattered `try:` footer plus another "next action" card plus an inline recovery line for the same outcome. Convert the scattered material into one surface.

### JSON output

Keep existing JSON fields. Add a stable, additive shape where command output already describes a terminal or blocking outcome:

```json
{
  "verdict": {
    "kind": "failed",
    "label": "failed plan",
    "subject": "4b4fdc93",
    "recommended_command": "deadreckon show 4b4fdc93 --why-failed",
    "explanation": "task-3 could not launch because dependency inputs conflicted at index.html",
    "evidence": [
      ["plan", "4b4fdc93"],
      ["reason", "dependency source conflict at index.html"]
    ]
  },
  "primary_action": "deadreckon show 4b4fdc93 --why-failed"
}
```

If a command already has `next_actions`, leave it intact and ensure its first or marked action agrees with `primary_action`.

### Flag behavior

- `--quiet`: no noisy success output. Failure and refusal output may still include the verdict and recommended command.
- `--json`: structured verdict only, no human card.
- `--plain`: no decorative borders, but the same verdict, explanation, and recommended command.
- `--no-hints`: suppress secondary hints. Do not suppress the one recommended command when the command is failing, blocked, or needs user action.
- Non-TTY output must remain parseable and should not depend on terminal width.

## Recommended command priority

Use deterministic selection. Do not ask the provider to choose the command.

### Run outcomes

- Completed run with applicable changes: prefer the lifecycle command already chosen by `exit_summary_primary_action` or `lifecycle_actions`, commonly `deadreckon finish <run>` or `deadreckon apply <run>`.
- Failed run: `deadreckon show <run> --why-failed`.
- Paused run: `deadreckon resume <run>`.
- Killed run: `deadreckon show <run> --why-failed` if a failure reason exists, otherwise `deadreckon resume <run>` only when resumable.
- No-op or already-finished run: `deadreckon show <run>`.

### Plan and orchestration outcomes

- Completed plan with result: the existing finish/apply command selected by lifecycle logic.
- Failed plan with repairable merge/input conflict: `deadreckon merge <plan>` or the existing repair command if command names differ at HEAD.
- Failed plan without repair path: `deadreckon show <plan> --why-failed`.
- Running plan: `deadreckon attach <plan>`.
- Start preview: `deadreckon attach <after-start>` must not appear as a real command until the id exists. The preview should say which command will be available after launch.

### Campaign outcomes

- Completed or merged campaign: use the existing apply/finish/show command selected by campaign lifecycle logic.
- Cross-sub conflict or campaign-level failure with repair support: `deadreckon campaign repair <campaign>`.
- Campaign failure without repair support: `deadreckon show <campaign> --why-failed` or the current canonical inspection command.
- Running campaign: `deadreckon attach <campaign>`.

### Chain outcomes

- Failed chain: prefer the chain-specific why-failed command if it exists; otherwise use the canonical `deadreckon chain show <chain>` or `deadreckon show <chain> --why-failed` at HEAD.
- Paused chain: resume command.
- Completed chain: finish/apply/show according to lifecycle state.

### Recovery and control verbs

`kill`, `resume`, `undo`, `rewind`, `abandon`, and `cleanup` should each describe the state transition they just caused, then recommend the one safest next inspection or continuation command. A successful destructive or irreversible action should usually recommend `deadreckon show <id>` or `deadreckon status`, not another destructive command.

## Explanation panel rules

The panel must be factual, not a second summary. It should answer three questions:

- **What happened?** Example: `task-3 could not start`.
- **Why this verdict?** Example: `dependency inputs from task-1 and task-2 both wrote index.html with different content and no dependency ordering`.
- **What evidence supports it?** Example: `event #42`, `state path`, `run id`, `plan id`, `conflict path`, `gate proof`, or `provider exit code`.

Use existing sources before inventing new ones:

- failure reason stored in run/plan/campaign/chain state;
- recent event log;
- acceptance gate output;
- conflict paths from merge/compose code;
- provider process status;
- output directory and artifact path;
- the existing `render_why_failed` family.

If no detailed reason is available, say that explicitly in the explanation and recommend the best inspection command. Do not leave the panel blank.

## Golden-output examples

These examples define the intended stable shape for focused golden tests. The renderer may use borders, color, or card glyphs in TTY mode, but tests should normalize decoration and preserve this order: verdict, explanation, recommended command, secondary actions. Placeholder ids are deliberate and should be replaced by fixture ids in tests.

### Failed plan dependency conflict

```text
failed plan 4b4fdc93

Explanation
task-3 could not start because its dependency inputs conflict.
task-1 and task-2 both produced index.html with different content, and neither task depends on the other, so DeadReckon refused to choose a winner.

Evidence
plan: 4b4fdc93
task: task-3
conflict: index.html
reason: dependency source conflict

Recommended
deadreckon merge 4b4fdc93

Secondary
deadreckon show 4b4fdc93 --why-failed
deadreckon kill 4b4fdc93
```

Assertions:

- exactly one line is classified as the primary recommended command;
- `deadreckon merge 4b4fdc93` appears once in the primary slot;
- the explanation names `task-3`, both sibling tasks, and `index.html`;
- secondary commands are not rendered with primary styling.

### Campaign cross-sub conflict

```text
blocked campaign 16fd75c0

Explanation
The campaign completed its child orchestrations, but the final assembly stopped at a cross-sub file conflict.
This is a deterministic merge refusal, not a provider crash. A repair pass can inspect both sub-results and produce a consolidated artifact.

Evidence
campaign: 16fd75c0
result run: d0179589
reason: cross-sub file conflict
artifact library: /Users/gdc/.deadreckon/library/sub-3-929c19a1/d01795896e854713a51211cb7491f716

Recommended
deadreckon campaign repair 16fd75c0

Secondary
deadreckon show 16fd75c0 --why-failed
deadreckon apply 16fd75c0
```

Assertions:

- the verdict is `blocked`, not `failed`, when repair is available;
- the explanation distinguishes deterministic refusal from provider failure;
- repair is the only primary action;
- `apply` may only appear as secondary until repair or assembly succeeds.

### Completed run with applicable changes

```text
completed run d0179589

Explanation
The provider finished and acceptance checks passed. The run produced changes that are ready to land in the current project workspace.

Evidence
run: d0179589
checks: passed
workspace: /Users/gdc/.deadreckon/runstate/runs/d0179589/work
changed files: 7

Recommended
deadreckon finish d0179589

Secondary
deadreckon show d0179589
deadreckon export d0179589
```

Assertions:

- completion output does not also render a competing `try:` footer;
- the primary action matches existing lifecycle resolution for the fixture;
- changed-file details live under evidence or facts, not as a second verdict.

### Paused run

```text
paused run 06446626

Explanation
The run is still resumable. DeadReckon stopped before the provider completed, so no final acceptance verdict exists yet.

Evidence
run: 06446626
phase: execute
provider: cli:claude-code
state: paused

Recommended
deadreckon resume 06446626

Secondary
deadreckon show 06446626
deadreckon kill 06446626
```

Assertions:

- paused output recommends resume exactly once;
- it does not call the run completed or failed;
- the explanation says acceptance has not produced a final verdict.

### Start preview before id allocation

```text
preview start

Explanation
DeadReckon has classified the goal and is ready to launch a campaign orchestration. No run id or plan id exists until you confirm the launch.

Evidence
path: campaign orchestration
provider: cli:codex
roles: planner=cli:codex, child=cli:claude-code
done: create from goal before launch

Recommended
confirm this launch

After start
deadreckon attach <id>
deadreckon kill <id>
deadreckon finish <id>
```

Assertions:

- preview output does not render `deadreckon attach <after-start>` as an executable primary command;
- the primary recommendation is the local confirmation action;
- post-launch commands are grouped under `After start`, not primary recovery.

### JSON failed plan shape

```json
{
  "plan_id": "4b4fdc93",
  "status": "failed",
  "next_actions": [
    "deadreckon merge 4b4fdc93",
    "deadreckon show 4b4fdc93 --why-failed"
  ],
  "primary_action": "deadreckon merge 4b4fdc93",
  "verdict": {
    "kind": "failed",
    "label": "failed plan",
    "subject": "4b4fdc93",
    "recommended_command": "deadreckon merge 4b4fdc93",
    "explanation": "task-3 could not start because dependency inputs conflict at index.html",
    "evidence": [
      ["task", "task-3"],
      ["conflict", "index.html"],
      ["reason", "dependency source conflict"]
    ]
  }
}
```

Assertions:

- existing fields such as `plan_id`, `status`, and `next_actions` remain;
- `primary_action` equals `verdict.recommended_command`;
- the first `next_actions` entry agrees with `primary_action` when `next_actions` is ordered.

## In-scope command matrix

The following surfaces must be inspected and either normalized or explicitly shown to already satisfy the contract:

- `deadreckon run`
- `deadreckon start`
- `deadreckon orchestrate`
- `deadreckon plan`
- `deadreckon fork`
- `deadreckon merge`
- `deadreckon campaign`
- `deadreckon campaign repair`
- `deadreckon chain`
- `deadreckon finish`
- `deadreckon apply`
- `deadreckon export`
- `deadreckon materialize`
- `deadreckon kill`
- `deadreckon resume`
- `deadreckon undo`
- `deadreckon rewind`
- `deadreckon abandon`
- `deadreckon cleanup`
- `deadreckon show --why-failed`
- `deadreckon status`
- setup and diagnostic commands from `/Users/gdc/deadreckon/docs/FRIENDLINESS-AUDIT.md` that print terminal recovery or completion surfaces: `init`, `config`, `completion`, `acceptance`, `def-done`, `doctor`, `detect`, `providers`, `update`, `list`, `library`, `history`, `import`, `learn`, `improve`, and `doc`.

Read-only commands are not required to become cards. They are required to avoid multiple competing recommended commands when they print a terminal verdict or recovery hint.

## Phases (eleven)

Each implementation phase must start with failing depth tests. Keep tests narrow enough to run repeatedly. After implementation, run the focused tests for the phase, then `cargo fmt --check` and `git diff --check`. Commit locally at sensible phase boundaries using the repo's conventional style.

### P1 - Inventory and contract lock

- Build a source inventory of every terminal failure/completion surface in the in-scope command matrix.
- Update or add a test fixture that encodes the high-level contract: one verdict, one primary action, one explanation panel.
- The test should fail against at least one known current offender from `/Users/gdc/deadreckon/docs/FRIENDLINESS-AUDIT.md`.
- Do not fix rendering yet except for test harness plumbing.

Depth tests:

- `friendliness_audit_tracks_terminal_outcome_surfaces`
- `verdict_surface_contract_rejects_multiple_primary_actions`
- `verdict_surface_contract_requires_explanation_panel`

### P2 - Shared verdict primitive and renderer adapter

- Add the smallest shared primitive needed to represent the contract.
- Prefer adapting `Card`/`HintLine` rather than replacing them.
- Provide render paths for card, plain text, and JSON.
- Ensure the renderer has a single source of truth for the primary command.
- Add helpers that make it difficult for callers to render multiple primary actions.

Depth tests:

- `verdict_surface_renders_one_recommended_command`
- `verdict_surface_plain_output_contains_same_verdict_and_command`
- `verdict_surface_json_is_additive_and_matches_primary_action`
- `verdict_surface_no_hints_suppresses_secondary_not_primary`

### P3 - Run and direct lifecycle normalization

- Wire the shared primitive through run completion, run failure, paused/killed run output, and existing exit summary cards.
- Keep the current lifecycle command choice unless it violates the one-primary-action rule.
- Normalize `finish`, `apply`, `export`, and `materialize` success/failure surfaces.
- Preserve existing artifact paths and summaries, but place them inside facts or explanation rather than scattered footers.

Depth tests:

- `run_completed_surface_has_one_verdict_one_primary_action`
- `run_failed_surface_recommends_show_why_failed_once`
- `run_paused_surface_recommends_resume_once`
- `finish_apply_export_materialize_surfaces_share_verdict_contract`

### P4 - Plan, orchestrate, fork, and merge

- Normalize plan/orchestrate completion, failure, conflict, and preview surfaces.
- Make merge repair recommendations decisive when the plan has a repair provider or repairable conflict.
- For non-repairable plan failures, recommend the canonical why-failed inspection command.
- Ensure fork/input-composition conflicts explain whether the failure happened before a child launched or during final merge.

Depth tests:

- `plan_failed_surface_recommends_why_failed_or_merge_once`
- `plan_dependency_conflict_explanation_names_conflict_path`
- `plan_completed_surface_uses_lifecycle_primary_action`
- `merge_conflict_surface_recommends_repair_command_once`

### P5 - Campaign and campaign repair

- Normalize campaign start, completion, failed cross-sub merge, repair preview, repair success, and repair failure surfaces.
- If a campaign can be repaired, the primary action must be `deadreckon campaign repair <id>` or the exact command name at HEAD.
- If a campaign cannot be repaired, explain why and recommend inspection.
- The explanation panel for cross-sub conflicts must distinguish deterministic refusal from provider failure.

Depth tests:

- `campaign_cross_sub_conflict_recommends_campaign_repair_once`
- `campaign_repair_failure_explains_repair_attempt`
- `campaign_completed_surface_recommends_apply_or_finish_once`
- `campaign_json_primary_action_matches_human_primary_action`

### P6 - Chain surfaces

- Normalize chain status, failure, pause, resume, and completion surfaces.
- Respect chain-specific terminology if it exists, but make the primary action unambiguous.
- Avoid printing both a chain next-action list and a general lifecycle footer as competing primaries.

Depth tests:

- `chain_failed_surface_has_single_inspection_or_recovery_command`
- `chain_paused_surface_recommends_resume_once`
- `chain_completed_surface_has_one_verdict_and_explanation`

### P7 - Recovery, control, and rollback verbs

- Normalize `kill`, `resume`, `undo`, `rewind`, `abandon`, and `cleanup`.
- These commands often mutate state. Their explanation panel must say what changed and what remains recoverable.
- Successful irreversible actions should recommend inspection, not another irreversible action.
- Refusals must include one recommended safe command.

Depth tests:

- `kill_surface_explains_state_transition_and_next_inspection`
- `undo_rewind_surfaces_do_not_offer_multiple_primary_actions`
- `cleanup_refusal_surface_has_one_safe_recovery_command`

### P8 - Start previews, setup, and diagnostics

- Normalize start previews without pretending `<after-start>` is a real id.
- Fix setup/diagnostic commands that currently fail the friendliness clause when they print terminal or recovery output: `init`, `config`, `completion`, `acceptance`, `def-done`, `doctor`, `detect`, `providers`, `update`, `list`, `library`, `history`, `import`, `learn`, `improve`, and `doc`.
- Do not decorate simple read-only listings unnecessarily. The contract applies when the command is presenting a completion, failure, refusal, or recovery path.

Depth tests:

- `start_preview_surface_has_one_future_watch_command`
- `setup_refusal_surfaces_use_one_try_command`
- `diagnostic_completion_surfaces_do_not_compete_with_lifecycle_hints`

### P9 - Mode parity and non-TTY behavior

- Audit `--json`, `--plain`, `--quiet`, `--no-hints`, and non-TTY output for the changed commands.
- Ensure `--json` receives additive verdict fields and no human card text.
- Ensure `--plain` preserves all information without borders.
- Ensure `--quiet` remains quiet on successful non-action output.
- Ensure terminal width cannot split the primary command into confusing fragments.

Depth tests:

- `json_verdict_shape_is_additive_for_terminal_outcomes`
- `plain_verdict_surface_preserves_primary_action`
- `quiet_success_does_not_gain_noisy_card`
- `non_tty_primary_command_is_parseable`

### P10 - Friendliness audit burn-down and focused golden smoke

- Update `/Users/gdc/deadreckon/docs/FRIENDLINESS-AUDIT.md`.
- Burn down `One verdict + ONE primary action` failures for every in-scope terminal surface.
- Any remaining failures must be explicitly reclassified as not a terminal failure/completion surface, or logged to V1 with a concrete reason.
- Add focused golden/snapshot tests for representative command families. Do not attempt the full V1 command-matrix golden suite.

Depth tests:

- `friendliness_one_verdict_primary_action_burndown_has_no_in_scope_failures`
- `representative_command_goldens_lock_verdict_panel_order`
- `remaining_audit_failures_are_out_of_scope_or_v1_logged`

### P11 - Docs, architecture, and changelog

- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` with a concise section describing Verdict Surface:
  - problem solved;
  - invariant;
  - renderer/helper location;
  - what remains V1.
- Update `/Users/gdc/deadreckon/docs/FRIENDLINESS-AUDIT.md` with before/after status.
- Update `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` only if implementation discovers larger output-layout work that remains intentionally out of scope.
- Append a CHANGELOG entry for the user-visible CLI behavior.
- Run final verification and make a local commit.

No depth test required for P11, but docs must cite the test names or command families that enforce the behavior.

## Refusal and edge-case rules

- If a command cannot determine a recommended command, this is a bug in the new helper. The fallback should be `deadreckon show <id>` or `deadreckon status`, never an empty recommendation.
- If there are multiple plausible repair commands, choose the safest non-destructive one. Put destructive or state-changing alternatives in secondary actions.
- If a command result spans multiple subjects, choose the aggregate subject for the verdict and put per-subject details in facts.
- If a command partially succeeds, use `Blocked` or `Failed` only when user action is required before the workflow can continue. Explain the partial success in the panel.
- If an output is purely informational and has no completion/failure/refusal semantics, leave it alone unless it currently prints competing next actions.

## Verification commands

Run these before the final commit:

```sh
cargo test -p deadreckon verdict_surface
cargo test -p deadreckon friendliness
cargo test -p deadreckon
cargo fmt --check
git diff --check
```

If `cargo test -p deadreckon` is too slow during early phases, run focused tests per phase and reserve the full package run for final verification.

## Stop conditions

Stop only when all are true:

- Every in-scope failure/completion surface has one verdict, one recommended command, and one explanation panel.
- JSON output is backward-compatible and additive.
- `--plain`, `--quiet`, `--json`, `--no-hints`, and non-TTY behavior are checked for representative surfaces.
- FRIENDLINESS-AUDIT no longer lists in-scope failures for the one-primary-action clause.
- AS-BUILT and CHANGELOG are updated.
- Final verification is green or any skipped command is documented with the exact reason.
- Work is committed locally and not pushed.
