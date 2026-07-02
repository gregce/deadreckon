# deadreckon — Course Rider (the harness plots the course)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-07-01-2010-deadreckon-course-goal.md`.
It supersedes nothing in prior riders (guided-start, uniform-surface,
orchestrate-campaign-narration, polyglot, verdict); their invariants still
apply. This rider adds: a deterministic **SignalBundle**, a durable
**launch-plan.json**, a provider **planner** (classify→plan upgrade), the
**course card** launch surface, launch **JSON parity**, plan **collapse**,
checkpoint-gated **reshape proposals**, and closure of the campaign/chain
auto-detect friendliness cells.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `~/.deadreckon`.

The mental model is a query planner: goal = query, done contract = schema +
assertions, `start` = planner, the course card = EXPLAIN, execution = the
existing run/plan/campaign/chain machinery, reshaping = adaptive re-planning.
Nothing below invents a new execution engine — Course decides and records
*which existing engine runs, with what pieces, under what money*.

## Posture (decided — do not redesign)

- **Maturity stays stable** (0.4.0 shipped; lands under a `Course` CHANGELOG section).
- **No `PipelineState` / `Plan` / `Campaign` / `Chain` schema changes.** The launch plan is a FILE — `launch-plan.json` — written before dispatch and copied into whatever root the dispatched shape creates (run root, plan dir, campaign dir). Files, not fields.
- **Deterministic floor everywhere.** The shape ladder must produce a sane plan with zero provider calls (no-provider and `--smoke` paths included). The provider planner is one bounded call, clamped, advisory until the card is accepted.
- **Asymmetric spend safety is non-negotiable.** Wrong-single costs a retry; wrong-campaign costs real money. Single is the tie-breaker bias. Auto-accept (`--yes`) requires confidence ≥ threshold AND estimated ceiling ≤ `shape_auto_spend_ceiling`. Campaign shape above the ceiling ALWAYS confirms interactively or refuses with `try:` in non-TTY. This mirrors the existing $50 confirmation-gate doctrine.
- **Existing verbs stay callable and behavior-stable.** `run`/`orchestrate`/`campaign`/`chain` continue to work directly; Course only changes what `start` resolves and how the decision is recorded. Direct verbs also write a (trivial, operator-shaped) launch-plan.json so downstream surfaces can rely on its presence.
- **The planner proposes; the operator (or the guardrail policy) disposes.** No reshape executes without an accepted proposal. No plan file is executed unless it validates against the schema and clamps.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Auto-reshape-without-approval, cross-machine plans, learned per-user shape priors, and planner-chosen per-sub campaign breadth beyond the existing clamp go to V1-CANDIDATES.
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Data model (files, not fields)

### `launch-plan.json` (schema 1)

Written by `start` (or trivially by direct verbs) to a staging path, then
copied into the dispatched root. This is the single durable record of "what
we decided to do and why".

```json
{
  "schema": 1,
  "created_at": "2026-07-01T20:10:00Z",
  "goal": "add rate limiting to the API",
  "shape": "single | plan | campaign | chain-extend",
  "pieces": [
    {
      "id": "p1",
      "goal": "token-bucket limiter core",
      "done_hint": "unit tests for bucket refill + burst",
      "role": "coder",
      "provider": "cli:claude-code",
      "model": null,
      "budget_usd": 5.0
    }
  ],
  "n": 3,
  "providers": { "planner": "cli:claude-code", "coder": "cli:claude-code", "reviewer": "cli:codex" },
  "budget": { "ceiling_usd": 12.0, "split": [5.0, 3.0, 4.0], "wall_seconds": null },
  "contract": {
    "source": "detected | operator | inferred | asked | none",
    "kind": "Node(pnpm)",
    "summary": "pnpm test",
    "caveat": null
  },
  "signals": { /* the SignalBundle verbatim — audit of what the decision saw */ },
  "resolution": {
    "source": "provider | ladder | operator | replay",
    "confidence": 0.82,
    "rationale": "three independently testable pieces; workspace has 3 members",
    "clamps_applied": ["n clamped 8->6"]
  },
  "escape": { "kill": "deadreckon kill latest", "undo": "deadreckon undo latest" },
  "accepted_by": "operator | yes-flag-guardrail | replay",
  "parent": null
}
```

- `pieces` is 1 element for `single`, 2..=6 for `plan` (the existing clamp), and per-sub goals for `campaign`.
- `signals` embeds the bundle so the plan is self-explaining forever (verdict/narrative can cite it).
- `parent` carries a run id when the plan is a reshape proposal (below).
- Serde is additive-tolerant: unknown fields ignored, `schema` checked, invalid plan = refusal with `try:`, never a guess.

### `reshape-proposal.json`

Same schema, written to `<run_root>/reshape-proposal.json` with `parent` set,
by the escalation path (P12). It is INERT until `deadreckon reshape <id>`
accepts it.

### SignalBundle (in-memory, embedded in the plan)

```
struct SignalBundle {
    decomposability: DecompositionHints, // conjunction count, enumerations, imperative verbs, goal length
    contract: ContractSignal,            // ProjectKind + default command + caveat (reuses acceptance_defaults)
    workspace: WorkspaceSignal,          // workspace member count + names (Cargo members / pnpm workspaces / go.work), tree file-count bucket
    history: HistorySignal,              // prior runs on this task_key: count, last status, last shape, verified?
    budget: BudgetSignal,                // requested/default ceiling, per-shape feasibility (plan needs ~n*min_piece, campaign ~n*plan)
}
```

All five computed deterministically, no provider, no network, total (never
error — degraded fields carry `None`).

## Shape resolution (the spec — match it in code)

New module `crates/deadreckon/src/commands/course.rs` (binary-private, like
other command families), with the pure core in functions unit-testable
without I/O.

### Deterministic ladder (the floor; also the no-provider path)

Evaluated in order; first rule that fires decides. Every decision records
which rule fired into `resolution.rationale`.

1. `history.last_verified_same_task` → shape=single continuation (extend bias); note in card.
2. `budget.ceiling < plan_feasible_floor` → single (money decides; never propose a shape that cannot fit).
3. `decomposability.strong` (≥2 enumerated imperatives OR explicit list of ≥2 deliverables) AND `workspace.members ≥ 2` → plan, n = min(enumerated, members, 6).
4. `decomposability.strong` AND single-package tree → plan, n = min(enumerated, 4).
5. goal length < 1 sentence, weak decomposition → single.
6. campaign is NEVER chosen by the ladder — campaign requires the provider planner or the operator. (Deterministic campaign selection is a spend hazard with no evidence floor.)
7. default → single.

### Provider planner (one call, grounded, clamped)

- Prompt receives the goal AND a compact rendering of the SignalBundle
  (contract summary, workspace members, history, budget) — the planner sees
  what the ladder saw, so its output is grounded, not vibes. Prompt lives as
  a const beside `goal_shape_prompt` and supersedes it.
- Response: typed `ProviderCoursePlanDraft { shape, pieces[{goal, done_hint}], n, confidence (0..1), rationale }`, parsed with the same tolerant slice-extraction discipline as `ProviderGoalShapeDraft`, then CLAMPED: n into 2..=6, pieces truncated to n, shape downgraded to the ladder result if confidence < `shape_confidence_floor`, budget split normalized to the ceiling.
- Failure/timeout/parse-miss → ladder result, `resolution.source = "ladder"`. The planner can never fail a launch.
- Reuses the existing `goal_shape_provider_route` selection (read-only, cheap-model preference), spend recorded with the existing classifier label.

### Budget fit + guardrails

```
fn plan_feasible_floor(n) -> f64        // n * MIN_PIECE_BUDGET
fn campaign_feasible_floor(n) -> f64
fn accept_policy(resolution, budget, tty, yes_flag) -> AcceptDecision
   // Interactive card | AutoAccept | RefuseWithTry
```

- `AutoAccept` iff `yes_flag && confidence >= shape_confidence_floor && ceiling <= shape_auto_spend_ceiling && shape != campaign-above-line`.
- Non-TTY without `--yes`: refuse with `try: deadreckon start "<goal>" --yes` (existing convention) — never hang.
- Campaign above `campaign_confirm_line`: interactive confirm always; non-TTY refuse with `try:` naming `--shape plan` and `--max-spend`.

## The course card (the launch surface — style matters here)

Rendered through the existing `ui_card` helpers + `VerdictSurface` discipline
(one primary action). Target shape (spec-pinned by a golden):

```
┌─ course ─ plot → preview → sail ────────────────────────────┐
│ GOAL   add rate limiting to the API                          │
│ SHAPE  plan · 3 pieces (parallel)            confidence 0.82 │
│        1 token-bucket core  2 config surface  3 wiring       │
│ WHO    coder cli:claude-code · reviewer cli:codex            │
│ COST   ceiling $12 · split 5 / 3 / 4                         │
│ DONE   pnpm test                                  [detected] │
│ WHY    three independently testable pieces; 3 ws members     │
│ ESCAPE kill latest · undo latest                             │
└──────────────────────────────────────────────────────────────┘
  Enter sail    e edit    s single    q abort
```

- The card is calm, bounded, and self-explaining: WHAT/WHO/COST/DONE/WHY/ESCAPE always present; WHY is one line of `resolution.rationale`; confidence shown, never hidden.
- `e` opens the existing inquire select/confirm flow (not a new TUI): change shape, n, ceiling, provider route; edits re-validate and re-render the card; `resolution.source` becomes `operator`.
- `s` is the one-keystroke de-escalation (bias made tactile).
- `--plain` renders the same fields as aligned key/value lines; `--json` emits the plan and NEVER renders the card; `--quiet` prints the accept line only.
- Style rule: pizzazz here is *precision* — alignment, the plot→preview→sail header, glyph-stable columns. No animation in a one-shot card. Golden-tested so whitespace is spec.

## Verb signatures

```
start <goal>
    [--plan <file>]            # replay a saved/edited launch-plan.json (skips planning)
    [--shape single|plan|campaign]   # operator override, recorded source=operator
    [--n <2..6>]               # with --shape plan/campaign
    [--max-spend <usd>] [--yes] [--json] [--plain] [--quiet]
    [--no-provider-plan]       # ladder only
    # existing start flags (provider/model/source-mode/etc.) unchanged

reshape <run-id|latest>
    [--yes] [--json]           # preview + accept <run_root>/reshape-proposal.json
```

Refusal cases:

| Case | Behavior |
|---|---|
| `--plan` file missing/invalid schema | refuse, `try: deadreckon start "<goal>"` |
| `--plan` budget exceeds current `--max-spend` | refuse, name both numbers, `try:` raise or edit |
| campaign above confirm line, non-TTY | refuse, `try: … --shape plan` |
| `reshape` with no proposal present | refuse, `try: deadreckon status <id>` |
| `reshape` on a still-running run | refuse, `try: deadreckon attach <id>` |
| `--shape campaign --yes` above ceiling | refuse (guardrail beats flags) |

## Phases (fourteen)

Each phase: named depth test(s) **first** (watch fail) → implement →
`make verify` green (fmt-check, clippy, public-surface, test, build) →
conventional-commit → one-line CHANGELOG entry naming the SHA.

### P1 — LaunchPlan schema + module skeleton
- `course.rs`: `LaunchPlan`, `Piece`, `ContractSignal`, `Resolution`, serde (additive-tolerant, schema-checked), load/save via `atomic_write_json`. No call-site changes.

Depth tests:
- `launch_plan_roundtrips_serde`
- `launch_plan_unknown_fields_tolerated_schema_checked`
- `invalid_plan_schema_refuses_with_try`

### P2 — SignalBundle: decomposability + workspace
- Goal-structure analysis (enumerations, conjunctions, imperative count) and workspace scan (Cargo members / pnpm workspaces / go.work; file-count bucket). Pure, total.

Depth tests:
- `enumerated_goal_yields_strong_decomposability`
- `single_sentence_goal_is_weak`
- `cargo_workspace_members_counted`

### P3 — SignalBundle: contract + history + budget
- Contract signal reuses `detect_project_kind`/`default_checks_for`; history from `list_runs` on the task_key; budget feasibility floors.

Depth tests:
- `contract_signal_reuses_polyglot_detection`
- `prior_verified_run_sets_continuation_signal`
- `budget_below_plan_floor_marks_plan_infeasible`

### P4 — Deterministic ladder
- The seven rules, in order, rationale recorded; campaign never ladder-chosen.

Depth tests:
- `ladder_prefers_continuation_on_verified_history`
- `small_budget_forces_single`
- `enumerated_goal_plus_workspace_yields_plan_n_clamped`
- `ladder_never_selects_campaign`

### P5 — Provider planner (classify→plan upgrade)
- Grounded prompt (bundle rendered in), `ProviderCoursePlanDraft` parse + clamps, confidence-floor downgrade, failure→ladder. Supersedes `classify_goal_shape_for_start` (old path deleted, not shimmed).

Depth tests:
- `planner_prompt_includes_contract_and_workspace_signals`
- `low_confidence_draft_downgrades_to_ladder_shape`
- `oversized_n_clamped_and_recorded_in_clamps_applied`
- `planner_failure_falls_back_to_ladder_source`

### P6 — Guardrails + accept policy
- `accept_policy` matrix (TTY × yes × confidence × ceiling × shape); campaign confirm line; non-TTY refusals.

Depth tests:
- `yes_flag_autoaccepts_only_above_confidence_and_under_ceiling`
- `campaign_above_line_always_confirms_or_refuses`
- `non_tty_without_yes_refuses_with_try`

### P7 — The course card
- `ui_card` rendering with WHAT/WHO/COST/DONE/WHY/ESCAPE, Enter/e/s/q keys via existing prompt primitives; `--plain` parity; golden-pinned layout.

Depth tests:
- `course_card_golden_snapshot_pins_layout`
- `card_always_names_done_contract_and_escape`
- `s_key_forces_single_and_records_operator_source`

### P8 — One-question flow
- When `contract.source == none/Unknown`: ask exactly "How will you know it worked?" (one line → compiled into an operator `acceptance.yaml` via the existing parse path; `asked` source). Detected/operator contract → zero questions. Never asks under `--yes`/`--json`/non-TTY (caveat instead, per Polyglot doctrine).

Depth tests:
- `unknown_contract_asks_exactly_one_question`
- `detected_contract_asks_zero_questions`
- `yes_flag_skips_question_and_carries_caveat`

### P9 — Dispatch reads the plan (indirection collapse)
- `dispatch_start_command` consumes `LaunchPlan` (StartLaunchDecision folds into it or delegates); the accepted plan is copied into the dispatched root (run root / plan dir / campaign dir); direct verbs write their trivial plan file.

Depth tests:
- `accepted_plan_lands_in_run_root`
- `plan_shape_dispatches_orchestrate_with_planned_n`
- `direct_run_writes_trivial_operator_plan`

### P10 — Replay + launch JSON parity
- `start --plan <file>` validates, re-clamps against current budget flags, replays byte-identically (`resolution.source = replay`); `start --json` emits `{kind:"launch", plan, dispatched:{shape, ids}, next_actions}` and suppresses the card.

Depth tests:
- `start_plan_replays_identical_shape_and_pieces`
- `replay_with_smaller_budget_refuses_naming_numbers`
- `start_json_emits_launch_envelope_no_card`

### P11 — De-escalation: plan collapse
- A planner/decomposition result of exactly 1 task collapses to a single run (today's refusal path becomes graceful fallback), recorded as a `collapse` event + card note.

Depth tests:
- `single_task_decomposition_collapses_to_run`
- `collapse_recorded_as_event_and_rationale`

### P12 — Reshape proposals (escalation, checkpoint-gated)
- Turn-loop seam: when a run's agent output carries a decomposition signal (a `reshape` action variant in the existing Action enum pattern — additive serde) OR the run pauses at cap with pieces named in implementation notes, write `reshape-proposal.json` (INERT) and emit an event. `deadreckon reshape <id>` renders the same course card for the proposal, seeds pieces from the run's library artifact on accept, dispatches a plan with `parent` lineage.

Depth tests:
- `reshape_action_writes_inert_proposal_and_event`
- `reshape_verb_previews_proposal_card`
- `accepted_reshape_dispatches_plan_with_parent_lineage`
- `proposal_never_executes_without_accept`

### P13 — Start-then-watch
- After dispatch, TTY sessions print the one attach line and (config `[defaults] start_attach = true`) drop directly into attach; `--json`/`--quiet`/non-TTY never do.

Depth tests:
- `start_attach_config_drops_into_attach_on_tty`
- `json_and_quiet_never_auto_attach`

### P14 — Friendliness closure + docs (doc + contract phase)
- Flip campaign/chain "Auto-detect, don't ask" cells: planner-chosen n removes the `--n` over-ask; update `friendliness_contract.rs` + `FRIENDLINESS-AUDIT.md` in lockstep (doc==code test enforces).
- Insert AS-BUILT `## 46. Course: launch planning and reshaping` (SignalBundle, ladder, planner, card, guardrails, plan file, reshape) + update §22 shipped list and §26/§30/§36 cross-references.
- Append CHANGELOG:
  ```
  ## Course (stable) — <date>
  - start plots the course: deterministic signal bundle + one grounded planner call resolve a durable launch-plan.json; the course card previews shape/pieces/cost/contract; zero questions when the contract is detected; plan collapse and checkpoint-gated reshape proposals correct the shape mid-voyage; campaign/chain auto-detect friendliness cells closed.
  ```

## Integration matrix

| Concern | single | plan | campaign | chain-extend (continuation) |
|---|---|---|---|---|
| Ladder can choose | yes | yes | no (provider/operator only) | yes (verified history) |
| Card pieces shown | 1 | 2..6 | subs | 1 + prior-run note |
| Plan file lands in | run root | plan dir + each child | campaign dir + subs | chain dir |
| Auto-accept possible | yes | yes (under ceiling) | never above confirm line | yes |
| Reshape source | proposal → plan | collapse → single | out of scope (V1) | n/a |
| JSON envelope | yes | yes | yes | yes |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| invalid `--plan` file | `try: deadreckon start "<goal>"` |
| plan budget > current cap | `try: deadreckon start --plan <file> --max-spend <ceiling>` |
| campaign above line, non-TTY | `try: deadreckon start "<goal>" --shape plan` |
| no reshape proposal | `try: deadreckon status <id>` |
| reshape on running run | `try: deadreckon attach <id>` |

(Each parameterized by a depth test.)

## Config additions

```toml
[defaults]
shape = "auto"                    # auto | ask | single  (ask = always show card even under --yes-able confidence)
shape_confidence_floor = 0.7
shape_auto_spend_ceiling = 20.0   # USD; --yes auto-accept allowed under this
campaign_confirm_line = 25.0      # USD; campaign always confirms above
start_attach = false              # drop into attach after TTY launch
```

## Out of scope (explicitly → V1-CANDIDATES)

- Auto-reshape without operator accept (policy-driven self-escalation).
- Campaign-level reshaping and planner-chosen per-sub breadth beyond the clamp.
- Learned shape priors from run history (self-improvement loop integration).
- Cross-machine / shared launch plans.
- Multi-contract (monorepo per-package) planning — rides the Polyglot monorepo deferral.
- Port/env brokering for the parallel pieces (options file A3).

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (in tree, free): `serde_json`, existing `inquire` prompts, `ui_card`
helpers, `acceptance_defaults`, `list_runs`, provider router. Tier 2: none
expected — the planner reuses the classifier route. Tier 3 (blocked): no new
TUI frameworks, no scheduler/queue crates (queue is a separate option), no
network beyond the one provider call.

## Engineering invariants (do not violate)

- **The plan file is the decision.** Anything dispatch does must be readable
  from `launch-plan.json` alone; no side-channel decision state survives.
- **Deterministic floor total and provider-free.** The ladder never errors,
  never calls out, and is the exact behavior of `--no-provider-plan`.
- **Campaign is never auto-chosen by deterministic rules** and never
  auto-accepted above the confirm line. Guardrail beats every flag.
- **One provider call per launch, clamped, failure-silent.** The planner can
  never fail or stall a launch (bounded like the existing classifier).
- **One question maximum**, and only the done-contract question, and never in
  non-interactive modes.
- **Card layout is spec** — golden-pinned; whitespace changes are contract
  changes.
- **Reshape proposals are inert artifacts** until an explicit accept.
- **One depth test before each phase.** A phase whose tests were never red is
  suspect.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing and a CHANGELOG entry naming the SHA.
- Planner depth tests use a scripted/fake provider (existing smoke pattern), never a live model; card tests golden-pin `--plain` output; fixture workspaces are tempfile trees with sentinel files only.
- After P14, capture a short demo cast of `start` → card → sail → attach under `docs/assets/` (user-visible slice; worth the cast).
- If a phase reveals a V1-architecture decision, stop and log it in V1-CANDIDATES; do not silently expand scope.
