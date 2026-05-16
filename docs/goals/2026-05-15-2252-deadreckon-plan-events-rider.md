# deadreckon — Plan Events Rider (orchestration observability + attach navigation)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-15-2252-deadreckon-plan-events-goal.md`.
It supersedes nothing in prior riders
(`2026-05-11-1444-deadreckon-orchestrate-rider.md`,
`2026-05-11-2208-deadreckon-autonomous-chain-rider.md`,
`2026-05-11-2248-deadreckon-overnight-rider.md`,
`2026-05-13-1900-deadreckon-coherence-rider.md`) — their invariants still
apply. This rider adds a first-class plan event stream and makes the plan TUI
navigation behave like a coherent stack: plan overview -> child run detail ->
back to the same plan context.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided — do not redesign)

- **Maturity stays `alpha`.** This is observability and navigation parity for the existing orchestration model.
- **No `PipelineState` schema changes.** Child run execution remains the existing run/extend turn loop and run-root `events.jsonl`.
- **No `Plan` schema expansion unless a field is truly needed for persistent semantics.** Event state lives in append-only `plan-events.jsonl`.
- **Keep `messages.jsonl`.** It remains typed coordinator communication, not the canonical lifecycle/activity stream.
- **No arbitrary child-to-child chat.** Children communicate through existing summaries and coordinator messages only.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Rich web UI, push notifications, remote collaboration, and arbitrary inter-agent chat go to `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Data model (files, not fields)

### `plan-events.jsonl`

Path: `/Users/gdc/.deadreckon/plans/<plan-id>/plan-events.jsonl`.

Each line is one `PlanEvent`:

```json
{
  "timestamp": "2026-05-15T22:52:00Z",
  "plan_id": "<plan-uuid>",
  "event": {
    "kind": "task_run_discovered",
    "task_id": "task-1",
    "task_index": 1,
    "run_id": "<child-run-id>",
    "pid": 12345
  }
}
```

The core module should expose:

```rust
pub const PLAN_EVENTS_JSONL: &str = "plan-events.jsonl";

pub enum PlanEventKind {
    PlanCreated { mode: PlanMode, task_count: usize },
    PlanStarted,
    TaskReady { task_id: String, task_index: usize },
    TaskStarted { task_id: String, task_index: usize },
    TaskRunDiscovered { task_id: String, task_index: usize, run_id: String, pid: Option<u32> },
    TaskCompleted { task_id: String, task_index: usize, run_id: Option<String>, status: String },
    TaskBlocked { task_id: String, task_index: usize, reason: String },
    TaskFailed { task_id: String, task_index: usize, reason: String },
    TaskKilled { task_id: String, task_index: usize, run_id: Option<String> },
    MergeStarted,
    MergeConflict { conflict_count: usize },
    MergeCompleted { merged_run_id: String },
    PlanCompleted,
    PlanFailed { reason: String },
    PlanKilled,
}

pub struct PlanEvent {
    pub timestamp: DateTime<Utc>,
    pub plan_id: String,
    pub event: PlanEventKind,
}
```

Naming can adjust to existing Rust style, but the JSON wire shape must stay
stable and `serde(tag = "kind", rename_all = "snake_case")` like `RunEventKind`.

### Existing plan files remain

```text
~/.deadreckon/plans/<plan-id>/
  plan.json
  coordinator.json
  messages.jsonl
  plan-events.jsonl
  worker-specs/
  summaries/
  merge-working/
  merge-proofs/
  launch/<task-id>/run-id
```

`plan-events.jsonl` is the orchestration timeline. `messages.jsonl` is still
the coordinator mailbox. Child run roots still own run-level `events.jsonl`,
`traces.jsonl`, `spend.jsonl`, acceptance progress, and provider-native ingest.

## Attach navigation contract

Plan attach owns a small navigation stack:

```rust
enum PlanAttachView {
    PlanOverview,
    ChildRunDetail { task_index: usize, run_id: String },
}
```

The exact Rust shape may differ, but the behavior is fixed:

1. `deadreckon attach <plan-id>` opens the plan overview.
2. The selected task starts where it did today: first running task, else first non-completed task, else first task.
3. `Enter` on a task with a discovered child run id pushes `ChildRunDetail`.
4. The child run detail uses the existing run attach render path and data collectors. Do not create a second run UI.
5. `Esc`, `Backspace`, or `b` from child detail returns to the plan overview.
6. Returning restores selected task, plan scroll offsets, activity scroll, and footer state.
7. `q` detaches from either view without killing plan or child.
8. The breadcrumb is visible in both views:
   - Plan: `plan <prefix> / overview`
   - Child: `plan <prefix> / task-2 / run <prefix>`
9. The child footer includes a back hint: `[Esc/b] Back to plan`.

## Event emission rules

Emit events at durable transitions only. Do not flood `plan-events.jsonl` with
every child trace row; child run `events.jsonl` already owns turn/tool detail.

Required emit points:

- `plan_command` / `orchestrate --preview` when a plan is saved:
  `PlanCreated`.
- `fork_command` when status becomes `Forked`:
  `PlanStarted`.
- Ready task discovery:
  `TaskReady`.
- Task status set to running:
  `TaskStarted`.
- Child pid observed:
  `TaskRunDiscovered` with pid and run id when known. If pid arrives before run id, emit a second `TaskRunDiscovered` when run id is written.
- Child process exits and run state is loaded:
  `TaskCompleted`, `TaskFailed`, `TaskBlocked`, or `TaskKilled` according to run/plan task status.
- Dependency-blocked pending tasks:
  `TaskBlocked`.
- Merge command starts:
  `MergeStarted`.
- Merge conflict refusal:
  `MergeConflict` followed by `PlanFailed` if merge cannot proceed.
- Merge success:
  `MergeCompleted` then `PlanCompleted`.
- Plan kill:
  `PlanKilled`, plus `TaskKilled` for discovered live children.

Events must be append-only and best-effort idempotent. Re-running `fork` after
failure may append new events; do not rewrite old lines.

## Plain / non-TTY contract

`deadreckon attach <plan-id> --plain` or off-TTY summary must show:

- plan status and latest plan event
- task table with task id, role, provider, status, run prefix if known
- explicit commands:
  - `deadreckon attach <plan-id>`
  - `deadreckon attach <child-run-id>`
  - `deadreckon show <child-run-id>`
  - `deadreckon show <plan-id> --why-failed` when not completed

Plain output must not require terminal control or raw mode.

## Verb signatures

No new top-level verbs in this milestone.

```text
deadreckon attach <plan-id>
deadreckon attach <plan-id> --plain
deadreckon attach <child-run-id>
deadreckon show <plan-id> --why-failed
deadreckon history grep <pattern> --plan <plan-id>
```

Internal helper additions are expected:

```rust
append_plan_event(paths, plan_id, event)
read_plan_events(paths, plan_id)
tail_plan_events(path)
```

## Refusal cases

| Case | Behavior | `try:` |
|---|---|---|
| Enter on task with no child run id yet | Stay in plan overview and show footer notice | `deadreckon fork <plan-id>` |
| Child run id missing on disk but task says running | Stay in plan overview and show stale-child notice | `deadreckon show <plan-id> --why-failed` |
| Child run id exists but run root is gone | Stay in plan overview; mark child detail unavailable | `deadreckon list --all` |
| Malformed `plan-events.jsonl` line | Ignore bad line, show warning in activity | `deadreckon show <plan-id> --why-failed` |
| Plan id ambiguous | Existing resolver behavior | existing resolver `try:` |

Each refusal must have a depth test or be covered by a parameterized render test.

## Phases (eleven)

Each phase: write the named depth test(s) **first** and watch them fail;
implement; green on focused tests for the touched surface and, at milestone
boundaries, `cargo build --release && cargo test --workspace && cargo clippy
--workspace -- -D warnings && cargo fmt --check`; conventional local commit;
one-line CHANGELOG entry.

### P1 — Plan event data model

- Add `PLAN_EVENTS_JSONL`, `PlanEventKind`, `PlanEvent`, `append_plan_event`, and `read_plan_events` to `deadreckon-core`.
- Re-export from `deadreckon-core/src/lib.rs` consistently with `RunEvent`.
- Keep serialization tag style aligned with `RunEventKind`.

Depth tests:
- `plan_event_jsonl_roundtrips_all_kinds`
- `append_plan_event_writes_under_plan_dir`
- `plan_event_kind_uses_snake_case_tags`

### P2 — Plan creation and start events

- Emit `PlanCreated` when `plan.json` is first saved by `plan` or `orchestrate --preview`.
- Emit `PlanStarted` when `fork` moves a plan to `Forked`.
- Avoid duplicate `PlanCreated` when loading or previewing existing plans.

Depth tests:
- `plan_writes_plan_created_event_when_saved`
- `fork_writes_plan_started_event_once`
- `orchestrate_preview_writes_plan_created_without_fork_events`

### P3 — Task readiness and start events

- Emit `TaskReady` for ready pending tasks at the moment the coordinator batches them.
- Emit `TaskStarted` when a task status becomes running.
- Include task id and task index in every task event.

Depth tests:
- `fork_emits_task_ready_for_ready_batch`
- `fork_emits_task_started_before_child_launch`
- `blocked_dependency_does_not_emit_task_started`

### P4 — Child run discovery events

- Emit `TaskRunDiscovered` when child pid is known.
- Emit/update discovery when `launch/<task-id>/run-id` appears.
- For review mode, the reviewer child launched through `extend` must emit the same discovery event.

Depth tests:
- `fork_emits_task_run_discovered_with_pid_and_run_id`
- `review_mode_emits_reviewer_extend_run_discovered`
- `plan_kill_can_use_discovered_run_id_from_event_or_sidecar`

### P5 — Task terminal events

- Emit `TaskCompleted` for completed child runs.
- Emit `TaskFailed` for failed child runs.
- Emit `TaskBlocked` for dependency-blocked pending tasks and coordinator blockers.
- Emit `TaskKilled` for killed child runs and plan kill cascades.

Depth tests:
- `fork_emits_task_completed_with_run_status`
- `fork_emits_task_failed_for_red_child`
- `blocked_pending_task_gets_task_blocked_event`
- `kill_plan_emits_task_killed_for_live_child`

### P6 — Merge and plan terminal events

- Emit `MergeStarted` at merge entry after plan resolution.
- Emit `MergeConflict` when conflicts are detected.
- Emit `MergeCompleted` with merged run id on success.
- Emit `PlanCompleted` when merged, and `PlanFailed` when merge or fork terminal failure prevents completion.

Depth tests:
- `merge_emits_started_and_completed_events`
- `merge_conflict_emits_conflict_event_before_refusal`
- `failed_plan_emits_plan_failed_event`
- `merged_plan_emits_plan_completed_after_merge_run_id`

### P7 — Plan attach event feed

- Add a plan TUI event feed equivalent to the run attach file-tail path.
- Tail `plan-events.jsonl` and reload `plan.json` / messages / child state on each tick.
- Render plan activity from plan events first; render coordinator messages as a secondary panel/detail.

Depth tests:
- `plan_attach_tails_plan_events_without_restart`
- `plan_attach_activity_prefers_plan_events_over_messages`
- `plan_attach_handles_partial_plan_event_line`

### P8 — Drill-down and back navigation

- Introduce explicit plan attach view state.
- `Enter` opens selected child run detail when a run id exists.
- `Esc`, `Backspace`, or `b` returns to plan overview.
- Preserve selection and scroll offsets.

Depth tests:
- `attach_plan_enter_opens_selected_child_run_detail`
- `attach_plan_back_returns_to_same_selected_task`
- `attach_plan_q_detaches_from_child_without_killing`
- `attach_plan_enter_without_run_id_shows_try_footer`

### P9 — Breadcrumbs, footer hints, and plain output

- Add breadcrumbs to plan and child detail views.
- Add `[Esc/b] Back to plan` in child detail.
- Extend non-TTY / `--plain` plan summary with child attach/show commands.
- Keep `--quiet` behavior unchanged for run/fork success stdout.

Depth tests:
- `plan_attach_overview_breadcrumb_names_plan`
- `plan_attach_child_breadcrumb_names_task_and_run`
- `plan_attach_child_footer_includes_back_hint`
- `plan_plain_summary_lists_child_attach_and_show_commands`

### P10 — Integration hardening and failure friendliness

- Ensure `show <plan-id> --why-failed` can cite latest plan events.
- Ensure `history grep --plan <plan-id>` can search `plan-events.jsonl` alongside child run traces/provenance.
- Add malformed-event tolerance.
- Verify `run`, `extend`, `resume`, and `chain` event behavior is not regressed.

Depth tests:
- `show_why_failed_plan_cites_latest_plan_event`
- `history_grep_plan_searches_plan_events`
- `malformed_plan_event_line_does_not_break_attach`
- `run_extend_resume_chain_event_paths_unchanged`

### P11 — Architecture doc update + CHANGELOG

- Insert a new top-level section into `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:

  ```text
  ## 32. Plan Observability

  32.1 Plan event stream
  32.2 Relationship to child run events and chain events
  32.3 Attach navigation stack
  32.4 Plain/headless summaries
  32.5 Current limits
  ```

- Update §22 "What's Built vs Scaffolding-Thin":
  - Add to shipped: plan-level event stream, plan attach drill-down/back navigation, plan event grep/failure surfacing.
  - Note this closes the §30.5 "broadcast-backed plan event stream remains future work" gap for file-backed plan events; if no broadcast bus lands, say broadcast remains future.
- Update §30.5 Current Limits accordingly.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:

  ```text
  ## Plan observability (alpha) - 2026-05-15

  - Added plan-events.jsonl as the orchestration-level event timeline.
  - Added plan attach drill-down/back navigation between plan overview and child run detail.
  - Added plan event surfacing in plain attach, why-failed, and history grep.
  ```

No depth test required beyond doc/changelog review and existing doc assertions.

## Integration matrix

| Surface | Existing event source | After this rider |
|---|---|---|
| `run` | run-root `events.jsonl` | unchanged |
| `extend` | child run-root `events.jsonl` | unchanged |
| `resume` | same run-root `events.jsonl` | unchanged |
| `chain` | `chain-events.jsonl` plus step run events | unchanged |
| `plan` / `fork` / `orchestrate` | `messages.jsonl`, child run state, child summaries | adds `plan-events.jsonl` |
| `merge` | merge manifest, synthetic promoted run | adds merge plan events |
| `attach <plan-id>` | reloads plan/messages/child state | tails plan events and supports navigation stack |
| `history grep --plan` | child traces/provenance | includes plan events |
| `show --why-failed <plan>` | child blockers/messages | includes latest relevant plan events |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| selected task has not launched | `deadreckon fork <plan-id>` |
| child run missing | `deadreckon show <plan-id> --why-failed` |
| plan event file malformed | `deadreckon show <plan-id> --why-failed` |
| merge conflict blocks plan completion | `deadreckon merge <plan-id> --strategy prefer-child --prefer-child <idx>` |
| plan already completed | `deadreckon show <plan-id>` |

## Out of scope

- Live in-process `PlanEventBus` broadcast is optional in this rider. File-backed `plan-events.jsonl` is required; if broadcast does not land, leave it in V1 or AS-BUILT current limits.
- Web UI, remote streaming, notifications, and team presence.
- Arbitrary child-to-child chat.
- Changing child run `events.jsonl`, `PipelineState`, or `Plan` schema for data already represented by events.
- Reworking run attach layout beyond what is necessary to embed/reuse it from child detail.
- Cloud sync of plan/run event streams.

## Dependencies

Tier 1:
- Existing `serde`, `serde_json`, `chrono`, `tokio`, and `ratatui` only.
- Existing `append_json_line` for durable JSONL writes.

Tier 2:
- None expected. Do not add a watcher crate unless file-tail polling proves insufficient in tests.

Tier 3:
- No database, daemon, web server, or remote service dependency.

## Engineering invariants

- **No `PipelineState` schema changes.**
- **Files, not fields.** Plan lifecycle observations live in `plan-events.jsonl`.
- **Child runs stay normal runs.** Do not duplicate turn/tool traces into plan events.
- **Append-only event log.** Do not rewrite or compact `plan-events.jsonl`.
- **Navigation is reversible.** Drill-down must always offer a back path to the exact plan context.
- **Plain mode parity.** Anything essential in TUI has a script-friendly command in plain output.
- **Depth tests first.** Every P1-P10 phase has named tests that fail before implementation.
- **No silent expansion.** Anything beyond P1-P11 goes into `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Each implementation phase ends with focused tests passing and a CHANGELOG line.
- Full verification target at major milestones:
  `cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`.
- If full verification is too slow during a narrow phase, run the touched test targets plus clippy/fmt and record the deferred full gate before P11.
- After P11, optionally capture a short terminal transcript of `attach <plan-id>` drilling into a child and returning.
- If a phase reveals a V1 architecture decision, log it in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`; do not expand scope silently.
