GOAL: Make codebase-aware running the **default** mode of deadreckon at `/Users/gdc/deadreckon/`. Today `deadreckon run` starts the agent in an empty `working/` dir (`state.rs:178-184`); the agent never sees the user's project. After: in a git repo, the default is a fresh `git worktree` off the current branch; outside git, the binary offers `git init` / copy / cancel; today's empty-dir flow stays behind `--fresh`. **Friendliness is the headline** — one command, one preview, one confirmation, one apply or abandon.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — substrate; only working-dir-seeding changes. `PipelineState` unchanged (mode metadata in `working/.deadreckon/codebase.json` per files-not-fields).
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-1502-deadreckon-codebase-rider.md` — mode resolution, schemas, signatures, preview, depth tests.
- `/Users/gdc/stoa/docs/research/2026-05-10-unmet-needs/REPORT.md` — primary needs #3, #2.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes. No `git push`. Edits inside `/Users/gdc/deadreckon/`. V1 decisions → `docs/V1-CANDIDATES.md`.

**Four modes, auto-resolved.**

- **`worktree` (default in a git repo).** `git worktree add ~/.deadreckon/worktrees/<scope>-<id>` on branch `dr/<task-slug>-<id>` off `--base` (default current branch). `working_dir` = the worktree.
- **`copy` (default outside git).** `.gitignore`-aware copy into runstate `working_dir`. `materialize` writes back.
- **`in-place`.** `working_dir = <source-path>`. Agent edits user's tree directly. Requires `--in-place --i-know-its-a-lot`; snapshots in runstate so `undo` works.
- **`fresh`.** Today's empty-working-dir behavior; preserved for existing tests.

**New verbs.**

- `apply <run-id> [--strategy merge|squash|cherry-pick] [--no-confirm]` — merges the run's branch into the user's current branch (default `squash`; worktree only).
- `abandon <run-id> [--keep-branch] [--force]` — removes worktree + branch; idempotent.

**Friendliness as a verifiable contract.**

- **Auto-detect** in a git repo (no prompt). Outside (TTY): offer `git init` / copy / cancel. Non-interactive without a mode flag refuses with `try:` line.
- **Preflight + preview.** Single-screen block (goal, source + git state, mode/branch/base/wt-path, sandbox, provider, caps, next verbs) before any file change. `--yes` skips; `--preview` exits 0.
- **Refuse dirty / detached / mid-merge / mid-rebase / no-commits** with `try:` lines (rider table is canonical).
- **Rollback is one command.** Failed/killed leaves branch + worktree; `abandon <id>` removes both.
- **Lifecycle hints.** Post-`run`: `apply`/`abandon` + wt path. Post-`apply`: `git log -1 --stat`. Errors end with `try: <command>`.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check` green → conventional-commit → CHANGELOG. P11 adds a "Codebase Modes" section to AS-BUILT and updates §22.

**Verification.**

- Commands green on every commit; every rider depth test present and passing.
- Worktree smoke (clean git repo): `run "rename Foo to Bar" --yes` makes the branch + rename; `apply <id>` squash-merges; `abandon <id>` cleans up.
- Copy smoke: `run "make hello.py" --from . --yes` writes hello.py to library; `materialize <id> --dest ./hello` exports.
- In-place smoke: `--in-place --i-know-its-a-lot --yes` edits user's files; `undo` reverts.
- Refusal smoke: dirty / detached / mid-merge / non-git-non-interactive each fail with rider's `try:` line.
- Fresh + preview unchanged. No edits outside `/Users/gdc/deadreckon/`. No `git push`. No `PipelineState` schema changes.

**Stop when** verification passes, AS-BUILT updated, CHANGELOG has a "Codebase modes (alpha)" section, committed locally, no invariant violated.
