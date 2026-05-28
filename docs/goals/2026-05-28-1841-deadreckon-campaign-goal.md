GOAL: Let one task spawn N orchestrators. Add `deadreckon campaign <goal> --n <2..=6>`: a **meta-coordinator** that decomposes a root goal into N independent sub-goals, launches each as its own full orchestrator (`orchestrate full-plan`) in an isolated scope, then composes the N merged results into one promoted run. It is the existing `fork`->`merge` pattern lifted exactly one level: each child is a sub-orchestrator whose merged result is itself a normal run, so the meta-merge reuses the same compose primitives. Bounded by a **hard depth cap of 2**, a **tree-wide spend ceiling**, a **cycle guard**, and a **gate-verdict roll-up** so nesting cannot quietly erode the §35 tamper-evident gate. Headline word: **Campaign**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` — §30 plans/orchestration, §35 tamper-evident gate.
- `/Users/gdc/deadreckon/docs/goals/2026-05-28-1841-deadreckon-campaign-rider.md` — full contract.
- `/Users/gdc/deadreckon/crates/deadreckon-core/src/plan.rs`, `.../src/tamper.rs`.
- `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs` — `fork_command`, `run_plan_child`, `merge_command`, `compose_plan_merge_working`, `resolve_plan_result_run`, `kill_command`.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` — invariants hold.

**Posture.** Production-release track. Files-not-fields: meta-plan, lineage, and roll-up are new files under `~/.deadreckon/plans/<campaign-id>/`; **no** `Plan`/`PlanTask`/`PipelineState`/provider schema changes. Child work stays normal `deadreckon` subprocesses (§30.1). Depth **hard-capped at 2** — sub-orchestrators cannot fan out again. No `git push`. Edits inside `/Users/gdc/deadreckon/`. Depth>2, cross-level dependencies/merge-repair, recursive live attach → `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

**User contract.**

- `deadreckon campaign <goal> --n <2..=6>` previews mode, N, depth cap, tree budget, and provider roles, then (with `--yes`) launches N independent sub-orchestrators and composes their results.
- A campaign invoked from inside a sub-orchestrator (depth would reach 2) is **refused**; no meta-plan is written. A sub-goal resolving to an ancestor's `task_key`/scope is **refused** (cycle guard).
- `--max-spend` is a **tree ceiling**: split across sub-orchestrators, aggregate spend tracked across every leaf run; the meta refuses to launch more once exhausted.
- The campaign result reports **clean only if every leaf run signed clean**; any refused leaf fails the campaign, any caveat leaf surfaces a caveat (roll-up over §35 verdicts).
- `attach <campaign-id>` shows the N sub-plan rows with a breadcrumb; `kill <campaign-id>` cascades to sub-coordinators and their children; `show <campaign-id> --why-failed` explains a failed campaign.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implement -> focused tests green -> conventional local commit -> CHANGELOG. P1 lands the depth/cycle/lineage guard (RED first). P11 adds AS-BUILT §36 and logs deferrals.

**Verification.**

- Every rider depth test present and passing; `cargo fmt --check`; `git diff --check`.
- Smoke (smoke provider, `--sandbox none`): a 2-sub-goal campaign launches two sub-orchestrators, composes one promoted result, and `campaign-rollup.json` records both leaf verdicts.
- Smoke: a campaign attempted at depth 2 is **refused** (no meta-plan, clear reason).
- Smoke: one leaf refused (hollow pass) makes the **campaign fail**, never `Completed`; one leaf caveat surfaces a caveat on the meta summary.
- No edits outside the repo; no `git push`; no `Plan`/`PlanTask`/`PipelineState` schema changes.

**Stop when** verification passes, campaign launches N orchestrators within the depth cap and tree budget, the meta-merge produces one promoted run, the gate roll-up refuses on any leaf refusal, attach/kill/why-failed work for campaigns, AS-BUILT and CHANGELOG record the behavior, deferrals are in V1-CANDIDATES, and the work is committed locally.
