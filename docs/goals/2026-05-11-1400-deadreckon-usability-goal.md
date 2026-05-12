GOAL: Close deadreckon's lifecycle gap — add `materialize` and `extend`.

Two ergonomic holes: (1) completed artifacts stay trapped in `/Users/gdc/.deadreckon/library/<scope>/<run-id>/` with no way to land at a user path; (2) `resume` refuses completed runs (`main.rs:947`), so a finished app can't be extended with a new prompt. Add two CLI verbs, **no schema changes**, wire into list/show/post-run hints. Stop when the rider's verification passes and prior invariants hold.

**What you're building.**

- `deadreckon materialize <run-id> [--dest <path>] [--force] [--include-manifest]` — copy `library/<scope>/<run-id>/` to user dest; refuse non-empty dest without `--force`; write `.deadreckon/parent.json` + reverse marker in library.
- `deadreckon extend <run-id> "<new-goal>" [--dest <path>] [--max-context-turns N] [--no-context]` — mint a new run in parent's scope/task-key, seed `working/` from parent library, prepend parent-summary to history, enter turn loop with new goal. Parent must be `Completed`.
- **No new `PipelineState` fields.** Parent lineage lives in `working/.deadreckon/parent.json` + a synthetic first `traces.jsonl` entry.
- `list` annotates `MATERIALIZED` from `.materialized-to` markers. `show` reveals lineage. `run`/`attach` print hint lines after completion (suppress with `--no-hints`).

**References — read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-05-11-1400-deadreckon-usability-rider.md` — signatures, schemas, summary format, test names, README diff.
- Prior riders hold: `2026-05-10-1400-deadreckon-build-rider.md`, `2026-05-11-1400-deadreckon-primary-flow-rider.md`, `2026-05-11-1400-deadreckon-robust-rider.md`.
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/{state.rs,promotion.rs,artifacts.rs}` — `PipelineState`, promotion, `copy_tree`.
- `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs` — CLI verbs, resume guard at line 947.
- `/Users/gdc/deadreckon/crates/deadreckon/tests/agentic_loop.rs` — integration test pattern.

**Phase plan — commit locally each boundary; no `git push`.**

1. **`materialize`.** Copy `library/<scope>/<run-id>/` → `--dest` (default `./<run-id-prefix-8>`). Refuse non-empty without `--force`. Write `.deadreckon/parent.json` + library `.materialized-to`. Reuse `copy_tree`.
2. **`extend`.** Via existing `create_run`; same scope/task_key as parent. Copy parent library into new `working/` (skip manifest + `.materialized-to`). Prepend parent-summary to `history.json`; emit synthetic `traces.jsonl` entry. Refuse if parent ≠ `Completed`. Reset resource caps.
3. **Integration.** `list` `MATERIALIZED` column. `show` prints `Extended from <parent-id>` when lineage file present. `run` + `attach` emit hint lines on completed runs (`--no-hints` suppresses). Both verbs under Lifecycle in `--help`.
4. **Docs.** `## Lifecycle` in README (init → run → list → attach → materialize → extend) with three-block example. Update `DESIGN.md` CLI. CHANGELOG per phase.
5. **Verify.**

**Verification.**

- `cd /Users/gdc/deadreckon && cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`.
- New `crates/deadreckon/tests/lifecycle.rs` tests (rider names them) pass — including `materialize_then_extend_roundtrip`, `extend_refuses_incomplete_parent`, `materialize_refuses_dest_inside_runstate`.
- `deadreckon --help` lists `materialize` + `extend` under Lifecycle.
- After completion, stdout has both `materialize:` and `extend:` lines (unless `--no-hints`).
- `grep "^pub struct PipelineState" /Users/gdc/deadreckon/crates/deadreckon-core/src/state.rs -A 40` shows **no new fields**.
- All prior rider invariants pass.
- No `git push`.

**Checkpoints.** Each phase: verify, commit locally, write one progress line (phase, verified, remaining, blockers). Conflicts to `GAP-ANALYSIS.md`.

**Stop when:** both verbs end-to-end, named tests pass, list/show/hints wired, README + DESIGN updated, prior invariants hold.
