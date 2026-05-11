GOAL: Make codebase-aware running the **default** mode of deadreckon at `/Users/gdc/deadreckon/`. Today `deadreckon run` starts the agent in an empty `working/` dir under `~/.deadreckon/` (`state.rs:178-184`); the agent never sees the user's project. After this goal: in a git repo, the default is a fresh `git worktree` off the current branch; outside git, the binary offers `git init` / copy / cancel; today's empty-dir flow stays behind `--fresh`. **Friendliness is the headline** — one command, one preview, one confirmation, one apply or abandon.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — substrate. Only the working-dir-seeding path changes; `PipelineState` stays unchanged (mode metadata in `working/.deadreckon/codebase.json` per the usability-rider files-not-fields pattern).
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-deadreckon-codebase-rider.md` — mode resolution, branch naming, schemas, verb signatures, preview format, depth tests.
- `/Users/gdc/stoa/docs/research/2026-05-10-unmet-needs/REPORT.md` — primary needs #3, #2.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold. Orchestrate will adopt this substrate in a future amendment; do not preempt.

**Posture.** Stays `alpha`. No `PipelineState` schema changes. No `git push`. Edits inside `/Users/gdc/deadreckon/`. V1-architecture decisions go to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**Four modes, auto-resolved.**

- **`worktree` (default in a git repo).** `git worktree add ~/.deadreckon/worktrees/<scope>-<id>` on branch `dr/<task-slug>-<id>` off `--base` (default current branch). `working_dir` = the worktree.
- **`copy` (default outside git, or `--from <path>`).** `.gitignore`-aware copy into runstate `working_dir`. `materialize` writes back.
- **`in-place` (`--in-place --i-know-its-a-lot`).** `working_dir = <source-path>`. Agent edits user's tree directly. Snapshots still in runstate so `undo` works.
- **`fresh` (`--fresh`).** Today's empty-working-dir behavior; preserved for existing tests.

**New verbs.**

- `apply <run-id> [--strategy merge|squash|cherry-pick] [--no-confirm]` — squash-merges the run's branch into the user's current branch (default `squash`; worktree only).
- `abandon <run-id> [--keep-branch] [--force]` — removes worktree + branch; idempotent.

**Friendliness as a verifiable contract.**

- **Auto-detect, don't ask** in a git repo. Outside (TTY): offer `git init` (recommended) / copy / cancel. Non-interactive without a mode flag → refuse with `try:` line.
- **Preflight + preview.** Before any file changes, print a one-screen block (goal, source + git state, mode/branch/base/worktree-path, sandbox, provider, caps, next verbs). `--yes` skips confirm; `--preview` exits 0.
- **Refuse dirty / detached / mid-merge / mid-rebase / no-commits** with specific `try:` lines (rider's table is canonical).
- **Rollback is one command.** Failed/killed leaves branch + worktree for inspection; `abandon <id>` removes both.
- **Lifecycle hints.** Post-`run` prints `apply`/`abandon` + worktree path. Post-`apply` prints `git log -1 --stat`. Every error footer ends with `try: <command>`.

**Phases.** Eleven (P1–P11) in the rider. Each phase: depth test first → implementation → `cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check` green → conventional-commit local commit → CHANGELOG entry. P11 adds a "Codebase Modes" top-level section to AS-BUILT and updates §22.

**Verification.**

- Commands above green on every commit; every rider-named depth test present and passing.
- Default smoke (worktree): in a clean git repo, `deadreckon run "rename Foo to Bar" --yes` creates `dr/rename-foo-to-bar-<id>` with the rename; `apply <id>` squash-merges; `abandon <id>` cleans up.
- Non-git smoke: `deadreckon run "make hello.py" --from . --yes` writes hello.py to the library; `materialize <id> --dest ./hello` exports it.
- In-place smoke: `--in-place --i-know-its-a-lot --yes` edits user's files; `undo` reverts.
- Refusal smoke: dirty / detached / mid-merge / non-git-non-interactive each fail with the rider's `try:` line.
- Fresh smoke (`--fresh`) and `--preview` unchanged.
- No edits outside `/Users/gdc/deadreckon/`. No `git push`. No `PipelineState` schema changes.

**Stop when** verification passes, AS-BUILT updated, CHANGELOG has a "Codebase modes (alpha)" section, committed locally, no invariant violated.
