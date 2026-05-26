# deadreckon - TUI Responsiveness Rider (Responsive)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-26-1924-deadreckon-tui-responsiveness-goal.md`.
It supersedes nothing in prior riders, especially narrative attach and
orchestration event bus. Their invariants still apply. This rider adds a
responsiveness contract for every ratatui attach path.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided - do not redesign)

- **Maturity stays `alpha`.**
- **No durable schema changes by default.** Do not change `PipelineState`,
  `Plan`, `Chain`, `RunEvent`, `PlanEvent`, `NarrativeSnapshot`, or
  `AcceptanceMarker` unless a phase proves an in-memory/file-local alternative
  cannot satisfy the contract.
- **Attach remains an observer.** It may write narrative projection files that
  narrative attach already owns; it must not rewrite source events, provider
  logs, flight logs, chain events, or plan events.
- **Responsiveness beats perfect freshness.** A stale pane with an age label is
  better than a frozen terminal.
- **No renderer rewrite.** Keep `ratatui`/`crossterm`; this is scheduling,
  caching, and I/O discipline, not a new TUI framework.
- **No long-lived daemon.** Per-attach background jobs are in scope; a shared
  attach service is V1.
- **No `git push`.** Phased local commits only.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Code-path audit

### Run attach

Entry: `attach_command` -> `attach_tui` -> `attach_tui_with_parent` in
`/Users/gdc/deadreckon/crates/deadreckon/src/main.rs`.

Current loop behavior:

- reloads run state with `load_run`;
- rereads full `spend.jsonl` and `traces.jsonl` through `read_jsonl`;
- tails run events through `TuiEventFeed::file_tail`;
- calls `collect_attach_live`;
- computes panel counts and draws;
- handles keyboard input with `event::poll(Duration::from_millis(200))`.

Blocking hazards:

- Manual `r` calls `refresh_run_narrative_with_provider(...).await` inside the
  key handler. During the await, `q`, Esc, Ctrl-D, `n`, `v`, scroll, and redraw
  are unavailable.
- Automatic event and quiet refreshes call
  `refresh_run_narrative_with_provider_for_kind(...).await` before drawing the
  frame. A slow provider call can freeze the UI without a keypress.
- `collect_attach_live` calls `inventory_files(&state.working_dir)` and only
  filters `node_modules`/`.git` after the walk has already visited them.
- `collect_provider_activity` reads all `flight-events.jsonl` rows and may
  recursively scan provider log roots on every tick, then parse matching files.
- `run_narrative_lines` calls `ensure_run_projection`, which reads the latest
  snapshot and may persist projection files. Rendering should not perform
  blocking writes or full snapshot scans.
- `read_chain_step_marker` can be called in the key guard and body; keep key
  guards cheap.

### Plan attach

Entry: `attach_command` -> `attach_plan_tui`.

Current loop behavior:

- refreshes `PlanEventBus::file_tail`;
- extends in-memory plan/feed buffers capped at 1000;
- rereads plan messages;
- may auto-refresh plan narrative;
- draws and handles keyboard input.

Blocking hazards:

- Manual `r` awaits `refresh_plan_narrative_with_provider`.
- Automatic plan event/quiet refresh awaits
  `refresh_plan_narrative_with_provider_for_kind`.
- Plan narrative projection can walk child run evidence and child narrative
  state from the UI loop.
- `Enter` intentionally suspends the TUI and opens child attach; that is okay,
  but returning must not inherit stale background jobs.

### Chain attach

Entry: `chain_attach_tui`.

Current loop behavior:

- reloads `chain.json`;
- rereads full `chain-events.jsonl`;
- draws;
- polls keyboard.

Blocking hazards:

- Large chain event files grow linearly expensive.
- `Enter` suspends the TUI and runs `show_command`; that is intentionally
  modal, but the base timeline should remain cheap outside modal actions.

### Plain/off-TTY attach

Plain attach is allowed to block for a bounded snapshot because there is no
interactive key loop to preserve. Do not overfit the TTY architecture onto
plain/json paths.

## Data model (files, not fields)

No new durable schemas are required for the main fix. Add in-memory models:

```rust
struct AttachTickBudget {
    target_frame_ms: u64,
    max_sync_io_ms: u64,
    slow_warning_ms: u64,
}

struct AttachRefreshJob {
    target: AttachNarrativeTarget,
    kind: NarrativeRefreshKind,
    started_at: DateTime<Utc>,
    token: CancellationToken,
    handle: JoinHandle<Result<String>>,
}

struct AttachCached<T> {
    value: T,
    loaded_at: DateTime<Utc>,
    source_signature: AttachSourceSignature,
}
```

If phase work needs temporary diagnostics, write only under the observed run or
plan root:

```json
{
  "version": 1,
  "target": "run|plan|chain:<id>",
  "samples": [
    {
      "at": "RFC3339",
      "loop_ms": 12,
      "draw_ms": 4,
      "sync_io_ms": 5,
      "slow_path": "collect_attach_live"
    }
  ]
}
```

Do not add this file unless tests and UX require it. If added, it must be
best-effort and never block attach.

## Design rules

- Rendering functions must be pure over already-loaded data. No provider calls,
  tree walks, or full JSONL reads from `render_*`.
- The input loop must never await a provider subprocess.
- At most one narrative refresh job may be in flight per attach target.
- Manual `r` while a refresh is in flight coalesces: update the notice, record
  the newest requested kind, and do not spawn a second provider.
- Detach cancels in-flight refreshes via `ProviderRequest.cancellation_token`
  where supported, then exits without waiting for provider completion.
- Automatic refreshes should skip or coalesce when the provider is already
  running; they should not queue unbounded work.
- Heavy collectors get TTLs, source signatures, or file offsets. Tick-time work
  should usually read deltas, not whole files.
- Attach-specific file inventory must prune ignored directories before walking
  their children.
- Any stale pane must say why it is stale or how old it is.

## Phases (eleven)

Each phase: write the named depth test(s) first and watch them fail; implement;
run focused verification for touched crates; conventional local commit;
one-line CHANGELOG entry.

### P1 - Responsiveness budget and loop instrumentation

- Add internal timing helpers around run, plan, and chain attach loop stages.
- Keep instrumentation in-memory by default.
- Surface slow paths in tests, not in normal UI output unless a later phase adds
  an explicit diagnostic flag.

Depth tests:

- `attach_tick_budget_records_slow_sync_stage_without_panicking`
- `run_attach_loop_model_marks_provider_refresh_as_async_work`
- `plan_attach_loop_model_marks_provider_refresh_as_async_work`

### P2 - Background run narrative refresh

- Introduce an attach-local run refresh job type.
- Manual `r` spawns a job and redraws immediately.
- Completion updates `narrative_notice` and reloads the latest projection.
- Detach cancels the job and exits.

Depth tests:

- `run_attach_manual_refresh_does_not_block_quit`
- `run_attach_manual_refresh_coalesces_when_in_flight`
- `run_attach_refresh_completion_updates_notice_once`
- `run_attach_detach_cancels_in_flight_refresh`

### P3 - Background automatic run refresh

- Move event-triggered and quiet-threshold run narrative refreshes to the same
  job mechanism.
- Coalesce automatic refreshes with manual ones.
- Preserve cadence, budget, redaction, and validation semantics.

Depth tests:

- `run_attach_event_refresh_spawns_background_job`
- `run_attach_quiet_refresh_does_not_block_frame_draw`
- `run_attach_auto_refresh_skips_when_manual_refresh_in_flight`
- `run_attach_refresh_failure_remains_visible_until_replaced`

### P4 - Background plan narrative refresh

- Mirror P2/P3 for `attach_plan_tui`.
- Keep selected child, feed buffers, and key handling responsive while the
  plan-level narrator runs.
- Cancel plan refresh jobs on detach or child-drill suspension.

Depth tests:

- `plan_attach_manual_refresh_does_not_block_quit`
- `plan_attach_event_refresh_spawns_background_job`
- `plan_attach_refresh_coalesces_by_plan_id`
- `plan_attach_child_drill_cancels_or_suspends_refresh_cleanly`

### P5 - Attach-specific file inventory pruning

- Replace `inventory_files` use in `collect_attach_live` with an attach
  inventory walker that prunes ignored directories before descending.
- Ignore at least `.git`, `node_modules`, `.deadreckon` cache noise where safe,
  `.tmp/chrome-profile*`, and other browser profile/build cache directories
  named by tests.
- Preserve visible project files and promoted-library artifact behavior.

Depth tests:

- `attach_live_inventory_prunes_node_modules_before_descending`
- `attach_live_inventory_prunes_chrome_profile_tmp_before_descending`
- `attach_live_inventory_still_counts_recent_project_files`
- `attach_live_inventory_caps_display_without_losing_total_count`

### P6 - Incremental JSONL and flight readers

- Add reusable tail/offset readers for attach-owned JSONL streams.
- Apply to run spend/traces/flight where safe.
- Keep `TuiEventFeed` behavior for events; do not regress malformed/partial
  line tolerance.
- Preserve plain/off-TTY full reads where simpler.

Depth tests:

- `attach_jsonl_tail_reads_only_appended_rows`
- `attach_jsonl_tail_tolerates_partial_last_line`
- `run_attach_spend_and_trace_cache_updates_from_mtime`
- `run_attach_flight_activity_uses_incremental_rows`

### P7 - Provider log scan throttling

- Stop recursively scanning descriptor provider roots every frame.
- When `flight-events.jsonl` has current rows, prefer it and delay fallback
  provider-log discovery.
- Cache provider-log candidates by root, mtime, and freshness window.
- Keep descriptor ingest correctness for Codex, Claude Code, Gemini, OpenCode,
  Copilot, and Pi.

Depth tests:

- `provider_activity_does_not_rescan_roots_each_tick`
- `provider_activity_prefers_flight_rows_over_fallback_scan`
- `provider_activity_fallback_scan_respects_freshness_and_cwd`
- `provider_activity_cache_invalidates_when_matching_log_changes`

### P8 - Render purity and narrative projection cache

- Ensure `render_attach`, `render_plan_attach`, and narrative line builders do
  not do provider calls, expensive scans, or unnecessary disk writes.
- Cache latest narrative projections by coverage and snapshot id.
- Avoid reading all of `snapshots.jsonl` on every frame.

Depth tests:

- `run_narrative_render_uses_cached_projection_when_coverage_unchanged`
- `plan_narrative_render_uses_cached_projection_when_feed_unchanged`
- `render_attach_text_does_not_append_narrative_snapshots`
- `stale_provider_snapshot_survives_redraw_without_churn`

### P9 - Chain attach event cache

- Replace full `chain-events.jsonl` rereads in `chain_attach_tui` with an
  attach-local tail cache.
- Preserve existing chain controls and modal `Enter` behavior.
- Add an age/status hint if event reading falls behind.

Depth tests:

- `chain_attach_uses_incremental_event_tail`
- `chain_attach_large_event_file_keeps_tick_under_budget`
- `chain_attach_partial_event_line_is_ignored_until_complete`

### P10 - Cross-surface responsiveness smokes

- Add deterministic slow-provider and slow-filesystem fixtures.
- Exercise run attach, plan attach, child attach, and chain attach loop models.
- Verify critical keys are observed while background work is pending.

Depth tests:

- `slow_run_narrator_still_allows_quit`
- `slow_plan_narrator_still_allows_visual_toggle`
- `large_worktree_live_files_still_draws_recent_files`
- `large_chain_timeline_still_scrolls`

### P11 - Architecture doc, CHANGELOG, and V1 candidates

- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` §18, §22,
  §28, §30/§32, and the narrative attach paragraph to describe the
  responsiveness contract.
- Add CHANGELOG entries for background refresh, cache/incremental readers, and
  known alpha limits.
- Add any deferred daemon/shared-broadcaster/diagnostic-dashboard ideas to
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md`.

Depth tests:

- `as_built_documents_nonblocking_attach_contract`
- `changelog_mentions_tui_responsiveness_alpha_slice`
- `v1_candidates_records_out_of_scope_attach_daemon`

## Integration matrix

| Surface | Current risk | Required result |
|---|---|---|
| Run activity attach | full-file/tree reads per tick | cached or incremental reads; keys responsive |
| Run narrative attach | manual/auto provider await in loop | background job, coalesced refresh, cancellable detach |
| Plan activity attach | child/feed disk work in loop | bounded feed refresh and cached child facts |
| Plan narrative attach | provider await in loop | background job and visible pending state |
| Plan child attach | inherits run attach risks | same run fixes apply with breadcrumb intact |
| Chain attach | full `chain-events.jsonl` reread | incremental tail cache |
| Plain/json attach | can block for snapshot | unchanged except shared helpers may be reused |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| narrative refresh provider unavailable | `deadreckon attach <id> --view narrative --no-narrative-provider` |
| refresh already running | `press r after the current refresh finishes` |
| attach cache fell behind | `deadreckon attach <id> --plain` |
| provider log scan skipped by budget | `deadreckon show <id> --flight` |

## Out of scope

- A daemonized attach service shared across terminals.
- Remote/web attach UI.
- Full async rewrite of all ratatui/crossterm handling.
- New graph renderer or visualization crate.
- Replacing durable JSONL logs with a database.
- Changing provider transcript formats or provider-owned logs.
- Changing `PipelineState`, `Plan`, `Chain`, or event schemas for convenience.

## Dependencies

- **Tier 1:** none expected. Existing `tokio`, `tokio-util`, `ratatui`,
  `crossterm`, `walkdir`, and JSONL helpers should be enough.
- **Tier 2:** none expected. If a file-watcher crate is proposed, log the
  design in `DEPENDENCIES.md` and prove polling/caching cannot satisfy the
  depth tests.
- **Tier 3:** no new UI frameworks, graph engines, local databases, or daemon
  supervisors in this slice.

## Engineering invariants

- No durable schema changes unless explicitly justified in the phase commit.
- Every phase starts with the named depth tests failing.
- No provider call is awaited inside a TUI input/draw loop.
- No render function performs tree walks, provider-log discovery, subprocess
  calls, or append-only writes.
- File readers used every tick are incremental, cached, capped, or throttled.
- Detach must not wait for a provider to finish.
- Background jobs must be cancelled or detached cleanly; no orphaned narrator
  subprocesses from attach.
- Stale data must be labeled; silent staleness is a bug.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with focused tests passing and a CHANGELOG entry naming the
  slice.
- Run `cargo fmt --check` after touched Rust phases; stable-rustfmt warnings
  about unstable config keys are acceptable only if there are no diffs.
- Use focused command-level or render-model smokes; avoid broad release/stress
  verification unless the human asks.
- If a phase reveals a need for a daemon, DB, or renderer replacement, record it
  in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` and continue with the
  bounded alpha fix.
