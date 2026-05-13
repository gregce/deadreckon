GOAL: Extend deadreckon at `/Users/gdc/deadreckon/` from single-run harness into an alpha multi-agent orchestrator. Support **split mode** (one goal -> provider-planned child runs -> merge -> final gate) and **review mode** (one provider codes, another reviews/fixes the result). Provider assignment is explicit: planner, default child, per-child, coder, and reviewer providers are previewed and overridable. Headline word: **Orchestrated**.

**Read first.**

- `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` - turn loop, gate, promotion, locks, scopes, TUI.
- `/Users/gdc/deadreckon/docs/goals/2026-05-11-1444-deadreckon-orchestrate-rider.md` - schemas, provider roles, modes, signatures, tests.
- Prior riders in `/Users/gdc/deadreckon/docs/goals/` - invariants hold.

**Posture.** Stays `alpha`. No `PipelineState` schema changes; plan, child lineage, provider roles, and coordinator state live in files. No `git push`. Edits stay inside `/Users/gdc/deadreckon/`. V1 decisions -> `docs/V1-CANDIDATES.md`.

**Core idea.**

- **Split mode:** `plan` decomposes a goal into a task graph with providers, dependencies, worker specs, capability requests, and done criteria. `fork` runs ready child tasks through the existing turn loop in isolated scopes/worktrees. `merge` composes accepted artifacts, gates again, and promotes the merged artifact.
- **Review mode:** `orchestrate "goal" --mode review --coder-provider cli:claude-code --reviewer-provider cli:codex` runs one normal coding task, then launches a fresh reviewer lane via `extend` to inspect, write `.deadreckon/REVIEW.md`, apply review fixes when allowed, and gate the reviewed artifact.
- A child is still just a normal deadreckon run plus `parent_plan_id`, `task_id`, `child_index`, `plan_role`, provider metadata, and a self-contained `worker-spec.md`.
- Coordinator messages are typed files (`progress`, `blocker`, `review_request`, `review_response`, `capability_request`, `shutdown_*`); no arbitrary child-to-child chat.

**New verbs.**

- `orchestrate <goal>` - one-command wrapper for split or review mode.
- `plan <goal>` - writes `plan.json`; accepts `--planner-provider`, `--provider`, `--child-provider`, `--coder-provider`, `--reviewer-provider`.
- `fork <plan-id>` - spawns split children or coder->reviewer lane.
- `attach <plan-id>` - multi-pane TUI; Enter drills into one child.
- `merge <plan-id>` - gates/promotes merged or reviewed output.
- `kill <plan-id>` - cascades to coordinator + children.
- `history grep <pattern>` and `show <id> --why-failed` - plan-aware inspection.

**Ergonomics.**

- Preview prints resolved providers, task graph, planned working dirs, and requested capabilities before work starts.
- Review mode is the common path for "Claude codes, Codex reviews" without requiring decomposition.
- TUI shows each child task, role, provider, dependency state, status, spend/context, latest activity, summaries, and final gate state.
- Every refusal has a `try:` line; `--quiet`/`--plain` work for headless use.

**Phases.** Eleven (P1-P11) in the rider. Each: depth test first -> implementation -> targeted verification or full verification when practical -> conventional local commit -> CHANGELOG. P11 updates AS-BUILT and §22.

**Verification.**

- Every rider-named depth test present and passing.
- Split smoke: `plan "tiny hello rust in two files" --n 2 && fork <plan-id> && attach <plan-id>` shows two panes; children complete; `merge <plan-id>` promotes a merged artifact.
- Review smoke: `orchestrate "tiny hello rust" --mode review --coder-provider smoke:coder --reviewer-provider smoke:reviewer --plain` creates coder + reviewer runs and promotes the reviewed artifact.
- Single-run smoke: `run "tiny hello rust" --plain --quiet` unchanged.
- No edits outside `/Users/gdc/deadreckon/`. No `git push`.

**Stop when** verification passes, AS-BUILT updated, CHANGELOG has an "Orchestration milestone (alpha)" section, and work is committed locally.
