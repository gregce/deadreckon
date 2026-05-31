# deadreckon — Navigable Rider (campaign attach becomes a live TUI)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-29-1354-deadreckon-navigable-goal.md`.
It supersedes nothing in prior riders (notably
`2026-05-28-1841-deadreckon-campaign-rider.md` §36.9,
`2026-05-29-1600-deadreckon-decompose-rider.md` (the post-decompose module layout, AS-BUILT §38),
`2026-05-26-1546-deadreckon-narrative-attach-rider.md`,
`2026-05-18-2226-deadreckon-orchestration-eventbus-rider.md`) — their invariants
still apply. This rider replaces the campaign-attach plain-text stub (campaign P10)
with a live, interactive ratatui TUI that is **navigated by drill-in**.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime `~/.deadreckon/`.
**Post-decompose layout (AS-BUILT §38, all under `crates/deadreckon/src/`):**
`commands/attach.rs` (`attach_command`, `attach_plan_tui`, `attach_tui_with_parent`,
`suspend_tui`/`resume_tui`), `commands/attach_runtime.rs` (`AttachSurface`,
`AttachTickTiming`, `handle_key`), `commands/campaign.rs` (`resolve_campaign`,
`campaign_attach_summary`), `tui/render.rs` (render layer), `tui/attach_state.rs`
(`AttachParentPlan`), `plan_event_bus.rs` (`PlanEventFeed`, `JsonlTail`),
`ui_card.rs`. Test modules are lifted to src-level files (e.g. `campaign_spawn_tests.rs`,
`tui_tests.rs`) — **never inline in `main.rs`**.

## The mismatch this closes (read before designing)

`attach <campaign-id>` today resolves the campaign (`resolve_campaign`,
`commands/campaign.rs`), prints `campaign_attach_summary` (`commands/campaign.rs`), and
**returns before the ratatui loop** — no TUI, no live feed, no drill-in; its footer
literally tells the user to retype `attach <sub-plan-id>`. Plan
attach (`attach_plan_tui`, `commands/attach.rs`) is the opposite: a multi-pane ratatui
TUI with a tailing event feed and `Enter` drill-in into a child run
(`attach_tui_with_parent`, `commands/attach.rs`). The event model is two-level:
`PlanEventFeed` holds a single `plan_id` and discovers children by scanning that
one plan's `tasks[]` (`plan_event_bus.rs:74`, `:161-206`). A campaign is a
three-level tree (campaign -> N sub-plans -> child runs). We make attach navigate
that tree by **drilling**, reusing the plan/run TUIs unchanged — not by flattening
events into one feed.

## Posture (decided — do not redesign)

- **Production-release track. Presentation + READ-ONLY event tailing only.** No
  changes to gate/sandbox/promotion/providers/the campaign engine, and **none to
  `attach_plan_tui` / the run TUI** — campaign attach *calls* them. If a change
  would edit those, it is out of scope.
- **Files-not-fields.** No `PipelineState`/`Plan`/`Campaign`/provider schema
  changes. Every file the feed reads (`campaign.json`, `campaign-events.jsonl`,
  per-sub `plan-events.jsonl`, `campaign-rollup.json`) already exists. The change is
  read-side: no new event emission.
- **Reuse the primitives.** ratatui/crossterm, `ui_card` (`Card`, `TitleLine`,
  `MetricColumn`, `HintLine`, `Tone`, `render_card`), the generic `JsonlTail<T>`
  (`plan_event_bus.rs:334`), `AttachTickTiming`, `suspend_tui`/`resume_tui`. No new
  rendering framework, no new color/tree crate.
- **Drill-in, not flattened.** The navigation model is the existing one lifted one
  tier: descend into a sub-plan's own TUI, do not interleave leaf-run events at the
  campaign level.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** A flattened 3-level event stream, campaign narrative, a
  unified single-loop nav-stack rewrite, and mouse tree expansion are V1.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Data model (read-side only; no schema changes)

No new persisted files. Two in-memory types and one feed event:

```rust
// AttachSurface gains a Campaign variant (alongside Run/Plan/Chain) so the existing
// tick/handle_key machinery (commands/attach_runtime.rs) covers campaigns.
enum AttachSurface { Run, Plan, Chain, Campaign }

// In-memory render/selection state for the campaign pane (no disk schema).
struct CampaignAttachState {
    campaign: Campaign,                 // from campaign.json (read-only)
    rollup: Option<CampaignRollup>,     // from campaign-rollup.json if present
    aggregate_spend_usd: f64,           // summed live from leaf result runs
    feed: VecDeque<CampaignFeedEvent>,  // bounded, like plan attach's feed
    selected: usize,                    // selected sub-plan card index
}

// One-tier feed event (mirror PlanFeedEvent one level up).
enum CampaignFeedEvent {
    Campaign { event: CampaignEvent },          // from campaign-events.jsonl
    SubPlan { sub_id: String, event: PlanEvent },// from each sub-plan's plan-events.jsonl
    Snapshot { campaign: Box<Campaign> },
    Warning { message: String },
}
```

## CampaignEventFeed (the one structural addition)

`CampaignEventFeed` mirrors `PlanEventFeed` one tier up. It holds the `campaign_id`,
a `JsonlTail<CampaignEvent>` over `campaign-events.jsonl`, and a
`BTreeMap<sub_id, JsonlTail<PlanEvent>>` over each sub-plan's `plan-events.jsonl`.

- **Discovery:** read `campaign.json`; for every `sub_goals[].sub_plan_id` that is
  `Some`, register a `plan-events.jsonl` tail keyed by `sub_id`. Sub-plans appear as
  the campaign forks, so re-discover each poll (idempotent; absent files tolerated,
  like `PlanEventFeed`).
- **Emit:** drain the campaign tail as `Campaign{..}` and each sub tail as
  `SubPlan{ sub_id, .. }`. Dedup via a `seen` set as `PlanEventFeed` does. A sub-plan's
  `plan-events` already summarizes ITS child transitions, so the campaign pane shows
  real activity ("sub-1: task-0 completed") **without** tailing leaf runs.
- **Aggregate spend:** sum `total_spend_usd` across the sub result runs
  (`sub_goals[].result_run_id` -> `load_run`), refreshed on the feed cadence, shown
  against `campaign.tree_budget_usd` in the header.

This is the only place the event layer grows; it is additive (a sibling of
`PlanEventFeed`), changing nothing in the plan/run feed paths.

## Navigation & drill-in (the spec)

- The campaign loop renders the campaign pane and polls input on the same cadence as
  plan attach (feed refresh `Duration::ZERO`, input poll ~250ms), staged through
  `AttachTickTiming` with `AttachSurface::Campaign`.
- `Up`/`Down`/`j`/`k`/`Tab` move `selected` across the sub-plan cards (clamped).
- `Enter` on the selected sub-plan: `suspend_tui` the campaign terminal, call the
  existing `attach_plan_tui(sub_plan_id, … , parent = Campaign{campaign_id, sub_id})`,
  then `resume_tui` and continue the campaign loop. The plan TUI's own `Enter`
  descends into a child run as it already does — giving full `campaign -> plan -> run`
  navigation with zero changes to the plan/run TUIs.
- `b`/`Backspace`/`Esc`/`q` behave exactly as elsewhere (back one level / detach).
- **Nesting:** `campaign -> plan -> run` is two nested suspend/resume levels. If
  `suspend_tui`/`resume_tui` track a single boolean, make them a depth counter (or a
  small stack) so re-entrancy is correct. This is the highest-risk change; P6 owns it
  with a depth test.

## Verb behavior (`attach <campaign-id>`)

```
attach <campaign-id>             # resolve_campaign matches; then:
    (TTY, not --plain/--json)    -> enter the campaign ratatui TUI
    (off-TTY or --plain)         -> print campaign_attach_summary (reused), no TUI
    (--json)                     -> structured campaign attach JSON
```

`attach campaign latest` and an unambiguous id prefix both resolve (extend
`resolve_campaign` to honor `latest`/prefix like plan/run attach do).

## Testable seams (no real terminal)

- `render_campaign_attach_text(&CampaignAttachState, plain: bool) -> String` mirrors
  the plan render-to-string helpers in `tui/render.rs`: header + sub-plan cards +
  feed + footer rendered to a string the tests assert on.
- `handle_key` over `CampaignAttachState` (mirror the pattern in
  `commands/attach_runtime.rs`) returns the next
  action (`Select(i)` / `DrillInto(sub_id)` / `Back` / `Quit` / `Refresh`); tests drive
  keys and assert state/action, never pixels.
- The drill-in nesting test exercises `suspend_tui`/`resume_tui` depth without a TTY
  by asserting the depth counter returns to zero after `down; enter; <plan back>;
  <run back>` simulated transitions.

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them fail; implement;
green on `make verify` (the post-decompose gate: fmt --check + clippy -D warnings +
public-surface + `cargo test --workspace` + release) at every commit, with the
decompose CLI characterization snapshots green and the off-TTY attach golden updated
deliberately; conventional-commit local commit; one-line CHANGELOG entry. The live
VIEW (P1-P5) lands before DRILL-IN (P6).

### P1 — CampaignEventFeed (one-tier, read-side)

- Add `CampaignEventFeed` + `CampaignFeedEvent` reusing `JsonlTail<T>`; discover
  sub-plans from `campaign.json`; tail campaign + per-sub plan events; dedup.

Depth tests (a lifted src-level test file, e.g. `crates/deadreckon/src/navigable_tests.rs`, registered like `campaign_spawn_tests`/`tui_tests` — never inline in `main.rs`):
- `campaign_feed_discovers_all_sub_plans_with_plan_ids`
- `campaign_feed_emits_campaign_and_sub_plan_events_deduped`
- `campaign_feed_tolerates_absent_event_files`

### P2 — CampaignAttachState + AttachSurface::Campaign

- Add the `Campaign` surface variant and the state struct; seed from `campaign.json`
  + `campaign-rollup.json`; compute aggregate spend from sub result runs.

Depth tests:
- `campaign_state_seeds_from_campaign_and_rollup`
- `campaign_aggregate_spend_sums_sub_result_runs`

### P3 — render_campaign_attach_text

- Header (goal, status, roll-up verdict, tree budget, aggregate spend) + selectable
  sub-plan cards (sub-goal, status, result-run prefix, leaf spend) + feed + footer,
  via `ui_card`.

Depth tests:
- `render_campaign_attach_shows_header_subs_rollup_budget`
- `render_campaign_attach_marks_selected_sub`
- `campaign_footer_shows_keybindings_on_tty`

### P4 — handle_key + tick

- `handle_key` over the state (`Up`/`Down`/`j`/`k`/`Tab` select; `Enter` ->
  `DrillInto`; `b`/`Esc`/`q` -> back/quit; `r` -> refresh); wire `AttachTickTiming`.

Depth tests:
- `campaign_keys_move_selection_clamped`
- `campaign_enter_yields_drill_into_selected_sub`

### P5 — Wire the TTY loop

- `attach_command` campaign branch enters the ratatui loop on TTY; off-TTY/`--plain`
  keeps `campaign_attach_summary`. The loop refreshes the feed and redraws.

Depth tests:
- `attach_campaign_off_tty_prints_summary_not_tui`
- `attach_campaign_tty_path_constructs_campaign_state`

### P6 — Drill-in + two-deep nesting

- `Enter` -> `suspend_tui` -> `attach_plan_tui(sub_plan_id, parent=Campaign{..})` ->
  `resume_tui`. Verify/repair suspend/resume re-entrancy at depth 2.

Depth tests:
- `campaign_drill_into_sub_then_child_then_back_back_returns_to_campaign`
- `suspend_resume_depth_returns_to_zero_after_nested_drill`

### P7 — Campaign-tier breadcrumb

- Add an optional `CampaignParent { campaign_id, sub_id }` to `attach_plan_tui`;
  render `campaign <id> / <sub-id>` prefix; the run breadcrumb (`AttachParentPlan`,
  `tui/attach_state.rs`) shows the full `campaign/sub/plan/task -> run` chain when present.

Depth tests:
- `plan_drilled_from_campaign_shows_campaign_breadcrumb`
- `run_breadcrumb_shows_full_campaign_chain`

### P8 — Off-TTY plain + `--json`

- Off-TTY/`--plain` reuses `campaign_attach_summary`; `--json` emits a structured
  campaign attach object (id, status, rollup, subs with status/result/spend).

Depth tests:
- `attach_campaign_json_has_subs_rollup_and_budget`
- `campaign_summary_footer_keeps_drill_hint_in_plain_only`

### P9 — Live aggregate spend + TTY-aware footer

- Header shows live aggregate spend vs tree budget and the roll-up verdict; the TUI
  footer shows keybindings and drops the "retype `attach <sub-plan-id>`" hint (that
  hint stays only in the plain summary).

Depth tests:
- `campaign_header_shows_aggregate_spend_against_tree_budget`
- `tui_footer_omits_retype_hint_present_in_plain_summary`

### P10 — Parity + resolution + refusals

- Keybindings match plan attach (`Enter`/`b`/`q`/`r`/arrows/`Tab`); `attach campaign
  latest` and id-prefix resolve; refusals carry `try:` footers.

Depth tests:
- `campaign_keybindings_match_plan_attach_set`
- `attach_campaign_latest_resolves_most_recent_campaign`

### P11 — AS-BUILT + CHANGELOG + V1-CANDIDATES (doc only; no depth test)

- Rewrite AS-BUILT §36.9 (attach is now a live TUI, not a one-hop summary) and
  extend §18 (attach) with a campaign-surface subsection covering the
  `CampaignEventFeed`, drill-in nesting, breadcrumb tier, and off-TTY/JSON fallback.
- Update the "shipped vs scaffolding-thin" list: campaign attach moves from
  thin-summary to live TUI; state plainly it reuses `attach_plan_tui` unchanged.
- Append a `## Navigable (production release) — 2026-05-29` CHANGELOG section.
- Log to `V1-CANDIDATES.md`: a flattened 3-level event stream with ancestry tags; a
  campaign narrative projection (`--view narrative`); a unified single-loop
  nav-stack refactor of attach; mouse-driven tree expansion / graph layouts.

## Integration matrix

| Aspect | run attach | plan attach | campaign attach (this rider) |
|---|---|---|---|
| TUI | yes | yes | yes (new) |
| live feed | run events | plan + child-run events | campaign + per-sub-plan events |
| drill-in | n/a | Enter -> run TUI | Enter -> plan TUI (-> run TUI) |
| nesting depth | 0 | 1 | 2 (tested) |
| breadcrumb | plan/task | n/a | campaign/sub (-> plan/task -> run) |
| off-TTY | summary | summary | `campaign_attach_summary` (reused) |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| `attach: no campaign matches <id>` | `deadreckon list` then `deadreckon attach <campaign-id>` |
| `attach: campaign has no sub-plans yet` (pre-fork) | `deadreckon attach <campaign-id>` after fork, or `deadreckon status <campaign-id>` |
| `attach: not a TTY` (with `--json` absent) | `deadreckon attach <campaign-id> --plain` |

(Each footer is exercised by a P5/P10 depth test.)

## Out of scope (explicitly V1 candidates)

- A fully-flattened 3-level event stream interleaving every leaf-run event at the
  campaign level with ancestry tags (the drill-in model makes it unnecessary now).
- Campaign narrative projection / `--view narrative` for campaigns.
- A unified single-event-loop nav-stack rewrite of attach (keep nested loops if P6's
  two-deep nesting works).
- Mouse-driven tree expansion, collapsible nodes, and graph layouts.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (in-tree): `ratatui`, `crossterm`, `serde`/`serde_json`, the existing
`JsonlTail`. **No new crates expected.** Tier 2: none. Tier 3: same blocks as prior
riders.

## Engineering invariants (do not violate)

- **No edits to `attach_plan_tui` / the run TUI / the plan or run feed paths**, other
  than adding the optional `CampaignParent` breadcrumb param to `attach_plan_tui`.
- **No `PipelineState`/`Plan`/`Campaign`/provider schema changes; no new persisted
  files.** The feed is read-only over existing JSONL.
- **One depth test before each phase implementation.** TUI is tested via
  render-to-string + `handle_key`, never a live terminal.
- **Drill-in nesting is depth-correct.** `suspend_resume_depth_returns_to_zero_after_nested_drill`
  guards re-entrancy at depth 2.
- **No silent expansion.** Anything beyond P1-P11 -> `V1-CANDIDATES.md`.
- **Render shapes are spec-pinned.** `render_campaign_attach_text` output is
  depth-tested; changing its layout changes the spec.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing, `make verify` green (the
  post-decompose gate, incl. the CLI characterization snapshots), and a CHANGELOG
  entry naming the SHA.
- If P6 reveals that two-deep nested loops are structurally unsound (not just a flag
  fix), stop and log the unified nav-stack refactor in `V1-CANDIDATES.md` rather than
  expanding scope.
- Optional after P11: an asciinema cast of `attach <campaign-id>` drilling into a sub
  and back, under the repo demo assets.
