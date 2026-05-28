GOAL: Build consolidated documentation for orchestration plan results. Today child docs and task summaries can exist while a finished plan's merged/apply artifact still shows an empty synthetic wrapper: `prepare_plan_result_apply_state` creates a zero-turn `deadreckon:orchestrate-apply` run, `ensure_docs_started` writes templated `RUN-*` docs, and merge/apply copy rules skip `.deadreckon` plus public `docs/RUN-*`. Land a provider-backed plan-doc consolidation pipeline that reads every child run's docs and summaries in task-graph order, writes durable plan-level docs, and carries those docs into result surfaces. Headline word: **Consolidated**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`
- `/Users/gdc/deadreckon/docs/goals/2026-05-28-0943-deadreckon-plan-doc-consolidation-rider.md`
- `/Users/gdc/deadreckon/docs/goals/2026-05-26-1546-deadreckon-narrative-attach-rider.md`
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-2122-deadreckon-doc-depth-rider.md`
- `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs`
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/docs.rs`

**Posture.** Production-release track. This is release-blocking plan-result behavior, not provisional scaffolding. Plan docs remain projections over existing evidence. Avoid `PipelineState`, `Plan`, and provider registry schema changes unless the rider names a files-not-fields alternative. No `git push`. Edits stay inside `/Users/gdc/deadreckon/`. Release-policy questions go to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**User contract.**

- A completed orchestration plan has consolidated `PLAN-NARRATIVE.md`, `PLAN-AS-BUILT.md`, `PLAN-DECISIONS.md`, and child-index docs under the plan dir.
- The merged result library and plan apply/export artifact carry those plan docs in `.deadreckon/docs/` and public `docs/`.
- Synthetic plan-result apply runs no longer look like empty work. Their `RUN-NARRATIVE.md` says they are wrappers and links to the consolidated plan docs.
- Consolidation uses child run docs first, task summaries second, and plan events/worker specs as fallback evidence.
- If a documentation provider is configured, an LLM consolidates the child docs into one plan narrative with citations. If the provider is missing, disabled, over budget, or fails validation, deterministic rollup docs are still produced.

**Provider contract.**

- Use the existing documentation provider routing model; do not invent a separate provider registry.
- Build bounded, redacted input bundles with stable evidence ids for tasks, child runs, docs, files, summaries, plan events, merge repairs, and acceptance.
- Validate provider output before writing it: every concrete claim must cite known evidence, no invented file paths, no uncited child task omissions, and no raw secret-like material.
- Store request/response metadata and fallback reason in files under the plan docs directory.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> focused verification green -> conventional local commit -> CHANGELOG. P11 updates AS-BUILT, README/HOWTO touchpoints if needed, and V1-CANDIDATES for any deferred plan-doc questions.

**Verification.**

- Focused tests cover the `4ca07d2a` pattern: child docs/summaries exist, plan result apply is synthetic, and output docs still tell the whole plan story.
- `deadreckon doc`/`docs`, `show`, `attach`, `merge`, `finish`, `apply`, and `export` surfaces find plan docs for plan ids and plan-result apply runs.
- Provider-backed consolidation and deterministic fallback both produce cited, non-empty plan docs.
- Run `cargo fmt --check`, focused Rust tests for touched modules, and `git diff --check`; avoid full `make verify` unless requested.

**Stop when** focused verification passes, plan result docs are consolidated and materialized, empty synthetic wrapper docs are replaced with cross-linking wrapper docs, AS-BUILT and CHANGELOG record the behavior, and the work is committed locally.
