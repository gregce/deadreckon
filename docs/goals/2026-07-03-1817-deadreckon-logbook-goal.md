GOAL: Consolidate every way you read a run behind one model — so `show`, `verdict`, `doc`, a new static `report`, and the Helm attach TUI all project the same picture, and no run file is unreadable. Today each read command parses raw JSONL on its own: seven artifacts have no reader at all (the actual diff, `sandbox.toml`, the per-turn model exchange in `history.json`, `events.jsonl`), and a diff a narrative summarises is derived differently from git, so facts drift. This slice lands a `RunView` read model that owns the join across a run's files, rewires the single-fact commands as projections of it, adds a per-turn diff primitive over snapshots, ships `deadreckon report` as one self-contained artifact, and gives every file a reader via `show --diff/--turn/--raw`. Helm keeps its live surface; it now reads the shared model. Land this slice named Logbook.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-07-03-1817-deadreckon-logbook-rider.md` — RunView schema, per-turn diff, verb signatures, eleven phases, depth tests.
- `crates/deadreckon-core/src/{paths.rs,artifacts.rs,events.rs,tamper.rs,gate.rs}` — `DeadreckonPaths::run_root`, `snapshot_working`/`restore_snapshot` (the snapshots the diff reads), evidence ledgers.
- `crates/deadreckon/src/commands/{inspection.rs,verdict.rs,doc.rs}` — `show`, `verdict`, `doc` handlers to rewire as projections.
- `crates/deadreckon/src/narrative.rs` — Helm's `pub(crate)` projection types; the model to lift into core and share.
- `docs/AS-BUILT-ARCHITECTURE.md` §33/§35 (flight recorder, tamper gate), §44/§45 (narrator), §47 (Helm); `docs/UNMET-NEEDS-OPTIONS-2026-07-01.md` C2/C3/F2; `docs/V1-CANDIDATES.md`. Prior riders hold; Helm claims §47, Contract §48, Logbook takes §49.

**Posture.** Stable track (0.5.0). RunView is a read-only projection — no `PipelineState` schema changes, no new check kinds, state stays files under `run_root`. The per-turn diff excludes build output (`target/`). Behavior-preserving rewires are guarded by characterization goldens. No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → V1-CANDIDATES.

**One model, many projections.**

- `RunView` owns the join: verdict, changed files, why, per-turn timeline, proof — assembled from run files, degrading (not panicking) when one is absent.
- `show`, `verdict`, `doc` become projections of RunView; the diff in `show` and the diff in `report` are the same computed value.
- Helm attach reads RunView instead of tailing raw JSONL directly — one picture, live or static.

**Close the blind files.**

- Per-turn diff primitive over snapshots powers `show --diff` (full run) and `show --turn <N>` (one turn).
- `show --turn <N>` surfaces that turn's diff, model exchange (`history.json`), and sandbox events.
- `show --raw <artifact>` dumps any run file; `--json` parity everywhere; `history grep --kind events`.

**New surface.**

- `deadreckon report <run> [--html] [--dest <p>]` — one self-contained artifact (five bands), archivable and shareable (UNMET-NEEDS C3).

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `make verify` green → conventional-commit → CHANGELOG. P11 adds AS-BUILT §49.

**Verification.**

- Every rider depth test present and passing; `make verify` green each commit; characterization goldens unchanged where behavior is preserved.
- `show --diff` on a fixture run prints the source diff with `target/` excluded; `show --turn 1 --raw sandbox.toml` reaches both; `report --html` writes one file with no external refs.
- `verdict` and `show` derive the same signature/tamper facts from RunView; attach timeline turn count equals RunView turns.

**Stop when** verification passes, AS-BUILT §49 + V1-CANDIDATES + a `Logbook (stable)` CHANGELOG section are updated, committed locally.
