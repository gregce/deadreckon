# deadreckon — Effortless Rider (a whole-surface friendliness pass)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-28-2032-deadreckon-effortless-goal.md`.
It supersedes nothing in prior riders (notably
`2026-05-11-1400-deadreckon-usability-rider.md`,
`2026-05-27-*-production-command-model`,
`2026-05-28-1556-deadreckon-tamper-evident-gate-rider.md`,
`2026-05-28-1841-deadreckon-campaign-rider.md`) — their invariants still apply.
This rider adds a presentation-layer friendliness pass: it evaluates the whole
verb surface against a written contract, then closes the gaps. **No core
mechanism changes.**

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime `~/.deadreckon/`.

## Posture (decided — do not redesign)

- **Production-release track. Presentation, UX, and ONE advisory provider call.**
  No changes to the gate (`gate.rs`, `dr-gate`), sandbox, promotion, provider
  *routing*, tamper-evidence (§35), or the campaign engine (§36). The single new
  behavior that talks to a provider is the read-only **goal-shape classifier**
  (below): it reuses existing provider routing, is bounded and validated, and falls
  back deterministically — it never blocks a run and never auto-launches anything.
  Anything needing a core-mechanism change is out of scope — log it in
  `V1-CANDIDATES.md`.
- **Files-not-fields.** No `PipelineState`/`Plan`/`Campaign`/`AcceptanceMarker`/
  provider-config field additions. New state (notification config, the audit
  record) lives in files / existing `config.toml` `[notify]` table only.
- **Reuse the rendering primitives.** Build on `ui_card.rs` (`Card`, `TitleLine`,
  `TitleGlyph`, `MetricColumn`, `HintLine`, `Tone`, `render_card`), `ui.rs`, and
  `glossary`. No new rendering framework, no new color crate.
- **Opt-in only for outward effects.** Notifications never fire unless configured.
  No telemetry, no background profiling, no network calls added.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Themable palettes, localization, a card template engine, a
  long-lived notifier daemon, and LLM-backed goal classification are V1.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## The friendliness contract (the spec P1 codifies)

Every top-level verb is scored against six clauses. A verb "passes" when each
applicable clause holds:

1. **Auto-detect, don't ask** — the single obvious case proceeds without a prompt
   or a refusal (e.g. one detected provider, the `latest` run, the only plan).
2. **Preview before mutate** — any state-changing verb shows a preflight and (on a
   TTY) confirms, or accepts `--yes` headless; `--preview` stops before the change.
3. **Refuse with `try:`** — every refusal names a specific next command.
4. **One-command rollback** — anything that changed the workspace can be undone in
   one command, and the verb says how.
5. **One verdict + ONE primary action** — a returning surface leads with a single
   bolded outcome and exactly one primary next action (extra actions are demoted,
   not equal-weight).
6. **Lifecycle hint** — every action ends pointing at the natural next step.

Not every clause applies to every verb (read-only verbs skip 2/4). The audit
records which clauses apply and which fail.

## Data model (files, not fields)

### `~/.deadreckon/config.toml` — new `[notify]` table

```toml
[notify]
enabled = false                  # master switch; default off (opt-in)
on = ["accepted", "paused", "failed"]   # which transitions fire
native = true                    # use osascript (macOS) / notify-send (Linux)
command = ""                     # optional: shell command run with env vars set
# webhook = "https://..."        # optional: POST a small JSON body
```

`command`/webhook receive a small, redacted context (run id, verdict, spend
summary, narrative path) via env vars / JSON — never secrets or file contents.

### Goal-shape classification record (preview-scoped, files-not-fields)

When `start` classifies a goal, the result is written to a preview record
(`~/.deadreckon/runstate/<scope>/preview/<task-key>.json`) — never a schema field:

```jsonc
{
  "schema_version": 1,
  "goal": "…",
  "shape": "single" | "orchestrate" | "campaign",
  "n": 3,                          // present for orchestrate/campaign; clamped 2..=6
  "rationale": "three independent services, each its own project",
  "source": "provider" | "fallback",   // which path produced this
  "provider": "cli:claude-code"    // null when source == fallback
}
```

### `~/.deadreckon/runstate/<scope>/runs/<run-id>/notify.jsonl`

Append-only record of notifications attempted (transition, channel, ok, ts) so a
run's notification history is auditable and a depth test can assert a fire.

### `<repo>/docs/FRIENDLINESS-AUDIT.md` — the P1 evaluation output

A checklist table: one row per verb × the six clauses, marked pass/fail/n-a with a
one-line note. This is the durable evaluation the goal asks for; later phases must
flip the failing cells it lists.

## Verb signatures (new / changed)

```
deadreckon try
    [--plain] [--json]           # zero-arg keyless demo; smoke provider, sandbox none
```

`try` resolves provider=`smoke`, sandbox=`none`, a fixed throwaway goal, runs the
real turn loop + real `dr-gate`, and prints the proof block (below). It never
touches the user's checkout (its own working dir) and needs no config.

```
deadreckon start <goal>          # unchanged surface; new behavior:
```

- When `auto_subscription_cli_provider(&registry)` returns exactly one provider and
  no provider is configured, adopt it inline: print
  `provider: cli:<x> (detected) — run deadreckon config … to make permanent` and
  proceed, instead of refusing with the `deadreckon init` footer (main.rs:2465/3301).
- When the goal parses as several independent pieces (rider heuristic below),
  offer the campaign path in the interactive picker.

```
deadreckon campaign <goal>       # --n becomes optional:
    [--n <2..=6>]                # omitted => planner proposes a count, shown in preview
```

The editable preflight (TTY) lists the proposed sub-goals and accepts: launch /
edit a sub-goal / drop a sub-goal / change count / cancel. Headless `--yes` keeps
the proposed decomposition.

### Proof block (the `try` and first-success format — depth-tested verbatim shape)

```
gate: SIGNED by dr-gate — the agent could not have written this
proof:  <run-root>/proofs/turn-acceptance.json
story:  <library>/RUN-NARRATIVE.md
lineage: <one changed file> ← turn <n> · <provider> · <tool>
→ deadreckon apply <id>        (or, for try:  → deadreckon start "build the real thing")
```

## Detection rules (non-obvious logic)

- **Single-provider adopt:** adopt inline only when exactly one subscription CLI is
  detected AND config has no provider; ≥2 detected keeps the picker; 0 keeps the
  `try:` refusal.
- **Goal-shape routing (start → run / orchestrate / campaign):** the PRIMARY path is
  the provider classifier (Provider contract below). The DETERMINISTIC FALLBACK,
  used whenever no provider is configured / it is disabled / over budget / returns
  invalid output, is the conjunction-list heuristic: a top-level conjunction or list
  (", and ", " and ", ";", " then ") joining ≥2 noun-ish clauses suggests a campaign.
  Either way the result is a *suggestion* shown in the picker, never forced and never
  auto-launched.
- **Planner-proposed `--n`:** when `--n` is omitted, the classifier's recommended
  count (clamped `2..=6`) seeds the editable preflight; the user can change it.

## Provider contract (the goal-shape classifier)

`classify_goal_shape(goal, provider) -> GoalShapeRecommendation` is a single,
read-only, bounded provider call that reuses existing provider routing (no new
registry, no routing change). It mirrors the validated-provider-with-deterministic-
fallback pattern already used by narrative, plan-doc, and tamper consolidation.

- **Prompt:** the goal plus a short rubric — `single` = one cohesive change a single
  run handles; `orchestrate` = one project with parallelizable subtasks; `campaign` =
  several *independent* projects each warranting their own coordination — and a
  request for `shape`, `n` (2..=6 when not single), and a one-line `rationale`.
- **Bounded:** one call, capped by a small classification token/spend budget; on
  timeout, error, missing provider, or over-budget → deterministic fallback. The call
  is made only for interactive `start` (or `start --classify`), never inside
  `run`/`campaign` execution, and never blocks.
- **Validated before use:** `shape` ∈ the three values; `n` clamped to `2..=6`;
  `rationale` non-empty. Invalid output → fallback (recorded as `source: "fallback"`).
- **Advisory only:** the recommendation populates the picker/preflight as a
  suggestion with its rationale; the user confirms. A campaign is never spawned
  without explicit confirmation (or `--yes` on an explicit `campaign` invocation).

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them fail; implement;
green on `cargo test -p <touched crate>` for touched modules plus
`cargo fmt --check`; conventional-commit local commit; one-line CHANGELOG entry.
Do not run `make verify`/release/stress/broad suites unless the human asks. Smokes
use the `smoke` provider with `--sandbox none`.

### P1 — Friction audit (the evaluation) — BOUNDED, do not over-build

- Codify the six clauses as a small `friendliness_contract` data table (verb →
  applicable clauses) and hand-author the findings into `docs/FRIENDLINESS-AUDIT.md`
  (one row per verb × clause, pass/fail/n-a + a one-line note). Do **not** reflectively
  walk all 38k lines of handlers — that is the balloon. Instead the audit is a curated
  checklist plus *targeted* tests over the specific surfaces later phases change (exit
  card, `start`, refusals). The audit doc is the backlog the later phases burn down.

Depth tests (`crates/deadreckon/tests/friendliness.rs`):
- `friendliness_contract_table_covers_every_top_level_verb`
- `audit_doc_lists_a_row_per_verb_and_clause`

### P2 — `deadreckon try`

- New top-level `Try` command; resolves smoke/none/throwaway-goal; reuses the run
  loop and real gate; emits the proof block.

Depth tests:
- `try_runs_keyless_and_signs_a_gate`
- `try_prints_proof_block_with_narrative_and_lineage`

### P3 — Proof block on first success

- Factor the proof-block renderer (P2) and emit it on a real run's accepted exit
  card too (not just `try`).

Depth tests:
- `accepted_exit_card_shows_proof_block`
- `proof_block_shape_is_stable` (spec-pinned whitespace)

### P4 — Self-bootstrapping `start`

- Single-provider inline adopt; ≥2 keeps picker; 0 keeps refusal.

Depth tests:
- `start_adopts_single_detected_subscription_provider_inline`
- `start_with_two_detected_providers_still_prompts`
- `start_with_no_provider_refuses_with_try_line`

### P5 — One verdict + ONE primary action — single localized slot

- Add ONE additive `primary_action` field to the card model in `ui_card.rs` (a
  distinguished `HintLine` rendered first, with an accent `Tone`); the existing card
  builders for exit/status/finish set it and demote remaining hints. This is a single
  localized change to the shared primitive, not a per-surface rewrite. Accepted/
  paused/failed variants set the appropriate verb (`apply`/`resume`/`show --why-failed`).

Depth tests:
- `exit_card_leads_with_one_verdict_and_one_primary_action`
- `paused_and_failed_cards_each_have_one_primary_action`

### P6 — Consistency sweep (spend + gate verdicts)

- Confirm honest subscription spend (`not metered (subscription) …`) and per-check
  gate verdicts (`gate: PASSED N/N` / `FAILED i/N`) render on the exit card,
  `status`, `finish`, plan/campaign summaries; fix any surface still showing
  `~$0.000000` or a bare count.

Depth tests:
- `no_surface_renders_zero_dollar_subscription_spend`
- `gate_verdict_is_per_check_on_every_outcome_surface`

### P7 — Notification config + channels

- Parse `[notify]`; implement native + `command` + webhook senders; write
  `notify.jsonl`. Pure senders behind a trait, testable with a fake channel.

Depth tests:
- `notify_config_parses_and_defaults_off`
- `command_channel_receives_redacted_context`
- `notify_records_attempt_to_jsonl`

### P8 — Fire notifications on the three transitions

- Emit on accepted / paused-at-cap / failed from the lifecycle, gated by config.

Depth tests:
- `accepted_run_fires_notification_when_enabled`
- `disabled_notify_fires_nothing`

### P9 — Provider-backed goal-shape routing + campaign friendliness

- Implement `classify_goal_shape` per the Provider contract: bounded read-only call,
  validation/clamp, deterministic fallback, preview record. Wire it into interactive
  `start` so the picker leads with the recommended shape (`run`/`orchestrate`/
  `campaign`) + rationale as a suggestion. Make campaign `--n` optional (seeded by the
  recommendation) with an editable preflight (drop/edit/change-count/cancel). The
  deterministic heuristic remains the fallback path.

Depth tests:
- `goal_shape_classifier_validates_and_clamps_provider_output`
- `goal_shape_falls_back_to_deterministic_when_provider_unavailable`
- `classified_campaign_shape_is_suggested_not_auto_launched`
- `single_change_goal_classifies_as_single`
- `campaign_without_n_uses_recommended_count`
- `campaign_preflight_can_drop_a_subgoal_before_launch`

### P10 — Vocabulary unification + error-footer coverage

- The guarantee noun is decided (no taste call for the executor): **"verified run"** /
  the run is **"verified by dr-gate"**, with `dr-gate` described as "the process that
  verifies the run." Add one `glossary` constant for the term and route all
  user-facing copy through it; collapse def-done/`acceptance.yaml`/done-criteria under
  one umbrella ("the done contract"); keep five-to-live-by + `help-all`. Sweep every
  refusal for a specific `try:` footer. Verdict word on cards is `VERIFIED`.

Depth tests:
- `guarantee_noun_is_consistent_across_surfaces` (asserts the single glossary term)
- `every_refusal_carries_a_try_line` (parameterized over the refusal table)

### P11 — AS-BUILT + CHANGELOG + V1-CANDIDATES (doc only; no depth test)

- New AS-BUILT section:
  ```
  ## 37. Effortless: the friendliness contract

  37.1 The six-clause contract and the audit (FRIENDLINESS-AUDIT.md)
  37.2 deadreckon try and the proof block
  37.3 Self-bootstrapping start
  37.4 One verdict + one primary action
  37.5 Spend + gate-verdict consistency
  37.6 Opt-in notifications ([notify], channels, notify.jsonl)
  37.7 Provider-backed goal-shape routing (classifier + deterministic fallback)
  37.8 Campaign friendliness (recommended --n, editable preflight)
  37.9 Vocabulary ("verified run") + error-footer coverage
  37.10 Limits (one bounded classifier, opt-in effects, no daemon)
  ```
  Update the "shipped vs scaffolding-thin" list: add the contract + audit, `try`,
  notifications, and the vocabulary pass; state plainly that this changed no core
  mechanism.
- Append a `## Effortless (production release) — 2026-05-28` CHANGELOG section.
- Log to `V1-CANDIDATES.md`: themable palettes, localization, a card template
  engine, a long-lived notifier daemon, LLM-backed multi-piece classification, and
  richer guided onboarding.

## Integration matrix

| Surface | Today | After |
|---|---|---|
| first run | `run --smoke --sandbox none` (buried) | `deadreckon try` + proof block |
| `start`, one provider | refuses → `deadreckon init` | adopts inline, proceeds |
| accepted exit | 3-4 equal `try:` hints | one verdict + one primary action |
| subscription spend | mostly honest (tamper-evident) | honest on every surface (swept) |
| walk-away return | poll `status` | opt-in notification fired |
| goal routing | you choose run/orchestrate/campaign | provider classifies + suggests (fallback heuristic) |
| `campaign --n` | required | optional, recommendation-seeded, editable |
| refusals | most have `try:` | all have `try:` (parameterized test) |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `no provider configured` (0 detected) | `deadreckon try` then `deadreckon config provider …` |
| `start: goal looks like several pieces` (info, not error) | `deadreckon campaign "<goal>"` |
| `notify command failed` | `deadreckon config notify.command "<cmd>"` and re-run |
| `campaign preflight cancelled` | `deadreckon campaign "<goal>" --n <2..=6> --preview` |

(Each footer is exercised by a P4/P9/P7/P10 depth test.)

## Config additions

```toml
[notify]
enabled = false
on = ["accepted", "paused", "failed"]
native = true
command = ""
```

## Out of scope (explicitly V1 candidates)

- Themable palettes / `NO_COLOR` beyond what `ui.rs` already does.
- Localization of status words, nouns, prompts, hints.
- A card template engine replacing hand-built cards.
- A long-lived notifier daemon or push service (this milestone is fire-and-forget).
- LLM goal routing *beyond* the single bounded classification call: multi-turn
  clarification dialogue, routing learned from run history, and per-sub provider
  planning. (The one bounded classifier IS in scope this milestone.)
- Any change to gate/sandbox/promotion/provider/campaign mechanism.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (in-tree): `serde`/`toml` (notify config), `serde_json`, existing process
spawn for the `command` channel. **No new crates expected** — native notification
uses `osascript`/`notify-send` via the existing subprocess path; webhook (if built)
reuses the existing HTTP client. Tier 2: none. Tier 3: same blocks as prior riders.

## Engineering invariants (do not violate)

- **No core-mechanism change.** Touch presentation, detection, config, and copy
  only. A diff to `gate.rs` evaluate/sign, sandbox, promotion, or the campaign
  engine means scope creep — stop and reconsider.
- **No `PipelineState`/`Plan`/`Campaign`/provider field additions.**
- **Notifications are opt-in and side-effect-isolated.** Disabled config fires
  nothing (`disabled_notify_fires_nothing`); senders never block or fail the run.
- **The goal-shape classifier is advisory, bounded, and never authoritative.** It
  reuses provider routing without changing it, validates+clamps output, falls back
  deterministically, never blocks a run, and never auto-launches a campaign
  (`classified_campaign_shape_is_suggested_not_auto_launched`).
- **One depth test before each phase implementation.** P1's failing audit test is
  the backlog; later phases flip its cells.
- **Spec-pinned shapes.** The proof block and the one-verdict card layout are
  depth-tested; changing their whitespace changes the spec.
- **No silent expansion.** Anything beyond P1–P11 → `V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing, `cargo fmt --check` clean, and a
  CHANGELOG entry naming the SHA.
- If a phase reveals that a friendliness win needs a mechanism change, stop and log
  it in `V1-CANDIDATES.md`; do not silently change core behavior.
- Optional after P11: an asciinema cast of `deadreckon try` under the repo demo
  assets.
