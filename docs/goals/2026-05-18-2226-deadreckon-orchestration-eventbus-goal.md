GOAL: Land the V1 orchestration live-UX slice: make `plan`, `fork`, `merge`, and `orchestrate` read as one family, and move plan attach onto a shared plan event stream. The primitives are already present: `plan-events.jsonl`, child drill-down, provider-role vocabulary, merge repair events, and coherence helpers. This goal connects them through shared builders and a `PlanEventBus` abstraction so users see provider roles, dependencies, parallelism, repair state, and fresh plan/child/repair logs without accidental footer/render drift. Headline word: **Live**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - §§18, 26, 30, 32.
- `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md` - O1-O7 deferrals.
- `/Users/gdc/deadreckon/docs/goals/2026-05-18-2226-deadreckon-orchestration-eventbus-rider.md` - implementation contract.
- `/Users/gdc/deadreckon/docs/goals/2026-05-15-2252-deadreckon-plan-events-rider.md` - existing plan event model.
- `/Users/gdc/deadreckon/docs/goals/2026-05-17-1403-deadreckon-coherence-closure-rider.md` - vocabulary, streams, and style invariants.
- `/Users/gdc/deadreckon/crates/deadreckon/src/{main.rs,ui.rs,cli.rs,tui_events.rs}`
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/{plan.rs,events.rs}`

**Posture.** Stays `alpha`. No `PipelineState` schema changes. Do not rename stored `Plan`/task status variants. Keep `plan-events.jsonl` as the durable source of truth; the bus is a runtime stream adapter over existing JSONL replay/tail plus in-process broadcast. No persistent event DB. No `git push`. Edits stay inside `/Users/gdc/deadreckon/`. V1-scale decisions go to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**Core deliverables.**

- Shared orchestration preflight/result builders for `plan`, `fork`, `merge`, and `orchestrate`.
- Provider role tables with role, route, model, and source when known.
- Dependency and parallelism summaries that say which children can start now and which wait.
- Merge repair summaries that show mode, attempts, provider, conflict paths, repair run id, and next action.
- Standard plan attach footer/breadcrumb grammar aligned with run and chain attach.
- `PlanEventBus`/feed API that replays durable plan events, streams live plan events, and multiplexes child and repair run events for attach.

**Verification.**

- Depth tests first for every rider phase; keep tests focused on the touched surface.
- Prefer: `cargo nextest run -p deadreckon --test orchestrate`, `cargo nextest run -p deadreckon --test coherence`, `cargo test -p deadreckon attach_plan`, `cargo test -p deadreckon plan_event_bus`, `cargo test -p deadreckon-core plan_event`, `cargo fmt --check`, and targeted `cargo clippy -p deadreckon --all-targets -- -D warnings` when code changed.
- Do not run `make verify`, release builds, smoke, stress, or full-workspace tests by default. Use them only if the implementation expands beyond this surface or the user explicitly asks.

**Stop when** the shared builders and event bus are implemented, focused verification is green, AS-BUILT/CHANGELOG/USER-FACING-MATRIX/V1-CANDIDATES are updated, and the work is committed locally.
