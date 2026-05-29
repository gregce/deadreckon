# Friendliness Audit

This is the P1 Effortless evaluation. It is intentionally bounded: the table
scores the canonical top-level command surface against the six-clause
friendliness contract, then later Effortless phases burn down the failing cells.

Legend: `pass` means the current surface satisfies the clause, `fail` means it is
part of the Effortless backlog, and `n-a` means the clause does not apply to that
verb.

| Verb | Clause | Status | Note |
|---|---|---|---|
| `init` | Auto-detect, don't ask | pass | Detects subscription CLIs during setup and offers the obvious provider path. |
| `init` | Preview before mutate | pass | Interactive setup confirms before writing config and completions. |
| `init` | Refuse with try: | pass | Setup refusals point at `doctor`, `detect`, or provider configuration. |
| `init` | One-command rollback | n-a | Setup writes user config rather than a workspace change. |
| `init` | One verdict + ONE primary action | fail | Setup output still presents several equivalent next steps. |
| `init` | Lifecycle hint | pass | Help and completion install end by pointing to `doctor` or `start`. |
| `config` | Auto-detect, don't ask | pass | Direct get/set/provider commands do not prompt when the target is explicit. |
| `config` | Preview before mutate | pass | Provider/model interactive changes show the chosen route before writing. |
| `config` | Refuse with try: | fail | Some invalid key/value paths still need canonical `try:` coverage. |
| `config` | One-command rollback | pass | Config changes can be corrected with a single `config set` command. |
| `config` | One verdict + ONE primary action | fail | Config results do not yet lead with one primary action. |
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
| `completion` | One verdict + ONE primary action | fail | Completion output still has several peer next actions. |
| `completion` | Lifecycle hint | pass | Help points users back to setup/start. |
| `acceptance` | Auto-detect, don't ask | pass | Compatibility path maps to explicit done-criteria actions. |
| `acceptance` | Preview before mutate | pass | Draft/check/init paths expose criteria before launch use. |
| `acceptance` | Refuse with try: | pass | Criteria errors point to `def-done` recovery. |
| `acceptance` | One-command rollback | pass | Criteria can be overwritten or replaced with `def-done`. |
| `acceptance` | One verdict + ONE primary action | fail | Compatibility output has not adopted the Effortless primary-action slot. |
| `acceptance` | Lifecycle hint | pass | Help points to `def-done` and run/start use. |
| `def-done` | Auto-detect, don't ask | pass | Existing criteria are reused or displayed directly. |
| `def-done` | Preview before mutate | pass | Check/show/draft flows expose the done contract before runs depend on it. |
| `def-done` | Refuse with try: | pass | Invalid criteria and suppression refusals include concrete recovery. |
| `def-done` | One-command rollback | pass | Re-running `def-done` or `def-done add` changes the contract directly. |
| `def-done` | One verdict + ONE primary action | fail | Output still needs the one-verdict card treatment. |
| `def-done` | Lifecycle hint | pass | Help points to `start` after criteria are defined. |
| `try` | Auto-detect, don't ask | pass | It always uses the local smoke provider and throwaway workspace. |
| `try` | Preview before mutate | n-a | It mutates only isolated demo runstate under `DEADRECKON_HOME`. |
| `try` | Refuse with try: | pass | Any failed smoke proof points at inspection and normal `start`. |
| `try` | One-command rollback | n-a | It does not change the caller checkout. |
| `try` | One verdict + ONE primary action | pass | The proof block leads with a signed gate and one start command. |
| `try` | Lifecycle hint | pass | It ends with `deadreckon start "build the real thing"`. |
| `start` | Auto-detect, don't ask | fail | Single detected subscription providers still need inline adoption. |
| `start` | Preview before mutate | pass | Guided launch previews and confirms before creating run/plan state. |
| `start` | Refuse with try: | pass | Non-TTY missing provider/done/source refusals include recovery lines. |
| `start` | One-command rollback | pass | Launched worktree runs point at cleanup/abandon paths. |
| `start` | One verdict + ONE primary action | fail | Launch completion still shows multiple equal lifecycle hints. |
| `start` | Lifecycle hint | pass | Successful launches print attach/status/kill/finish commands. |
| `run` | Auto-detect, don't ask | pass | Defaults resolve provider, source mode, and done criteria without prompts when explicit. |
| `run` | Preview before mutate | pass | Worktree/in-place/high-spend paths preview or require confirmation. |
| `run` | Refuse with try: | pass | Common provider/source/safety refusals end with `try:` guidance. |
| `run` | One-command rollback | pass | Worktree runs can be abandoned and in-place runs have undo snapshots. |
| `run` | One verdict + ONE primary action | fail | Exit cards still need the distinguished primary action. |
| `run` | Lifecycle hint | pass | Completion hints point to attach/show/apply/finish/cleanup. |
| `orchestrate` | Auto-detect, don't ask | pass | Explicit mode/provider flags avoid extra prompts; auto chooses a conservative path. |
| `orchestrate` | Preview before mutate | pass | Preview and final confirmation precede plan creation/forking. |
| `orchestrate` | Refuse with try: | pass | Planner/provider/source refusals include concrete launch commands. |
| `orchestrate` | One-command rollback | pass | Plan state can be killed/cleaned and merged results are applied explicitly. |
| `orchestrate` | One verdict + ONE primary action | fail | Plan lifecycle output still lists several peer actions. |
| `orchestrate` | Lifecycle hint | pass | Output names attach/merge/finish. |
| `campaign` | Auto-detect, don't ask | fail | `--n` is still required instead of recommendation-seeded. |
| `campaign` | Preview before mutate | pass | Campaign preview shows sub-goals before launch. |
| `campaign` | Refuse with try: | pass | Depth/count/cross-sub refusals include recovery lines. |
| `campaign` | One-command rollback | pass | Campaigns can be killed and result application is explicit. |
| `campaign` | One verdict + ONE primary action | fail | Campaign summaries need one primary action and verified vocabulary. |
| `campaign` | Lifecycle hint | pass | Campaign output points at attach/show/finish. |
| `plan` | Auto-detect, don't ask | pass | Explicit planning inputs run without extra prompts. |
| `plan` | Preview before mutate | pass | Preview mode writes no plan state. |
| `plan` | Refuse with try: | pass | Planner and graph validation errors include recovery. |
| `plan` | One-command rollback | pass | Draft plan state can be killed/removed before merge. |
| `plan` | One verdict + ONE primary action | fail | Plan output does not yet use one primary action. |
| `plan` | Lifecycle hint | pass | Help/output point at fork/attach/merge. |
| `fork` | Auto-detect, don't ask | pass | Ready tasks and provider routes are read from the plan. |
| `fork` | Preview before mutate | pass | Fork refuses already-forked or invalid plans before spawning. |
| `fork` | Refuse with try: | pass | Blocked dependency and provider errors include next commands. |
| `fork` | One-command rollback | pass | Live plans can be killed. |
| `fork` | One verdict + ONE primary action | fail | Fork completion still lacks one primary action. |
| `fork` | Lifecycle hint | pass | Output points at attach and merge. |
| `merge` | Auto-detect, don't ask | pass | Completed children and repair providers resolve from plan/config. |
| `merge` | Preview before mutate | pass | Conflict repair/refusal happens before promotion. |
| `merge` | Refuse with try: | pass | Conflict refusals point to merge repair/prefer-child options. |
| `merge` | One-command rollback | pass | Promotion results are library entries until explicitly applied/exported. |
| `merge` | One verdict + ONE primary action | fail | Merge result output needs one primary action. |
| `merge` | Lifecycle hint | pass | Output points at finish/apply/export. |
| `chain` | Auto-detect, don't ask | fail | Chain breadth/step planning still asks more than the effortless path should. |
| `chain` | Preview before mutate | pass | Plan/run/apply modes preview or confirm state changes. |
| `chain` | Refuse with try: | pass | Chain refusals include scoped recovery commands. |
| `chain` | One-command rollback | pass | Undo/redo/reapply are one-command recovery paths. |
| `chain` | One verdict + ONE primary action | fail | Chain attach/status cards still need one primary action. |
| `chain` | Lifecycle hint | pass | Chain output points at attach/resume/finish-style actions. |
| `doctor` | Auto-detect, don't ask | pass | It probes local setup without prompting. |
| `doctor` | Preview before mutate | n-a | Read-only diagnostic. |
| `doctor` | Refuse with try: | pass | Findings include concrete setup/provider commands. |
| `doctor` | One-command rollback | n-a | Read-only diagnostic. |
| `doctor` | One verdict + ONE primary action | fail | Multi-finding output lacks a single primary action. |
| `doctor` | Lifecycle hint | pass | Help points to init/start. |
| `detect` | Auto-detect, don't ask | pass | Provider probes are automatic and bounded. |
| `detect` | Preview before mutate | n-a | Read-only diagnostic. |
| `detect` | Refuse with try: | pass | Unknown provider and ping guidance are actionable. |
| `detect` | One-command rollback | n-a | Read-only diagnostic. |
| `detect` | One verdict + ONE primary action | fail | Detection output still presents a table, not one primary action. |
| `detect` | Lifecycle hint | pass | Help points to config/start. |
| `providers` | Auto-detect, don't ask | pass | Configured/default provider rows resolve automatically. |
| `providers` | Preview before mutate | n-a | Read-only listing. |
| `providers` | Refuse with try: | pass | Unknown route/model paths include provider list guidance. |
| `providers` | One-command rollback | n-a | Read-only listing. |
| `providers` | One verdict + ONE primary action | fail | Provider listing lacks a single primary next action. |
| `providers` | Lifecycle hint | pass | Help points to start/run with provider flags. |
| `update` | Auto-detect, don't ask | pass | Install channel is read from the receipt. |
| `update` | Preview before mutate | pass | Update checks and shell swaps preview before replacing binaries. |
| `update` | Refuse with try: | pass | Unsupported channels and failed swaps print recovery. |
| `update` | One-command rollback | pass | Shell updates write backups and native channels print native rollback paths. |
| `update` | One verdict + ONE primary action | fail | Update output still lists several hints. |
| `update` | Lifecycle hint | pass | Success points at doctor. |
| `list` | Auto-detect, don't ask | pass | Defaults to current project scope and latest inventory. |
| `list` | Preview before mutate | n-a | Read-only listing. |
| `list` | Refuse with try: | pass | Scope/ref errors point at `--all` or show commands. |
| `list` | One-command rollback | n-a | Read-only listing. |
| `list` | One verdict + ONE primary action | fail | Inventory output is still multi-action. |
| `list` | Lifecycle hint | pass | Rows point at show/attach/finish. |
| `library` | Auto-detect, don't ask | pass | Library list/search/show default to scoped artifacts. |
| `library` | Preview before mutate | n-a | Read-only artifact inspection. |
| `library` | Refuse with try: | pass | Missing artifact paths include list/search recovery. |
| `library` | One-command rollback | n-a | Read-only artifact inspection. |
| `library` | One verdict + ONE primary action | fail | Library output lacks one primary next action. |
| `library` | Lifecycle hint | pass | Help points at finish/materialize. |
| `finish` | Auto-detect, don't ask | pass | Chooses apply/export based on run/source mode. |
| `finish` | Preview before mutate | pass | Destructive apply/export paths respect confirmations. |
| `finish` | Refuse with try: | pass | Incomplete/dirty/conflict paths include recovery commands. |
| `finish` | One-command rollback | pass | Apply/export paths expose cleanup or git recovery. |
| `finish` | One verdict + ONE primary action | fail | Finish still needs the shared one-primary-action slot. |
| `finish` | Lifecycle hint | pass | Output points at the chosen keep action. |
| `materialize` | Auto-detect, don't ask | pass | Completed artifact mode is resolved from the run/plan. |
| `materialize` | Preview before mutate | pass | Existing destination and overwrite behavior is explicit. |
| `materialize` | Refuse with try: | pass | Destination and state refusals include export/finish recovery. |
| `materialize` | One-command rollback | pass | Exported directories can be removed or overwritten explicitly. |
| `materialize` | One verdict + ONE primary action | fail | Export output has not adopted the primary-action slot. |
| `materialize` | Lifecycle hint | pass | Help points to show/extend. |
| `apply` | Auto-detect, don't ask | pass | Run/plan mode chooses the git apply strategy path. |
| `apply` | Preview before mutate | pass | Dirty/conflict/autostash paths preview or refuse before merge. |
| `apply` | Refuse with try: | pass | Conflict and dirty-tree refusals include recovery lines. |
| `apply` | One-command rollback | pass | Git merge/squash/cherry-pick recovery is one command from the branch. |
| `apply` | One verdict + ONE primary action | fail | Apply output needs one primary post-action. |
| `apply` | Lifecycle hint | pass | Hints point at cleanup or git log. |
| `abandon` | Auto-detect, don't ask | pass | Targets latest/current-scope runs when unambiguous. |
| `abandon` | Preview before mutate | pass | Refuses in-place and requires force for live cleanup. |
| `abandon` | Refuse with try: | pass | Unsafe abandon paths point at cleanup/undo. |
| `abandon` | One-command rollback | pass | Worktree branch/path removal is explicit; keep-branch is available. |
| `abandon` | One verdict + ONE primary action | fail | Abandon output lacks one primary next action. |
| `abandon` | Lifecycle hint | pass | Help points to cleanup. |
| `cleanup` | Auto-detect, don't ask | pass | Defaults to current-scope cleanup candidates. |
| `cleanup` | Preview before mutate | pass | Candidate cleanup requires explicit target/confirmation. |
| `cleanup` | Refuse with try: | pass | No-candidate and unsafe paths include cleanup alternatives. |
| `cleanup` | One-command rollback | pass | Completed work remains in library unless explicitly removed. |
| `cleanup` | One verdict + ONE primary action | fail | Cleanup summary needs one primary action. |
| `cleanup` | Lifecycle hint | pass | Help points at stale/completed cleanup modes. |
| `extend` | Auto-detect, don't ask | pass | `latest` and current-scope history resolve automatically. |
| `extend` | Preview before mutate | pass | Follow-up run creation uses normal launch preview. |
| `extend` | Refuse with try: | pass | Incomplete/missing parent paths include start/run alternatives. |
| `extend` | One-command rollback | pass | Follow-up worktree runs can be abandoned. |
| `extend` | One verdict + ONE primary action | fail | Extend launch output still lists equal hints. |
| `extend` | Lifecycle hint | pass | Help points at attach/finish. |
| `doc` | Auto-detect, don't ask | pass | Defaults to latest run narrative and deterministic docs. |
| `doc` | Preview before mutate | pass | Provider polish prints a preview/confirmation before regeneration. |
| `doc` | Refuse with try: | pass | Missing provider/doc paths include polish/config recovery. |
| `doc` | One-command rollback | n-a | Read paths are immutable; polish can be re-run. |
| `doc` | One verdict + ONE primary action | fail | Doc output does not lead with one action. |
| `doc` | Lifecycle hint | pass | Help points at export/polish/finish paths. |
| `attach` | Auto-detect, don't ask | pass | `latest`, run/plan/chain kind, and child refs resolve automatically. |
| `attach` | Preview before mutate | n-a | Attach is observational unless the user presses an explicit action key. |
| `attach` | Refuse with try: | pass | Unsupported narrative/target paths include recovery. |
| `attach` | One-command rollback | n-a | Read-only watch surface. |
| `attach` | One verdict + ONE primary action | fail | Completed footers still expose several equal keys. |
| `attach` | Lifecycle hint | pass | Footer points at finish/show/apply or detach. |
| `kill` | Auto-detect, don't ask | pass | Run/plan/chain/campaign ids resolve by prefix/latest. |
| `kill` | Preview before mutate | pass | Confirms destructive stop unless explicitly scripted. |
| `kill` | Refuse with try: | pass | Missing/not-running targets include status/list recovery. |
| `kill` | One-command rollback | pass | Killed runs can be resumed when state permits. |
| `kill` | One verdict + ONE primary action | fail | Kill summary needs one primary action. |
| `kill` | Lifecycle hint | pass | Output points at resume/cleanup/status. |
| `resume` | Auto-detect, don't ask | pass | `latest` and partial traces resolve automatically. |
| `resume` | Preview before mutate | pass | Resume shows provider/source context before entering the loop. |
| `resume` | Refuse with try: | pass | Completed/missing run paths include extend/start guidance. |
| `resume` | One-command rollback | pass | Resumed worktree runs remain abandonable/undoable. |
| `resume` | One verdict + ONE primary action | fail | Resume output still lacks one primary action. |
| `resume` | Lifecycle hint | pass | Help points at attach. |
| `undo` | Auto-detect, don't ask | pass | Snapshot targets resolve from run metadata. |
| `undo` | Preview before mutate | pass | Preview/target selection prevents accidental snapshot restoration. |
| `undo` | Refuse with try: | pass | Missing snapshot paths include show/rewind guidance. |
| `undo` | One-command rollback | pass | A later snapshot or rerun can restore forward state. |
| `undo` | One verdict + ONE primary action | fail | Undo output needs one primary action. |
| `undo` | Lifecycle hint | pass | Help points at show. |
| `rewind` | Auto-detect, don't ask | pass | Flight checkpoints resolve by event target. |
| `rewind` | Preview before mutate | pass | Hash-guarded preview is the default before apply. |
| `rewind` | Refuse with try: | pass | Missing/unrelated checkpoint paths include show flight guidance. |
| `rewind` | One-command rollback | pass | Rewind is preview-first and applied changes are explicit. |
| `rewind` | One verdict + ONE primary action | fail | Rewind output needs one primary action. |
| `rewind` | Lifecycle hint | pass | Help points at show/flight. |
| `show` | Auto-detect, don't ask | pass | `latest`, prefixes, plans, and child refs resolve automatically. |
| `show` | Preview before mutate | n-a | Read-only inspection. |
| `show` | Refuse with try: | pass | Missing/ambiguous ids include list/status recovery. |
| `show` | One-command rollback | n-a | Read-only inspection. |
| `show` | One verdict + ONE primary action | fail | Show output is intentionally raw and needs a primary-action wrapper. |
| `show` | Lifecycle hint | pass | Output includes attach/finish/doc hints. |
| `history` | Auto-detect, don't ask | pass | Defaults to current scope and evidence kind. |
| `history` | Preview before mutate | n-a | Read-only search. |
| `history` | Refuse with try: | pass | Invalid regex/scope paths include corrected commands. |
| `history` | One-command rollback | n-a | Read-only search. |
| `history` | One verdict + ONE primary action | fail | Search output lacks one primary next action. |
| `history` | Lifecycle hint | pass | Help points at show/import. |
| `status` | Auto-detect, don't ask | pass | Defaults to latest/current-scope run or plan. |
| `status` | Preview before mutate | n-a | Read-only orientation. |
| `status` | Refuse with try: | pass | Missing state points at start/list. |
| `status` | One-command rollback | n-a | Read-only orientation. |
| `status` | One verdict + ONE primary action | fail | Status still needs one verdict plus one primary action. |
| `status` | Lifecycle hint | pass | Status includes natural next commands. |
| `import` | Auto-detect, don't ask | pass | Descriptor sessions auto-discover by cwd/source when unambiguous. |
| `import` | Preview before mutate | pass | Preview/list modes show candidates before creating a run. |
| `import` | Refuse with try: | pass | Ambiguous/stale/unknown imports include exact retry commands. |
| `import` | One-command rollback | pass | Imported runs can be abandoned or replaced explicitly. |
| `import` | One verdict + ONE primary action | fail | Import output needs one primary next action. |
| `import` | Lifecycle hint | pass | Help points at show. |
| `learn` | Auto-detect, don't ask | pass | Index/report/propose use existing run evidence by default. |
| `learn` | Preview before mutate | pass | Import/export/propose candidate paths preview before durable writes. |
| `learn` | Refuse with try: | pass | Weak evidence and bundle errors include recovery. |
| `learn` | One-command rollback | pass | Proposals/candidates are file-backed and can be replaced. |
| `learn` | One verdict + ONE primary action | fail | Learning output needs one primary action. |
| `learn` | Lifecycle hint | pass | Help points at improve/report. |
| `improve` | Auto-detect, don't ask | pass | Proposal ids and goal files resolve directly. |
| `improve` | Preview before mutate | pass | Self-improve is preview-first and uses isolated worktrees. |
| `improve` | Refuse with try: | pass | Risk/evidence refusals include concrete recovery. |
| `improve` | One-command rollback | pass | Candidate worktrees can be abandoned and PR opening is opt-in. |
| `improve` | One verdict + ONE primary action | fail | Self-improve summaries need one primary action. |
| `improve` | Lifecycle hint | pass | Help points at learn/report/PR dry-run. |
