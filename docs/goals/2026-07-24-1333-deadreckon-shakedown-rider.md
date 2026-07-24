# deadreckon — Shakedown Rider (one reference resolver, one meaning for `latest`)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-07-24-1333-deadreckon-shakedown-goal.md`.
It supersedes nothing in prior riders — their invariants still apply. This
rider adds: a **`reference` module** owning all id resolution, the
**`ResolvedRef` kind union**, **one `latest` rule**, a **kind-aware refusal
table** that never points an operator back where they came from, **`list`
plan-child folding**, and a **cross-verb journey test** that pins coherence
between verbs rather than within one.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `~/.deadreckon`.

## Posture (decided — do not redesign)

- **Maturity stays stable** (lands under a `Shakedown` CHANGELOG section).
- **No durable state, no schema changes.** This slice adds no file, no field, no
  config key. `PipelineState`, `Plan`, `Chain`, `Campaign` and every on-disk
  form are untouched. If a phase seems to want persistence, it is out of scope.
- **Success output is frozen.** Every verb's *successful* human and `--json`
  output is byte-for-byte unchanged. Only three things may move: refusal text,
  refusal `try:` targets, and `list` row rendering. The `show`/`attach`
  characterization goldens are the guard and must not be regenerated.
- **This is a consolidation, not a capability slice.** No new verbs. No new
  flags except where a rewired verb loses an existing one by accident — restore
  it rather than redesign it.
- **`resolve_ref` is the only resolver at the end.** P8 deletes
  `load_cli_run`, `load_cli_run_with_scope`, `latest_run`,
  `resolve_verdict_run`, `resolve_latest_run` and the per-verb cascades. A
  surviving second path is a failed slice, not a compromise.
- **No `git push`.** Phased local commits. **No V1 invention.**
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Why this defect survived (read before writing tests)

`docs/FRIENDLINESS-AUDIT.md` scores every one of the 44 verbs against six
clauses. `status` scores **pass** on "Refuse with try:" with the note *"Missing
state points at start/list"*. `verdict` scores **pass** with *"Unknown or
ambiguous run ids refuse with a `deadreckon list` retry"*. Both notes are
accurate, both verbs are individually well-behaved, and together they form a
closed loop. A per-verb matrix cannot express "the command this refusal names
must accept this id". That sentence is the deliverable of this slice, and it
belongs in a test, not a table.

Reproduction on `main` at `8816ba7`, in a checkout whose scope has plans but no
runs:

```
$ deadreckon status
error: not found: latest run for current project (deadreckon-6283b242)
  hint: try: run `deadreckon list` to find valid run ids or config keys
$ deadreckon list
id        status   ...
0c11f68e  pending  orchestrate  full-plan  fork  Build from review shorthand
Recommended
deadreckon status latest
$ deadreckon status latest
error: not found: latest run for current project (deadreckon-6283b242)
$ deadreckon status 0c11f68e
error: not found: run 0c11f68e
  hint: try: run `deadreckon list` to find valid run ids or config keys
$ deadreckon verdict 0c11f68e
error: invalid input: unknown run 0c11f68e
try: deadreckon list
```

`show 0c11f68e` and `attach 0c11f68e` resolve the same id correctly. Pin this
exact sequence in P4's depth tests.

## Module layout

```
crates/deadreckon/src/commands/reference.rs   // new; the only resolver
```

`reference.rs` is a sibling of the existing `commands/*` modules and is
declared in `commands/mod.rs`. It depends only on `deadreckon_core` loaders and
the existing plan/chain/campaign readers — it must not depend on any command
module, so the dependency runs commands → reference, never back.

## Data model (types, not files)

This slice persists nothing. The model is three types.

```rust
/// What an operator-supplied reference turned out to name.
pub(crate) enum ResolvedRef {
    Run(Box<PipelineState>),
    PlanChild {
        selection: PlanChildSelection,
        state: Box<PipelineState>,
    },
    Plan(Box<Plan>),
    Chain(Box<Chain>),
    Campaign {
        dir: PathBuf,
        campaign: Campaign,
    },
}

/// The set of kinds a calling verb can actually handle.
#[derive(Clone, Copy)]
pub(crate) struct RefKinds(u8);   // bitflags; RefKinds::RUN, ::PLAN, ::CHAIN,
                                  // ::CAMPAIGN, ::PLAN_CHILD, ::ALL

/// One resolution request.
pub(crate) struct RefQuery<'a> {
    pub reference: Option<&'a str>, // None => latest
    pub accepts: RefKinds,
    pub all_scopes: bool,           // the existing --all flag
    pub verb: &'static str,         // for refusal text only
}
```

`ResolvedRef::kind() -> RefKind` returns a plain enum for matching and for the
refusal table. `RefKind::noun()` returns the operator-facing word: `run`,
`plan child`, `plan`, `chain`, `campaign`. Route the nouns through
`deadreckon_core::glossary` where a constant already exists; add none.

## Resolution rules (the spec — match it exactly)

### Probe order

**Revised in P1 — first-match-wins was wrong.** The rider originally specified
"probe in a fixed order, take the first match". Implementing it exposed the
contradiction: under first-match-wins, a prefix matching both a run and a plan
resolves silently to the run and the operator who meant the plan gets the wrong
object with no signal. That is the same class of defect as the `status`/`list`
loop — the tool quietly deciding what the operator meant. The spec is therefore:

1. **Plan-child ref first, on syntax, not precedence.** A plan-child reference
   contains `:` or `/` (`resolve_plan_child_ref`, now in `reference.rs`), which
   a bare id never does. The shapes are disjoint, so checking it first is
   disambiguation, not ranking.
2. **Every other kind is probed — regardless of `accepts` — and all matches are
   collected**: run (`deadreckon_core::load_run`), plan (`plan_ids_matching`),
   chain (`commands::chain::list_chain_records`), campaign
   (`commands::campaign::resolve_campaign`). Exactly one match resolves; two or
   more is the cross-kind ambiguity refusal below.
3. **`accepts` narrows the decision, not the probe** (settled in P3). Probing
   only the accepted kinds is exactly what produced `not found: run 0c11f68e`
   for an id that existed and was simply a plan — a false statement, and the far
   end of the loop. Identify what the reference names first, then decide whether
   this verb can take it; if it cannot, refuse by kind via the table below.

The listed order still governs which id a refusal names first, so messages stay
deterministic. Probing every kind costs no more than the old cascade: the plan
and campaign probes read the same `plans/` directory, and `show`/`kill` already
walked it on every invocation.

### Prefix rule

One rule for all kinds: a reference matches if it is a prefix of exactly one
id of that kind. The existing `load_run` prefix semantics
(`core/src/state.rs:584`, `:609`) are canonical; the other kinds adopt them.

### Ambiguity

Two distinct failure modes, two distinct refusals — do not collapse them:

- **Within a kind:** the existing "ambiguous" message from the kind's loader is
  passed through unchanged.
- **Across kinds:** a prefix matching one run *and* one plan is a refusal that
  names both full ids and asks for more characters. This case cannot arise
  today because no verb probes more than one kind by prefix, so it needs a
  fixture, not a search for a live example. After Shakedown every multi-kind
  verb can reach it, which is why collect-all replaced first-match-wins.

### `latest`

`None`, `"latest"` and `"last"` all mean: **the most recently updated item, of
any kind in `accepts`, in the current scope, ordered by `updated_at`
descending.** `--all` widens the search to every scope and changes nothing
else.

This ends two divergences at once. `latest_run` (`main.rs:11012`) is
scope-bound and takes the head of `list_runs` order; `resolve_latest_run`
(`verdict.rs:77`) ignores scope entirely and sorts by `updated_at`. After P2
there is one function and one ordering key.

When `accepts` includes more than `RUN`, `latest` may now resolve to a plan.
That is intended: in a scope holding only plans, `deadreckon status` should
describe the plan, not refuse.

## Verb acceptance matrix

`accepts` per verb. `→ verb` means the kind is refused with a `try:` naming
that verb.

| Verb | Run | PlanChild | Plan | Chain | Campaign |
|---|---|---|---|---|---|
| `status` | yes | yes | yes | yes | yes |
| `show` | yes | yes | yes | yes (new) | yes |
| `attach` | yes | yes | yes | yes | yes |
| `kill` | yes | yes (new) | yes | yes | yes |
| `finish` | yes | yes | yes | yes | → `show` |
| `verdict` | yes | yes | → `show` | → `show` | → `show` |
| `report` | yes | yes | → `show` | → `show` | → `show` |
| `resume` | yes | yes | → `fork` | → `chain resume` | → `attach` |
| `steer` | yes | yes | → `attach` | → `attach` | → `attach` |
| `doc` | yes | yes | yes | → `show` | → `show` |
| `undo` | yes | yes | → `show` | → `show` | → `show` |
| `rewind` | yes | yes | → `show` | → `show` | → `show` |
| `extend` | yes | yes | → `fork` | → `chain` | → `show` |
| `merge` | — | — | yes | → `show` | → `show` |

Cells marked "(new)" are coverage this slice adds because the shared resolver
provides it, not because the verb is being redesigned. Where a verb's current
behavior for a kind is richer than this table (for example `finish` on a plan),
keep the richer behavior and correct the table in P11 — the matrix describes
minimum coverage, not a ceiling.

## Refusal-footer canonical pairs

Every refusal is `<reference> is a <noun>, not a <expected>.` plus one `try:`.

| Situation | Refusal | `try:` |
|---|---|---|
| plan id given to `status` (pre-fix) | `0c11f68e is a plan, not a run` | `deadreckon show 0c11f68e` |
| plan id given to `verdict` | `0c11f68e is a plan; verdicts describe gated runs` | `deadreckon show 0c11f68e` |
| chain id given to `verdict` | `<id> is a chain, not a run` | `deadreckon show <id>` |
| campaign id given to `report` | `<id> is a campaign, not a run` | `deadreckon show <id>` |
| plan id given to `steer` | `<id> is a plan; steering targets one executing run` | `deadreckon attach <id>` |
| plan id given to `resume` | `<id> is a plan, not a run` | `deadreckon fork <id>` |
| chain id given to `resume` | `<id> is a chain, not a run` | `deadreckon chain resume <id>` |
| unknown reference, runs exist | `no run, plan, chain or campaign matches <id>` | `deadreckon list` |
| unknown reference, nothing exists | `no runs or plans yet` | `deadreckon start "<goal>"` |
| ambiguous within a kind | existing loader message, unchanged | `deadreckon list` |
| ambiguous across kinds | `<id> matches run <a> and plan <b>` | `deadreckon show <a>` |
| `latest` with an empty scope | `nothing in this project yet` | `deadreckon start "<goal>"` |
| `latest` empty, other scopes have work | `nothing in this project yet; other projects have runs` | `deadreckon list --all` |

`deadreckon list` remains a legal `try:` target **only** for a reference that
`list` did not print — that is, genuine typos and ambiguity. The journey test
enforces the distinction.

## The journey test (the point of the slice)

Lives in `crates/deadreckon/tests/coherence.rs`, which already holds 45
cross-cutting invariants and is the right home.

Fixture: one `DEADRECKON_HOME` containing a completed run, a pending plan with
two child tasks, one completed plan child, a chain, and a campaign — built with
the existing `save_state` / `save_plan` / `save_chain` helpers that
`coherence.rs` already imports. No provider is invoked.

The test parses the id column from `deadreckon list` and `deadreckon list
--all`, then for each id crosses it with the covered verb set:

```
for id in ids_printed_by_list:
    for verb in [status, show, verdict, attach, finish, kill]:
        out = run(verb, id)
        if out.is_success(): continue
        try_cmd = parse_try_line(out)          // must exist
        assert try_cmd is not None
        assert try_cmd != "deadreckon list"    // the loop this slice closes
        assert run(try_cmd).is_success()       // the named verb really accepts it
```

`attach` runs non-interactively (it already prints a summary and returns when
stdout is not a TTY, as the reproduction above shows). `kill` runs last and
against a clone of the fixture home so earlier assertions see live state.

The verb set grows one phase at a time (P4 → `status`, P5 → `verdict`, P6 →
`show`/`attach`, P7 → `finish`/`kill`) so each phase's test is red before its
own implementation and green after. Do not write the full six-verb version in
P4 and leave it failing.

## Phases (eleven)

Each phase: write the named depth tests **first** and watch them fail;
implement; `make verify` green; conventional-commit local commit; one-line
CHANGELOG entry naming the SHA.

### P1 — `reference.rs` skeleton, types, probe order

- Add `commands/reference.rs` with `ResolvedRef`, `RefKind`, `RefKinds`,
  `RefQuery` and `resolve_ref`, implementing the probe order, prefix rule and
  both ambiguity refusals.
- Move `resolve_plan_child_ref`, `resolve_plan_child_task`, `resolve_plan_id`
  and `PlanChildSelection` out of `main.rs` into `reference.rs` unchanged, and
  re-export them at the crate root with `pub(crate) use` so the
  `use super::super::*` in every command module keeps resolving and no existing
  call site changes with the move.
- Factor `plan_ids_matching` out of `resolve_plan_id` so plans can be probed
  without a refusal on zero matches.
- No verb is rewired. No behavior changes. Because nothing calls `resolve_ref`
  until P4, the module carries a `#![allow(dead_code)]` with a comment naming
  P8 as its removal point — `make clippy` lints the non-test build and would
  otherwise fail the phase.

Depth tests (`crates/deadreckon/src/commands/reference/tests.rs`):
- `probe_order_prefers_plan_child_over_run_prefix`
- `prefix_matching_both_a_run_and_a_plan_is_refused_as_ambiguous`
  (replaces `run_prefix_resolves_before_plan_with_same_prefix`, which asserted
  the first-match-wins behavior the revised probe-order spec rejects)
- `chain_id_resolves_when_no_run_or_plan_matches`
- `campaign_id_resolves_when_no_other_kind_matches`
  (replaces `campaign_probe_runs_last_and_is_skipped_when_a_run_matches`;
  under collect-all no probe is skipped, so the old name asserted a property
  the spec no longer has)
- `ambiguous_prefix_within_runs_passes_through_loader_message`
- `ambiguous_prefix_across_run_and_plan_names_both_full_ids`
- `unknown_reference_with_no_state_refuses_with_start_not_list`
- `unknown_reference_with_existing_state_refuses_with_list`
- `accepts_narrows_which_kinds_a_verb_will_take` (renamed in P3: every kind is
  probed and `accepts` narrows the decision, not the probe)
- `ref_kinds_all_contains_every_kind`
- `plan_ids_matching_with_empty_prefix_lists_every_plan`
- `plan_ids_matching_ignores_directories_without_plan_json`

### P2 — One `latest`

- Implement `latest` inside `resolve_ref` per the rule above: newest accepted
  kind in scope by `updated_at`, `--all` widens scope only.
- `latest_run` and `resolve_latest_run` become thin shims over it. Deletion is
  P8.

Depth tests (`reference.rs`):
- `latest_is_scope_bound_by_default`
- `latest_all_widens_to_every_scope`
- `latest_orders_by_updated_at_across_kinds`
- `latest_resolves_to_a_plan_when_the_scope_has_no_runs`
- `latest_and_last_are_the_same_reference`
- `latest_in_empty_scope_names_other_scopes_when_they_have_work`

### P3 — Refusal table

- Add `refusal_for(kind, verb) -> (String, String)` returning message and
  `try:` per the canonical-pairs table, parameterized so every row is exercised
  by one test.
- Route it through the existing `deadreckon_core::user_error` /
  `VerdictSurface` refusal path — no new rendering.

Depth tests (`reference.rs`):
- `every_refusal_pair_in_the_table_has_a_message_and_a_try` (table-driven,
  iterates all kind × verb cells)
- `refusal_try_target_is_never_the_originating_verb`
- `refusal_names_the_operator_noun_not_the_rust_type`

### P4 — Rewire `status`

- `status_command` (`main.rs:11026`) calls `resolve_ref` with
  `RefKinds::ALL` and renders per kind, reusing the existing plan / chain /
  campaign summary renderers that `show` already calls.
- Successful run output is unchanged, byte-for-byte.

Depth tests (`crates/deadreckon/tests/coherence.rs`):
- `status_on_a_plan_id_describes_the_plan_instead_of_refusing`
- `status_on_a_chain_id_describes_the_chain`
- `status_latest_resolves_a_plan_when_the_scope_has_no_runs`
- `status_list_status_sequence_terminates_in_an_answer` (the exact
  reproduction above)
- `journey_ids_from_list_are_accepted_by_status`

### P5 — Rewire `verdict` and `report`

- Both call `resolve_ref` with `RUN | PLAN_CHILD` and refuse other kinds via
  the P3 table.
- Delete `verdict.rs`'s private `resolve_latest_run`; `latest` now comes from
  the shared rule, which also fixes verdict's silent cross-scope reach.

Depth tests (`coherence.rs`):
- `verdict_on_a_plan_id_points_at_show_not_list`
- `report_on_a_campaign_id_points_at_show_not_list`
- `verdict_latest_is_scope_bound_like_every_other_verb`
- `journey_ids_from_list_are_accepted_or_redirected_by_verdict`

### P6 — Rewire `show` and `attach` (behavior-preserving)

- Replace both hand-rolled cascades with `resolve_ref(RefKinds::ALL)`.
- `show` gains chain support, which it lacks today.
- The `show`/`attach` characterization goldens must pass **without
  regeneration**. If a golden moves, the rewire is wrong — fix the rewire.

Depth tests (`coherence.rs`):
- `show_resolves_a_chain_id`
- `show_output_for_run_plan_and_campaign_is_unchanged_after_rewire`
- `attach_non_tty_summary_is_unchanged_after_rewire`
- `journey_ids_from_list_are_accepted_by_show_and_attach`

### P7 — Rewire the lifecycle tail

- `kill`, `finish`, `steer`, `doc`, `resume`, `undo`, `rewind`, `extend`,
  `merge` each call `resolve_ref` with their matrix row.
- Mechanical: swap the cascade, declare `accepts`, let P3 handle refusals.
  Where a verb currently handles a kind more richly than the matrix requires,
  keep the richer path.

Depth tests (`coherence.rs`):
- `kill_resolves_a_plan_child_ref`
- `steer_on_a_plan_id_points_at_attach`
- `resume_on_a_chain_id_points_at_chain_resume`
- `every_id_taking_verb_declares_its_accepted_kinds` (source-level assertion
  that no command module calls a kind loader directly)
- `journey_ids_from_list_are_accepted_or_redirected_by_finish_and_kill`
  (completes the six-verb journey matrix)

### P8 — Delete the old cascades

- Remove `load_cli_run`, `load_cli_run_with_scope`, `latest_run`,
  `resolve_verdict_run`, `resolve_latest_run` and every per-verb probe
  sequence. All 58 call sites now route through `resolve_ref`.
- Remove the `#![allow(dead_code)]` P1 added to `reference.rs`. If anything in
  the module is still unreachable with every verb rewired, it is unused code
  and gets deleted, not re-allowed.
- Update the `public_surface` baseline in the same commit, deliberately.
- Net line count in `main.rs` must go **down**; record the delta in the commit
  body.

Depth tests (`coherence.rs`):
- `no_command_module_calls_load_run_directly` (source scan, same shape as the
  existing `raw_ansi_escapes_stay_in_ui_module` test at `coherence.rs:28`)
- `reference_module_is_the_only_resolver`

### P9 — `list` coherence

- One row per plan; child runs fold under their parent and are not printed as
  peer top-level rows.
- Strip launch scaffolding from displayed goal text — the
  `This is one full-plan child run in a larger plan. Root goal: …` prefix is
  prompt plumbing and must not reach the inventory column.
- The plan's goal is printed once, on the plan row.
- `list --json` keeps its current flat shape; folding is a rendering concern.

Depth tests (`crates/deadreckon/tests/coherence.rs` and
`tests/cards_status.rs` as the row-rendering home):
- `plan_children_fold_under_the_parent_plan_row`
- `child_goal_scaffolding_is_stripped_from_the_goal_column`
- `plan_goal_is_printed_once_per_plan`
- `list_json_shape_is_unchanged_by_folding`

### P10 — Secondary-action cap and `--json` parity

- `VerdictSurface` caps `secondary_actions` at three; overflow becomes one
  `deadreckon help-all` pointer. `doctor` currently emits ten
  (`commands/doctor.rs:282`, `:344`) — it collapses here with no doctor-specific
  logic.
- Every new refusal carries the same `try:` in `--json` as in human output.

Depth tests (`crates/deadreckon/tests/friendliness.rs`):
- `secondary_actions_never_exceed_three`
- `doctor_secondary_actions_collapse_to_the_cap`
- `refusal_try_line_is_identical_in_json_and_human_output`

### P11 — AS-BUILT §56 + CHANGELOG (doc only; no depth test)

- Insert into `docs/AS-BUILT-ARCHITECTURE.md`:
  ```
  ## 56. Shakedown: One Reference Resolver

  56.1 The problem: per-verb cascades and two meanings of `latest`
  56.2 ResolvedRef and the acceptance matrix
  56.3 Probe order, prefix rule, ambiguity
  56.4 Kind-aware refusals and the no-loop invariant
  56.5 The cross-verb journey test
  56.6 List folding and the secondary-action cap
  56.7 V1 boundaries
  ```
  §53 and §54 stay reserved for Capstan and Drydock; do not renumber Pennant
  (§55).
- Correct `docs/FRIENDLINESS-AUDIT.md`: the `status` and `verdict` "Refuse with
  try:" notes are now accurate rather than accidentally so. Add one line above
  the table stating that per-verb scoring is complemented by the journey test,
  and name it.
- Append a `## Shakedown (stable) — one reference resolver — <date>` CHANGELOG
  section listing each phase SHA, matching the Keel/Rudder format.

## Out of scope (explicitly → V1-CANDIDATES)

- **A `RunView`-backed read model for plans, chains and campaigns.** This slice
  unifies *resolution*; unifying *projection* is the larger follow-on the map
  calls for and needs its own slice.
- **Namespacing the 44 top-level verbs.** Progressive disclosure via `help-all`
  already works; regrouping is a separate product decision.
- **A durable id index.** Probing directories is fast enough at current scale;
  an index is state, and this slice adds none.
- **Renaming or aliasing any verb.** Coherence here is about agreement, not
  vocabulary.
- **Cleaning up the 53-day-old pending plans the reproduction surfaced.** That
  is a `cleanup` policy question, not a resolver question.
- **`list --json` folding.** Rendering only, this slice.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (utility, free): none needed — `RefKinds` is a hand-rolled `u8` newtype,
not a `bitflags` dependency, because it has five flags and one crate boundary.

Tier 2 (architectural, log to `DEPENDENCIES.md`): none expected.

Tier 3 (blocked): same blocks as prior riders.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes, and no durable state of any kind.**
- **One depth test before each phase implementation.** A phase whose tests were
  never red is suspect.
- **Successful output is frozen.** Only refusals and `list` rows may change.
  `show`/`attach` goldens passing without regeneration is the proof.
- **No refusal may name the verb it came from, and no refusal for an id that
  `list` printed may name `list`.** This is the slice's reason to exist.
- **`resolve_ref` is the only resolver after P8.** No second path, no
  "temporary" fallback.
- **The refusal table is the spec.** Changing a message changes the table and
  its test together.
- **No silent expansion.** Anything beyond P1–P11 goes to `V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing, `make verify` green, and a
  CHANGELOG entry naming the SHA.
- `make verify` includes `fmt-check`, `clippy -D warnings`, `public-surface`,
  workspace tests and a release build. CI runs a thinner set; green CI is not
  evidence a phase is done.
- If a phase reveals a V1 architecture decision, log it in `V1-CANDIDATES.md`
  and continue — do not expand scope.
- After P11, capture an asciinema cast of the closed reproduction
  (`status` → `list` → `status <id>`) under `/Users/gdc/deadreckon`. This
  change is user-visible, so the demo is not optional.
