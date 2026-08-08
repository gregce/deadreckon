# VIZ-DRILLDOWN-SPEC.md — WAVE 2: visualization + drill-down on the goal-run surfaces

Companion to `/DESIGN.md` (the visual constitution — tokens, chips, one-accent law),
`design/REDESIGN-SPEC.md` (nomenclature + surface grammar, normative), and
`PRODUCT.md` (the five jobs). Operator brief (2026-08-08): *"1) More Visualization
(find awesome libraries) and 2) more drill down into detail where it can be
seamlessly provided"* across goal-run surfaces.

Library decision, made: **Swift Charts** (first-party, ships with macOS, zero
dependencies, honors the system's Reduce Motion / accessibility plumbing) for every
plotted chart, and **SwiftUI `Canvas`** for the architecture map (the data is small;
a graph library would be a dependency buying nothing). No third-party chart
dependency, ever. The app targets macOS 14+, so `chartXSelection(value:)` and
`chartXSelection(range:)` (both macOS 14 APIs) are available for hover and brushing.

Data corroboration for this spec: real runs under `~/.deadreckon` were read on
2026-08-08 (runstate runs `f3529e49…`, `ecc5e67b…`, `a5ab5dec…`; plan graph for the
driver job `aa49e5aa…`). Facts cited as (live) below were observed in those files,
not inferred.

Ground rules, restated as law:

1. **Every chart answers one of the five jobs** (PRODUCT.md) or it does not exist.
   No decorative dashboards. A chart that merely restyles a number the surface
   already prints is deleted in review.
2. **Charts are quiet instruments in the committed palette.** There is NO
   categorical chart palette in this app — identity is carried by position and
   printed labels, never by hue. The only inks a mark may wear are the chart tokens
   in §V0. Accent appears on exactly one datum per chart: the newest datum of a
   LIVE run (the live marker, DESIGN.md §6). Semantic colors keep their fixed
   meanings (danger = failed/error, warn = degradation) and never decorate.
3. **Charts degrade honestly.** A chart renders only recorded rows: no smoothing,
   no interpolation, no extension of a line past its last record, no forecast, no
   fake baseline. Zero data → the chart is absent (the surface's existing empty
   words carry it). Sparse data → discrete honest marks (§V0 sparse law), never
   thin fake bins. A stopped feed freezes the chart exactly where the ledger
   stopped, beside the existing warn chip — the chart never guesses "now".
4. **Every plotted value is also printed.** The feed rows, turn rows, check rows,
   and $-figures the app already prints are each chart's table twin; tooltips
   enhance, they never gate. No value is reachable only by hover.
5. **Drill-down is reading, never authority.** Every drill surface renders durable
   files or CLI envelopes verbatim (mono, selectable); nothing here adds a write
   path, and nothing re-derives a status word. Trust rules untouchable.
6. **One drill grammar** (§G). No new windows, no third pane, sheets stay
   write-only.
7. **Views stay thin.** Every series, bin, scale, duration, layout position, and
   decoded detail is a pure DeadreckonKit derivation with tests (§K). A view maps
   published values to marks — nothing else.

---

## V0. CHART TOKENS, MARK SPECS, AND THE SPARSE LAW (shared by every chart)

### Tokens — add to `Theme` (app target), the only inks a mark may wear

```swift
extension Theme {
    enum Chart {
        static let markQuiet  = Theme.textTertiary          // at-rest bars, ticks (≥3:1 on panel — graphics-safe)
        static let markLine   = Theme.textSecondary         // line strokes (5.8:1 on panel)
        static let markFill   = Theme.textSecondary.opacity(0.08)  // area washes — a wash, never a block
        static let gridline   = Theme.border                // hairline, SOLID, never dashed
        static let capRule    = Theme.textTertiary          // budget-cap hairline + caption label
        static let liveDatum  = Theme.accent                // ONLY the newest datum of a live run
        static let fail       = Theme.danger                // failed/error marks (fixed meaning)
        static let brushFill  = Theme.well                  // brush selection band
        static let brushEdge  = Theme.borderHover           // brush edges
    }
}
```

No multi-hue categorical set exists, so there is no palette to validate; the
contrast facts are DESIGN.md §2's (checked): `textSecondary` 5.8:1 and `accent`
5.4:1 on `panel`; `textTertiary` ~3:1 on `panel` is used for non-text graphical
marks only, always beside printed values.

### Mark specs (fixed)

- Bars ≤ 24px thick, square at the baseline; adjacent/stacked fills separated by a
  **2px gap in the surface color** — never a stroke drawn around a mark.
- Lines 2px, round cap/join (1.5px inside strips ≤ 20px tall). End-dots ≥ 4px with
  a 2px surface ring.
- Gridlines/axes: hairline `Chart.gridline`, solid, recessive; most strips hide
  axes entirely and carry their domain in captions ("14:02 … now").
- Text never wears a data color: values, labels, and captions use text tokens;
  identity comes from position and printed words.
- No chart animation. Value changes may ease 150ms; no entrance choreography; the
  only ambient motion in the app remains the breathing dot — charts never breathe.

### The sparse law

Each chart declares a floor. Below it, the chart either disappears (its surface's
empty words already exist) or degrades to **discrete marks** — one honest tick per
recorded row — with the count printed. Bins, scales, and axes appear only when the
data earns them. Above the floor, bin widths come from a fixed "nice" ladder
(1s, 2s, 5s, 10s, 15s, 30s, 1m, 2m, 5m, 10m, 30m, 1h, …) chosen so the span yields
≤ 72 bins; a zero-count bin renders as zero (absence of events is a fact), except
when the feed's tailer reported corruption — then the strip stops at the last
trusted row and the existing `…feed stopped` warn chip is the voice.

### Accessibility

Every chart carries an `accessibilityLabel` stating its summary sentence ("Spend
$4.12 of $25.00 over 7 turns"). Micro-marks that sit beside their own printed
numbers (turn token bars, check duration bars, PLAN captions) are
`accessibilityHidden(true)` — the numbers are the accessible content (the sparkbar
precedent). Hover tooltips repeat on keyboard focus where Swift Charts provides it;
nothing is hover-only (law 4).

---

## V. VISUALIZATION MAP

Eight candidates were weighed. Six are in, two are argued out. For each: where it
lives and which job it answers · exact data source · marks · empty/sparse behavior
· interaction.

### V1. Run header — spend burn strip  *(job 1: always know what's going on)*

**Where.** `RunHeaderView` (RunDetailView.swift), far right of the facts line
(row 2), 140×18. The text meter `"$4.12 of $25.00"` STAYS — numbers are facts
(DESIGN.md §9); the strip augments with the one thing the number cannot say: the
*shape* of the burn (steady, accelerating, stalled) against the cap.

**Data.** `detail.spendSeries` (new, §K1) — retained fold of `spend.jsonl` loop
rows via the existing spend tailer. Per point: `timestamp`, `turn`, `totalUSD`
(the running head `total_cost_usd`), `deltaUSD`, tokens, `model`, `provider`,
`wallSeconds`, `estimated`, `subscription`. `capUSD` from the last loop row
carrying one. Narrator rows are excluded by fold law (never summed across kinds —
TAILING.md). (live: `{"turn":1,…,"cost_usd":0.0,"total_cost_usd":0.0,"cap_usd":3.0,
"wall_time_seconds":50.7,"kind":"loop"}`.)

**Marks (Swift Charts).**
- `AreaMark(x: timestamp, y: totalUSD)` in `Chart.markFill` — the wash.
- `LineMark` 1.5px `Chart.markLine`, step interpolation
  (`.interpolationMethod(.stepEnd)`) — spend is a step function between records;
  a smooth curve would invent spend between turns (law 3).
- `RuleMark(y: capUSD)` hairline `Chart.capRule` when a cap exists; y-domain
  `0…max(capUSD, maxTotalUSD)` so the headroom is visible truth.
- End-dot 4px at the last point: `Chart.liveDatum` while
  `row.projection.phase != .terminal`, else `Chart.markQuiet`.
- X-domain `firstPoint…lastPoint`. Never extended to "now": a live run's flat
  right edge would claim knowledge the ledger doesn't have.
- Axes hidden (`.chartXAxis(.hidden)`, `.chartYAxis(.hidden)`); the text meter
  beside it is the axis.

**Empty/sparse.** Fewer than 2 loop points → the strip is absent (a one-point
line is a dot pretending). `spendIssue != nil` → strip freezes at the last row;
the existing `spend feed stopped` warn chip (already in the facts line) is the
voice. Subscription runs whose totals are honestly $0.00 (live case above) render
the flat zero line only when a cap exists to measure against; capless all-zero
series → absent (a zero line against nothing answers nothing).

**Interaction.** Hover: tooltip with the nearest point's `turn N · $total ·
+$delta` (via `chartXSelection(value:)`). Click: pins the **spend point popover**
(§D3). If the header is too narrow (`ViewThatFits`), the strip drops before any
text fact does.

### V2. Activity tab — event-density timeline, brushable  *(jobs 1+2)*

**Where.** `ActivityPaneView` (DetailCenterTabs.swift): a full-width density strip
(height 36 including its caption row) between the Activity header and the stream.
It REPLACES the header's per-turn sparkbar (REDESIGN-SPEC §A3.3's sparkbar is
superseded by this spec; the header keeps only the `N events` count). Rendered in
both Stream and Turns modes; it brushes the Stream (switching to Turns keeps the
chip but the filter applies on return to Stream).

**Data.** `detail.density` (new, §K2) — incremental fold of the events tail
(`events.jsonl` via the existing activity tailer): per event `timestamp`, plus two
marked subsets: `kind == "error"` stamps and `kind == "turn_started"` stamps.
(live kinds observed: turn_started, token_usage_delta, spend_delta,
tool_call_started, tool_call_result, docs_checkpoint, error, run_completed,
steer_delivered.)

**Marks (Swift Charts).**
- Bins: `BarMark(x: binStart, y: count)`, bar width from the bin ladder, 2px
  surface gaps, fill `Chart.markQuiet`. The newest bin's bar is `Chart.liveDatum`
  ONLY while the run is live — the "now" marker.
- Error events: full-height 2px `RuleMark` in `Chart.fail` at each error stamp,
  drawn over the bars (a state with fixed meaning, never decoration).
- Turn boundaries: 1px `Chart.gridline` ticks anchored to the bottom 6px at each
  `turn_started` stamp — structure, recessive.
- Caption row beneath: `HH:mm:ss` of domain start left, domain end right
  (`caption`/`textTertiary`, monospaced digits). No y-axis: counts live in hover.

**Empty/sparse.** 0 events → strip absent (the feed's own empty state stands).
Fewer than 12 events or span < 60s → **tick mode**: one 2px `RuleMark`
(`Chart.markQuiet`; errors `Chart.fail`) per event, no bars, no y encoding —
"events happened here", nothing more. `activityIssue != nil` → the strip stops at
the last trusted row; the existing `activity feed stopped:` line is the voice.

**Interaction.** Hover: crosshair + tooltip `14:02:10–14:02:15 · 8 events ·
1 error` (`chartXSelection(value:)` snapped to bin). Brush: drag
(`chartXSelection(range:)`) selects a time window — band fill `Chart.brushFill`,
edges `Chart.brushEdge` — and filters the Stream to that window, composing AND
with the text search. An active brush renders a neutral chip in the header:
`14:02–14:31 · 214 shown  ✕` (✕ = text-button, clears). Click without drag clears.
The brush is also the landing target for the PLAN phase drill (§D5).

### V3. Turns view — per-turn token bars (in/out) + duration  *(job 2)*

**Where.** `TurnsListView` rows (DetailCenterTabs.swift). Each turn row gains a
fixed-width micro-mark column so bars align down the list and turns become
comparable at a glance — the row list is the chart; no separate panel.

**Data.** `detail.turns` (`[TurnModel]`, existing) with two additions (§K4):
`TurnModel.wallSeconds` (folded from the events ledger's `spend_delta.
wall_time_seconds`, newly decoded — live: `"wall_time_seconds":80.696…`), and the
shared scale `TurnScale.derive(turns:)` → `(maxTokens, maxWallSeconds)` so every
row draws against the same maxima. Tokens exist today (`inputTokens`,
`outputTokens` from `token_usage_delta`).

**Marks (micro-marks, not Chart instances).** Plain SwiftUI rectangles — a
`Chart` per lazy row is weight without benefit; the Kit scale keeps them honest:
- Token bar: one 96×5 track per row: `in` segment `Chart.markQuiet`, 2px surface
  gap, `out` segment `Chart.markLine`; width = value / maxTokens. Key printed once
  in the Turns header: `▪ in · ▪ out` (10pt, textTertiary) — a legend, not
  per-row labels; the existing `in 1.2k out 3.4k` text stays on every row (the
  table twin).
- Duration bar: 96×3 track beneath, `Chart.markQuiet` at 45%, width =
  wallSeconds / maxWallSeconds; printed duration (`1m 21s`, mono 9.5) beside it.
- Both `accessibilityHidden(true)` — their numbers are printed on the row.

**Empty/sparse.** No turns → existing "No turns recorded yet." A turn with zero
tokens (no `token_usage_delta` yet) renders an empty track (honest absence), not
a zero-width sliver pretending to be data. `wallSeconds == nil` (legacy ledgers
without the field) → no duration bar, text `—`.

**Interaction.** The row itself is the drill (§D1): click expands the turn inline
(existing), where each trace entry now opens full tool I/O. Bars have no separate
interaction — they are reading aids on an already-interactive row.

### V4. Checks tab — per-check duration bars on terminal runs  *(job 4)*

**Where.** `ContractChecksView` → RECORDED CHECK RESULTS rows (and the per-attempt
history rows inside the §D2 expansion). Terminal runs only — recorded facts, not
streams; live rows keep their text durations.

**Data.** `detail.report?.deterministicChecks` (`[AcceptanceProgressRow.
CheckResult]` — `durationMS`, `passed`, `kind`, `command`, `cwd`) via
`report --json`; history from `report.attempts[].checks`. Scale from
`CheckDurations.derive(results:)` (§K5) → rows + `maxMS`.

**Marks.** Micro-mark: an 80×5 track right-aligned on each recorded check row,
fill `Chart.markQuiet` at 35% for passed, `Chart.fail` at 35% for failed (state,
fixed meaning); width = durationMS / maxMS, minimum 2px when a duration exists;
the printed `4.1s` stays (table twin). `accessibilityHidden(true)`.

**Empty/sparse.** Bars render only when ≥ 2 results carry `durationMS` — a lone
duration is a number, not a comparison. Checks without `durationMS` show no bar
and keep their text row. No report / report issue → the band's existing honest
words stand.

**Interaction.** The check row is the drill (§D2). Bars themselves add none.

### V5. PLAN band — elapsed-per-phase duration marks  *(job 3)*

**Where.** `PlanBandView` (RunDetailView.swift). Under each real pipeline step's
name: a quiet duration caption. This is deliberately NOT a chart — a strip of
seven phases does not earn bars; the numbers, set consistently, are the
visualization. Band grows ~8px (to ~72).

**Data.** `PhaseDurations.derive(phases:runStartedAt:currentPhaseID:status:now:)`
(§K6) over `detail.runState` — each `RunStateDoc.Phase` carries `updatedAt` (the
stamp of its last status change) and the doc carries `startedAt`. Durations are
**derived**: phase *i*'s elapsed = `updatedAt(i) − updatedAt(i−1)` (phase 1
baselines on `startedAt`), only when the stamps are monotonic and the phases
completed in order; the current executing phase shows a live `now − prev` clock.

**Marks.** Caption `mono 9`, `textTertiary`, monospaced digits: completed `2m41s`,
current phase `3m02s` ticking with the poll (no accent — the breathing dot is the
live marker), pending/failed-out-of-order → nothing. Tooltip on the caption:
"derived from the phase's status-change stamps (`updated_at`), not a recorded
duration" — the honesty caveat, always.

**Empty/sparse.** Fallback lifecycle strip (no `state.json` plan): no durations,
ever — the five-stage strip is a position, not a schedule. Non-monotonic stamps →
that phase shows no duration (never a negative, never a guess).

**Interaction.** Click a step → phase popover (§D5).

### V6. Overview + sidebar micro-sparklines — **ARGUED OUT**

Out, on two grounds, either sufficient:

1. **The bounded-tails contract forbids the data.** A per-row spend/activity
   sparkline needs a per-run time series; series exist only in run-local ledgers
   (`spend.jsonl`, `events.jsonl`), and the app's architecture holds tails for
   exactly ONE run — the selected one (JobDetailStore is "the whole tail budget",
   CONTRACTS.md). Fleet rows carry rollup heads, not series. Sparklines across
   the sidebar/Overview would mean tailing every listed run — the exact thing
   APP-3 was built to prevent — or inventing a new fleet-wide read path for
   decoration.
2. **They fail the taste bar on their own.** The craft floor names "sparklines …
   standing in for content" a refusal; triage rows answer "does this need me?"
   with a state word and durable facts. A 12-point spend squiggle on a 36px row
   adds ink, not decision power — the decision-relevant spend fact (`$4.12 of
   $25.00`) is already printed.

If a future Rust rollup ever carries a compact per-run spend series in
`list --json`, revisit under law 1; until then, no sparklines outside the open
run. Register nothing — this is a design decision, not a gap.

### V7. Story tab — the architecture map, two honest tiers  *(jobs 2+3)*

The real files decide this one (both inspected live):

- **Run-scope graph** (`runstate/<scope>/runs/<id>/narrative/architecture-graph.json`):
  a STAR — one `run` node → one `provider` node (`uses`) + up to 8 `file` nodes
  (`touches`), `layout.kind: "layered-tree"`, nodes truncated to 10 while
  `source_window.files` lists hundreds. A node-link render of a star is a worse
  Changes list wearing circles — decorative, anti-goal. **OUT as node-link.**
- **Plan-scope graph** (`plans/<jobID>/narrative/architecture-graph.json`, exists
  for driver/supervised jobs): a REAL DAG — 9 nodes across `run`/`task`/
  `provider` kinds, 20 edges including `spawns`, `owns`, `depends_on` with
  `blocks` labels between tasks, `layout.kind: "swimlane"`, per-node `status`,
  `weight`, `style_token`, a legend, and `layout.warnings`. This is progression-
  against-plan truth no list shows. **IN as Canvas.**

So the Story tab gains one section, **MAP**, with two presentations chosen by the
data itself:

**Tier 1 — evidence strip (run-scope star).** One row that says what the star
says, as a sentence of chips: `[run f3529e49 ((completed))] — uses → [cli:codex]
— touches → [143 files]` + the top-4 file nodes by `weight` as mono chips +
`+139 more →` (jump to Changes). Chip anatomy per DESIGN.md §5; each chip's tint
from the node's `style_token` via the §K7 token map. Hover a chip → its
`evidence` paths (mono tooltip). This renders the graph's actual information
content at star topology — no invented geometry.

**Tier 2 — Canvas DAG (plan-scope).** A bordered `panel` block, height 220,
drawn with `Canvas`:

- **Layout** (pure, §K7): columns = BFS depth from `layout.root_ids` (min depth
  over roots); within a column, order by `weight` desc then `id` — deterministic,
  no physics, no animation. Both observed `layout.kind` values ("swimlane",
  "layered-tree") are depth-layered left→right; the kind word renders as the
  block's caption verbatim.
- **Nodes**: the chip atom (radius 4, fill = token color at 10%, stroke at 45%,
  label 10.5 — `run`/`task` labels in UI type, `file` labels mono,
  middle-truncated to 18ch). `style_token` → color: `primary → accent` (the
  file's own legend says "active work" — the live marker, a lawful accent use),
  `success → success`, `warning → warn`, `danger → danger`, `muted →
  textTertiary`, unknown token → textTertiary with the raw word in the tooltip.
- **Edges**: 1px `Chart.gridline` cubic curves, left→right; no arrowheads (flow
  is stated once in the caption: "flows left → right"); edge labels (`spawns`,
  `blocks`, `uses`) appear on hover only, caption type near the edge midpoint.
- **Legend**: one caption row rendering the file's own `legend[]` verbatim
  (`● active work · ● done · ● risk or stale evidence · ● blocked or failed`).
- **Warnings**: `layout.warnings[]` verbatim beneath, warn tint, when non-empty.

**Data.** `detail.archGraph` + `detail.archGraphIssue` (new, §K8): the store
polls `plans/<jobID>/narrative/architecture-graph.json` first (driver jobs), else
`<runRoot>/narrative/architecture-graph.json`, mtime-cached like checkpoints,
decoded off-main into `ArchitectureGraphDoc` (§K7).

**Empty/sparse.** No graph file → no MAP section (silence, not placeholder).
Decode failure → `map unreadable: {reason}` verbatim, warn, one line. Node count
> 40 → the Canvas refuses and Tier 1's strip renders instead with `graph too
large to draw honestly — {n} nodes` (never a hairball). `generated_at` renders as
a quiet caption (`drawn 45s ago`), staleness words only via the existing
narrative staleness chip — the map never claims freshness itself.

**Placement.** MAP sits after the deterministic snapshot body, before the AI
overlay block — it is deterministic evidence and must never sit inside or below
the unverified overlay's frame.

**Interaction.** Hover node → name + status + kind (tooltip). Click node → node
popover (§D7). Hover edge → its label. No pan/zoom/drag — the graph fits or it
refuses (sparse law); dragging nodes is physics theater.

### V8. Recorder tab — checkpoint timeline scrubber  *(jobs 2+3)*

**Where.** `FlightView` (DetailCenterTabs.swift), a full-width strip (height 44
incl. caption) between the RECORDER facts and the checkpoint cards.

**Data.** `CheckpointTimeline.derive(checkpoints:sessions:runStartedAt:now:live:)`
(§K9) over `detail.flight.checkpoints` (each: `checkpointID`, `createdAt`,
`deadreckonTurn`, `trigger`, `fullAnchor`) and `detail.flight.manifest?.sessions`
(each: `startedAt`, `status`, `provider` — sessions carry NO end stamp, so no
span is ever drawn for them). Domain: `runState.startedAt … last checkpoint`
(live runs extend to the last *recorded* stamp, never to now — law 3).
(live: 9 checkpoints `cp-000001…9` across one turn.)

**Marks (Swift Charts).**
- One `RuleMark(x: createdAt)` per checkpoint: full-height; `fullAnchor` ("full
  snapshot") ticks 2px `Chart.markLine`, incremental ticks 1px `Chart.markQuiet`.
  The newest checkpoint's tick is `Chart.liveDatum` while the run is live.
- Session starts: 1px `Chart.gridline` ticks in a bottom 6px lane (boundaries
  only — an end would be invented); a `failed`/`killed` session's start tick is
  `Chart.fail`.
- Caption row: domain start `HH:mm:ss` left, end right; `N checkpoints ·
  M sessions` centered (all caption/textTertiary).

**Empty/sparse.** No checkpoints → strip absent ("No checkpoints captured yet."
stands). Exactly 1 → a single tick with its stamp as the only caption — no axis
pretense. Zero-width domain (all stamps equal) → ticks at center, stamps printed.

**Interaction.** Hover tick → `cp-000007 · turn 3 · after a tool ran · 14:02:11`
(trigger words via the existing `Lexicon.checkpointTrigger`). Click tick →
scrolls the matching checkpoint card into view and flashes its border
(`borderHover`, 250ms ease-out) — chart-as-index; the card remains the drill
(and its armed `Rewind…` flow is unchanged). This is the scrubber: it scrubs the
eye, never the run — rewind stays a preview-first sheet behind its capability
probe.

---

## G. THE DRILL-DOWN GRAMMAR (one grammar, stated once)

"Seamlessly provided" = the detail arrives where the eye already is, and its
mechanism is predictable everywhere:

1. **A row in a scrolling list expands INLINE on single click** (chevron
   discipline, one visual grammar): turn rows (exists), check rows (partial →
   §D2), activity/event rows (new), Changes file rows (exists — verified §D6),
   checkpoint cards (already fully shown = no-op). Expansion pushes content down
   in flow; collapse restores. Multiple rows may be open except where the surface
   already enforces single-open (Changes keeps its one-patch-at-a-time).
2. **A datum in a fixed band or chart opens an anchored POPOVER on single click**
   (bands must not reflow: header burn strip, PLAN steps, density strip, map
   nodes). Hover always shows the value tooltip first; click pins the full detail.
   Popover chrome: `panel` fill, 1px `border`, radius 8, overlay shadow (black
   25% / 24 / y8 — the overlay class, DESIGN.md §4), padding 12, max-width 380.
   Esc or click-away closes. **The popover body is the same detail component the
   inline expansion renders** where both exist — drill content is defined once
   (§T's `DrillViews.swift`).
3. **Jumps are mono text-buttons** (accent — links are a lawful accent use) that
   navigate within the run surface: they switch the center tab and hand the
   target pane a `DrillTarget` (§G1). Never a new window, never a third pane.
4. Chart marks whose owning rows are on the SAME surface skip the popover and
   navigate directly (the Recorder scrubber → card flash) — an index, not a
   detail.

### G1. DrillTarget routing (app target, view state — not Kit)

```swift
enum DrillTarget: Equatable {
    case turn(Int)                       // Activity tab → Turns mode, expand + scroll
    case changesFile(String)             // Changes tab, expand that path (triggers loadPatch)
    case recordedCheck(kind: String, command: String?)   // Checks tab, expand that row
    case activityWindow(ClosedRange<Date>)               // Activity tab → Stream, set brush
}
```

`RunSurfaceView` owns `@State private var drill: DrillTarget?` and passes a
binding into `DetailCenterTabsView`; setting it switches `tab`, the receiving
pane consumes it (expand/scroll/brush) and clears it. Pure view plumbing; no
store involvement.

---

## D. DRILL-DOWN MAP

### D1. Turn row → full tool I/O  *(grammar: inline, two levels)*

Level 1 exists (turn expands to interleaved entries). Level 2 is new: **trace
entries expand inline to the full tool exchange.**

- **Data.** `TurnModel.Entry` gains `raw: String?` — the verbatim ledger line the
  entry was decoded from, retained for trace-kind entries under the §K10 ceiling.
  On expansion, `TraceDetailDoc.decode(rawTraceLine:)` (§K11 — pure, lenient)
  decodes it on demand: provider/model, `binary`, `duration_ms`, `exit_code`,
  `sandbox_backend` (+ `sandbox_warning` verbatim when present), `workspace_access`,
  `stdout_path`, the prompt (last `args` element), and `flight_rows[]` — each with
  `tool_name`, `tool_category`, `status`, `summary`, and (decoded from the row's
  embedded `raw` item) `command`, `aggregated_output`, `exit_code`, changed paths.
  (live: a single trace line carries the full codex exchange including every
  command and its aggregated output.)
- **Render.** Inside the entry's expansion: a fact line (`codex · gpt-5.6-sol ·
  50.7s · exit 0 · sandboxed: sandbox-exec · read-write`, mono 10, failed exit ≠ 0
  in `dangerText`); then one block per flight row: `tool_name` + status glyph
  (✓/✗ per `status`), the `command` in a mono well (monoS on `well`, horizontal
  scroll), `aggregated_output` in a second well capped at 160px vertical scroll,
  `(clipped when recorded)` note when the provider clipped. All selectable. A
  `changes:` line lists changed paths as mono jump-lines → `.changesFile(path)`.
- **Event entries** (tool_call_started/result, error, docs) keep their one-line
  summaries here — their full records drill in Activity (§D4); duplication would
  be noise. The turn header row also gains a quiet jump: `activity in this turn →`
  → `.activityWindow(turnStart…nextTurnStart)`.
- **Degrade.** `raw == nil` (past the retention ceiling): the expansion says
  `raw line no longer in memory — the full ledger is on disk` + the mono path
  `…/traces.jsonl` (the JobDetailStore honesty pattern). Undecodable raw → the
  raw line itself in a mono well, verbatim (never a guess).

### D2. Check row → full evidence  *(grammar: inline; completes the partial)*

Today `CheckResultDetail` shows `detail` + stdout/stderr behind a text-button.
The full expansion, on every recorded check row (Checks tab + the same component
inside Review & Approve later — out of scope here):

- **Facts row:** `command` in a mono well (the exact thing the gate ran — live
  rows carry it on `AcceptanceProgressRow.CheckResult.command`), `cwd` mono
  beneath when present, `durationMS` printed, `must pass` chip, passed/failed
  glyph as today.
- **Output:** stdout and stderr wells as today (`(clipped when recorded)` note
  kept), both selectable, each with a `copy` text-button (NSPasteboard).
- **History across attempts:** from `report.attempts[]` (each attempt carries
  `runID`, `status`, `provider`, `spendUSD`, `checks[]`). Match key: `(kind,
  command, cwd)` exact triple (§K5). Render newest-first: `attempt 2 ✓ 4.1s ·
  attempt 1 ✗ 3.9s` — each attempt line expandable to ITS stdout/stderr (same
  component, recursion depth 1). An attempt with no matching record renders
  `attempt 1 · not recorded` — absence stated, never inferred.
- Live band rows (CHECKS RUNNING NOW) get the same expansion minus history, and
  keep their `as they stream — not evidence` label; nothing here upgrades display
  data into evidence (law 5).

### D3. Spend chart point → that turn's cost breakdown  *(grammar: band → popover)*

Click a burn-strip point (§V1):

```
turn 7 · 14:02:41
$0.412 this turn · $4.12 total          (monospaced digits)
in 112,814 · out 1,575 tokens
gpt-5.6-sol · cli:codex                 (verbatim mono)
wall 50.7s · subscription · estimated? no
open turn 7 →                           (jump → .turn(7))
```

All fields from the `SpendSeries.Point` (§K1) — the popover renders a retained
ledger row, no re-read. `estimated == true` renders `estimated` as a warn-tinted
word (it IS a flagged degradation of fact quality, per the schema).

### D4. Event row → raw record inspector  *(grammar: inline)*

Activity Stream rows become expandable. `ActivityEntry` gains `raw: String?`
(§K10 ceiling): the verbatim `events.jsonl` line.

- **Render.** The expansion is a mono well: the raw JSON line pretty-printed
  (`JSONSerialization` re-indent of the SAME bytes for reading; a `raw` toggle
  shows the untouched single line — the pretty form is a view, the line is the
  fact), `monoS` on `well`, selectable, with `copy` (copies the untouched line).
  A `kind` + timestamp fact line on top. For `steer_delivered` rows the existing
  chip grammar is unchanged — this is purely additive reading.
- **Degrade.** `raw == nil` (trimmed): `raw line no longer in memory — full
  ledger in events.jsonl` + the drawer pointer (`CONSOLE → Raw events` keeps its
  last-2000 window; the file path is the durable escape hatch). Unmodeled lines
  (record == nil today) already render raw in the feed; their expansion is the
  same line in the well — consistent.

### D5. Plan phase → timestamps + duration  *(grammar: band → popover)*

Click a PLAN step (real pipeline steps and the lifecycle fallback both):

```
implement                                (name verbatim; fallback: stage word)
status executing                         (verbatim)
updated 2026-08-08 14:02:41              (absolute, mono)
elapsed 3m02s — derived from status-change stamps, not a recorded duration
view activity in this window →           (jump → .activityWindow(prevStamp…stamp|now))
```

The fallback lifecycle steps show only the stage word and the projection phase
word (they have no stamps — no duration is ever shown there, §V5). The jump wires
the phase to the Activity brush — the "what happened" answer is the actual
ledger, filtered, not a paraphrase.

### D6. Changes file row → patch view  *(existing — verified seamless, two nits)*

The existing inline expansion (click row → unified patch fetched on demand via
`show --diff --patch --file`, truncation honesty, one-open-at-a-time) already
matches grammar rule 1. Verified; keep. Two nits to land while touching the file:
(1) the expanded header shows the full path mono + selectable (today it can
middle-truncate with no recourse); (2) add `copy path` text-button. No behavior
change; `loadPatch` and its issues rendering untouched. `.changesFile` targets
select + expand + trigger the existing lazy load — no new fetch path.

### D7. Graph node → files/components detail  *(grammar: band → popover)*

Click a map node (Tier 2 Canvas; Tier 1 chips share the component):

```
task:task-1                              (id, mono)
task · pending                           (kind + status, verbatim)
weight 3
evidence:                                (each line mono, selectable)
  file:…/plans/aa49e5aa…/tasks/task-1.json
```

Jumps by kind: `file` nodes whose label matches a path in the loaded diff →
`open in Changes →` (`.changesFile`); `run`/`task`/`provider` nodes carry facts
only (their surfaces are other runs' — this app drills one run; a cross-run jump
would be a new navigation class, out of scope and registered as a possible WAVE 3
item, not faked now). Group chips (`143 files`) jump to the Changes tab.

---

## S. SKETCHES (changed surfaces only)

Legend: `#` seam, `((chip))`, `▁▃▅` marks, `⌇` popover, `▸/▾` disclosure.

### S1. Run header + PLAN + NOW (burn strip + phase durations)

```
| Ship the durable ledger rewrite          [Send back…] [[Review & Approve]] |
| ((Working)) ▸C claude · r-8f2… · deadreckon · ●12s · active 2.1h ·        |
| $4.12 of $25.00 · ((5/5 checks))                        ▂▄▅▆_____╱ ·cap·  |   <- V1 strip 140×18
|############################################################################|
| PLAN  ✓ 1 plan    ✓ 2 scaffold   ● 3 implement   · 4 test   · 5 docs      |
|         2m41s        4m12s          3m02s…                        att 2    |   <- V5 captions
|############################################################################|
| WORKING ON            ON TRACK              ATTENTION       LAST ACTIVITY  |
| implement · turn 7    $4.12/$25 · 2.1h      Nothing flagged     5s ago     |
```

### S2. Activity tab (density strip + brush)

```
| [Stream|Turns]  ⌕ Search…   ((14:02–14:31 · 214 shown ✕))     1,204 events |
|############################################################################|
| ▁▂▁▄▆▂▁▁▃▁█▁▁▂▄▁▁▁▂▁▁▅▂▁▁▁▁▂▁▁ ▏error tick (red)  ▂▁▁▃▂▁▁▄▆(live bin)    |   <- V2, 36px
| 14:02:10                    '   '    turn ticks    '            14:31:44   |
|############################################################################|
| 14:02:11  tool codex                                                    ▸  |
| 14:02:13  result ok · Implemented and verified: …                       ▾  |
|   ┌ kind tool_call_result · 14:02:13.918 ────────────────── [copy] [raw] ┐ |   <- D4 inline
|   │ { "timestamp": "…", "event": { "kind": "tool_call_result", … } }     │ |
|   └──────────────────────────────────────────────────────────────────────┘ |
```

### S3. Turns rows (token/duration micro-bars + tool I/O drill)

```
| ▪ in · ▪ out                                                               |
| ▸ turn 7  14:02:11   ██████▏█▁ 96px   in 112.8k out 1.6k   ▃ 50.7s  $0.41 |
| ▾ turn 6  14:00:02   ████▏▊            in 84.2k out 0.9k   ▂ 38.5s  $0.32 |
|     14:00:04  llm.complete 50820ms                                      ▾  |
|       codex · gpt-5.6-sol · 50.7s · exit 0 · sandboxed: sandbox-exec       |   <- D1 level 2
|       ✓ command_execution                                                  |
|       ┌ /bin/zsh -lc "pwd && rg --files …"                    (command) ┐  |
|       └ /Users/gdc/.deadreckon/worktrees/…  app.py  …          (output) ┘  |
|       changes: app.py →   implementation-notes.html →                      |
|     14:00:41  result ok · Implemented and verified …                       |
|     activity in this turn →                                                |
```

### S4. Checks tab (duration bars + evidence expansion + history)

```
| RECORDED CHECK RESULTS                                                     |
| ▾ ✓ shell   app exits successfully…   ((must pass))   4.1s  ████████▏ 80px |
|     ┌ python3 - <<'PY' … PY                                   (command) ┐  |
|     └ cwd /Users/gdc/.deadreckon/worktrees/…                            ┘  |
|     stdout (clipped when recorded)  [copy]     stderr (empty)              |
|     attempts:  #2 ✓ 4.1s ▸   #1 ✗ 3.9s ▸                                   |   <- D2 history
| ▸ ✗ no-warnings  build log                             0.9s  ██▏(red)      |
```

### S5. Story tab MAP (Tier 1 strip · Tier 2 canvas)

```
| MAP                                    drawn 45s ago · flows left → right  |
| [run f3529e49 ((completed))] —uses→ [cli:codex] —touches→ [143 files]      |   <- Tier 1
|   app.py · implementation-notes.html · .gitignore · .data/… · +139 more →  |
|  — or, plan-scope —                                                        |
| ┌────────────────────────────────────────────────────────────────────────┐ |
| │ [plan aa49e5aa]══╗→ [task-0 ●]──→ [run d7524b52] ─→ [cli:codex]        │ |   <- Tier 2
| │                  ╠→ [task-1 ·]  (blocks: task-0 → task-1 on hover)     │ |
| │                  ╚→ [task-2 ·] …                                       │ |
| │ ● active work · ● done · ● risk or stale evidence · ● blocked/failed   │ |
| └────────────────────────────────────────────────────────────────────────┘ |
```

### S6. Recorder scrubber

```
| RECORDER   245 events this session · 2 sessions                            |
| │      │    │ │   ║     │      │   │    ║(live)                            |   <- V8: ║ = full snapshot
| '      '  session-start ticks (bottom lane)                                |
| 14:56:18                          9 checkpoints · 2 sessions      15:07:44 |
|  ⌇ cp-000007 · turn 3 · after a tool ran · 15:02:11  (hover)               |
| [cp-000009 ((full snapshot)) … cards as today, flash on scrubber click]    |
```

### S7. Popover anatomy (shared chrome, §G rule 2)

```
        ⌇ anchored at the datum
┌──────────────────────────────┐   panel · 1px border · r8 · overlay shadow
│ turn 7 · 14:02:41            │   facts: mono, monospaced digits, selectable
│ $0.412 this turn · $4.12 …   │
│ open turn 7 →                │   jump: accent text-button, mono
└──────────────────────────────┘   Esc / click-away closes
```

---

## K. KIT DERIVATION SPECS (DeadreckonKit — pure, Sendable, tested)

New file `Models/ChartSeries.swift` (K1–K6, K9), new file
`Models/ArchitectureGraph.swift` (K7), edits to `Models/TurnsDerivation.swift`
(K4, K10), `Models/DetailModels.swift` (K4, K11), `Services/JobDetailStore.swift`
(K8, K10). Views never compute a series, a bin, a scale, or a position.

### K1. SpendSeries

```swift
public struct SpendSeries: Equatable, Sendable {
    public struct Point: Equatable, Sendable, Identifiable {
        public let ordinal: Int            // id — ledger order
        public let turn: Int
        public let timestamp: Date
        public let deltaUSD: Double        // record.costUSD
        public let totalUSD: Double        // record.totalCostUSD (the running head)
        public let inputTokens: Int
        public let outputTokens: Int
        public let model: String           // verbatim
        public let provider: String        // verbatim
        public let wallSeconds: Double?
        public let estimated: Bool
        public let subscription: Bool
        public var id: Int { ordinal }
    }
    public private(set) var points: [Point] = []
    public private(set) var capUSD: Double?          // last non-nil loop cap
    public private(set) var droppedPoints = 0        // ceiling honesty
    public var maxTotalUSD: Double { points.last?.totalUSD ?? 0 }

    /// Fold new spend records in ledger order. LOOP rows only — narrator rows
    /// are a split ledger and never enter the series (TAILING.md). Ceiling:
    /// 5,000 points, oldest dropped WITH the counter (the burn shape needs the
    /// tail; the header prints the head regardless).
    public mutating func fold(_ records: [SpendRecord])
}
```

Tests: loop/narrator split; cap adoption (last non-nil wins); ceiling drop +
counter; fold(a+b) == fold(a);fold(b) equivalence.

### K2. DensitySeries

```swift
public struct DensitySeries: Equatable, Sendable {
    public struct Bin: Equatable, Sendable, Identifiable {
        public let start: Date; public let count: Int; public let errorCount: Int
        public var id: Date { start }
    }
    public enum Presentation: Equatable, Sendable {
        case absent                                    // no events
        case ticks(events: [Date], errors: [Date])     // sparse mode
        case bins([Bin], width: TimeInterval)
    }
    public private(set) var eventCount = 0
    public private(set) var errorStamps: [Date] = []
    public private(set) var turnStamps: [Date] = []    // turn_started boundaries
    public private(set) var domain: ClosedRange<Date>?
    public private(set) var droppedStamps = 0          // ceiling honesty

    public struct Accumulator: Equatable, Sendable {
        public init(sparseFloor: Int = 12, sparseSpanSeconds: TimeInterval = 60,
                    maxBins: Int = 72)
        /// Fold (timestamp, kind) pairs from newly decoded events. Maintains
        /// bins incrementally at the current ladder width; when the span
        /// outgrows maxBins the width steps up the fixed nice ladder
        /// (1,2,5,10,15,30s,1,2,5,10,30m,1h,…) and bins rebuild once from the
        /// retained stamps (O(n), amortized rare). Stamp retention ceiling
        /// 200_000 with droppedStamps (domain start is then pinned honest).
        public mutating func fold(_ events: [(timestamp: Date, kind: String)]) -> DensitySeries
    }
    public var presentation: Presentation { get }      // applies the sparse law
}
```

Tests: sparse→bins flip at exactly the floor; ladder width selection per span;
rebin equivalence (incremental == one-shot); error/turn stamp routing; ceiling.

### K3. *(reserved — folded into K2's presentation; no separate type)*

### K4. Turn wall-time + raw retention (edits)

- `RunEventRecord.Detail` gains `wallTimeSeconds: Double?` (`wall_time_seconds`,
  additive decode — live: present on every `spend_delta`).
- `TurnModel` gains `wallSeconds: Double` (accumulated from `spend_delta` rows in
  `Accumulator.fold`, exactly as `costUSD` accumulates).
- `TurnModel.Entry` gains `raw: String?`.
- `TurnsDerivation.Accumulator.fold` signature becomes
  `fold(events: [RunEventRecord], traces: [(row: TraceRow, raw: String?)]) -> [TurnModel]`
  — the store already holds each trace's source line at decode time; the one-shot
  `group` convenience gains a matching overload and the old signature stays for
  existing tests (raw = nil).
- `TurnScale.derive(turns: [TurnModel]) -> (maxTokens: Int, maxWallSeconds: Double)`
  (max of in+out per turn; zero-safe).

Tests: wall accumulation; raw carried onto trace entries; scale maxima; legacy
events without the field decode unchanged.

### K5. CheckDurations + attempt history matching

```swift
public enum CheckDurations {
    public struct Row: Equatable, Sendable {
        public let key: CheckKey; public let durationMS: Int?; public let passed: Bool
    }
    public struct CheckKey: Equatable, Hashable, Sendable {   // identity across attempts
        public let kind: String; public let command: String?; public let cwd: String?
    }
    /// Bars render only when 2+ rows carry a duration (§V4 floor).
    public static func derive(results: [AcceptanceProgressRow.CheckResult])
        -> (rows: [Row], maxMS: Int, showBars: Bool)
    /// Newest-first per-attempt history for one check identity; attempts with
    /// no matching record yield .notRecorded — absence stated, never inferred.
    public static func history(for key: CheckKey, attempts: [JobReportEnvelope.Attempt])
        -> [(attemptIndex: Int, result: AcceptanceProgressRow.CheckResult?)]
}
```

Tests: triple-key matching (same kind, different command → distinct); floor at
exactly 2 durations; not-recorded attempts.

### K6. PhaseDurations

```swift
public enum PhaseDurations {
    public enum Mark: Equatable, Sendable {
        case completed(seconds: Double)
        case current(secondsSoFar: Double)   // recomputed each tick via now
        case none                            // pending / non-monotonic / fallback
    }
    /// Baseline: phase[0] measures from runStartedAt; phase[i] from
    /// phase[i-1].updatedAt. A negative interval, an out-of-order completion,
    /// or an unrecognized status yields .none — never a negative, never a guess.
    public static func derive(phases: [RunStateDoc.Phase], runStartedAt: Date,
                              currentPhaseID: Int, status: String, now: Date) -> [Mark]
}
```

Tests: happy path; non-monotonic stamps → .none; current-phase live clock;
killed/failed runs freeze the current mark (status word gates the ticking).

### K7. ArchitectureGraphDoc + layout + token map

```swift
public struct ArchitectureGraphDoc: Codable, Equatable, Sendable {
    public struct Node: Codable, Equatable, Sendable {
        public let id, label, kind, status: String    // all verbatim
        public let weight: Int
        public let evidence: [String]
        public let styleToken: String                  // "style_token"
    }
    public struct Edge: Codable, Equatable, Sendable {
        public let from, to, label, kind: String
    }
    public struct Group: Codable, Equatable, Sendable {
        public let id, label: String; public let nodeIDs: [String]   // "node_ids"
    }
    public struct Layout: Codable, Equatable, Sendable {
        public let kind: String; public let rootIDs: [String]; public let warnings: [String]
    }
    public struct LegendEntry: Codable, Equatable, Sendable {
        public let styleToken, meaning: String
    }
    public let version: Int
    public let graphID: String            // "graph_id"
    public let scope: String              // "run" | "plan" — the tier switch
    public let targetID: String
    public let generatedAt: Date
    public let nodes: [Node]; public let edges: [Edge]; public let groups: [Group]
    public let layout: Layout; public let legend: [LegendEntry]
}

public enum GraphStyleToken: Equatable, Sendable {
    case primary, success, warning, danger, muted
    case unknown(String)                  // raw word preserved, renders muted
    public init(raw: String)
}

public enum GraphLayoutDerivation {
    public struct Placed: Equatable, Sendable {
        public let node: ArchitectureGraphDoc.Node
        public let column: Int            // BFS depth from layout.rootIDs (min over roots)
        public let row: Int               // within column: weight desc, id asc
    }
    public enum Result: Equatable, Sendable {
        case placed([Placed], columns: Int)
        case tooLarge(nodeCount: Int)     // > 40 → Canvas refuses (§V7)
    }
    /// Deterministic, no physics. Nodes unreachable from the roots land in a
    /// final overflow column (drawn, never dropped silently).
    public static func layered(_ doc: ArchitectureGraphDoc) -> Result
}
```

Tests: decode both live fixtures (run-star + plan-DAG, embedded as strings);
BFS depths for the plan fixture (plan=0, tasks=1, runs/provider=2); determinism;
tooLarge at 41; unknown style token preserved; unreachable-node overflow column.

### K8. JobDetailStore additions

```swift
@Published public private(set) var spendSeries = SpendSeries()            // K1, fed in applySpend
@Published public private(set) var density = DensitySeries()              // K2, fed in applyEvents
@Published public private(set) var archGraph: ArchitectureGraphDoc?       // K7
@Published public private(set) var archGraphIssue: String?                // decode failure, verbatim
```

- Graph read joins the stage-2 detached hop: try
  `home/plans/<jobID>/narrative/architecture-graph.json` first (driver jobs),
  else `<runRoot>/narrative/architecture-graph.json`; mtime-cached exactly like
  the checkpoints cache (unchanged mtime skips the read); absent → nil + nil
  issue; exists-but-undecodable → keep last good doc, set the issue verbatim
  (the projection.json pattern).
- `buildRunTailers` resets the new state with the rest (fresh attempt, fresh
  series); `close()` clears all four.
- All folds run where the existing decode hops run (off-main); only `@Published`
  assignment on the main actor. Per-tick cost stays O(new rows).

Tests (JobDetailStoreTests): series survive across ticks; per-run reset on
attempt change; graph mtime cache skip; plan-path precedence; issue on corrupt
graph while last good doc holds.

### K9. CheckpointTimeline

```swift
public struct CheckpointTimeline: Equatable, Sendable {
    public struct Tick: Equatable, Sendable, Identifiable {
        public let id: String             // checkpointID
        public let at: Date; public let turn: Int
        public let trigger: String; public let fullAnchor: Bool
    }
    public struct SessionMark: Equatable, Sendable {
        public let at: Date               // startedAt — boundaries only, no invented ends
        public let status: String; public let provider: String
    }
    public let domain: ClosedRange<Date>? // runStartedAt…lastRecordedStamp; nil when empty
    public let ticks: [Tick]; public let sessions: [SessionMark]
    public static func derive(checkpoints: [CheckpointManifestDoc],
                              sessions: [FlightManifestDoc.Session],
                              runStartedAt: Date?) -> CheckpointTimeline
}
```

Tests: domain never exceeds recorded stamps; single-checkpoint degenerate;
missing runStartedAt (domain from first stamp).

### K10. Raw-retention ceilings (documented, CONTRACTS.md alongside)

- `ActivityEntry` gains `raw: String?`. Retention: the newest
  **`rawInspectorCeiling = 4_000`** entries keep their raw line; older entries'
  raw drops to nil (the drill then names `events.jsonl` on disk, §D4). The
  parsed scrollback itself stays unbounded — this ceiling bounds only duplicated
  raw bytes.
- `TurnModel.Entry.raw` (trace entries): newest **`traceRawCeiling = 1_000`**
  trace lines retained (trace lines are large — live: 11KB each — but arrive
  ~2/turn; 1,000 covers a 500-turn run at ≈ 11MB worst case).
- Both ceilings live beside `rawEventLineCeiling` in JobDetailStore with the
  same honesty grammar.

### K11. TraceDetailDoc (on-demand decode)

```swift
public struct TraceDetailDoc: Equatable, Sendable {
    public struct FlightRow: Equatable, Sendable, Identifiable {
        public let id: String
        public let toolName: String?      // "tool_name"
        public let toolCategory: String?  // shell | edit | …, verbatim
        public let status: String?        // completed | failed, verbatim
        public let summary: String?
        // Decoded from the flight row's embedded `raw` item JSON when present:
        public let command: String?
        public let aggregatedOutput: String?
        public let exitCode: Int?
        public let changedPaths: [String]
    }
    public let provider: String?; public let model: String?
    public let binary: String?; public let durationMS: Int?; public let exitCode: Int?
    public let sandboxBackend: String?; public let sandboxWarning: String?
    public let workspaceAccess: String?; public let stdoutPath: String?
    public let promptArg: String?         // last element of detail.trace.args
    public let flightRows: [FlightRow]

    /// Lenient: any missing branch yields nils, never a throw; returns nil only
    /// when the line is not JSON at all (the view then shows the raw line
    /// verbatim). Pure — decoded on expansion, held only while expanded.
    public static func decode(rawTraceLine: String) -> TraceDetailDoc?
}
```

Tests: decode the live codex fixture (full exchange incl. a failed flight row);
non-JSON → nil; partial shapes (no flight_rows, no detail.trace) → facts-only doc.

---

## T. TOUCH LIST — one implementer, dependency order

Every step leaves the app buildable; Kit lands first, views consume second.

| # | File | Work |
|---|---|---|
| 1 | `DeadreckonKit/Sources/DeadreckonKit/Models/ChartSeries.swift` | NEW — K1 SpendSeries, K2 DensitySeries+Accumulator, K5 CheckDurations, K6 PhaseDurations, K9 CheckpointTimeline, K4's TurnScale |
| 2 | `DeadreckonKit/Sources/DeadreckonKit/Models/ArchitectureGraph.swift` | NEW — K7 doc + GraphStyleToken + GraphLayoutDerivation |
| 3 | `DeadreckonKit/Sources/DeadreckonKit/Models/DetailModels.swift` | EDIT — `Detail.wallTimeSeconds` (K4); K11 TraceDetailDoc |
| 4 | `DeadreckonKit/Sources/DeadreckonKit/Models/TurnsDerivation.swift` | EDIT — TurnModel.wallSeconds, Entry.raw, fold overload with trace raws (K4, K10) |
| 5 | `DeadreckonKit/Sources/DeadreckonKit/Services/JobDetailStore.swift` | EDIT — K8 published state + graph read + K10 ceilings; feed folds in applySpend/applyEvents/applyTraces |
| 6 | `DeadreckonKit/Tests/DeadreckonKitTests/ChartSeriesTests.swift` | NEW — K1/K2/K5/K6/K9 test lists above |
| 7 | `DeadreckonKit/Tests/DeadreckonKitTests/ArchitectureGraphTests.swift` | NEW — K7 fixtures + layout |
| 8 | `DeadreckonKit/Tests/DeadreckonKitTests/TraceDetailTests.swift` | NEW — K11 |
| 9 | `DeadreckonKit/Tests/DeadreckonKitTests/TurnsGroupingTests.swift` + `JobDetailStoreTests.swift` | EDIT — K4 accumulation, K8 store additions |
| 10 | `Sources/Views/Theme.swift` | EDIT — `Theme.Chart` tokens (§V0), nothing else |
| 11 | `Sources/Views/RunCharts.swift` | NEW — `SpendBurnStrip` (V1), `ActivityDensityStrip` (V2, brush binding out), `CheckpointScrubber` (V8), `TokenMicroBar`/`DurationMicroBar` (V3/V4 marks). `import Charts` here only |
| 12 | `Sources/Views/DrillViews.swift` | NEW — shared drill blocks: `SpendPointDetail` (D3), `EventRawInspector` (D4), `ToolIOView` (D1), `CheckEvidenceView` (D2, absorbs `CheckResultDetail`'s internals), `PhaseDetail` (D5), `GraphNodeDetail` (D7), `DrillPopoverChrome` + `JumpLine` atoms (§G) |
| 13 | `Sources/Views/StoryMapView.swift` | NEW — Tier 1 strip + Tier 2 Canvas (V7), token→color map, legend/warnings rows |
| 14 | `Sources/Views/RunDetailView.swift` | EDIT — burn strip in `RunHeaderView.factsLine` (ViewThatFits drop); PLAN captions + phase popover (V5/D5); `@State drill: DrillTarget?` plumbed into tabs (G1) |
| 15 | `Sources/Views/DetailCenterTabs.swift` | EDIT — Activity: density strip + brush chip + row expansion (V2/D4), sparkbar deleted; Turns: micro-bars + header key + tool I/O drill + turn jump (V3/D1); Checks: duration bars + full evidence + history (V4/D2); Story: MAP section placement (V7); Changes: D6 nits; Recorder: scrubber + card flash (V8); DrillTarget consumption per tab |
| 16 | `CONTRACTS.md` | EDIT — register: ChartSeries/ArchitectureGraph/TraceDetail contracts (pure derivations, display-only); JobDetailStore additions + the two new ceilings (K10) and graph read paths; the D7 cross-run-jump non-goal; note the superseded Activity sparkbar vs REDESIGN-SPEC §A3.3 |
| 17 | `project.yml` regen | `xcodegen generate` (Sources glob picks the new files; no target changes — Charts and Canvas are system frameworks, no linking edits) |

**Explicit non-goals of this wave:** fleet-wide/Overview charts (V6 argued out),
any new CLI verb or read beyond the two graph paths, chart animation, pan/zoom on
the map, cross-run navigation from graph nodes, sheet-surface charts
(Review & Approve keeps its evidence text — decision surfaces stay word-first),
and any change to write flows or trust rules.

**Gate (per project law):** `xcodegen generate` · `xcodebuild -project
deadreckon.xcodeproj -scheme deadreckon -configuration Debug build` ·
`swift test --package-path DeadreckonKit` — all green, then the operator script:

1. Open a live run: burn strip draws only recorded points, cap rule where a cap
   exists, end-dot accent; click a point → the turn's cost popover; jump lands
   expanded on that turn.
2. Activity: density strip bins honestly (or ticks when sparse); drag-brush
   filters the stream + chip clears; an error event shows a red tick at the
   right time.
3. Turns: bars align on one scale; expanding a trace entry shows the real
   command + output verbatim; past-ceiling entries say so and name the file.
4. Checks (terminal run): duration bars only when ≥2 durations; expansion shows
   command/cwd/outputs + per-attempt history with `not recorded` where true.
5. PLAN: durations only between completed monotonic stamps; phase popover's
   "view activity in this window" lands on a brushed stream.
6. Story: a plain run shows the strip sentence (no node-link); a driver job's
   run shows the DAG with the file's own legend; node click → facts popover;
   a file node jumps into Changes.
7. Recorder: scrubber ticks match the cards; clicking one flashes its card;
   full snapshots read heavier than incrementals.
8. Kill the spend feed mid-run (or pick a run with a corrupt tail): every chart
   freezes at the last trusted row beside the existing warn chip — nothing
   extrapolates, nothing guesses.
