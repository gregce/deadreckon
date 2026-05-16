GOAL: Make orchestration merge failures semantically repairable instead of forcing the user to choose a raw child artifact. The `aa20e565` flight-sim plan showed the current limit: all children completed, then `merge` failed on `src/entities/airplane.js` between child 0 and child 1 even though child 1 depended on and extended child 0. Land DAG-aware merge plus automatic planner-mediated repair for true conflicts. Headline word: **Integrative**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - sections 30, 32, 22.
- `/Users/gdc/deadreckon/docs/goals/2026-05-16-1122-deadreckon-semantic-merge-repair-rider.md` - repair flow and depth tests.
- `/Users/gdc/deadreckon/docs/goals/2026-05-15-2252-deadreckon-plan-events-rider.md` - plan lifecycle/event invariants.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-1444-deadreckon-orchestrate-rider.md` - plan model, worker specs, coordinator.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` - invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes. Avoid `Plan` schema expansion unless unavoidable; merge repair state lives under `~/.deadreckon/plans/<plan-id>/merge-proofs/`. Child execution remains normal `run`/`extend`. No `git push`. Edits inside `/Users/gdc/deadreckon/`. V1 decisions -> `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**Core idea.**

- Merge should understand the plan DAG before declaring a conflict. If task B depends on task A, B's changed version of the same file can supersede A's ancestor version when safe.
- True conflicts become a structured merge-repair problem, not a terminal dead end: collect child versions, dependency context, worker specs, summaries, and latest plan events into a conflict bundle.
- A planner provider can produce a deterministic repair decision: prefer descendant, prefer child with rationale, synthesize file, spawn a repair child, or refuse with a concrete reason.
- Repair execution is a normal run from `merge-working`, with the conflict bundle and a precise integration goal. The repair output is gated, summarized, recorded, and then merge is retried automatically.
- Default safety is bounded automatic repair: plan-local writes only, conflict-path synthesis only, one attempt by default, rationale recorded, and `--no-repair` for raw conflict failure.

**User experience.**

- `merge <plan-id>` runs DAG merge, then automatically plans/executes semantic repair for true conflicts when a provider is available.
- If automatic repair cannot safely proceed, it fails with the conflicting path, repair rationale, and the exact artifact paths to inspect.
- `attach <plan-id>` and `show <plan-id> --why-failed` show merge repair status, repair run id, conflict paths, and latest rationale.
- `orchestrate ...` carries repair through in the same command; no manual repair command is required for normal users.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> focused tests green; milestone boundaries run `cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`; conventional local commit -> CHANGELOG. P11 updates AS-BUILT sections 30/32 and 22 if a thin item closes.

**Verification.**

- Every rider depth test present and passing.
- DAG smoke: child 1 depending on child 0 edits the same file; merge keeps child 1 without manual `prefer-child`.
- Repair smoke: two parallel children edit the same file; `merge` creates a repair run, records rationale, retries merge, and promotes without a manual follow-up.
- Failure smoke: planner refuses unsafe repair; plan records a clear repair failure and leaves child artifacts intact.
- No edits outside `/Users/gdc/deadreckon/`. No `git push`. No `PipelineState` schema changes.

**Stop when** verification passes, AS-BUILT and CHANGELOG describe "Semantic merge repair (alpha)", V1 deferrals are logged, and the work is committed locally.
