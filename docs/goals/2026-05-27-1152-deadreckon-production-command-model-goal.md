GOAL: Move DeadReckon's command surface from alpha discovery to a production-facing control model. Today the harness is powerful, but first-screen help and docs expose too many peer verbs before the user has a mental model. This goal lands a small default model - `start`, `attach`, `status`, `list`, `finish`, `doctor`, `kill`, `resume`, `cleanup` - while setup stays near `init`/`def-done` and every other verb stays findable through `help-all`, `<command> --help`, completions, and hints. Headline word: **Production**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`
- `/Users/gdc/deadreckon/docs/goals/2026-05-27-1152-deadreckon-production-command-model-rider.md`
- `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md`
- `/Users/gdc/deadreckon/README.md` and `/Users/gdc/deadreckon/HOWTO.md`
- `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs` and `/Users/gdc/deadreckon/crates/deadreckon/src/cli.rs`
- Prior coherence, guided-experience, start-picker, and provider/done setup riders.

**Posture.** Production-use command-model slice, not a runtime rewrite. No `PipelineState`, plan, run, chain, provider, learning, or config schema changes. Do not delete commands, remove aliases, or break completion. Prefer catalog metadata, shared copy/prompt helpers, docs, and focused tests. No `git push`. Edits stay inside `/Users/gdc/deadreckon/`. Major release-policy questions go to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**Production-facing model.**

- **Begin**: `start "goal"` is the entry point for one-run, follow-up, and multi-agent work.
- **Watch**: `attach latest` explains what is happening; narrative/raw modes remain discoverable there.
- **Orient**: `status` tells the next action; `list` finds runs and plans by project scope.
- **Keep**: `finish latest` owns normal apply/export decisions.
- **Repair setup**: `doctor` is the health check; `init` and `def-done` stay nearby as setup verbs.
- **Control**: `kill`, `resume`, and `cleanup` cover stop, recover, and tidy operations.
- **Continue**: in a repo with DeadReckon history, `start` can extend a completed run or launch a new review/full-plan pass.
- **Done**: every done-criteria prompt shows the current criteria, can evaluate/check it, can update it, and never asks users to accept an opaque gate.

**Discovery contract.**

- Default help should teach the production model, not the whole implementation topology.
- Advanced verbs remain friendly to find: `help-all`, command-specific help, completions, post-action hints, and docs must point to them without making them first-screen peers.
- `run`, `orchestrate`, `chain`, `plan`, `fork`, `merge`, `apply`, `export`, `doc`, `show`, `history`, `import`, `learn`, `improve`, `library`, and compatibility surfaces remain available as power-user or advanced commands.
- README, HOWTO, AS-BUILT, matrix, and help examples agree that DeadReckon is for local production use of supervised agent CLI work, while 0.x caveats are precise and not first-contact copy.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> focused verification green -> conventional local commit -> CHANGELOG. P11 updates AS-BUILT, README/HOWTO, USER-FACING-MATRIX, and V1-CANDIDATES.

**Verification.**

- Focused commands by default: command catalog tests, top-help/help-all snapshots, docs copy assertions, touched output/hint tests, `cargo fmt --check`, and `git diff --check`.
- Smokes: `--help` shows the model with `list`; `help-all` lists advanced verbs; repo-history `start` can extend/orchestrate; done prompts expose view/check/update; completion stays intact.
- Do not run `make verify`, release builds, stress tests, or full-workspace tests by default while executing this goal unless the human explicitly asks.

**Stop when** focused verification passes, default help is simple enough for production first contact, all other verbs remain findable, docs and AS-BUILT record the model, and the work is committed locally.
