# deadreckon - Narrative Attach Rider (Narrated)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-26-1546-deadreckon-narrative-attach-goal.md`.
It supersedes nothing in prior riders, especially provider flight recorder,
orchestration event bus, self-documenting runs, coherence closure, and guided
experience. Their invariants still apply. This rider adds a human-readable
operator projection for live and completed work.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided - do not redesign)

- **Maturity stays `alpha`.**
- **No `PipelineState` schema changes.** Narrative state lives in files under
  run or plan roots.
- **Narrative is a projection, not evidence.** Evidence remains run events,
  traces, flight events, checkpoints, docs, plan events, child run state, and
  provider-owned logs.
- **Do not mutate source logs.** Provider logs remain read-only; `flight` stays
  the durable provider-native event layer.
- **Do not write prose into `flight-events.jsonl`, `ProviderActivity`, or
  `plan-events.jsonl`.** Those streams keep their current semantics.
- **Narration must be optional at the surface and safe by default.** If provider
  summarization fails, the run/plan continues and attach falls back to
  deterministic facts.
- **No full-workspace verification by default.** Avoid `make verify`, release
  builds, stress tests, broad smoke suites, and full-workspace tests unless the
  human explicitly requests them.
- **No V1 invention.** Long-lived narrator daemon, cloud telemetry, historical
  run mining, personal style learning, and remote collaboration views go to
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.
- **No `git push`.** Phased local commits only.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Current code assessment

`deadreckon attach` enters through `Commands::Attach` in
`/Users/gdc/deadreckon/crates/deadreckon/src/cli.rs` and dispatches through
`attach_command` in `/Users/gdc/deadreckon/crates/deadreckon/src/main.rs`.

Run attach:

- Resolves plan child refs, run ids, plan ids, and chain ids.
- TTY run attach enters `attach_tui_with_parent`.
- Each loop reloads `state.json`, tails `events.jsonl`, reads `spend.jsonl` and
  `traces.jsonl`, collects `AttachLive`, then calls `render_attach`.
- The central-left pane currently renders "tool calls / provider activity".
- Completed runs already have a `d` docs toggle that renders `RUN-NARRATIVE.md`
  through the local `pulldown-cmark` to `ratatui` renderer.

Plan attach:

- Uses `attach_plan_tui`, not the run renderer.
- `PlanEventBus::file_tail` replays `plan-events.jsonl`, emits snapshots, and
  multiplexes discovered child and repair run `events.jsonl`.
- `render_plan_attach` owns the plan header, child panes, activity/feed pane,
  and footer.
- `Enter` on a child suspends plan attach and opens the child run attach.

Provider activity:

- `collect_provider_activity` prefers `flight-events.jsonl` and appends
  descriptor-ingested provider logs only as a fallback/freshness hint.
- Flight rows carry source path, line, raw hash, event kind, file refs, usage,
  and checkpoint ids. Narrative must cite those rows, not masquerade as them.

Clean integration points:

- Add an attach view mode to CLI args and TUI state.
- Branch the run central-left pane near the existing completed-docs branch.
- Add a narrative mode to `PlanAttachRenderState` and render the plan activity
  area from plan narrative snapshots.
- Add non-TTY branches in `attach_command` after id resolution.

## Research posture

### Rust UI packages

Use existing workspace crates first:

- `ratatui` is the right TUI substrate. Docs describe it as a lightweight Rust
  terminal UI library with widgets/layout/styling and an immediate-render app
  loop.
- `crossterm` remains the raw-mode, alternate-screen, event, and terminal
  command backend.
- `pulldown-cmark` is already used and is a pull parser for CommonMark. Prefer
  the current renderer unless Markdown complexity becomes the actual bottleneck.
- `tokio::sync::watch` is appropriate for "latest snapshot" delivery if a
  same-process narrator feed appears; `broadcast` fits event streams but lagging
  receivers can miss bounded backlog.

Candidate additions, only after local complexity is proven:

- `tui-markdown`: simple Markdown-to-`ratatui::Text` bridge using
  `pulldown-cmark`; useful if the current renderer becomes a maintenance cost.
- `tui-scrollview`: smooth scrollable Ratatui views; useful for richer narrative
  panes with independent sections.
- `textwrap`: plain/off-TTY wrapping and indentation.
- `minus`: static pager for completed narrative in non-TUI contexts.
- `anstream`/`anstyle`: adaptive ANSI output for plain/pager surfaces, not
  inside Ratatui frames.

Avoid in the first implementation unless a depth test proves the need:

- `notify`: current file-tail/poll model is already reliable across processes;
  file watchers add platform/editor caveats.
- `indicatif`: progress bars fight alternate-screen TUIs.
- `syntect`: code highlighting is not required for an operator narrative.
- `comrak`: heavier AST/GFM parser; not needed unless exact transforms matter.
- `tui-textarea`: this view is not an editor.

Visual rendering constraints:

- Use `ratatui` layout, `Block`, `Paragraph`, `List`, `Gauge`, `Line`, `Span`,
  and existing `ui::TUI_PALETTE` before adding new visual crates.
- Make the view feel distinct with semantic color, focused borders, compact
  badges, connectors, progress rails, status glyphs, and section rhythm. Keep it
  work-focused, not decorative.
- Color must never be the only signal. Labels such as `active`, `blocked`,
  `stale`, `done`, and evidence ids must remain visible under `NO_COLOR`.
- Prefer ASCII-compatible map rendering first. Optional Unicode box or braille
  rendering may be a later enhancement only if capability detection and tests
  preserve clean ASCII fallback.
- Avoid force-directed or animated graph crates in the first slice. A stable
  layered tree, swimlane, or file-tree map is easier to read in a terminal.

Research references:

- Ratatui docs: https://docs.rs/ratatui/latest/ratatui/
- Crossterm docs: https://docs.rs/crossterm/latest/crossterm/
- tui-markdown docs: https://docs.rs/tui-markdown/latest/tui_markdown/
- tui-scrollview docs: https://docs.rs/tui-scrollview/latest/tui_scrollview/
- pulldown-cmark docs: https://docs.rs/pulldown-cmark/latest/pulldown_cmark/
- textwrap docs: https://docs.rs/textwrap/latest/textwrap/
- minus docs: https://docs.rs/minus/latest/minus/
- anstream docs: https://docs.rs/anstream/latest/anstream/
- Tokio sync docs: https://docs.rs/tokio/latest/tokio/sync/index.html
- notify docs: https://docs.rs/notify/latest/notify/

### Observability and privacy ideas to borrow

- Treat source events as an append-only event store and summaries as
  replayable projections. OpenTelemetry's log model is useful for thinking
  about timestamp, observed timestamp, trace/span correlation, severity, body,
  resource, attributes, and event name.
- Model multi-agent plans as a DAG of supervisor, task, child run, provider
  invocation, tool, checkpoint, gate, and repair events. W3C Trace Context is a
  useful analogy for parent/child correlation, not a requirement to implement
  HTTP headers.
- Redact before provider calls. OWASP logging guidance is the floor: never send
  secrets, access tokens, passwords, connection strings, private keys, sensitive
  PII, or unsanitized hostile log text to a summarizer provider.

References:

- OpenTelemetry log data model:
  https://opentelemetry.io/docs/specs/otel/logs/data-model/
- W3C Trace Context: https://www.w3.org/TR/trace-context/
- OWASP Logging Cheat Sheet:
  https://cheatsheetseries.owasp.org/cheatsheets/Logging_Cheat_Sheet.html

## Data model (files, not fields)

All files are append-only or atomically rewritten under run or plan roots.

### `<run-root>/narrative/state.json`

```json
{
  "version": 1,
  "scope": "run",
  "target_id": "83f9e869...",
  "latest_snapshot_id": "nar-000012",
  "latest_status": "fresh|stale|failed|disabled|redacted",
  "latest_created_at": "<RFC3339|null>",
  "latest_covered": {
    "run_event_seq": 42,
    "trace_count": 7,
    "flight_event_seq": 113,
    "checkpoint_id": "cp-000104",
    "doc_inputs_hash": "sha256:...",
    "architecture_graph_hash": "sha256:..."
  },
  "cadence": {
    "mode": "event-driven",
    "min_seconds_between_provider_calls": 45,
    "quiet_seconds": 30,
    "max_provider_calls_per_attach": 20
  },
  "provider": {
    "route": "cli:codex",
    "source": "flag|config|doc_provider|run_provider|deterministic|none",
    "model": "provider default",
    "calls": 3,
    "cost_usd": 0.0,
    "subscription_seconds": 71.2
  },
  "last_error": null
}
```

### `<run-root>/narrative/snapshots.jsonl`

Each row is a durable narrative projection:

```json
{
  "version": 1,
  "snapshot_id": "nar-000012",
  "scope": "run",
  "target_id": "83f9e869...",
  "created_at": "<RFC3339>",
  "status": "fresh|stale|failed|deterministic",
  "source_window": {
    "run_events": {"from_seq": 31, "to_seq": 42},
    "traces": {"from_index": 4, "to_index": 7},
    "flight_events": {"from_seq": 91, "to_seq": 113},
    "checkpoints": ["cp-000102", "cp-000104"],
    "files": ["src/main.rs", "crates/deadreckon/src/main.rs"],
    "docs_hash": "sha256:..."
  },
  "coverage": {
    "skipped_events": 0,
    "redacted_events": 2,
    "known_gaps": []
  },
  "headline": "The agent is wiring a narrative attach view into the existing TUI.",
  "current_work": [
    {
      "text": "It has changed the run attach renderer and is adding focused render tests.",
      "evidence": ["run:83f9e869:event:41", "flight:83f9e869:seq:111"],
      "confidence": "high"
    }
  ],
  "architecture_notes": [
    {
      "text": "Narrative state is kept under the run root, leaving PipelineState unchanged.",
      "evidence": ["file:crates/deadreckon/src/main.rs", "run:83f9e869:event:39"],
      "confidence": "medium"
    }
  ],
  "risks": [
    {
      "text": "The summarizer is behind the newest provider rows.",
      "evidence": ["flight:83f9e869:seq:113"],
      "confidence": "high"
    }
  ],
  "next_likely": [
    {
      "text": "Expect a render snapshot test or CLI output test next.",
      "evidence": ["trace:turn-1:docs_checkpoint"],
      "confidence": "low"
    }
  ],
  "citations": [
    {
      "id": "flight:83f9e869:seq:111",
      "kind": "flight_event",
      "path": "/Users/gdc/.deadreckon/.../flight-events.jsonl",
      "summary": "provider tool edited attach renderer"
    }
  ]
}
```

Rules:

- `current_work`, `architecture_notes`, `risks`, and `next_likely` items must
  cite at least one evidence id.
- Low-confidence predictions are allowed only under `next_likely`; never phrase
  them as fact.
- Unsupported claims must render as "No evidence yet" or be omitted.
- Snapshots are immutable. Corrections create a newer snapshot that marks the
  earlier claim as superseded in text or metadata.

### `<plan-dir>/narrative/state.json`

Same shape as run state, with `scope: "plan"` and plan-specific coverage:

```json
{
  "version": 1,
  "scope": "plan",
  "target_id": "plan-...",
  "latest_snapshot_id": "plan-nar-000009",
  "latest_status": "fresh|stale|failed|disabled|redacted",
  "latest_covered": {
    "plan_event_seq": 64,
    "child_runs": {
      "task-1": {"run_id": "run-a", "run_event_seq": 12, "flight_event_seq": 33},
      "task-2": {"run_id": "run-b", "run_event_seq": 8, "flight_event_seq": 21}
    },
    "repair_run_id": null
  },
  "cadence": {"mode": "event-driven"},
  "provider": {"route": "cli:codex", "source": "doc_provider"}
}
```

### `<plan-dir>/narrative/snapshots.jsonl`

Same row style as run snapshots, with these additional sections:

```json
{
  "plan_status": "running",
  "agent_table": [
    {
      "task_id": "task-1",
      "role": "researcher",
      "provider": "cli:claude-code",
      "status": "running",
      "summary": "Inspecting attach data sources.",
      "evidence": ["plan:abc:event:22", "run:run-a:event:9"]
    }
  ],
  "coordination_notes": [
    {
      "text": "task-3 is blocked on task-1 finishing because it depends on the evidence schema.",
      "evidence": ["plan:abc:event:18", "task:task-3:deps"]
    }
  ]
}
```

### `<run-root>/narrative/architecture-graph.json`

The architecture graph is a deterministic projection that the TUI can render
beside the prose. The same schema is used for plan graphs at
`<plan-dir>/narrative/architecture-graph.json`.

```json
{
  "version": 1,
  "graph_id": "arch-000012",
  "scope": "run|plan",
  "target_id": "83f9e869...",
  "generated_at": "<RFC3339>",
  "source_window": {
    "run_events": {"from_seq": 31, "to_seq": 42},
    "flight_events": {"from_seq": 91, "to_seq": 113},
    "plan_events": null,
    "files": ["crates/deadreckon/src/main.rs"]
  },
  "default_visual": "architecture",
  "nodes": [
    {
      "id": "module:attach",
      "label": "attach command",
      "kind": "command|module|file|task|provider|run|checkpoint|gate|doc",
      "status": "active|done|blocked|waiting|risk|stale|neutral",
      "weight": 3,
      "evidence": ["file:crates/deadreckon/src/main.rs", "run:83f9e869:event:42"],
      "style_token": "primary|success|warning|danger|muted"
    }
  ],
  "edges": [
    {
      "from": "module:attach",
      "to": "file:crates/deadreckon/src/main.rs",
      "label": "renders",
      "kind": "owns|reads|writes|depends_on|spawns|summarizes|validates",
      "evidence": ["flight:83f9e869:seq:111"]
    }
  ],
  "groups": [
    {
      "id": "group:run-attach",
      "label": "Run attach",
      "node_ids": ["module:attach", "file:crates/deadreckon/src/main.rs"],
      "evidence": ["file:crates/deadreckon/src/main.rs"]
    }
  ],
  "layout": {
    "kind": "layered-tree|swimlane|file-tree|evidence-chain",
    "root_ids": ["module:attach"],
    "warnings": []
  },
  "legend": [
    {"style_token": "primary", "meaning": "active work"},
    {"style_token": "warning", "meaning": "risk or stale evidence"}
  ]
}
```

Graph rules:

- Nodes and edges must be created from deterministic evidence: changed file
  paths, directory structure, plan tasks/dependencies, provider ids, run ids,
  event kinds, checkpoints, acceptance gates, docs, and cheap symbol extraction
  when available.
- The summarizer provider may suggest better labels, grouping, and explanatory
  notes for existing nodes/edges. It may not invent new nodes or edges.
- Every node and edge must carry at least one evidence id.
- Missing evidence yields an empty or sparse graph with a visible "not enough
  evidence" row, not a fabricated diagram.
- Graph layout is stable across refreshes when inputs are stable. Avoid
  reordering nodes every tick.
- Plain output may render an ASCII tree. JSON output emits the graph object.

## Evidence window builder

Implement a small module, likely `crates/deadreckon/src/narrative.rs`, with
source-neutral helpers used by both run attach and plan attach.

Run evidence sources:

1. `PipelineState`: goal, provider, sandbox, phase, status, working dir, run
   root, started/completed timestamps, parent plan if known.
2. `events.jsonl`: recent durable run events with sequence/order metadata.
3. `traces.jsonl`: tool/provider/checkpoint/docs records.
4. `spend.jsonl`: token/cost/wall-clock context.
5. `flight-events.jsonl` and `flight-manifest.json`: provider-native events,
   checkpoints, source rows, usage, and files.
6. `proofs/acceptance-progress.jsonl` and signed acceptance marker.
7. `working/.deadreckon/docs/RUN-NARRATIVE.md`,
   `RUN-DECISIONS.md`, and implementation notes freshness when present.
8. Live file inventory already gathered by `collect_attach_live`.
9. Architecture graph candidates: changed files, touched directories, command
   modules, provider/tool kinds, checkpoints, docs, and known parent plan task.

Plan evidence sources:

1. `Plan`: root goal, mode, capability preview, tasks, deps, providers, roles,
   child run ids, status.
2. `PlanEventFeed`: plan events, snapshots, child run events, repair run events.
3. Child run `narrative/state.json` and latest snapshot when present.
4. Child run deterministic fallback windows when no child narrative exists.
5. Merge repair sidecars and final gate status.
6. Plan architecture graph candidates: task DAG, provider roles, child run ids,
   repair/final-gate nodes, child changed-file clusters, and child narrative
   graph summaries.

Windowing rules:

- Build overlapping windows by event count, estimated token size, and semantic
  triggers.
- Semantic triggers: file edits, provider tool calls, command/test completion,
  acceptance pass/fail, docs checkpoint, plan child start/finish, repair start,
  blocker, user interruption, kill/cancel, idle quiet threshold.
- Pin high-value events outside normal windows: user instructions, failures,
  stack traces, changed-file lists, acceptance failures, redactions, handoffs,
  final decisions, and privacy warnings.
- Keep `from`/`to` coverage and known gaps in every snapshot.
- Never pass full source files to the summarizer in this slice. Pass paths,
  bounded diff summaries, bounded stdout/stderr samples, and cited snippets.
- Build graph candidates before the summarizer call. The provider can only
  label and group candidate nodes/edges.

## Redaction and prompt discipline

Redact before provider calls:

- access tokens, passwords, private keys, connection strings, session cookies,
  auth headers, cloud credentials, SSH keys, npm/PyPI/GitHub tokens;
- obvious sensitive PII;
- terminal control sequences and delimiter tricks that could turn log text into
  instructions;
- full provider-owned raw rows unless the row is already normalized into safe
  evidence.

Prompt shape:

1. Stable system contract: "You are a narrative projector over cited evidence,
   not a source of truth."
2. Output schema, including optional label/group suggestions for known graph
   ids only.
3. Style contract: plain language, concise, no guessing, cite every claim.
4. Redaction contract.
5. Evidence window.
6. Prior snapshot summary, if any, only as context and never as evidence.

Provider output must be validated. Reject and fall back when:

- JSON is malformed;
- a claim has no evidence;
- an evidence id does not exist in the window;
- a claim uses forbidden causal language without supporting events;
- output includes raw secrets or terminal control sequences;
- output tries to instruct the harness or user outside the schema.
- output proposes graph nodes or edges whose ids were not supplied as
  candidates.

## Cadence

Narration should feel alive without becoming another stream.

Default cadence:

- deterministic fallback refresh every attach loop when cheap;
- provider-backed summary no more often than every 45 seconds per target;
- quiet threshold refresh after 30 seconds of no meaningful events while status
  is running;
- immediate refresh eligibility for blocker, failure, acceptance pass/fail,
  child completion, repair start/finish, user-input-needed, provider crash, or
  privacy redaction.

Budget controls:

- hard max provider calls per attach session, default 20;
- `--narrative-max-spend <usd>` or reuse doc-provider max-spend if already
  available in local config;
- subscription CLI providers record wall time instead of USD;
- manual `r` obeys budget and cadence unless `--force-refresh` is later added;
- no provider call in `--plain --json` unless explicitly requested by a future
  command. Plain attach should print latest or deterministic fallback.

## Verb signatures

Prefer extending `attach` instead of inventing a separate command.

```text
deadreckon attach <id>
    [--view activity|narrative|split]
    [--visual architecture|agents|files|evidence|none]
    [--narrative-provider <route>]
    [--narrative-max-spend <usd>]
    [--plain]
    [--json]
    [--no-hints]
```

Flag behavior:

- `--view activity` is the current behavior and remains default if product
  judgment says the first alpha slice should be conservative.
- `--view narrative` opens/prints the narrative projection.
- `--view split` may degrade to narrative-only on narrow terminals.
- `--visual architecture` shows the emerging command/module/file map.
- `--visual agents` shows plan task/child run/provider swimlanes.
- `--visual files` shows touched file clusters and ownership.
- `--visual evidence` shows the citation chain from events to claims.
- `--visual none` hides the side visual even in wide terminals.
- `--narrative-provider` overrides config/doc-provider/run-provider resolution.
- `--narrative-max-spend` caps provider-backed narration only.
- `--json --view narrative` prints the latest structured snapshot plus state.
- `--plain --view narrative` prints wrapped prose with citations and staleness.

TUI keys:

- `n`: toggle activity/narrative.
- `v`: cycle `architecture -> agents -> files -> evidence -> none`.
- `r`: request narrative refresh if provider summarization is enabled.
- `a`: activity when narrative view is open, only if it does not conflict with
  completed-run apply. If it conflicts, use footer text to show the actual key.
- `d`: completed run docs remain separate from live narrative.
- `Enter` in plan attach still drills into selected child.

Refusals:

| Case | Behavior |
| --- | --- |
| No provider configured for summarization | Render deterministic fallback and show `try: deadreckon config provider ...` only when actionable. |
| Provider over budget | Render latest snapshot/fallback, status `stale`, cite last covered event. |
| Redaction removes too much evidence | Render "summary withheld; redacted evidence" plus safe deterministic facts. |
| Evidence files missing or malformed | Render what exists and cite known gaps; do not crash attach. |
| No graph evidence | Show sparse "not enough architecture evidence yet" visual, not a fake map. |
| Narrow terminal | Collapse to headline/current work/risks/footer; hide visual first. |
| `NO_COLOR` or dumb terminal | Use labels, ASCII connectors, and no color-only status. |
| Non-TTY without snapshot | Print deterministic fallback, not an error. |

## Rendering contract

Overall layout:

- Wide run attach: keep the existing header/meters/files/processes/footer, but
  split the central-left activity area into narrative prose plus a right-side
  visual map when there is enough width.
- Wide plan attach: keep child panes visible and use the plan activity/feed area
  for narrative plus an agent/architecture visual.
- Narrow terminals: prose wins; visuals collapse to a compact ASCII summary or
  disappear before text truncates badly.

Run narrative pane should show:

1. Header line: freshness, coverage, provider/source, age.
2. Headline.
3. Current work: 2-5 cited bullets.
4. Architecture/system evolution: 0-4 cited bullets.
5. Risks/blockers: 0-4 cited bullets, highlighted.
6. Next likely action: 0-3 low-confidence bullets.
7. Evidence footer: latest covered run event, flight event, checkpoint, files.
8. Visual hint: current visual mode and `v` cycle help.

Plan narrative pane should show:

1. Plan headline and freshness.
2. Agent table: task, role, provider, status, what it is doing.
3. Coordination notes: dependencies, blockers, merge/repair status.
4. Cross-agent architecture evolution.
5. Risks and next likely orchestration moves.
6. Selected-child detail or footer hint to press `Enter`.

Visual pane modes:

- **Architecture** - layered map of commands/modules/files affected by the run.
- **Agents** - swimlane of plan tasks, roles, providers, child runs, deps, and
  status.
- **Files** - touched directories/files grouped by subsystem with change
  intensity and latest checkpoint/event.
- **Evidence** - compact chain from provider event/run event to claim, file, and
  checkpoint.

Style contract:

- Use semantic colors: cyan/blue for identity/evidence, magenta for active,
  green for done, yellow for risk/stale, red for blocker/error, dim for old or
  unavailable.
- Use compact badges like `[active]`, `[risk]`, `[stale]`, `[done]` so the view
  works without color.
- Use connectors and progress rails sparingly to guide the eye. The map should
  fit at a glance, not become a dense graph editor.
- Respect existing `NO_COLOR`, plain, quiet, and JSON rules.

Do not fill the pane with raw diffs, raw JSON, or long stdout. Raw evidence
stays in activity/show/history/flight views.

## Deterministic fallback

Provider-backed narration improves the prose, but the feature must be useful
without a provider call.

Fallback run summary:

- current status, phase, provider, wall time, acceptance status;
- latest run event and trace;
- last three flight events with files/checkpoints;
- live changed file count and top changed paths;
- docs/implementation-notes status if present;
- compact architecture map from changed paths and latest flight events;
- explicit "No provider-backed summary yet" line.

Fallback plan summary:

- plan status, completed/running/failed counts;
- each child task role/provider/status/latest event;
- dependencies and repair/final-gate status;
- compact agent/architecture map from task DAG and child run statuses;
- selected child run id and attach hint;
- explicit "No provider-backed plan summary yet" line.

## Phases (eleven)

Each phase: write the named depth test(s) first and watch them fail; implement;
run focused verification for touched crates; conventional local commit;
one-line CHANGELOG entry.

### P1 - CLI view mode and render state plumbing

- Add `AttachViewMode` with `activity|narrative|split`.
- Add `NarrativeVisualMode` with `architecture|agents|files|evidence|none`.
- Extend clap attach args and command dispatch.
- Thread view mode through run attach, plan attach, and plain/off-TTY branches.
- Keep current default behavior unless the implementation explicitly chooses
  narrative as a new default and updates help/docs/tests.

Depth tests:

- `attach_help_lists_view_modes_without_changing_activity_default`
- `attach_help_lists_visual_modes_without_implying_graph_fabrication`
- `attach_view_mode_round_trips_for_run_plan_and_child_refs`
- `plain_attach_activity_default_matches_previous_summary_contract`

### P2 - Narrative and graph schemas

- Add run/plan narrative state and snapshot types.
- Add architecture graph schema and IO helpers for run/plan roots.
- Implement atomic state writes and append-only snapshots.
- Add latest snapshot lookup with malformed-row tolerance.
- Keep files under `<run-root>/narrative/` and `<plan-dir>/narrative/`.

Depth tests:

- `run_narrative_state_round_trips_without_pipeline_state`
- `architecture_graph_requires_evidence_on_nodes_and_edges`
- `latest_snapshot_skips_malformed_rows_and_reports_gap`
- `plan_narrative_snapshot_preserves_child_coverage`

### P3 - Run evidence window collector

- Collect bounded evidence from run state, events, traces, spend, flight,
  acceptance, docs, and live file facts.
- Assign stable evidence ids.
- Produce graph candidates from changed files, command modules, checkpoints, and
  docs.
- Track coverage, redactions, and known gaps.
- Do not read entire source files.

Depth tests:

- `run_evidence_window_cites_flight_events_and_checkpoints`
- `run_graph_candidates_are_deterministic_for_same_evidence`
- `run_evidence_window_pins_acceptance_failure_outside_cap`
- `run_evidence_window_does_not_include_full_source_file_contents`

### P4 - Plan evidence window collector

- Collect plan events, feed events, child run latest facts, repair state, task
  deps, roles, and providers.
- Produce plan graph candidates from task DAG, provider roles, child runs, and
  child changed-file clusters.
- Prefer child narrative snapshots when available; fall back to child run
  evidence facts.
- Do not copy child flight events into `plan-events.jsonl`.

Depth tests:

- `plan_evidence_window_rolls_up_child_narratives`
- `plan_graph_candidates_render_task_dependencies_without_child_log_copy`
- `plan_evidence_window_preserves_dependencies_and_roles`
- `plan_evidence_window_does_not_mutate_plan_events`

### P5 - Redaction, validation, and deterministic fallback

- Implement redaction before summarizer input.
- Implement deterministic fallback render models for run and plan.
- Implement deterministic graph render models for run and plan.
- Validate claim citations against evidence ids.
- Reject unsupported or malformed provider output.

Depth tests:

- `narrative_redaction_removes_secret_like_values_before_provider_input`
- `claim_validation_rejects_missing_evidence_ids`
- `graph_validation_rejects_uncited_nodes_and_edges`
- `deterministic_run_fallback_is_useful_without_provider`
- `deterministic_plan_fallback_lists_children_without_provider`

### P6 - Provider-backed summarizer sidecar

- Resolve summarizer provider from flag, config/doc-provider, run provider, or
  deterministic fallback.
- Add bounded prompt builder and structured output parsing.
- Allow provider suggestions only for prose and labels/groups on known graph ids.
- Enforce cadence, max calls, spend/wall-clock accounting, and failure status.
- Ensure the sidecar writes only narrative files.

Depth tests:

- `summarizer_uses_fake_provider_and_writes_cited_snapshot`
- `summarizer_cannot_invent_architecture_graph_nodes`
- `summarizer_respects_min_interval_and_manual_refresh_budget`
- `summarizer_failure_keeps_attach_alive_with_stale_status`

### P7 - Run attach narrative TUI

- Branch `render_attach` central-left pane for narrative.
- Render a visual side pane when width allows; collapse cleanly when it does
  not.
- Add scroll handling for narrative rows.
- Add `n` toggle, `v` visual cycle, and `r` refresh.
- Keep files/processes/footer behavior coherent.
- Preserve completed docs view as separate from live narrative.

Depth tests:

- `run_attach_narrative_pane_renders_headline_current_work_and_citations`
- `run_attach_visual_pane_renders_architecture_map_with_badges`
- `run_attach_visual_cycle_preserves_scroll_and_footer`
- `run_attach_n_toggles_back_to_provider_activity`
- `run_attach_completed_docs_toggle_still_reads_run_narrative_md`
- `run_attach_narrow_terminal_keeps_footer_visible`

### P8 - Plan attach narrative TUI

- Add narrative mode to `PlanAttachRenderState`.
- Render plan narrative and visual map over the activity/feed pane or split
  view.
- Keep child panes visible.
- Keep `Enter` drilldown and return-to-plan behavior unchanged.

Depth tests:

- `plan_attach_narrative_renders_agent_table_and_coordination_notes`
- `plan_attach_agents_visual_renders_task_swimlanes_and_deps`
- `plan_attach_activity_feed_remains_one_key_away`
- `plan_attach_enter_still_drills_into_selected_child`

### P9 - Plain and JSON output

- `attach <id> --view narrative --plain` prints the latest snapshot or fallback.
- `attach <id> --view narrative --plain --visual architecture` includes an
  ASCII map when available.
- `attach <id> --view narrative --json` prints state, snapshot, and graph.
- Non-TTY never enters raw mode or blocks on provider calls by default.
- Chain attach should either support narrative with aggregate fallback or refuse
  with a clear `try:` line for run/plan attach.

Depth tests:

- `plain_narrative_attach_prints_staleness_and_citations`
- `plain_narrative_attach_renders_ascii_architecture_map`
- `json_narrative_attach_emits_state_snapshot_and_graph_objects`
- `non_tty_narrative_attach_does_not_call_provider_without_explicit_refresh`
- `chain_narrative_attach_has_clear_supported_behavior`

### P10 - Docs, help, and user-facing copy

- Update attach help, README/HOWTO where appropriate, and any user-facing
  matrix rows.
- Explain who the view is for: operators supervising longer/multi-agent work.
- Show examples without preselecting a provider brand.
- Document privacy/redaction and alpha limits.
- Document the visual map as an evidence-backed diagram, not a guessed
  architecture model.

Depth tests:

- `attach_help_explains_narrative_without_log_jargon`
- `docs_include_narrative_attach_example_without_provider_brand_lock`
- `docs_explain_visual_map_evidence_and_color_fallback`
- `privacy_docs_state_summarizer_redaction_limits`

### P11 - AS-BUILT, CHANGELOG, and V1 candidates

- Insert or update AS-BUILT sections:
  - `18.x Narrative attach view`
  - `18.x Visual architecture map`
  - `32.x Plan narrative rollup`
  - `33.x Flight evidence consumed by narrative`
- Append CHANGELOG section:

```markdown
## Narrative Attach (alpha) - 2026-05-26

- Added `deadreckon attach --view narrative` for cited run and plan overviews.
- Added run/plan narrative snapshots under runstate without changing
  `PipelineState`.
- Added evidence-backed architecture graph rendering with ASCII and no-color
  fallbacks.
- Added deterministic fallback, provider-backed refresh, and focused TUI/plain
  tests.
```

- Add V1 candidates for long-lived narrator daemon, richer trace DAG, learned
  summary preferences, richer graph layout, cloud/shareable observer views, and
  historical analytics.

No depth test required beyond docs assertions from P10.

## Verification matrix

Default verification for this goal:

```text
cargo fmt --check
cargo test -p deadreckon-core narrative
cargo test -p deadreckon narrative
cargo test -p deadreckon-runtime narrative
cargo test -p deadreckon narrative_graph
cargo clippy -p deadreckon-core -p deadreckon -p deadreckon-runtime -- -D warnings
```

Adjust package names to match where the implementation lands. If only
`crates/deadreckon` changes in an early phase, do not run runtime/core package
tests until their code is touched.

Required smokes before final commit:

1. Fake CLI run with `flight-events.jsonl` and checkpoints:
   `deadreckon attach <run> --view narrative --visual architecture --plain`
   prints cited current work and an ASCII architecture map.
2. Fake two-child plan:
   `deadreckon attach <plan> --view narrative --visual agents --plain` prints
   agent table, deps, and selected child hint.
3. Completed run with `RUN-NARRATIVE.md`:
   TUI docs toggle still works and narrative view remains separate.
4. Summarizer failure:
   attach continues and displays deterministic fallback with stale/failed status.

Do not run `make verify`, release builds, stress tests, broad smoke suites, or
full-workspace tests by default.

## Stop conditions

Stop only when:

- run attach narrative works for running and completed runs;
- plan attach narrative works for multi-child plans;
- visual maps render for run architecture, plan agents, files, and evidence,
  with no-color and narrow-terminal fallbacks;
- activity/raw logs remain accessible;
- every provider-backed claim has valid citations;
- summarizer outage, privacy redaction, missing evidence, and narrow terminal
  cases degrade cleanly;
- focused verification passes;
- AS-BUILT, CHANGELOG, and V1-CANDIDATES are updated;
- changes are committed locally with conventional commits.
