# deadreckon - Orchestration Event Bus Rider (live orchestration UX)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-18-2226-deadreckon-orchestration-eventbus-goal.md`.
It supersedes nothing in prior riders
(`2026-05-15-2252-deadreckon-plan-events-rider.md`,
`2026-05-16-1122-deadreckon-semantic-merge-repair-rider.md`,
`2026-05-17-1403-deadreckon-coherence-closure-rider.md`) - their invariants
still apply. This rider adds a shared orchestration output contract and a live
plan event feed that attach can consume without owning raw polling logic.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided - do not redesign)

- **Maturity stays `alpha`.** This is V1 polish on top of shipped alpha plans,
  plan events, child drill-down, and merge repair.
- **No `PipelineState` schema changes.** Child runs remain normal runs with
  normal run roots.
- **No durable `Plan` schema expansion unless a phase proves a tiny additive
  field is required.** User-visible state should come from existing plan fields,
  sidecars, `plan-events.jsonl`, and run state.
- **Keep `plan-events.jsonl` authoritative.** The event bus does not replace the
  file. It gives attach and summaries one replayable stream API.
- **Cross-process attach remains supported.** If an attach process cannot see an
  in-process broadcaster, the bus owns file replay/tailing internally; the TUI
  should not call `read_plan_events_lossy` in its redraw loop.
- **No persistent event database, web socket server, cloud sync, or arbitrary
  child-to-child chat.**
- **No `git push`.** Phased local commits only.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Verification budget

Each phase starts with named depth tests, then implementation, then focused
verification for the touched surface. Do not default to `make verify`, release
builds, smoke tests, stress tests, or `cargo test --workspace`. Use the smallest
set that proves the phase:

```sh
cargo nextest run -p deadreckon --test orchestrate
cargo nextest run -p deadreckon --test coherence
cargo test -p deadreckon attach_plan
cargo test -p deadreckon plan_event_bus
cargo test -p deadreckon-core plan_event
cargo fmt --check
cargo clippy -p deadreckon --all-targets -- -D warnings
```

Run broader verification only when the executor changes a broader crate contract
or the user explicitly requests it.

## Data model (files, not fields)

No new durable files are required by default.

Existing durable event sources:

```text
~/.deadreckon/plans/<plan-id>/plan-events.jsonl
~/.deadreckon/plans/<plan-id>/messages.jsonl
~/.deadreckon/plans/<plan-id>/launch/<task-id>/run-id
~/.deadreckon/runs/<run-id>/events.jsonl
~/.deadreckon/runs/<run-id>/traces.jsonl
~/.deadreckon/plans/<plan-id>/merge-proofs/repair-run.json
```

Runtime-only model, names may adjust to local Rust style:

```rust
pub enum PlanFeedEvent {
    Plan { event: PlanEvent },
    ChildRun { task_id: String, run_id: String, event: RunEvent },
    RepairRun { run_id: String, event: RunEvent },
    Snapshot { plan: Plan },
    Warning { message: String },
}

pub struct PlanEventBus { /* runtime senders, replay cursors, dedupe keys */ }
```

Do not serialize `PlanFeedEvent` unless a later phase proves a durable need.

## Event-stream algorithm

`PlanEventBus::subscribe(paths, plan_id)` should expose one async-ish stream or
receiver API to plan attach:

1. Load `plan.json`, `messages.jsonl`, and `plan-events.jsonl` for an initial
   snapshot.
2. Replay durable plan events in timestamp/file order before live events.
3. Deduplicate by a stable key derived from source, timestamp, plan id, event
   kind, task id/run id, and a compact payload hash. The exact key can differ,
   but replay plus broadcast must not double-render one event.
4. Subscribe to any in-process plan broadcaster when available.
5. Tail `plan-events.jsonl` internally for cross-process or late attach. Partial
   lines and malformed lines become warnings or are ignored without breaking the
   stream.
6. When a `TaskRunDiscovered` event or launch sidecar reveals a child run id,
   attach a child run feed and tag its `RunEvent`s with the task id.
7. When merge repair reveals a repair run id, attach the repair run feed and tag
   events as repair events.
8. Emit snapshots when plan state changes enough for selection, status, footer,
   or repair summary to refresh.

The plan TUI consumes this feed and keeps its existing child-run detail path.
Raw `read_plan_events_lossy` calls may remain in non-live summaries and tests,
but the attach redraw loop should no longer own direct plan-event polling.

## Shared orchestration output contract

Create a small renderer/model family rather than another broad UI framework.
It should support:

- Primary object id is the plan id. Result run ids and repair run ids are
  secondary.
- Provider role table columns: role, route, model, source, notes.
- Dependency summary columns: child, status, starts, waits_for, unblocks.
- Parallelism summary: "starts now: task-0, task-2" and "waits: task-1 after
  task-0" style text in preflight and fork summaries.
- Merge repair summary: enabled/disabled, mode, attempts, provider, conflicts,
  repair run id when known, proof paths, and next action.
- Footer grammar matches run/chain attach: detach first, focus/scroll next,
  view-specific action, then `try:` or lifecycle hint.

Keep list/status/history tables quiet; this rider targets orchestration
preflight/result/attach surfaces.

## Phases (eleven)

Each phase: write the named depth test(s) first and watch them fail; implement;
run focused verification from the budget above; conventional local commit;
one-line CHANGELOG entry when user-visible behavior changes.

### P1 - Freeze current orchestration surfaces

- Add narrow snapshots or render tests for current preflight, started, plan
  attach footer, and merge repair summary behavior.
- Keep the snapshots intentional: assert role/dependency/footer facts, not every
  space in a large terminal buffer.

Depth tests:
- `orchestration_preflight_snapshot_captures_provider_roles_and_parallelism`
- `plan_attach_footer_snapshot_captures_back_navigation_grammar`
- `merge_repair_summary_snapshot_captures_current_status`

### P2 - Shared orchestration summary model

- Introduce a small `OrchestrationSummary`/builder layer near existing CLI
  render helpers.
- Make plan id the primary id for plan-like flows.
- Keep `print_kv_block`, `ui::*`, glossary labels, and stream policy from the
  coherence closure.

Depth tests:
- `orchestration_summary_builder_renders_plan_primary_id`
- `orchestration_result_builder_keeps_result_run_secondary`
- `orchestration_summary_builder_uses_existing_kv_and_try_line_helpers`

### P3 - Provider role table

- Resolve role rows for full-plan and review modes: planner, default child,
  child override, coder, reviewer, and repair provider when relevant.
- Show route/model/source when the data is known. Use `-` or `config default`
  when unknown; do not invent provider metadata.

Depth tests:
- `orchestrate_preflight_prints_provider_role_table`
- `plan_summary_prints_provider_route_model_source_when_known`
- `review_mode_role_table_names_coder_and_reviewer`

### P4 - Dependency and parallelism summaries

- Compute ready-now children from current dependencies and statuses.
- Compute waiting children with their blockers.
- Render the summary in preflight and fork/start result surfaces.

Depth tests:
- `orchestrate_preflight_names_ready_parallel_children`
- `fork_summary_names_blocked_dependencies`
- `dependency_summary_updates_after_predecessor_completion`

### P5 - Shared plan/fork/merge/orchestrate result builders

- Route `print_plan_created`, `print_orchestrate_preflight`,
  `print_orchestrate_started`, `print_plan_summary`, and merge completion
  through the shared model where practical.
- Do not refactor unrelated run/chain summaries in this phase.

Depth tests:
- `plan_fork_merge_orchestrate_share_lifecycle_result_shape`
- `merge_result_keeps_finish_plan_id_before_direct_apply_or_export`
- `orchestrate_started_summary_lists_events_and_child_commands`

### P6 - Merge repair summary panel

- Replace terse `merge repair {line}` output with a structured summary.
- Include repair mode, attempts, provider, conflict count/paths, repair plan
  path, repair request path, repair run id/status, and next action.
- Preserve machine-readable fields already present in JSON outputs.

Depth tests:
- `merge_repair_summary_names_mode_attempt_provider_conflicts_run_and_next_action`
- `show_why_failed_plan_lists_repair_sidecars_and_latest_event`
- `plain_plan_summary_includes_repair_next_action_without_ansi`

### P7 - Standard plan attach footer and breadcrumbs

- Align overview footer grammar with run and chain attach.
- Keep child drill-down behavior but make back hints and breadcrumbs consistent.
- Ensure footer text changes when no child run exists or the run root is gone.

Depth tests:
- `plan_attach_footer_matches_run_chain_grammar`
- `plan_child_back_footer_uses_standard_breadcrumb`
- `plan_attach_missing_child_run_footer_has_try_line`

### P8 - PlanEventBus core

- Add a runtime bus/feed module, likely in `crates/deadreckon/src/plan_event_bus.rs`
  or a small core/runtime split if that fits ownership better.
- Reuse `tokio::sync::broadcast` patterns from `RunEventBus`.
- Provide replay, live subscription, dedupe, malformed/partial-line tolerance,
  and snapshot events.

Depth tests:
- `plan_event_bus_replays_existing_jsonl_before_live_events`
- `plan_event_bus_dedupes_replay_and_broadcast_events`
- `plan_event_bus_tolerates_partial_jsonl_line`
- `plan_event_bus_emits_snapshot_after_plan_status_change`

### P9 - Child and repair run multiplexing

- When child run ids are discovered, add tagged child run events to the plan
  feed without copying all child traces into `plan-events.jsonl`.
- When repair run ids are discovered, add tagged repair run events.
- Keep child turn/tool detail owned by existing run attach rendering.

Depth tests:
- `plan_event_bus_adds_child_stream_when_run_discovered`
- `plan_event_bus_tags_child_run_events_with_task_id`
- `plan_event_bus_tags_merge_repair_run_events`
- `plan_event_bus_does_not_duplicate_child_traces_as_plan_events`

### P10 - Wire attach to the bus

- Replace direct `read_plan_events_lossy` use in `attach_plan_tui`'s refresh
  loop with the shared feed.
- Keep non-live summaries free to call simple readers where appropriate.
- The redraw loop should react to feed updates, crossterm keys, and periodic
  health refreshes without busy polling plan events itself.

Depth tests:
- `attach_plan_consumes_plan_event_feed_not_read_plan_events_lossy_in_render_loop`
- `attach_plan_receives_live_plan_child_and_repair_events`
- `attach_plan_cross_process_feed_replays_jsonl_without_in_process_sender`

### P11 - Docs, matrix, and final audit

- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  - §18 data source: plan attach consumes a plan feed; the feed owns replay/tail.
  - §26.10 deferrals: remove or narrow richer orchestration UI and plan bus
    deferrals that this goal closes.
  - §30/§32 plans: document role tables, dependency/parallelism summary, repair
    summary, and event bus limits.
- Update `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md` O1-O7 with
  fixed/deferred status.
- Update `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` by removing closed
  candidates or narrowing remaining future work.
- Append a concise CHANGELOG section:

  ```text
  ## Orchestration live UX (alpha) - 2026-05-18

  - Added shared orchestration summaries for plan/fork/merge/orchestrate.
  - Added provider role, dependency, parallelism, and repair summaries.
  - Moved plan attach to a shared plan event feed with plan, child, and repair events.
  ```

No new depth test is required in P11 beyond doc assertions or scans already
created in earlier phases.

## Out of scope

- Full output-layout facade or template engine for all CLI surfaces.
- Themable palettes, localization, or stored enum renames.
- Rich graphical semantic merge UI or AST/language-aware merge engines.
- Persistent event database, web UI, web sockets, cloud sync, or notifications.
- Arbitrary child-to-child chat.
- Provider/done-criteria setup unification beyond fields needed in orchestration
  summaries.
- Command-matrix golden snapshots outside the orchestration surfaces named here.

## Engineering invariants

- `plan-events.jsonl` remains append-only and best-effort idempotent.
- Plan attach must tolerate missing, malformed, and partial event lines.
- Child runs remain inspectable as normal runs.
- JSON mode must contain no ANSI, hints, or human-only footer chatter.
- Plain mode must include the same essential ids and next actions as the TUI.
- Quiet mode suppresses success chatter, not requested data or errors.
- Provider wording is provider/route/model/kind for users; descriptor remains
  advanced registry vocabulary.

## Process invariants

- Do not revert unrelated user changes.
- Keep commits small and conventional.
- Keep verification focused unless the implementation broadens the blast radius.
- Record major unresolved decisions in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.
- Stop with local commits only; do not push.
