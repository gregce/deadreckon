# deadreckon — Codebase-Mode Rider (codebase as default; worktree as primary)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-11-deadreckon-codebase-goal.md`.
It supersedes nothing in prior riders
(`2026-05-10-deadreckon-build-rider.md`,
`2026-05-11-deadreckon-primary-flow-rider.md`,
`2026-05-11-deadreckon-robust-rider.md`,
`2026-05-11-deadreckon-usability-rider.md`,
`2026-05-11-deadreckon-orchestrate-rider.md`) — their invariants,
dependency policy, sandbox defaults, lifecycle hint convention,
files-not-fields lineage pattern, and existing verbs still apply. This
rider changes the **default** semantics of `deadreckon run`, adds two
verbs (`apply`, `abandon`), and prescribes the friendly preflight /
preview / rollback story.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided — do not redesign)

- **Maturity stays `alpha`.** Workspace stays `version = "0.1.0"`.
- **No `PipelineState` schema changes.** Mode metadata lives in
  `working/.deadreckon/codebase.json` (the usability-rider files-not-fields
  pattern, alongside `parent.json`).
- **Worktree is the primary path.** Branch + git worktree are the
  isolation primitive of choice because (a) they let the user inspect /
  diff / abandon trivially, (b) they match unmet need #2 ("five agents
  on five branches"), (c) they don't bake a copy cost into the happy
  path.
- **Today's empty-dir behavior is preserved verbatim** behind
  `--fresh`. Existing smoke tests stay green by either passing `--fresh`
  or running inside a temp-git-init test fixture.
- **Friendliness is verifiable.** Every preflight refusal, every
  preview-block field, and every lifecycle hint is exercised by a named
  depth test.
- **No `git push`** from this binary, no CI calls to `git push`. The
  user's `apply` is the only thing that mutates the user's checkout,
  and even then never pushes.
- **No V1 invention.** If a phase reveals a V1-architecture decision,
  log it in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` and continue.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Mode resolution algorithm

The mode is resolved deterministically by `deadreckon run` before any
file change. Pseudocode (the rider is the spec; the implementation
must match):

```
fn resolve_mode(flags, cwd, tty) -> Mode {
    if flags.fresh { return Fresh }
    if flags.in_place {
        require(flags.i_know_its_a_lot || (tty && double_confirm()),
                else_refuse_with: "try: --in-place --i-know-its-a-lot")
        return InPlace
    }
    if flags.worktree { return Worktree }
    if flags.from.is_some() { return Copy(flags.from) }
    // auto-resolution
    match find_git_root(cwd) {
        Some(_) => Worktree,                          // default in git
        None if tty => offer_prompt(),                // git init | copy | cancel
        None => refuse_with: "try: --fresh or --from . or git init"
    }
}
```

The `offer_prompt()` choices:

```
deadreckon: this is not a git repo. options:
  [1] git init for me, then run with worktree mode (recommended)
  [2] copy mode — agent works on a copy in ~/.deadreckon/runstate/...
  [3] cancel
choose [1]:
```

Default is option 1 (Enter accepts). Choice 1 runs
`git init -b main && git add -A && git commit -m "initial commit (deadreckon init)"`
then proceeds in worktree mode. Choice 2 sets mode = `Copy(cwd)`.
Choice 3 exits 0 with no state changes.

## `codebase.json` schema

Written into `<working_dir>/.deadreckon/codebase.json` at run start,
before the agent's first turn. Single source of truth for everything
mode-related.

```json
{
  "schema_version": 1,
  "mode": "worktree|copy|in-place|fresh",
  "source_path": "/Users/gdc/myproject",
  "source_git_root": "/Users/gdc/myproject",
  "branch_name": "dr/fix-off-by-one-ab12cd34",
  "base_ref": "main",
  "base_sha": "e7c4d23456...",
  "worktree_path": "/Users/gdc/.deadreckon/worktrees/myproject-ab12cd34",
  "dirty_files_seeded": false,
  "head_was_detached": false,
  "created_at": "<RFC3339>",
  "deadreckon_version": "<crate version>"
}
```

- `mode = "copy"`: `branch_name`, `base_ref`, `base_sha`,
  `worktree_path` are all `null`.
- `mode = "in-place"`: `worktree_path = null`, `branch_name = null`,
  `source_path = working_dir`.
- `mode = "fresh"`: everything except `mode`, `created_at`,
  `deadreckon_version` is `null` (or the file is omitted altogether —
  P1 chooses).

## Branch and worktree conventions

- **Branch name.** `dr/<task-slug-first-32>-<run-id-first-8>`. The
  `dr/` prefix is reserved — `apply` and `abandon` refuse to touch
  branches without it.
- **Worktree path.** `~/.deadreckon/worktrees/<scope>-<run-id-first-8>/`.
  Scope comes from `paths::workspace_scope` against
  `source_git_root`.
- **Collision.** If the worktree path or branch name already exists
  (astronomically unlikely but possible after a crash), append `-2`,
  `-3`, … until unique. Log the collision to traces.

## Preflight checks (worktree mode)

Run in this order before any file change. First failure aborts with
the rider's canonical `try:` line.

| Check | Pass condition | On fail: error | `try:` |
|---|---|---|---|
| Source is a git repo | `.git` discoverable from `source_path` | `<path> is not a git repo` | `git init or pass --from .` |
| HEAD exists | `git rev-parse HEAD` exits 0 | `git repo has no commits` | `git commit -m initial` |
| HEAD not detached | `git symbolic-ref -q HEAD` exits 0 | `HEAD is detached at <sha>` | `git switch -c <branch-name>` |
| Not mid-merge | `.git/MERGE_HEAD` absent | `git is in the middle of a merge` | `git merge --abort` |
| Not mid-rebase | `.git/rebase-merge` and `.git/rebase-apply` absent | `git is in the middle of a rebase` | `git rebase --abort` |
| Working tree clean | `git status --porcelain` empty | `working tree has uncommitted changes:\n  <files>` | `git stash && deadreckon run … (or pass --allow-dirty)` |
| Branch doesn't exist | `git rev-parse --verify <branch>` fails | `branch <name> already exists` | `pass --branch <other>` |
| Worktree path free | dir absent or empty | `worktree path <p> already exists` | `pass --worktree-path <other> or remove the dir` |

`--allow-dirty` mode: the uncommitted files are copied into the new
worktree after creation (`git diff && git diff --cached` patched in).
`codebase.json.dirty_files_seeded = true` records it.

## Preview / preflight UX

After mode resolution but before any state change, the binary prints
a single-screen block to stderr, then prompts. Suppressed by `--yes`;
when `--preview` is passed, the block is printed and the binary exits
0 with no state changes.

Multi-line (default):

```
deadreckon: ready to run

  goal:     fix the off-by-one in src/parser.rs
  source:   /Users/gdc/myproject (git: clean, branch=main @ e7c4d234)
  mode:     worktree
    branch:   dr/fix-off-by-one-ab12cd34
    base:     main (e7c4d234)
    worktree: /Users/gdc/.deadreckon/worktrees/myproject-ab12cd34
  agent:    cli:claude-code
  sandbox:  sandbox-exec (mac)
  caps:     spend ≤ $10, wall ≤ 1h
  on success: deadreckon apply <run-id>
  on fail:    deadreckon abandon <run-id>

continue? [Y/n]:
```

`--brief`: one-line form (rider's exact text):

```
mode=worktree branch=dr/fix-off-by-one-ab12cd34 base=main (e7c4d234) wt=~/.dr/worktrees/myproject-ab12cd34 agent=cli:claude-code cap=$10/1h
```

Non-TTY without `--yes`: refuse with `try: --yes (skip confirm) or run interactively`.

For non-worktree modes, the relevant lines are present (`copy` shows
source + dest; `in-place` shows the SOURCE-IS-USER-TREE warning
prominently; `fresh` shows just the runstate path).

In-place double-confirm (in TTY):

```
deadreckon: in-place mode WILL EDIT files in /Users/gdc/myproject directly.
            rollback via `deadreckon undo`; snapshots are kept in runstate.
type the project basename to confirm [type: myproject]:
```

## New verbs

### `deadreckon apply <run-id>`

```
deadreckon apply <run-id>
    [--strategy merge|squash|cherry-pick]   # default: squash
    [--branch <name>]                       # override target branch (default: HEAD)
    [--no-confirm]
    [--message <msg>]                       # override commit message
```

Behavior:

1. Refuse if `codebase.json.mode != "worktree"`. Hint depends on mode
   (`materialize` for copy, `undo`/edits-on-disk note for in-place).
2. Refuse if `state.status != Completed`. Hint: `deadreckon resume` or
   `--force` (not provided in alpha).
3. Refuse if user's current working tree is dirty.
4. Resolve target branch (default `git symbolic-ref HEAD`).
5. Compute strategy:
   - `merge` → `git merge --no-ff <branch>`.
   - `squash` (default) → `git merge --squash <branch>` + `git commit
     -m "<message-or-default>"`.
   - `cherry-pick` → `git cherry-pick <branch>~<turns>..<branch>` (each
     turn-commit preserved).
6. Show diff stat (`git diff --stat <target>..<branch>`); confirm
   unless `--no-confirm`.
7. Execute strategy. On success, print `git log -1 --stat`. On
   conflict, leave markers and print: `try: resolve conflicts, then
   git commit && deadreckon abandon <run-id>`.

Default commit message: `<goal-first-line> (deadreckon run <run-id-prefix>)`.

Refusal table (subset of full error footers):

| Condition | Error | Try |
|---|---|---|
| Mode != worktree (copy) | `apply requires worktree mode; run was copy` | `deadreckon materialize <run-id> --dest <path>` |
| Mode != worktree (in-place) | `apply requires worktree mode; run was in-place` | `deadreckon undo to revert if needed` |
| Mode != worktree (fresh) | `apply requires worktree mode; run was fresh` | `deadreckon materialize <run-id> --dest <path>` |
| Run not Completed | `run <id> is <status>` | status-specific (resume/show-why-failed) |
| User tree dirty | `your working tree has uncommitted changes` | `git stash && deadreckon apply <run-id>` |
| Branch missing | `branch <dr/...> not found` | `deadreckon abandon <run-id>; the run may have been cleaned up` |
| Merge conflict | `merge produced conflicts: <files>` | `resolve, then git commit && deadreckon abandon <run-id>` |

### `deadreckon abandon <run-id>`

```
deadreckon abandon <run-id>
    [--keep-branch]
    [--force]                # ok to remove worktree with uncommitted changes
```

Behavior:

1. Idempotent. If `codebase.json` absent or `mode == "fresh"`: no-op,
   print `nothing to abandon for run <id>`.
2. If run is `Running`: refuse unless `--force` (force kills first via
   the existing kill path, then proceeds).
3. Worktree mode: `git worktree remove <path>` (with `--force` when
   `--force`); then (unless `--keep-branch`) `git branch -D <branch>`.
4. Copy mode: removes runstate working dir copy (already cleaned by
   normal promotion; no-op if already promoted).
5. In-place mode: refuse with `cannot abandon in-place edits` /
   `try: deadreckon undo` (because the user's tree IS the working
   dir).
6. Records `~/.deadreckon/runstate/<scope>/runs/<run-id>/abandoned.json`
   with timestamp so `deadreckon list` can show ABANDONED status.

### `deadreckon run` flag additions

```
deadreckon run <goal>
    [--fresh]                              # today's behavior; empty working_dir
    [--worktree]                           # force worktree (must be in git repo)
    [--from <path>]                        # force copy mode from path
    [--in-place]                           # force in-place (requires --i-know-its-a-lot)
    [--base <ref>]                         # worktree base; default = current branch
    [--branch <name>]                      # worktree branch override
    [--allow-dirty]                        # worktree: seed uncommitted into the worktree
    [--init-git]                           # in non-git: silently `git init` and use worktree
    [--yes]                                # skip preview confirm
    [--preview]                            # print preview block, exit 0
    [--brief]                              # one-line preview format
    [...existing flags...]
```

The default invocation in a git repo (`deadreckon run "goal"`) prints
the preview and asks for confirmation; the user types `y` and the run
starts. That's the happy path.

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them
fail; implement; green on
`cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`;
conventional-commit local commit; one-line CHANGELOG entry.

### P1 — Mode resolution + `codebase.json` plumbing (no behavior change yet)

- New `crates/deadreckon-core/src/codebase.rs` with `Mode` enum,
  `CodebaseRecord` struct (matches schema above), `resolve_mode(...)`,
  read/write helpers.
- `RunOptions` extended with mode-related fields (in-process, not
  serialized to `state.json`).
- Existing `create_run` path: when mode is `Fresh`, writes
  `codebase.json` with `mode: "fresh"` (or omits — implementer's
  choice, documented in §22 update).

Depth tests (in `crates/deadreckon/tests/codebase.rs`):
- `mode_resolution_in_git_repo_defaults_to_worktree`
- `mode_resolution_outside_git_non_interactive_refuses`
- `codebase_json_roundtrip_for_each_mode`

### P2 — Worktree mode + preflight checks (the headline)

- Implement git invocations via `std::process::Command` (`git worktree
  add`, `git rev-parse`, `git symbolic-ref`, `git status --porcelain`,
  `git stash` flows for `--allow-dirty`).
- Each preflight check from the rider's table is a function in
  `codebase.rs`. The full chain runs in `create_run` before any state
  is written.
- On worktree creation success, `working_dir` is set to the worktree
  path (in-process only; not persisted on `PipelineState`).

Depth tests:
- `worktree_run_creates_dr_prefixed_branch_in_worktrees_dir`
- `worktree_run_sets_working_dir_to_worktree_path`
- `dirty_repo_refused_with_stash_hint`
- `detached_head_refused_with_switch_hint`
- `mid_merge_refused_with_abort_hint`
- `no_commits_refused_with_initial_commit_hint`
- `allow_dirty_seeds_uncommitted_into_worktree`
- `branch_collision_refused_with_branch_hint`
- `worktree_path_collision_appends_suffix`

### P3 — Copy mode (`.gitignore`-aware)

- Use the `ignore` crate (Tier 1) to walk the source tree honoring
  `.gitignore`, `.ignore`, and global ignore patterns. Copy each
  surviving file into `working_dir`.
- `materialize` already handles writing from library to a user dest;
  no changes needed there beyond a refusal-message check (the user
  who tries `apply` after a copy run gets pointed at `materialize`).

Depth tests:
- `copy_mode_respects_gitignore`
- `copy_mode_succeeds_in_non_git_dir`
- `copy_mode_materialize_writes_dest_unchanged_from_today`

### P4 — In-place mode (with the double-confirm)

- `working_dir = source_path`. Snapshots continue to live under
  `~/.deadreckon/runstate/<scope>/runs/<id>/snapshots/`.
- Interactive path: project-basename double-confirm.
- Non-interactive path: requires `--in-place --i-know-its-a-lot`.

Depth tests:
- `in_place_requires_double_confirm_or_i_know_flag`
- `in_place_run_edits_source_path_directly`
- `in_place_undo_reverts_via_runstate_snapshot`
- `in_place_refuses_apply_with_try_undo_hint`

### P5 — Preflight + preview UX

- Format the preview block exactly as the rider's template (the
  whitespace and field order are part of the depth test).
- `--yes` skips confirm; `--preview` exits 0 after print; `--brief` is
  the one-line form; non-TTY without `--yes` refuses.
- All output goes to stderr; stdout is reserved for post-action hints
  and tool output.

Depth tests:
- `preview_block_contains_required_fields_in_order`
- `preview_flag_exits_zero_without_state_change`
- `yes_flag_skips_confirm_prompt`
- `non_tty_without_yes_refuses`
- `brief_mode_is_one_line`

### P6 — `apply` verb

- See verb signature above. Implementation calls `git merge`,
  `git cherry-pick`, etc. via `Command::new("git")`.

Depth tests:
- `apply_squash_creates_commit_on_user_branch`
- `apply_merge_no_ff_creates_merge_commit`
- `apply_cherry_pick_preserves_turn_commits`
- `apply_refuses_on_dirty_user_tree`
- `apply_refuses_non_worktree_with_mode_specific_hint`
- `apply_refuses_uncompleted_run`
- `apply_conflict_leaves_markers_and_prints_resolve_hint`

### P7 — `abandon` verb

- See verb signature above.

Depth tests:
- `abandon_removes_worktree_and_branch`
- `abandon_keep_branch_keeps_branch`
- `abandon_idempotent_when_already_abandoned`
- `abandon_force_terminates_running_run_then_cleans`
- `abandon_in_place_refuses_with_undo_hint`
- `abandon_writes_abandoned_json_for_list_visibility`

### P8 — Integration with existing verbs (`materialize`, `extend`, `undo`, `list`, `show`)

- **`materialize`**: in worktree mode, refuses with `try: deadreckon
  apply <run-id>`. In copy/fresh modes, current behavior.
- **`extend`**: in worktree mode, creates a new run with mode worktree
  + branch off the parent's branch (`base = parent's dr/...`); writes
  the new child's `codebase.json` with `parent_branch` field. In copy
  mode, current behavior. In in-place, refuses with `try: deadreckon
  run --in-place "<new goal>"`.
- **`undo`**: snapshot-based, mode-agnostic. Per-turn snapshots
  continue to be the source of truth. In-place runs use snapshots to
  restore the user's tree directly.
- **`list`**: gains a `MODE` column. Possible values: `worktree`,
  `copy`, `in-place`, `fresh`, `abandoned` (if `abandoned.json`
  present).
- **`show <run-id>`**: prints mode + branch + worktree path lines in
  the header when `codebase.json` is present.

Depth tests:
- `materialize_in_worktree_refuses_with_apply_hint`
- `extend_in_worktree_chains_branches`
- `extend_in_copy_unchanged_from_today`
- `extend_in_in_place_refuses_with_run_hint`
- `list_shows_mode_column`
- `show_reveals_codebase_lineage`

### P9 — Friendliness pass: error footers + post-action hints

- Every codebase-mode error message routes through `user_error(msg,
  try_hint)` from the orchestrate rider's P10 (or this rider's own
  equivalent if orchestrate hasn't landed yet — see "Dependencies"
  below).
- Post-action hints:

  After successful `run` (worktree mode):
  ```
  run abc12345 completed (turns=N, spend=$X.YZ)
  agent's changes are on branch dr/<task>-<id-short> in /Users/gdc/.deadreckon/worktrees/<scope>-<id-short>
    inspect: cd /Users/gdc/.deadreckon/worktrees/<scope>-<id-short> && git log --oneline <base>..HEAD
    apply:   deadreckon apply abc12345
    abandon: deadreckon abandon abc12345
  ```

  After successful `run` (copy mode):
  ```
  run abc12345 completed
    materialize: deadreckon materialize abc12345 --dest ./<task-prefix>
  ```

  After successful `apply`:
  ```
  applied abc12345 onto <target-branch>:
    <commit-sha> <commit-subject>
   <file1> | <changes>
   <file2> | <changes>
  next: deadreckon abandon abc12345 (removes branch and worktree)
  ```

  After successful `abandon`:
  ```
  abandoned abc12345
    removed: branch dr/<task>-<id-short>
    removed: /Users/gdc/.deadreckon/worktrees/<scope>-<id-short>
  ```

Depth tests:
- `post_run_hint_lists_apply_and_abandon_lines`
- `post_apply_hint_includes_git_log_one_stat`
- `post_abandon_hint_lists_removed_paths`
- `error_footers_parameterized_over_canonical_pairs` (parameterized
  test over the refusal-tables in this rider)

### P10 — Non-git first-run flow (offer-init / copy / cancel)

- Interactive prompt with the three options, default = `git init`.
- `--init-git` skips the prompt and silently initializes.
- Non-interactive without a flag → refuse with `try: --fresh or
  --from . or git init`.

Depth tests:
- `non_git_interactive_offers_three_choices_with_init_default`
- `non_git_choice_init_runs_git_init_then_worktree`
- `non_git_choice_copy_resolves_to_copy_mode`
- `non_git_choice_cancel_exits_zero_no_changes`
- `non_git_non_interactive_refuses_with_try_line`

### P11 — AS-BUILT update + CHANGELOG (doc only)

- Insert new top-level section into
  `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:

  ```
  ## NN. Codebase Modes

  NN.1 Why codebase-aware running is the default
  NN.2 Mode resolution (the deterministic flow)
  NN.3 Worktree mode: branch & path conventions
  NN.4 Copy mode: gitignore-aware seeding
  NN.5 In-place mode: when to use it, when not
  NN.6 Fresh mode: preservation contract
  NN.7 codebase.json schema
  NN.8 Preflight checks (worktree)
  NN.9 The preview block (verbatim sample)
  NN.10 apply / abandon mechanics
  NN.11 Integration with materialize / extend / undo / list / show
  NN.12 What's not yet built (V1-CANDIDATES pointers)
  ```

  `NN` = next available top-level number. If orchestrate-§18 has not
  landed: `18`. If orchestrate-§18 has landed: `19` (and renumber any
  subsequent sections that depend on orchestration adopting codebase
  modes).
- Update §22 (or current "What's Built vs Scaffolding-Thin"):
  - **Add to "Built and reliable":** codebase-default running,
    worktree mode, copy mode, in-place mode, fresh-mode preservation,
    `apply`, `abandon`, preflight + preview UX,
    materialize/extend/undo integration.
  - **Note explicitly:** this rider does **not** close any prior §22
    thin items; it adds capability. The thin list is unchanged.
- Append to `/Users/gdc/deadreckon/docs/CHANGELOG.md`:

  ```
  ## Codebase modes (alpha) — <YYYY-MM-DD>

  - Default mode in a git repo is now `worktree` (`git worktree add` +
    branch `dr/<task>-<id>` off current branch). User's checkout is
    never touched until `apply`.
  - New verbs: `apply` (squash/merge/cherry-pick), `abandon` (idempotent).
  - Modes: `worktree` (default), `copy` (default outside git), `in-place`
    (opt-in with double confirm), `fresh` (today's behavior, behind --fresh).
  - Preflight refuses dirty / detached / mid-merge / mid-rebase / no-commits
    with specific `try:` lines.
  - Preview block before any file change; `--yes` to skip, `--preview` to
    exit 0 after printing.
  - Integration: `materialize` refuses on worktree (→ apply); `extend` in
    worktree chains branches; `undo` works across modes; `list` shows MODE.
  ```

## Integration matrix

| Verb | Worktree | Copy | In-place | Fresh |
|---|---|---|---|---|
| `run` | preflight + worktree | preflight + copy seed | preflight + double-confirm | empty wd |
| `apply` | works (default `squash`) | refuse → `materialize` | refuse → `undo` | refuse → `materialize` |
| `abandon` | removes branch + worktree | no-op (already cleaned) | refuse → `undo` | no-op |
| `materialize` | refuse → `apply` | works | refuse (notes edits on disk) | works |
| `extend` | new branch off parent | new run from library | refuse → `run --in-place` | new run with empty wd |
| `undo` | works (per-turn snapshots) | works | works (snapshots in runstate) | works |
| `list` | `MODE=worktree` | `MODE=copy` | `MODE=in-place` | `MODE=fresh` |
| `show` | header includes branch + wt path | header notes copy mode | header notes in-place + warning | header unchanged |

## Error-footer canonical pairs (parameterized over P9 test)

| Error | `try:` line |
|---|---|
| `<path> is not a git repo` | `git init or pass --from .` |
| `git repo has no commits` | `git commit -m initial` |
| `HEAD is detached at <sha>` | `git switch -c <branch>` |
| `git is in the middle of a merge` | `git merge --abort` |
| `git is in the middle of a rebase` | `git rebase --abort` |
| `working tree has uncommitted changes: <files>` | `git stash && deadreckon run … (or --allow-dirty)` |
| `branch <name> already exists` | `pass --branch <other-name>` |
| `worktree path <p> already exists` | `pass --worktree-path <other> or remove the dir` |
| `apply requires worktree mode; run was <mode>` | mode-specific (see refusal table above) |
| `run <id> is <status>` | `deadreckon resume <id>` or `deadreckon show <id> --why-failed` |
| `your working tree has uncommitted changes` | `git stash && deadreckon apply <run-id>` |
| `branch <dr/...> not found` | `deadreckon abandon <run-id>` |
| `merge produced conflicts: <files>` | `resolve, then git commit && deadreckon abandon <run-id>` |
| `cannot abandon in-place edits` | `deadreckon undo <run-id>` |
| `--in-place requires --i-know-its-a-lot or interactive confirm` | `add --i-know-its-a-lot or run in a TTY` |
| `non-interactive without a mode flag` | `--fresh or --from . or git init` |

## Dependencies

Tier 1 (utility, free):
- `ignore` — `.gitignore`-aware walking for copy mode. Add direct
  dependency to `deadreckon-core`. Same crate as ripgrep's
  underlying walker.

Tier 2 (architectural, log to `DEPENDENCIES.md`): none expected.
**Do not** add `git2`/`libgit2-sys`; use `Command::new("git")`. The
agent runs in a sandbox and git is already required (existing
provider machinery + import already shell out to git when needed).

Tier 3 (blocked): same blocks as prior riders.

If the orchestrate rider has not yet landed (no
`user_error(msg, try_hint)` helper in
`crates/deadreckon-core/src/error.rs`), this rider's P9 introduces it
in this crate. If orchestrate landed first, P9 reuses it.

## Out of scope

- **Submodule mode resolution.** If `.gitmodules` is present, P2
  warns once on the preview block ("submodules detected; agent may
  edit but not update") and proceeds.
- **Git LFS / smudge filters.** Worktree mode inherits LFS settings
  from the source repo; no custom handling.
- **Mono-repo path-scoped worktrees.** Worktree always covers the
  full git root.
- **Auto-stash.** `--allow-dirty` copies uncommitted files into the
  worktree but does NOT touch the user's index / stash stack.
- **Auto-rebase before run.** The user is responsible for being on
  the right base.
- **Conflict resolution during `apply`.** The user resolves
  manually; the binary leaves the markers and prints `try:` lines.
- **`apply --onto <ref>`.** V1-CANDIDATE: applying onto a branch
  other than the user's HEAD.
- **Multi-repo plans.** A future orchestrate amendment will handle
  one plan spawning children across multiple repos; out of scope
  here.
- **Remote refs as `--base`.** `--base origin/main` should work (it's
  a normal git rev), but the rider does not require any
  remote-specific UX beyond passing the ref through to `git worktree
  add`. No `git fetch` is performed automatically.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.** Mode lives in
  `codebase.json`. If you find a reason to add a field, stop and log
  it in `V1-CANDIDATES.md`.
- **Existing tests pass.** Either by passing `--fresh` explicitly or
  by wrapping fixtures in `git init` setup. The rider permits both;
  prefer the latter for tests that exercise file-system behavior.
- **One depth test before each phase implementation.** A phase whose
  tests all started green never failed; that's a smell.
- **All git operations go through `Command::new("git")`.** No
  `libgit2`. The sandbox already grants git access on the existing
  tool path.
- **The preview block is part of the spec.** Field names, order, and
  whitespace are exercised by P5 depth tests. Changing them changes
  the spec.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with the relevant depth tests passing and a
  CHANGELOG entry naming the SHA.
- After P11, run the smoke flows from the goal end-to-end and capture
  the asciinema cast at
  `/Users/gdc/deadreckon/demo-codebase.cast`.
- If a phase reveals a V1-architecture decision, stop and log it in
  `V1-CANDIDATES.md`; do not silently expand scope.
