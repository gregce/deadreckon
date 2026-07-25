# Friendliness Audit

This is the P1 Effortless evaluation. It is intentionally bounded: the table
scores the canonical top-level command surface against the six-clause
friendliness contract, then later Effortless phases burn down the failing cells.

Legend: `pass` means the current surface satisfies the clause, `fail` means it is
part of the Effortless backlog, and `n-a` means the clause does not apply to that
verb.

> **This table scores each verb alone, and that is its blind spot.** Shakedown
> (AS-BUILT §56) found `status` and `verdict` both scoring **pass** on "Refuse
> with try:" — accurately — while together forming a closed loop: `status`
> refused and named `list`, `list` recommended `status latest`, and that refused
> identically. No per-verb clause can express *the command a refusal names must
> accept the id it was given*, because that is a property of a pair of verbs.
> That sentence now lives in `tests/coherence.rs` as the cross-verb journey
> test, and the structural guards beside it. Read this table as necessary, not
> sufficient: a row can be honestly green while the surface is broken between
> rows.

| Verb | Clause | Status | Note |
|---|---|---|---|
| `init` | Auto-detect, don't ask | pass | Detects subscription CLIs during setup and offers the obvious provider path. |
| `init` | Preview before mutate | pass | Interactive setup confirms before writing config and completions. |
| `init` | Refuse with try: | pass | Setup refusals point at `doctor`, `detect`, or provider configuration. |
| `init` | One-command rollback | n-a | Setup writes user config rather than a workspace change. |
| `init` | One verdict + ONE primary action | pass | `init_completion_surface` renders one verdict and one `Recommended` command; `init_installs_shell_completion_by_default` guards it. |
| `init` | Lifecycle hint | pass | Help and completion install end by pointing to `doctor` or `start`. |
| `config` | Auto-detect, don't ask | pass | Direct get/set/provider commands do not prompt when the target is explicit. |
| `config` | Preview before mutate | pass | Provider/model interactive changes show the chosen route before writing. |
| `config` | Refuse with try: | fail | Some invalid key/value paths still need canonical `try:` coverage. |
| `config` | One-command rollback | pass | Config changes can be corrected with a single `config set` command. |
| `config` | One verdict + ONE primary action | pass | Config set/provider/model surfaces now render one primary command, with missing-key/provider refusals covered by lifecycle tests. |
| `config` | Lifecycle hint | pass | Help points back to `start` after configuration. |
| `help-all` | Auto-detect, don't ask | n-a | Static catalog; there is no environment-dependent choice. |
| `help-all` | Preview before mutate | n-a | Read-only command. |
| `help-all` | Refuse with try: | n-a | The command has no meaningful refusal path. |
| `help-all` | One-command rollback | n-a | Read-only command. |
| `help-all` | One verdict + ONE primary action | pass | The catalog has a single purpose: find the right command. |
| `help-all` | Lifecycle hint | pass | It ends with `deadreckon <command> --help`. |
| `completion` | Auto-detect, don't ask | pass | `completion install` detects the active shell when possible. |
| `completion` | Preview before mutate | pass | Install shows managed file/rc behavior and supports explicit shell output. |
| `completion` | Refuse with try: | pass | Unsupported shell and install errors include recovery commands. |
| `completion` | One-command rollback | pass | Generated files/managed rc blocks can be removed or regenerated in one command. |
| `completion` | One verdict + ONE primary action | pass | `completion_install_surface` has one `Recommended` command; `completion_install_detects_zsh_writes_script_and_managed_rc_block` guards it. |
| `completion` | Lifecycle hint | pass | Help points users back to setup/start. |
| `acceptance` | Auto-detect, don't ask | pass | Compatibility path maps to explicit done-criteria actions. |
| `acceptance` | Preview before mutate | pass | Draft/check/init paths expose criteria before launch use. |
| `acceptance` | Refuse with try: | pass | Criteria errors point to `def-done` recovery. |
| `acceptance` | One-command rollback | pass | Criteria can be overwritten or replaced with `def-done`. |
| `acceptance` | One verdict + ONE primary action | pass | Acceptance/def-done check and write paths route through VerdictSurface with one primary action. |
| `acceptance` | Lifecycle hint | pass | Help points to `def-done` and run/start use. |
| `def-done` | Auto-detect, don't ask | pass | Existing criteria are reused or displayed directly. |
| `def-done` | Preview before mutate | pass | Check/show/draft flows expose the done contract before runs depend on it. |
| `def-done` | Refuse with try: | pass | Invalid criteria and suppression refusals include concrete recovery. |
| `def-done` | One-command rollback | pass | Re-running `def-done` or `def-done add` changes the contract directly. |
| `def-done` | One verdict + ONE primary action | pass | `def-done` write/check success/failure surfaces render one verdict, explanation, and recommended command. |
| `def-done` | Lifecycle hint | pass | Help points to `start` after criteria are defined. |
| `try` | Auto-detect, don't ask | pass | It always uses the local smoke provider and throwaway workspace. |
| `try` | Preview before mutate | n-a | It mutates only isolated demo runstate under `DEADRECKON_HOME`. |
| `try` | Refuse with try: | pass | Any failed smoke proof points at inspection and normal `start`. |
| `try` | One-command rollback | n-a | It does not change the caller checkout. |
| `try` | One verdict + ONE primary action | pass | The proof block leads with a signed gate and one start command. |
| `try` | Lifecycle hint | pass | It ends with `deadreckon start "build the real thing"`. |
| `start` | Auto-detect, don't ask | pass | A single detected subscription provider is adopted inline; multiple detections keep the picker. |
| `start` | Preview before mutate | pass | Guided launch previews and confirms before creating run/plan state. |
| `start` | Refuse with try: | pass | Non-TTY missing provider/done/source refusals include recovery lines. |
| `start` | One-command rollback | pass | Launched worktree runs point at cleanup/abandon paths. |
| `start` | One verdict + ONE primary action | pass | Start preview and lifecycle launch surfaces use VerdictSurface; JSON goldens assert `primary_action` parity. |
| `start` | Lifecycle hint | pass | Successful launches print attach/status/kill/finish commands. |
| `run` | Auto-detect, don't ask | pass | Defaults resolve provider, source mode, and done criteria without prompts when explicit. |
| `run` | Preview before mutate | pass | Worktree/in-place/high-spend paths preview or require confirmation. |
| `run` | Refuse with try: | pass | Common provider/source/safety refusals end with `try:` guidance. |
| `run` | One-command rollback | pass | Worktree runs can be abandoned and in-place runs have undo snapshots. |
| `run` | One verdict + ONE primary action | pass | Exit cards now distinguish one primary next action and demote the rest. |
| `run` | Lifecycle hint | pass | Completion hints point to attach/show/apply/finish/cleanup. |
| `seams` | Auto-detect, don't ask | pass | Validation targets an explicit seam worker kind and config path without prompting. |
| `seams` | Preview before mutate | n-a | Seam validation is read-only and does not mutate workspace or run state. |
| `seams` | Refuse with try: | pass | Missing or invalid seam worker configuration reports a concrete validation/config recovery command. |
| `seams` | One-command rollback | n-a | Seam validation has no durable mutation to roll back. |
| `seams` | One verdict + ONE primary action | pass | Seam validation summaries and JSON now carry one `primary_action`; `seam_validation_*` tests guard it. |
| `seams` | Lifecycle hint | pass | Help and diagnostics point back to `run --no-seams` or focused seam validation. |
| `orchestrate` | Auto-detect, don't ask | pass | Explicit mode/provider flags avoid extra prompts; auto chooses a conservative path. |
| `orchestrate` | Preview before mutate | pass | Preview and final confirmation precede plan creation/forking. |
| `orchestrate` | Refuse with try: | pass | Planner/provider/source refusals include concrete launch commands. |
| `orchestrate` | One-command rollback | pass | Plan state can be killed/cleaned and merged results are applied explicitly. |
| `orchestrate` | One verdict + ONE primary action | pass | Orchestrate/plan lifecycle output uses `plan_verdict_surface`, with preview/setup rows demoted to non-primary guidance. |
| `orchestrate` | Lifecycle hint | pass | Output names attach/merge/finish. |
| `campaign` | Auto-detect, don't ask | pass | Course closed this: the launch planner seeds `--n` (grounded classify with a deterministic ladder floor) when the flag is absent. |
| `campaign` | Preview before mutate | pass | Campaign preview shows sub-goals before launch. |
| `campaign` | Refuse with try: | pass | Depth/count/cross-sub refusals include recovery lines. |
| `campaign` | One-command rollback | pass | Campaigns can be killed and result application is explicit. |
| `campaign` | One verdict + ONE primary action | pass | Campaign preview, rollup, repair, and JSON paths use `campaign_verdict_surface` with one primary command. |
| `campaign` | Lifecycle hint | pass | Campaign output points at attach/show/finish. |
| `plan` | Auto-detect, don't ask | pass | Explicit planning inputs run without extra prompts. |
| `plan` | Preview before mutate | pass | Preview mode writes no plan state. |
| `plan` | Refuse with try: | pass | Planner and graph validation errors include recovery. |
| `plan` | One-command rollback | pass | Draft plan state can be killed/removed before merge. |
| `plan` | One verdict + ONE primary action | pass | Plan refusal/completion/failure output uses `plan_verdict_surface`; attach footers use subordinate `next` hints. |
| `plan` | Lifecycle hint | pass | Help/output point at fork/attach/merge. |
| `fork` | Auto-detect, don't ask | pass | Ready tasks and provider routes are read from the plan. |
| `fork` | Preview before mutate | pass | Fork refuses already-forked or invalid plans before spawning. |
| `fork` | Refuse with try: | pass | Blocked dependency and provider errors include next commands. |
| `fork` | One-command rollback | pass | Live plans can be killed. |
| `fork` | One verdict + ONE primary action | pass | Fork completion and refusal paths render VerdictSurface; `fork_completion_uses_one_verdict_surface` guards it. |
| `fork` | Lifecycle hint | pass | Output points at attach and merge. |
| `merge` | Auto-detect, don't ask | pass | Completed children and repair providers resolve from plan/config. |
| `merge` | Preview before mutate | pass | Conflict repair/refusal happens before promotion. |
| `merge` | Refuse with try: | pass | Conflict refusals point to merge repair/prefer-child options. |
| `merge` | One-command rollback | pass | Promotion results are library entries until explicitly applied/exported. |
| `merge` | One verdict + ONE primary action | pass | Merge completion, conflict, repair, and invalid-option paths render one primary action. |
| `merge` | Lifecycle hint | pass | Output points at finish/apply/export. |
| `chain` | Auto-detect, don't ask | pass | Course closed this: the effortless path auto-detects continuation (verified same-task history resolves to a follow-up run with zero questions); explicit `chain` step lists are operator-authored input, not an ask. |
| `chain` | Preview before mutate | pass | Plan/run/apply modes preview or confirm state changes. |
| `chain` | Refuse with try: | pass | Chain refusals include scoped recovery commands. |
| `chain` | One-command rollback | pass | Undo/redo/reapply are one-command recovery paths. |
| `chain` | One verdict + ONE primary action | pass | Chain run/pause/completion/status surfaces use VerdictSurface; attach/help footers were demoted from `recommended:` to one `next` command. |
| `chain` | Lifecycle hint | pass | Chain output points at attach/resume/finish-style actions. |
| `doctor` | Auto-detect, don't ask | pass | It probes local setup without prompting. |
| `doctor` | Preview before mutate | n-a | Read-only diagnostic. |
| `doctor` | Refuse with try: | pass | Findings include concrete setup/provider commands. |
| `doctor` | One-command rollback | n-a | Read-only diagnostic. |
| `doctor` | One verdict + ONE primary action | pass | Doctor findings now aggregate behind one verdict and primary setup command, with JSON parity covered. |
| `doctor` | Lifecycle hint | pass | Help points to init/start. |
| `detect` | Auto-detect, don't ask | pass | Provider probes are automatic and bounded. |
| `detect` | Preview before mutate | n-a | Read-only diagnostic. |
| `detect` | Refuse with try: | pass | Unknown provider and ping guidance are actionable. |
| `detect` | One-command rollback | n-a | Read-only diagnostic. |
| `detect` | One verdict + ONE primary action | pass | Detection keeps probe rows as evidence and uses `detect_verdict_surface` for the primary next command. |
| `detect` | Lifecycle hint | pass | Help points to config/start. |
| `providers` | Auto-detect, don't ask | pass | Configured/default provider rows resolve automatically. |
| `providers` | Preview before mutate | n-a | Read-only listing. |
| `providers` | Refuse with try: | pass | Unknown route/model paths include provider list guidance. |
| `providers` | One-command rollback | n-a | Read-only listing. |
| `providers` | One verdict + ONE primary action | pass | Provider list/setup/update surfaces expose one `primary_action`; provider listing JSON guards this. |
| `providers` | Lifecycle hint | pass | Help points to start/run with provider flags. |
| `models` | Auto-detect, don't ask | pass | Defaults to the configured provider's catalog; recommended and configured defaults are marked automatically. |
| `models` | Preview before mutate | n-a | Read-only listing. |
| `models` | Refuse with try: | pass | Unknown routes point at `deadreckon providers list --all`. |
| `models` | One-command rollback | n-a | Read-only listing. |
| `models` | One verdict + ONE primary action | pass | Catalog listing renders one surface with one primary action. |
| `models` | Lifecycle hint | pass | Output points at `deadreckon config model <id>` and launch `--model` flags. |
| `update` | Auto-detect, don't ask | pass | Install channel is read from the receipt. |
| `update` | Preview before mutate | pass | Update checks and shell swaps preview before replacing binaries. |
| `update` | Refuse with try: | pass | Unsupported channels and failed swaps print recovery. |
| `update` | One-command rollback | pass | Shell updates write backups and native channels print native rollback paths. |
| `update` | One verdict + ONE primary action | pass | Native, shell, check, source, and failure update paths render one VerdictSurface primary command. |
| `update` | Lifecycle hint | pass | Success points at doctor. |
| `list` | Auto-detect, don't ask | pass | Defaults to current project scope and latest inventory. |
| `list` | Preview before mutate | n-a | Read-only listing. |
| `list` | Refuse with try: | pass | Scope/ref errors point at `--all` or show commands. |
| `list` | One-command rollback | n-a | Read-only listing. |
| `list` | One verdict + ONE primary action | pass | Empty/recovery list output uses VerdictSurface with additive JSON; normal inventory remains read-only rows. |
| `list` | Lifecycle hint | pass | Rows point at show/attach/finish. |
| `library` | Auto-detect, don't ask | pass | Library list/search/show default to scoped artifacts. |
| `library` | Preview before mutate | n-a | Read-only artifact inspection. |
| `library` | Refuse with try: | pass | Missing artifact paths include list/search recovery. |
| `library` | One-command rollback | n-a | Read-only artifact inspection. |
| `library` | One verdict + ONE primary action | pass | Library no-match, invalid date, and empty-list recovery output use one VerdictSurface primary command. |
| `library` | Lifecycle hint | pass | Help points at finish/materialize. |
| `finish` | Auto-detect, don't ask | pass | Chooses apply/export based on run/source mode. |
| `finish` | Preview before mutate | pass | Destructive apply/export paths respect confirmations. |
| `finish` | Refuse with try: | pass | Incomplete/dirty/conflict paths include recovery commands. |
| `finish` | One-command rollback | pass | Apply/export paths expose cleanup or git recovery. |
| `finish` | One verdict + ONE primary action | pass | Finish output now leads in-place completions with one primary action and secondary actions. |
| `finish` | Lifecycle hint | pass | Output points at the chosen keep action. |
| `materialize` | Auto-detect, don't ask | pass | Completed artifact mode is resolved from the run/plan. |
| `materialize` | Preview before mutate | pass | Existing destination and overwrite behavior is explicit. |
| `materialize` | Refuse with try: | pass | Destination and state refusals include export/finish recovery. |
| `materialize` | One-command rollback | pass | Exported directories can be removed or overwritten explicitly. |
| `materialize` | One verdict + ONE primary action | pass | Materialize/export completion uses `materialized_surface` with one recommended inspection command. |
| `materialize` | Lifecycle hint | pass | Help points to show/extend. |
| `apply` | Auto-detect, don't ask | pass | Run/plan mode chooses the git apply strategy path. |
| `apply` | Preview before mutate | pass | Dirty/conflict/autostash paths preview or refuse before merge. |
| `apply` | Refuse with try: | pass | Conflict and dirty-tree refusals include recovery lines. |
| `apply` | One-command rollback | pass | Git merge/squash/cherry-pick recovery is one command from the branch. |
| `apply` | One verdict + ONE primary action | pass | Apply completion and dirty/conflict refusals route through VerdictSurface with one primary recovery or inspection command. |
| `apply` | Lifecycle hint | pass | Hints point at cleanup or git log. |
| `abandon` | Auto-detect, don't ask | pass | Targets latest/current-scope runs when unambiguous. |
| `abandon` | Preview before mutate | pass | Refuses in-place and requires force for live cleanup. |
| `abandon` | Refuse with try: | pass | Unsafe abandon paths point at cleanup/undo. |
| `abandon` | One-command rollback | pass | Worktree branch/path removal is explicit; keep-branch is available. |
| `abandon` | One verdict + ONE primary action | pass | Abandon cleanup reuses `cleanup_result_surface`; `abandon_surface_recommends_inspection_after_removing_worktree` guards the primary action. |
| `abandon` | Lifecycle hint | pass | Help points to cleanup. |
| `cleanup` | Auto-detect, don't ask | pass | Defaults to current-scope cleanup candidates. |
| `cleanup` | Preview before mutate | pass | Candidate cleanup requires explicit target/confirmation. |
| `cleanup` | Refuse with try: | pass | No-candidate and unsafe paths include cleanup alternatives. |
| `cleanup` | One-command rollback | pass | Completed work remains in library unless explicitly removed. |
| `cleanup` | One verdict + ONE primary action | pass | Cleanup aggregate, single-run, no-candidate, and refusal surfaces now use one safe primary action. |
| `cleanup` | Lifecycle hint | pass | Help points at stale/completed cleanup modes. |
| `extend` | Auto-detect, don't ask | pass | `latest` and current-scope history resolve automatically. |
| `extend` | Preview before mutate | pass | Follow-up run creation uses normal launch preview. |
| `extend` | Refuse with try: | pass | Incomplete/missing parent paths include start/run alternatives. |
| `extend` | One-command rollback | pass | Follow-up worktree runs can be abandoned. |
| `extend` | One verdict + ONE primary action | pass | Extend refusal surfaces and follow-up launch lifecycle output use VerdictSurface primary-action selection. |
| `extend` | Lifecycle hint | pass | Help points at attach/finish. |
| `doc` | Auto-detect, don't ask | pass | Defaults to latest run narrative and deterministic docs. |
| `doc` | Preview before mutate | pass | Provider polish prints a preview/confirmation before regeneration. |
| `doc` | Refuse with try: | pass | Missing provider/doc paths include polish/config recovery. |
| `doc` | One-command rollback | n-a | Read paths are immutable; polish can be re-run. |
| `doc` | One verdict + ONE primary action | pass | Doc export, missing-provider, polish budget, and polish completion surfaces render one primary action. |
| `doc` | Lifecycle hint | pass | Help points at export/polish/finish paths. |
| `report` | Auto-detect, don't ask | pass | The run target is explicit and the report derives from the shared RunView without prompting. |
| `report` | Preview before mutate | n-a | It writes an additive report artifact or emits JSON; it does not alter run state or the workspace. |
| `report` | Refuse with try: | pass | Live runs refuse with an attach command, and invalid ids flow through normal run lookup guidance. |
| `report` | One-command rollback | n-a | Report files are additive inspection artifacts and can be regenerated. |
| `report` | One verdict + ONE primary action | pass | Successful report writes render one VerdictSurface with one recommended inspect command. |
| `report` | Lifecycle hint | pass | Output points at `show` and JSON report inspection. |
| `attach` | Auto-detect, don't ask | pass | `latest`, run/plan/chain kind, and child refs resolve automatically. |
| `attach` | Preview before mutate | n-a | Attach is observational unless the user presses an explicit action key. |
| `attach` | Refuse with try: | pass | Unsupported narrative/target paths include recovery. |
| `attach` | One-command rollback | n-a | Read-only watch surface. |
| `attach` | One verdict + ONE primary action | pass | Attach post-action notices and paused/plan TUI footers now derive one verdict or one subordinate `next` command. |
| `attach` | Lifecycle hint | pass | Footer points at finish/show/apply or detach. |
| `steer` | Auto-detect, don't ask | pass | Run prefixes and `latest` resolve through the shared run loader; the recorded route decides whether steering is supported. |
| `steer` | Preview before mutate | n-a | The explicit instruction is the append operation; no workspace files are changed. |
| `steer` | Refuse with try: | pass | Empty text, dead runs, and exec routes point at a concrete steer, extend, or provider-config command. |
| `steer` | One-command rollback | n-a | The durable append-only inbox is an audit ledger; delivered instructions are corrected with later rows rather than deleted. |
| `steer` | One verdict + ONE primary action | pass | Success reports one queued outcome; every refusal reports one recovery command. |
| `steer` | Lifecycle hint | pass | Success points at `deadreckon attach <run-id>` and help covers steer, attach, and kill. |
| `kill` | Auto-detect, don't ask | pass | Run/plan/chain/campaign ids resolve by prefix/latest. |
| `kill` | Preview before mutate | pass | Confirms destructive stop unless explicitly scripted. |
| `kill` | Refuse with try: | pass | Missing/not-running targets include status/list recovery. |
| `kill` | One-command rollback | pass | Killed runs can be resumed when state permits. |
| `kill` | One verdict + ONE primary action | pass | Run/plan/chain/campaign kill paths render one killed verdict and one inspection/recovery command. |
| `kill` | Lifecycle hint | pass | Output points at resume/cleanup/status. |
| `reshape` | Auto-detect, don't ask | pass | `latest` resolves; the proposal file is found without prompts. |
| `reshape` | Preview before mutate | pass | The course card previews the proposal before any dispatch. |
| `reshape` | Refuse with try: | pass | Missing proposal, still-running run, and non-TTY acceptance refuse with recovery lines. |
| `reshape` | One-command rollback | pass | The dispatched plan is killable and its children undoable like any orchestration. |
| `reshape` | One verdict + ONE primary action | pass | Acceptance delegates to the orchestrate completion surface (one recommended command). |
| `reshape` | Lifecycle hint | pass | Refusals and the worker's history hint name `deadreckon reshape <id>`. |
| `resume` | Auto-detect, don't ask | pass | `latest` and partial traces resolve automatically. |
| `resume` | Preview before mutate | pass | Resume shows provider/source context before entering the loop. |
| `resume` | Refuse with try: | pass | Completed/missing run paths include extend/start guidance. |
| `resume` | One-command rollback | pass | Resumed worktree runs remain abandonable/undoable. |
| `resume` | One verdict + ONE primary action | pass | Completed-run resume no-op and resumed run lifecycle output render one primary command. |
| `resume` | Lifecycle hint | pass | Help points at attach. |
| `undo` | Auto-detect, don't ask | pass | Snapshot targets resolve from run metadata. |
| `undo` | Preview before mutate | pass | Preview/target selection prevents accidental snapshot restoration. |
| `undo` | Refuse with try: | pass | Missing snapshot paths include show/rewind guidance. |
| `undo` | One-command rollback | pass | A later snapshot or rerun can restore forward state. |
| `undo` | One verdict + ONE primary action | pass | Undo and chain undo/redo paths use VerdictSurface and tests assert no competing primary actions. |
| `undo` | Lifecycle hint | pass | Help points at show. |
| `rewind` | Auto-detect, don't ask | pass | Flight checkpoints resolve by event target. |
| `rewind` | Preview before mutate | pass | Hash-guarded preview is the default before apply. |
| `rewind` | Refuse with try: | pass | Missing/unrelated checkpoint paths include show flight guidance. |
| `rewind` | One-command rollback | pass | Rewind is preview-first and applied changes are explicit. |
| `rewind` | One verdict + ONE primary action | pass | Rewind preview/apply surfaces emit one VerdictSurface primary command with JSON parity. |
| `rewind` | Lifecycle hint | pass | Help points at show/flight. |
| `show` | Auto-detect, don't ask | pass | `latest`, prefixes, plans, and child refs resolve automatically. |
| `show` | Preview before mutate | n-a | Read-only inspection. |
| `show` | Refuse with try: | pass | Missing/ambiguous ids include list/status recovery. |
| `show` | One-command rollback | n-a | Read-only inspection. |
| `show` | One verdict + ONE primary action | pass | `show --why-failed` and flight recovery paths use VerdictSurface; ordinary show remains read-only inspection. |
| `show` | Lifecycle hint | pass | Output includes attach/finish/doc hints. |
| `history` | Auto-detect, don't ask | pass | Defaults to current scope and evidence kind. |
| `history` | Preview before mutate | n-a | Read-only search. |
| `history` | Refuse with try: | pass | Invalid regex/scope paths include corrected commands. |
| `history` | One-command rollback | n-a | Read-only search. |
| `history` | One verdict + ONE primary action | pass | Invalid history inputs and no-match grep output use one VerdictSurface recovery command. |
| `history` | Lifecycle hint | pass | Help points at show/import. |
| `status` | Auto-detect, don't ask | pass | Defaults to latest/current-scope run or plan. |
| `status` | Preview before mutate | n-a | Read-only orientation. |
| `status` | Refuse with try: | pass | Resolves every kind (Shakedown §56); missing state points at `start` when the machine is empty and `list --all` when other projects have work — never back at `list` for an id `list` printed. |
| `status` | One-command rollback | n-a | Read-only orientation. |
| `status` | One verdict + ONE primary action | pass | Status now prints one primary action before secondary lifecycle actions. |
| `status` | Lifecycle hint | pass | Status includes natural next commands. |
| `import` | Auto-detect, don't ask | pass | Descriptor sessions auto-discover by cwd/source when unambiguous. |
| `import` | Preview before mutate | pass | Preview/list modes show candidates before creating a run. |
| `import` | Refuse with try: | pass | Ambiguous/stale/unknown imports include exact retry commands. |
| `import` | One-command rollback | pass | Imported runs can be abandoned or replaced explicitly. |
| `import` | One verdict + ONE primary action | pass | Import list, ambiguous/stale/unknown-source, selection, and completion surfaces carry one primary action plus additive JSON. |
| `import` | Lifecycle hint | pass | Help points at show. |
| `verdict` | Auto-detect, don't ask | pass | Defaults to the latest run; no prompt when the run is implied or named. |
| `verdict` | Preview before mutate | n-a | Read-only report; it never mutates run state. |
| `verdict` | Refuse with try: | pass | A plan or chain id refuses by kind and names `show` (Shakedown §56); `list` remains the retry only for genuine typos and ambiguous prefixes, which are references `list` did not hand over. |
| `verdict` | One-command rollback | n-a | Read-only; there is nothing to roll back. |
| `verdict` | One verdict + ONE primary action | pass | Renders through VerdictSurface with one state and one primary next action. |
| `verdict` | Lifecycle hint | pass | Points at finish or resume depending on the verdict. |
| `learn` | Auto-detect, don't ask | pass | Index/report/propose use existing run evidence by default. |
| `learn` | Preview before mutate | pass | Import/export/propose candidate paths preview before durable writes. |
| `learn` | Refuse with try: | pass | Weak evidence and bundle errors include recovery. |
| `learn` | One-command rollback | pass | Proposals/candidates are file-backed and can be replaced. |
| `learn` | One verdict + ONE primary action | pass | Learn index/report/export/import/propose surfaces render one primary command; propose success has focused coverage. |
| `learn` | Lifecycle hint | pass | Help points at improve/report. |
| `improve` | Auto-detect, don't ask | pass | Proposal ids and goal files resolve directly. |
| `improve` | Preview before mutate | pass | Self-improve is preview-first and uses isolated worktrees. |
| `improve` | Refuse with try: | pass | Risk/evidence refusals include concrete recovery. |
| `improve` | One-command rollback | pass | Candidate worktrees can be abandoned and PR opening is opt-in. |
| `improve` | One verdict + ONE primary action | pass | Improve preview/candidate/PR/missing-candidate surfaces use VerdictSurface with one primary command. |
| `improve` | Lifecycle hint | pass | Help points at learn/report/PR dry-run. |
