GOAL: Make `start` the only launch decision a user ever needs — the harness plots the course. Today the operator picks a verb (run/orchestrate/campaign), `start` softens it with one advisory text-only classifier, campaign over-asks `--n`, launches are text-first, and the decision dies in an ephemeral struct spread across three files. This slice inverts ownership, query-planner style: a deterministic signal bundle (goal structure, the DETECTED done contract, workspace shape, task history, budget fit) plus one grounded provider call resolves a typed, durable `launch-plan.json`; a "course card" previews WHAT/WHO/COST/DONE/ESCAPE; the only question `start` may ask is "how will you know it worked?" (and only when contract detection is Unknown); dispatch executes the plan file; the shape can be corrected mid-voyage (plan collapse, checkpoint-gated escalation). Land this slice named Course.

**Read first.**

- `/Users/gdc/deadreckon/docs/goals/2026-07-01-2010-deadreckon-course-rider.md` — plan schema, signal rules, card spec, fourteen phases, depth tests.
- `crates/deadreckon/src/commands/start.rs` — `StartLaunchDecision`, `GoalShape*`, `classify_goal_shape_for_start`, `dispatch_start_command`.
- `crates/deadreckon-core/src/acceptance_defaults.rs` — `detect_project_kind`, `default_checks_for` (the contract signal).
- `crates/deadreckon-core/src/{plan.rs,campaign.rs,chain.rs}` + `commands/orchestrate.rs` — shapes being planned over.
- `docs/AS-BUILT-ARCHITECTURE.md` §26/§30/§36/§37; `docs/FRIENDLINESS-AUDIT.md` (campaign/chain auto-detect cells fail); `docs/V1-CANDIDATES.md`. Prior riders hold.

**Posture.** Stable track (0.4.0). No `PipelineState` schema changes — the plan is a file (`launch-plan.json`), not fields. Shape resolution is deterministic-first with ONE clamped provider call; the planner NEVER launches campaign-scale spend without confirmation above the guardrail. Existing verbs stay callable (start is the front door, not a cage). No `git push`. Edits inside `/Users/gdc/deadreckon`. Decisions → V1-CANDIDATES.

**The course, planned then previewed.**

- SignalBundle: goal decomposability, detected done contract, workspace members, prior verified runs on the task_key, budget-fit — computed free, pre-provider.
- Provider planner upgrades classify→plan: typed pieces with per-piece goals + done hints, n, confidence, rationale; clamped; deterministic ladder is the floor.
- The course card is the launch surface: shape, pieces, per-role providers, budget split, contract, escape hatches; Enter sails, `e` edits, `s` forces single, `q` aborts. `--json` emits the plan (launch JSON parity, finally).
- Asymmetric guardrails: auto-accept under `--yes` only when confidence ≥ threshold AND spend ≤ ceiling; single is the bias; campaign always confirms above the dollar line.

**Course correction.** A plan that decomposes to one task collapses to a run (today it refuses). A run that discovers independent pieces pauses at a verified checkpoint and writes a reshape proposal (same plan format); `deadreckon reshape <id>` previews/approves it. Every reshape is an event.

**Phases.** Fourteen (P1–P14) in the rider — this slice earns it. Each: depth test first → implement → `make verify` green → conventional-commit → CHANGELOG line. P14 adds AS-BUILT §46 + friendliness closure.

**Verification.**

- Every rider depth test present and passing; `make verify` green each commit.
- `start` on a fixture workspace produces a durable `launch-plan.json`, renders the card, and dispatches the planned shape; `start --plan <file>` replays it byte-identically; `--json` emits the envelope.
- With a detected contract and high confidence, `start --yes` under the ceiling asks zero questions; campaign above the ceiling refuses with `try:`. Campaign/chain auto-detect cells flip to pass.

**Stop when** verification passes, AS-BUILT + V1-CANDIDATES + a `Course (stable)` CHANGELOG section are updated, committed locally.
