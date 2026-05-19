GOAL: Make deadreckon runs behave more like `Implement <SPEC>` work by converging live implementation notes with `RUN-DECISIONS.md`. Today `RUN-DECISIONS.md` is a retrospective, heuristic doc: it filters for multi-alternative decisions and often says no decisions were detected, even when the implementation made important spec interpretations, deviations, tradeoffs, or left open questions. This goal evolves `RUN-DECISIONS.md` into the canonical implementation decision ledger, backed by a live `implementation-notes.html` working artifact while code is being changed. Headline word: **Interpretation**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - sections 3, 6, 13, 17, 25, 30.
- `/Users/gdc/deadreckon/docs/goals/2026-05-18-2336-deadreckon-implementation-notes-rider.md` - implementation contract.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-1525-deadreckon-self-documenting-rider.md` - original run-doc invariants.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-2122-deadreckon-doc-depth-rider.md` - split doc-polish and narrator-decision invariants.
- `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/turn_loop.rs`
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/docs.rs`
- `/Users/gdc/deadreckon/crates/deadreckon/src/{cli.rs,main.rs}`
- `/Users/gdc/deadreckon/skills/{default-coding,narrator-decisions}/SKILL.md`

**Posture.** Stays `alpha`. No `PipelineState` schema changes. Do not rename `RUN-DECISIONS.md`; evolve it so it includes implementation interpretation sections while still preserving evidence-filtered multi-alternative decision details. No new top-level CLI verb. No provider-owned transcript rewrites. No `git push`. Edits stay inside `/Users/gdc/deadreckon/`. V1-scale decisions go to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**Core deliverables.**

- A spec-first prompt frame: "Implement the spec" means the user goal plus `acceptance.md`/`acceptance.yaml` copied into the run root.
- A seeded, agent-maintained `/implementation-notes.html` in the run working tree with four required sections: Design decisions, Deviations, Tradeoffs, Open questions.
- `RUN-DECISIONS.md` rendered with the same four sections, plus a separate evidence-filtered "Multi-alternative decision details" section.
- A freshness check that refuses `done` when documentable code changed after the latest notes update.
- CLI-subagent and JSON-action provider parity: both receive the same notes contract and same done-time check.
- `deadreckon doc <run-id> --kind decisions` becomes the primary inspection path for these notes; `--kind implementation-notes` may remain a direct HTML convenience if implemented.
- `narrator-decisions` can use implementation notes as source evidence for the four interpretation sections, while keeping real decision extraction evidence-filtered.

**Verification.**

- Depth tests first for every rider phase; keep tests focused on self-documenting run, prompt, and doc surfaces.
- Prefer: `cargo test -p deadreckon self_documenting`, `cargo test -p deadreckon agentic_loop`, `cargo test -p deadreckon doc_kind`, `cargo test -p deadreckon-runtime implementation_notes`, `cargo fmt --check`, and targeted `cargo clippy -p deadreckon --all-targets -- -D warnings` when code changed.
- Smoke: a fixture run that changes a source file but not `implementation-notes.html` cannot complete; after updating the notes with all four sections it can complete and `deadreckon doc latest --kind decisions` prints Design decisions, Deviations, Tradeoffs, Open questions, and Multi-alternative decision details.
- No edits outside `/Users/gdc/deadreckon/`. No `git push`. No `PipelineState` schema changes.

**Stop when** the prompt contract, seeded HTML artifact, freshness gate, converged `RUN-DECISIONS.md` rendering, focused verification, AS-BUILT/CHANGELOG updates, and local conventional commit are complete.
