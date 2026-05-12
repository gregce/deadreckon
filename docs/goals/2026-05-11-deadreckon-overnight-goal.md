GOAL: Polish deadreckon at `/Users/gdc/deadreckon/` for true overnight operation. Today an unattended `run` survives flakes but the surrounding UX is bare — no caffeinate (laptop sleeps mid-turn), a plain-text preview, a silent "completed" line, and globally-signed per-turn commits can hang on pinentry. This goal lands a card-shaped CLI vocabulary (preview / exit / status / show / list), `--prevent-sleep` on macOS + Linux, and unattended-git hardening so a run started before bed is still a run in the morning. Headline word: **Overnight**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — substrate; especially §17 CLI, §18 TUI, §22 thin.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-deadreckon-overnight-rider.md` — card primitives, schemas, depth tests, sleep-handshake shape, phase plan.
- gnhf exemplars: `https://github.com/kunchenguid/gnhf` (`src/core/{exit-summary,sleep,git}.ts`) — aesthetic + systemd-inhibit handshake to mine.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold; doctor + status polish lightly overlap audit-harden P7/P10 (rider says how to land non-conflicting).

**Posture.** Stays `alpha`. No `PipelineState` schema changes — sleep state in `working/.deadreckon/sleep-prevention.json`. No `git push`. Edits inside `/Users/gdc/deadreckon/`. Major decisions → `docs/V1-CANDIDATES.md`.

**Two pillars: caffeinate + cards.**

- **`--prevent-sleep <auto|on|off>`.** Defaults on under TTY, off under non-TTY, config-overridable. macOS spawns `caffeinate -di` for the run lifetime; Linux re-execs under `systemd-inhibit` with a tmpfs ready-path handshake (trusted-path checked both sides); Windows `doctor` reports unsupported. `sleep-prevention.json` records mode+pid; reap on every exit path.
- **One card renderer for every user-facing surface.** `ui_card` primitive — ANSI-safe length, terminal-width capped, `--plain` strips color and box-drawing, golden-file pinned. Pre-run **preview card** (mode/branch/base/worktree/provider+model/caps/sleep); end-of-run **exit summary card** (turns, spend with `~` when any turn subscription/estimated, files added/updated/deleted, snapshots, gate status, attach+show+apply hints). `status`, `show`, `list`, and the attach completion footer converge onto the same primitives.

**Unattended-git hardening.** Every `git` invocation in `deadreckon-core` and `deadreckon` routes through one helper that exports `GIT_TERMINAL_PROMPT=0` and inserts `-c commit.gpgsign=false -c tag.gpgsign=false` on commit-family verbs. No pinentry hangs on globally-signed checkouts. A depth test greps for raw `Command::new("git")` outside the helper.

**Friendliness.** Auto-detect TTY for default sleep prevention; preview before any state change; refuse with `try: <command>` on every footer; rollback is one command (`undo`/`discard`). `--plain` + `NO_COLOR` strip to ASCII; <40-col terminals fall back without crashing.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check` green → conventional-commit → CHANGELOG. P11 adds §26 "Overnight UX" to AS-BUILT, updates §22.

**Verification.**

- Commands green every commit; every rider depth test present and passing.
- Card smoke: a smoke run prints preview + exit cards; both pass ANSI-stripped width checks and contain `attach`/`show`/`apply` next-step commands.
- Caffeinate smoke (macOS): `pgrep -P <deadreckon-pid> caffeinate` returns a PID during the run and zero after.
- Gpgsign smoke: a per-turn worktree commit succeeds under a fake `gpg` wrapper that would otherwise block on pinentry.
- No edits outside `/Users/gdc/deadreckon/`. No `git push`. No schema changes.

**Stop when** verification passes, AS-BUILT updated, CHANGELOG has an "Overnight UX (alpha)" section, committed locally.
