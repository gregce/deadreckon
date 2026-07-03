# deadreckon — Helm Rider (mission control for the whole voyage)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-07-01-2011-deadreckon-helm-goal.md`.
It supersedes nothing in prior riders (attach-tui-uniformity, live-narrator,
orchestrate-campaign-narration, verdict, **course**); their invariants still
apply. Course (stable, AS-BUILT §46) landed after this rider was drafted and
introduces durable artifacts helm reads — `launch-plan.json` (the decision
record in every dispatched root: shape, pieces, budget ceiling, contract,
confidence), an inert `reshape-proposal.json` + `reshape.proposed` trace (a
worker-proposed decomposition — **non-terminal, the run keeps working**, not a
pause), and `[defaults] start_attach` making attach the auto-entered
post-launch surface. Helm surfaces these; it invents none of them. This
rider adds: the **status spine** contract, the flattened **event tree**, an
**event-driven async loop**, **render decomposition**, **command mode**,
**in-frame input**, the **why panel**, the **turn timeline**, a
**motion-policy effects layer**, and **chain narrative parity**.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime `~/.deadreckon`.

Design creed (from the best TUIs alive — lazygit, k9s, zellij): spatial
consistency, keyboard fluency, information density that respects attention;
expert speed without abandoning beginner discoverability. Helm's twist: every
pixel of comprehension is backed by a durable artifact — the spine reads
ledgers, the why panel cites proofs, the timeline scrubs snapshots. Nothing
rendered is ever a guess.

## Posture (decided — do not redesign)

- **Maturity stays stable** (0.4.0 shipped; lands under a `Helm` CHANGELOG section).
- **ratatui 0.29 + crossterm 0.29 stay.** No framework migration. Evaluated and rejected: iocraft (React-like, young, full rewrite), r3bl_tui (async-first but a rewrite), rooibos (maturity risk). ratzilla (`attach --web`, same render code → browser) is explicitly V1. New crates are WIDGETS, not frameworks, pinned to ratatui-0.29-compatible releases.
- **The non-blocking render contract is sacred and inherited verbatim** (AS-BUILT §18): render never calls a provider inline; feeds tail complete JSONL rows only; expensive work goes to caches or background `tokio::spawn` polled between frames; provider failure degrades to the stale deterministic snapshot; q/Esc detach is instant even mid-provider-call. Every new pane (tree, why, timeline) is bound by it.
- **Read models only.** Helm adds NO durable schema. The tree, spine, why report, and timeline are projections computed from existing files (state.json, events/spend/traces JSONL, plan-events, campaign-events, snapshots, proofs, and the Course artifacts `launch-plan.json` + `reshape-proposal.json`). A `spine.json`-style cache may be written per attach session under the run's `narrative/` dir but is never read as authority. `launch-plan.json` is authoritative for the budget ceiling and per-piece goal labels where present (an enhancement — the inferred values remain the fallback when a root predates Course); helm never writes it.
- **Behavior-preserving decomposition.** P1 splits render.rs with zero behavior change; the characterization goldens and public-surface baseline must not move. New behavior lands only in later phases.
- **Command mode maps to existing verbs only.** `:` commands shell to the same code paths as the CLI verbs (kill/resume/verdict/why). No new state-changing operations are invented inside the TUI.
- **Motion policy before any effect.** No tachyonfx call ships before the `[ui] motion` config and its plumbing exist (P14 gate). Effects are event-triggered, bounded (< 800ms), and never block input.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Attach daemon, ratzilla web mirror, replay-with-original-timing export, cross-machine attach, tui-term full pty emulation go to V1-CANDIDATES.
- **Edits stay inside `/Users/gdc/deadreckon`.**

## Data model (read models — files in, projections out)

### SpineSnapshot (the five questions, uniform)

```
struct SpineSnapshot {
    alive: Aliveness,        // Live{last_event_age}, Stale{age}, Dead{reason}, Done
    doing: String,           // "turn 6 · bash (12s)" | "piece 2/3 merging" | "step 3/5 applying"
    on_track: OnTrack,       // gate {passed}/{total}, spend used/ceiling (ceiling from launch-plan.json when present), turns used/max
    wrong: Vec<Attention>,   // gate failure, tamper caveat, provider error, stall (> threshold quiet), paused-at-cap, reshape-proposed (inert, non-terminal)
    next: PrimaryAction,     // exactly one recommended command (VerdictSurface doctrine, live); a pending reshape-proposal.json → `deadreckon reshape <id>`
}
fn spine_for_run(...) -> SpineSnapshot      // and _plan/_chain/_campaign
```

The **spine contract table** mirrors `friendliness_contract.rs`: a const
table of 5 questions × 4 surfaces; a depth test asserts every cell has a
non-placeholder computation and the AS-BUILT §47 doc table matches it
(doc==code, enforced).

### TreeModel (the voyage, flattened)

```
struct TreeNode {
    id: NodeId,              // campaign/sub/plan/task/run/chain-step identity
    kind: NodeKind,
    label: String,           // goal fragment, truncated display-width-aware
    status: NodeStatus,      // pending/running/gated/verified/failed/paused/killed
    gate: Option<(u32,u32)>, // checks passed/total
    spend: Option<f64>,
    children: Vec<TreeNode>,
}
fn build_tree(root: AttachTarget) -> TreeModel   // pure over durable files
fn fold_events(&mut TreeModel, batch: &[Event])  // incremental updates from tails
```

Built once from state files, then incrementally folded from the same JSONL
tails attach already consumes (run events, plan-events, campaign-events —
multiplexed the way plan attach already multiplexes child runs). Depth is
bounded by the existing CAMPAIGN_MAX_DEPTH=2, so the tree is ≤ 4 levels.

### WhyReport (cited evidence, deterministic)

```
struct WhyReport {
    verdict_line: String,            // one sentence: what stopped/failed and the operative cause
    causes: Vec<CitedCause>,         // {kind, summary, evidence_path, excerpt}
    next: PrimaryAction,
}
```

Deterministic classifier over: `failure_reason`/`pause_reason` in state,
last `acceptance.failed` trace + failing check detail, tamper verdict file,
last provider error trace, cancel marker, spend/wall cap events. Every cause
carries the real artifact path + a bounded excerpt. A pending reshape proposal
is **not** a why-cause (it is not a failure or pause) — it lives in the spine's
attention/next and as a timeline mark, never in the why panel. (This is the live seed of
options-file C1 `verdict --why`; a later slice may lift it into the verb —
Helm lands it as the attach panel.)

### TimelineModel

```
struct TimelineEntry { turn: u32, at: DateTime, story: String, files: (u32,u32,u32), spend_delta: f64, marks: Vec<Mark> }
// Mark: GatePass, GateFail, TamperCaveat, Reshape, Checkpoint
// Reshape mark source = the `reshape.proposed` trace (turn-stamped) written by Course C-P12.
```

Computed from turn-doc checkpoints + narrative snapshots + spend ledger +
flight checkpoints. Scrubbing selects an entry; the detail pane shows that
turn's story and diff counts. Read-only — rewind stays the existing verb.

## The frame (layout spec)

Wide (≥ `NARRATIVE_SPLIT_WIDTH`): three regions plus footer.

```
┌ voyage ────────────────┬ detail: <selected node> ──────────┐
│ ▾ campaign 9a2f    $18 │ [n]arrative [a]ctivity [d]ocs [w]hy│
│  ▾ sub-1  ✓ verified   │                                    │
│  ▾ sub-2  ● running    │  (existing panes, rendered for the │
│    ▸ task-1 ✓ 14/14    │   selected node)                   │
│    ▸ task-3 ● t6  $2   │                                    │
│  ▸ sub-3  ⏸ paused     │                                    │
├ timeline ──────────────┴────────────────────────────────────┤
│ t1──t2──t3──●t6────────────────────────── gate 9/14  $7.20 │
├ spine ──────────────────────────────────────────────────────┤
│ ● live 2s · turn 6 · gate 9/14 · $7.20/$18 · 1 attention    │
│ ▶ next: deadreckon attach sub-3 (paused: spend cap)         │
└ q quit  ? keys  : command  w why  Enter zoom  Tab pane ─────┘
```

- Single-run attach renders the same frame with a one-node tree collapsed to
  a slim header band (no wasted column) — the spine/timeline/detail are
  identical, which is the uniformity point.
- Narrow terminals: tree becomes a breadcrumb band (existing drill-in
  preserved as fallback); spine always renders.
- The existing four-band run frame survives as the detail pane's activity
  view — decomposition, not demolition.

## Keys and command mode

Contextual keys extend the existing shared navigation core
(`dispatch_navigation`); no surface redefines a shared key.

| Key | Everywhere |
|---|---|
| `?` | help overlay (exists; gains per-pane sections) |
| `Tab`/`BackTab` | cycle panes (exists) |
| `Enter` | zoom into selected node (replaces forced drill-in) |
| `w` | why panel for selected node |
| `t` | toggle timeline focus / scrub with ←/→ |
| `n`/`a`/`d` | narrative / activity / docs detail views (exists) |
| `:` | command mode |

Command mode (`:`) — in-frame single-line input (ratatui-textarea), prefix-
matched against a fixed verb table, confirm-before-destructive preserved:
`:kill [id]`, `:resume [id]`, `:verdict [id]`, `:why [id]`, `:reshape [id]`
(preview/accept a pending reshape proposal — Course's `deadreckon reshape`
verb; confirm-before-dispatch, non-TTY refuses with `try:`), `:attach <id>`
(retarget), `:motion full|reduced|off`, `:q`. Unknown command → inline
refusal with the nearest match as `try:`.

## Phases (sixteen)

Each phase: named depth test(s) **first** (watch fail) → implement →
`make verify` green (fmt-check, clippy, public-surface, test, build) →
conventional-commit → one-line CHANGELOG entry naming the SHA.

### P1 — Render decomposition (behavior-preserving)
- Split render.rs (~2,905 lines) into `tui/surfaces/{run,plan,chain,campaign}.rs` + `tui/panes/{header,activity,narrative,docs,footer}.rs` behind the existing traits. Zero behavior change; goldens + public-surface baseline unmoved.

Depth tests:
- `characterization_goldens_unchanged_after_split`
- `each_surface_module_renders_via_shared_navigation`

### P2 — SpineSnapshot model + contract table
- `spine.rs`: the struct, four `spine_for_*` builders (pure over durable files), the 5×4 contract table, doc==code test scaffold. A pending `reshape-proposal.json` in a run root is an `Attention` item whose `PrimaryAction` is `deadreckon reshape <id>` — read straight from the inert artifact (never self-executed); the run stays live, so aliveness/doing are unaffected.

Depth tests:
- `spine_contract_table_has_no_placeholder_cells`
- `run_spine_reports_aliveness_from_event_age`
- `spine_next_action_is_exactly_one_command`
- `pending_reshape_proposal_surfaces_as_attention_and_reshape_next`
- `reshape_proposal_does_not_mark_run_paused_or_dead`

### P3 — Spine band wired into all four surfaces
- Bottom band renders the snapshot uniformly; attention items render as count + first item; `--plain`/off-TTY attach summaries print the same five answers as lines.

Depth tests:
- `all_four_surfaces_render_spine_band`
- `plain_attach_prints_five_spine_lines`
- `paused_run_spine_names_pause_reason_and_next`

### P4 — TreeModel: build + incremental fold
- `tree.rs`: `build_tree` from state files for run/plan/chain/campaign roots; `fold_events` from the existing multiplexed tails; unit-pure, no UI.

Depth tests:
- `campaign_tree_builds_four_levels_from_fixtures`
- `fold_events_updates_node_status_without_rebuild`
- `tree_depth_bounded_by_campaign_max_depth`

### P5 — Voyage pane UI (tui-tree-widget)
- Left pane with glyphs (✓ ● ⏸ ✗ ○), gate counts, spend; selection state; display-width-safe truncation (existing CJK-safe column math); single-run collapses to header band.

Depth tests:
- `tree_pane_renders_status_glyph_gate_and_spend_per_node`
- `single_run_attach_collapses_tree_to_header`
- `tree_selection_survives_event_fold`

### P6 — Selection drives detail; Enter zooms
- Detail pane renders the selected node through the existing views (activity/narrative/docs); Enter retargets zoom (old drill-in behavior) with breadcrumb; Esc backs out of zoom before quitting.

Depth tests:
- `selecting_child_node_renders_its_activity_in_detail`
- `enter_zooms_and_breadcrumb_backs_out`
- `campaign_leaf_state_visible_without_any_zoom`

### P7 — Event-driven loop
- Replace the 250ms poll: `tokio::select!` over crossterm `EventStream` (event-stream feature), ledger-tail wakeups (existing tails behind a channel; adaptive idle backoff), narrator/broadcast rx, and a coarse fallback tick. Add `AttachLoopStage::InputToFrame`; `AttachTickBudget` gains an input-latency budget.

Depth tests:
- `input_event_triggers_frame_without_waiting_full_tick`
- `idle_attach_backs_off_polling`
- `input_to_frame_stage_recorded_and_budgeted`

### P8 — Latency proof under storm
- A replayed high-rate event fixture (JSONL storm) drives the loop headlessly; assert frame coalescing (no unbounded redraws), input responsiveness stage within budget, and memory bounded (tail buffers capped).

Depth tests:
- `event_storm_coalesces_frames_within_budget`
- `storm_does_not_grow_tail_buffers_unbounded`

### P9 — In-frame input + modals (ratatui-textarea)
- One modal primitive: confirm (y/n), single-line input; replaces the suspend-the-alternate-screen prompts in attach (kill confirm, return prompts). Closes the named V1 deferral.

Depth tests:
- `kill_confirm_renders_in_frame_without_screen_suspend`
- `modal_swallows_keys_and_esc_cancels`

### P10 — Command mode
- `:` opens the input modal wired to the fixed verb table; commands dispatch to the same code paths as CLI verbs (including Course's `deadreckon reshape` via `:reshape [id]`); destructive/dispatching commands reuse the in-frame confirm; unknown → nearest-match `try:` inline.

Depth tests:
- `colon_kill_routes_through_existing_kill_path_with_confirm`
- `unknown_command_refuses_inline_with_nearest_match`
- `command_table_contains_only_existing_verbs`
- `colon_reshape_routes_through_existing_reshape_path_with_confirm`

### P11 — Why panel
- `why.rs` deterministic classifier + `w` pane rendering `WhyReport` with cited artifact paths and bounded excerpts; works for any tree node; `--plain` attach gains `--why` text parity.

Depth tests:
- `gate_failed_run_why_cites_failing_check_and_proof_path`
- `paused_at_cap_why_names_cap_and_next_action`
- `tamper_caveat_surfaces_in_why_causes`
- `why_never_renders_uncited_cause`

### P12 — Timeline band + scrub
- `timeline.rs` model from turn checkpoints/snapshots/spend; `t` focuses the band; ←/→ scrub selects a turn; detail pane shows that turn's story + diff counts; marks for gate/tamper/reshape events.

Depth tests:
- `timeline_entries_match_turn_checkpoints`
- `scrubbing_selects_turn_story_and_diff_counts`
- `gate_events_render_as_timeline_marks`
- `reshape_proposed_trace_renders_as_timeline_mark`

### P13 — Chain parity
- Chain attach gains the narrative view (closing the AS-BUILT §18.2 unsupported gap) and full spine/tree/timeline participation (steps as nodes).

Depth tests:
- `chain_attach_renders_narrative_view`
- `chain_steps_appear_as_tree_nodes_with_status`

### P14 — Motion policy + effects layer (tachyonfx)
- `[ui] motion = full|reduced|off` config + `:motion`; effect registry with exactly three triggers: gate pass (brief shimmer on the gate meter), verdict/completion (one-shot card flash), node state change (120ms glyph pulse). All bounded < 800ms, input-preemptible, absent under `reduced`(non-TTY/replay default) except completion, absent entirely under `off`.

Depth tests:
- `effects_fire_only_on_registered_triggers`
- `motion_off_renders_zero_effect_frames`
- `effect_never_delays_input_processing`

### P15 — Discoverability + help polish
- `?` overlay gains per-pane key sections + command-mode list; footer hints follow selection context (lazygit-style); first-attach hint line ("Tab panes · w why · : commands") shown once per session.

Depth tests:
- `help_overlay_lists_command_mode_verbs`
- `footer_hints_follow_focused_pane`

### P16 — Architecture doc + CHANGELOG (doc only; no depth test)
- Insert `## 47. Helm: mission-control attach` into AS-BUILT (spine contract table, tree model, loop design + latency budgets, command mode, why/timeline, motion policy) and update §18/§25/§27/§32/§36 and §46 (Course — helm surfaces its `launch-plan.json`/`reshape-proposal.json`/`reshape.proposed` artifacts and adds `:reshape`) cross-references; update §22 shipped list (flattened campaign tree and in-frame input move from thin → shipped; attach daemon/ratzilla stay deferred, stated).
- DEPENDENCIES.md: log tui-tree-widget, ratatui-textarea (or tui-textarea), tachyonfx with pins + one-line justifications.
- Append CHANGELOG:
  ```
  ## Helm (stable) — <date>
  - attach is mission control: uniform five-question status spine, flattened campaign→plan→run tree (zoom optional), event-driven input loop with pinned latency budgets, : command mode, in-frame modals, w-for-why cited evidence, scrubable turn timeline, chain narrative parity, and a motion-policy effects layer.
  ```
- Capture a demo cast (attach on a live campaign fixture: tree → w → timeline → :kill) under `docs/assets/`.

## Integration matrix

| Capability | run | plan | chain | campaign |
|---|---|---|---|---|
| Spine band | P3 | P3 | P3 | P3 |
| Tree pane | header band | tasks | steps (P13) | full 4-level |
| Narrative view | exists | exists | NEW (P13) | exists |
| Why panel | P11 | P11 (per task) | P11 (per step) | P11 (per sub) |
| Timeline | P12 | per-task on zoom | per-step | root = subs summary |
| Command mode | P10 | P10 | P10 | P10 |
| Effects | P14 | P14 | P14 | P14 |

## Error-footer canonical pairs (TUI-inline + plain parity)

| Error | `try:` |
|---|---|
| unknown `:` command | `try: :<nearest-match>` (inline) |
| `w` on a node with no failure artifacts | "nothing wrong recorded" + `try: n` for narrative |
| tree root state files missing | fall back to legacy surface + `try: deadreckon list` |
| effects requested but motion=off | `:motion full` named in the inline notice |

## Config additions

```toml
[ui]
motion = "full"          # full | reduced | off; reduced is forced for non-TTY/replay
input_latency_budget_ms = 50
```

## Out of scope (explicitly → V1-CANDIDATES)

- `attach --web` via ratzilla (same render code → browser; the honest path to origin-need #7).
- Long-lived attach daemon / shared broadcaster across processes.
- tui-term full pty emulation of provider sessions (Helm renders captured output files; live pty embed is V1).
- Replay-with-original-timing export and cross-session timeline analytics.
- Lifting the why classifier into `verdict --why` (options C1 — a separate verb slice that reuses `why.rs`).
- Themable palettes (standing deferral; effects respect the single palette).

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (in tree): tokio, crossterm (add `event-stream` feature), existing tails/event bus. Tier 2 (architectural — log in DEPENDENCIES.md with pins + rationale): `tui-tree-widget` (the voyage pane; hand-rolling a correct tree view is real scope), `ratatui-textarea` or `tui-textarea` (in-frame input; pinned ratatui-0.29-compatible — already named Tier 3-until-designed in the attach-uniformity rider; the design now exists, so it promotes), `tachyonfx` (effects; feature-gated so `--no-default-features` builds without it). Tier 3 (blocked): iocraft/r3bl/rooibos/ratzilla (framework swaps or V1), `notify` (ledger wakeups use the existing tail channel + adaptive backoff, not an fs-watcher dep).

## Engineering invariants (do not violate)

- **Non-blocking render contract everywhere** — new panes included. No provider call inline; stale-snapshot degradation; q/Esc instant.
- **Nothing rendered is a guess.** Spine/tree/why/timeline read durable artifacts; the why panel never shows an uncited cause.
- **Spine and key vocabulary are uniform across surfaces** — the contract table + shared navigation core enforce it; no surface-local key overrides.
- **Command mode adds no new state-changing operations** — existing verb code paths only, confirm-before-destructive preserved.
- **Effects are decoration, never information.** Every state change an effect announces is also visible statically; `motion = off` loses zero information.
- **Latency budgets are pinned by tests**, not vibes — the storm fixture is the regression net.
- **Decomposition phases are behavior-preserving** and golden-guarded; behavior changes land only in their own phases.
- **One depth test before each phase.** A phase whose tests were never red is suspect.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with its depth tests passing and a CHANGELOG entry naming the SHA.
- TUI depth tests render into ratatui's `TestBackend` buffers (existing pattern) + JSONL fixtures; no live providers, no real TTY required in CI.
- P1 (decomposition) and P7 (loop) are the two highest-regression-risk phases: run the full characterization suite at each, and prefer two smaller commits over one big one inside each.
- If a phase reveals a V1-architecture decision (e.g. the attach daemon becomes unavoidable for some surface), stop and log it in V1-CANDIDATES; do not silently expand scope.
