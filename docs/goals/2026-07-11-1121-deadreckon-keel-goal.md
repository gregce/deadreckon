GOAL: Lay a keel under the ledgers — one pure protocol crate that every writer and reader shares. Today a run's truth is five parallel JSONL files (events, spend, traces, flight, narrative snapshots) whose line types live scattered across deadreckon-core with per-file ad-hoc schemas; Logbook's `RunView` exists precisely because those files drift, and every new surface re-learns the join. codex-rs shows the discipline: a pure protocol crate (no I/O, no tokio) at the bottom of the workspace, one `#[serde(tag)]` tagged union for everything a session persists (`RolloutItem`), schema generation from the same definitions (schemars), wire-rename aliases for evolution, and persistence policy isolated in one module instead of scattered ifs. This slice lands `deadreckon-protocol`: the ledger line types unified as one `LedgerItem` union, generated JSON Schemas checked into the repo (doc==code for the wire), a `policy.rs` that owns what-gets-persisted decisions, and writers/readers rewired through it — file layout unchanged, bytes unchanged, guarded by characterization goldens. Land this slice named Keel.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-07-11-1121-deadreckon-keel-rider.md` — crate layout, LedgerItem union, schema-gen harness, behavior-preservation rules, eleven phases, depth tests.
- `crates/deadreckon-core/src/{events.rs,state.rs,flight.rs,tamper.rs,run_view.rs}` — today's line types (`RunEvent`, `SpendRecord`, `TraceRecord`, flight rows) and their readers.
- `/Users/gdc/codex/codex-rs/protocol/src/protocol.rs` (`RolloutItem`, `EventMsg` tagging discipline), `rollout/src/policy.rs`, `app-server-protocol/src/export.rs` (schema generation).
- `docs/AS-BUILT-ARCHITECTURE.md` §33 (flight), §49 (Logbook/RunView). Prior riders hold; Keel takes §52.

**Posture.** Stable track. **Byte-identical on disk**: this slice moves TYPE DEFINITIONS, not file layout — the five JSONL files keep their paths, field names, and serialized forms; existing runs remain readable; characterization goldens and `show/verdict/report` outputs must not move. `deadreckon-protocol` depends on serde + schemars + chrono only — no tokio, no I/O, no deadreckon crates. Schema files are generated into `docs/schemas/*.json` by a test that fails on drift (doc==code, enforced). Consolidating the five files into one physical ledger is explicitly V1. No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → V1-CANDIDATES.

**The spine.**

- `LedgerItem` — one tagged union over every persisted line kind (`Event`, `Spend`, `Trace`, `Flight`, `NarrativeSnapshotRef`, …) with `#[serde(other)] Unknown` tolerance and alias-based rename support; per-file newtype wrappers keep today's on-disk forms exact.
- Schema generation: `cargo test -p deadreckon-protocol` regenerates and diffs `docs/schemas/` — a stale schema is a red test, the same doctrine as the friendliness contract table.
- `policy.rs` — the single answer to "does this item persist, and where": today's implicit rules made explicit and depth-tested.
- Rewire: core writers (`append_json_line` call sites) and readers (RunView, attach tails, history grep) consume the protocol types; `RunView`'s five-way join becomes a projection over one vocabulary.

**Phases.** Eleven (P1–P11) in the rider. Each: depth test first → implement → `make verify` green → conventional-commit → CHANGELOG. P11 adds AS-BUILT §52.

**Verification.**

- Every rider depth test present and passing; `make verify` green each commit; characterization goldens byte-identical throughout.
- A pre-Keel fixture run's five ledgers parse identically through the new types (round-trip test on recorded fixtures).
- `docs/schemas/*.json` regenerate clean; deleting one fails the drift test.

**Stop when** verification passes, AS-BUILT §52 + V1-CANDIDATES + a `Keel (stable)` CHANGELOG section are updated, committed locally.
