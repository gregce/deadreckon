GOAL: Add a selection-first interactive picker to `deadreckon start`. Today `start` is useful, but TTY users still mostly accept heuristics or type flags, and the only true prompt is a numeric non-git source fallback. This goal makes the happy path feel like a normal terminal wizard: choose launch shape, provider route, done-criteria action, source mode, and final confirmation from clear labels, while non-TTY behavior stays deterministic and script-safe. Headline word: **Picker**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - CLI and guided-start architecture.
- `/Users/gdc/deadreckon/docs/goals/2026-05-27-1032-deadreckon-start-picker-rider.md` - picker contract, phases, and depth tests.
- `/Users/gdc/deadreckon/docs/goals/2026-05-26-1510-deadreckon-guided-experience-rider.md` - prior selection-first posture.
- `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs`, `/Users/gdc/deadreckon/crates/deadreckon/src/cli.rs`, `/Users/gdc/deadreckon/crates/deadreckon/src/setup.rs`, `/Users/gdc/deadreckon/crates/deadreckon/src/prompt.rs`, and `/Users/gdc/deadreckon/crates/deadreckon/src/ui.rs`.
- Prior coherence, provider/done setup, orchestration event bus, and guided-experience riders - their output and safety invariants hold.

**Posture.** Stays `alpha`. No `PipelineState`, plan, run-state, or config schema changes except a prompt dependency entry. No durable launch profiles, LLM mode classification, provider-specific setup wizard, cloud state, or `git push`. `start` remains a thin decision layer over existing `run` and `orchestrate`. Do not make `start` an alternate-screen TUI.

**Interactive contract.**

- TTY `deadreckon start "<goal>"` presents a normal terminal picker unless explicit flags or `--yes` make the choice unambiguous.
- `--preview` may ask TTY users for picker choices, then prints a state-free preview. `--json`, `--plain`, `--quiet`, and non-TTY paths never prompt.
- Pickers use human labels first and internal route/mode ids second. Defaults appear first with a short reason.
- Provider choices can be used ephemerally for the launched run/plan; writing config requires an explicit confirmation or existing config command path.
- Every mutation is previewed before dispatch and has an equivalent manual `try:` command.

**Picker surfaces.**

- **Mode** - recommended path, single run, review orchestration, full-plan orchestration.
- **Provider** - configured default, detected ready CLI providers, configured routes, typed advanced route, or cancel.
- **Done criteria** - existing project criteria, create from goal through existing `def-done` flow, write manually, default gate where already supported, or cancel.
- **Source** - worktree, init git, copy current directory, fresh workspace, allow-dirty recovery, or cancel.
- **Confirm** - final preview with who works, where it runs, how done is checked, and watch/stop/finish commands.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> focused verification green -> conventional local commit -> CHANGELOG. P11 updates AS-BUILT, README/HOWTO if user-visible copy changes, and V1-CANDIDATES for anything intentionally deferred.

**Verification.**

- Focused commands only by default: parser/help snapshots, `start_` unit tests, fake-prompter tests, PTY smokes, and JSON/plain/quiet non-prompt tests.
- Smokes: TTY picker chooses full-plan preview; TTY picker chooses detected provider without preconfiguring it; non-TTY missing setup refuses with `try:` lines; `--json` never emits prompt text.
- Do not run `make verify`, release builds, broad smoke suites, stress tests, or full-workspace tests by default while executing this goal unless the human explicitly asks.

**Stop when** focused verification passes, interactive start can be driven by selection instead of typed flags, non-interactive contracts are unchanged, docs record alpha limits, and the work is committed locally.
