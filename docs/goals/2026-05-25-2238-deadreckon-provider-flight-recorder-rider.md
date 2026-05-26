# deadreckon - Provider Flight Recorder Rider (Recoverable)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-25-2238-deadreckon-provider-flight-recorder-goal.md`.
It supersedes nothing in prior riders
(`2026-05-13-1345-deadreckon-provider-cli-ingest-rider.md`,
`2026-05-13-1705-deadreckon-copilot-pi-providers-rider.md`,
`2026-05-15-2252-deadreckon-plan-events-rider.md`,
`2026-05-18-2226-deadreckon-orchestration-eventbus-rider.md`) - their
invariants still apply. This rider adds a truthful flight-recorder layer for
CLI providers: provider-native events are captured as subturns inside a normal
DeadReckon turn, then correlated to working-tree checkpoints for preview-first
rewind.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided - do not redesign)

- **Maturity stays `alpha`.**
- **No `PipelineState` schema changes.** New state lives in files under each run
  root.
- **DeadReckon turns remain the outer mutation boundary.** A CLI provider may
  produce many provider-native subturns, but state.turn still advances once for
  the CLI subprocess unless a future explicit design changes the runtime.
- **Provider-owned logs are read-only.** The recorder may store source paths,
  line offsets, timestamps, and raw hashes; it must not rewrite provider logs.
- **Checkpointing must be honest.** Provider log rows alone do not prove exact
  file state. Exact subturn rewind is allowed only for checkpoints captured
  while the provider was running.
- **No full-workspace verification by default.** Avoid `make verify`, release
  builds, stress tests, smoke suites, and full-workspace tests unless a phase
  changes a broad surface and justifies it.
- **No `git push`.** Phased local commits only.
- **Edits stay inside `/Users/gdc/deadreckon/`.**

## Current assessment

The current runtime already has the pieces to build from:

- `run_turn_loop` snapshots `turn - 1`, calls `router.complete`, then records
  one `llm.complete` for the provider response.
- CLI-backed responses have `trace.kind = "cli_subagent"`. The loop treats the
  entire subprocess as one `tool.cli_subagent`, compares files against the
  pre-turn snapshot, snapshots `turn`, writes provenance for the changed files,
  and commits one worktree commit: `turn N: cli_subagent`.
- `attach` already discovers provider-native logs through descriptor
  `[ingest]` metadata and renders normalized activity lines.
- `deadreckon import` already normalizes provider transcripts into trace-like
  rows, but imported rows are separate completed import runs, not live flight
  events attached to the original run.
- `undo` already restores a turn snapshot, but it has no preview, no file hash
  guard, no provider-subturn target, and no deleted-file diff hardening.

This goal connects these pieces without pretending provider-native rows are
normal DeadReckon turns.

## Data model (files, not fields)

All new files live under `<run_root>/`.

### `flight-manifest.json`

```json
{
  "version": 1,
  "run_id": "dr-...",
  "sessions": [
    {
      "flight_session_id": "flight-turn-1-attempt-1",
      "provider": "cli:codex",
      "schema": "codex-cli",
      "deadreckon_turn": 1,
      "attempt": 1,
      "status": "running|completed|failed|killed|superseded",
      "started_at": "<RFC3339>",
      "completed_at": "<RFC3339|null>",
      "source_paths": [
        {
          "path": "/Users/gdc/.codex/sessions/...jsonl",
          "first_line": 1,
          "last_line": 42,
          "content_hash": "sha256:..."
        }
      ]
    }
  ],
  "checkpoint_policy": {
    "mode": "delta-with-anchors",
    "quiet_ms": 750,
    "poll_ms": 500,
    "anchor_every": 20
  }
}
```

### `flight-events.jsonl`

Each row is append-only and source-neutral:

```json
{
  "version": 1,
  "seq": 17,
  "run_id": "dr-...",
  "flight_session_id": "flight-turn-1-attempt-1",
  "deadreckon_turn": 1,
  "attempt": 1,
  "provider": "cli:codex",
  "schema": "codex-cli",
  "timestamp": "<RFC3339|null>",
  "source_path": "/Users/gdc/.codex/sessions/...jsonl",
  "source_line": 41,
  "source_event": "response_item:function_call",
  "raw_hash": "sha256:...",
  "kind": "tool",
  "role": "assistant",
  "summary": "tool edit src/setup.rs",
  "tool_name": "apply_patch",
  "tool_category": "write",
  "files": ["src/setup.rs"],
  "usage": {
    "input_tokens": 1234,
    "output_tokens": 456,
    "context_window": 200000
  },
  "checkpoint_id": "cp-000017"
}
```

Kinds: `agent`, `thinking`, `tool`, `result`, `todo`, `tokens`, `session`,
`checkpoint`, `warning`, `error`.

### `checkpoints/<checkpoint-id>/manifest.json`

```json
{
  "version": 1,
  "checkpoint_id": "cp-000017",
  "run_id": "dr-...",
  "flight_session_id": "flight-turn-1-attempt-1",
  "deadreckon_turn": 1,
  "attempt": 1,
  "provider_event_seq": 17,
  "created_at": "<RFC3339>",
  "trigger": "provider_tool|file_quiet|provider_exit|manual",
  "base": {
    "kind": "turn-snapshot|checkpoint-anchor",
    "id": "turn-0"
  },
  "full_anchor": false,
  "files": [
    {
      "path": "src/setup.rs",
      "change": "modified",
      "before_hash": "sha256:...",
      "after_hash": "sha256:...",
      "after_bytes": "files/src/setup.rs"
    },
    {
      "path": "src/old.rs",
      "change": "deleted",
      "before_hash": "sha256:...",
      "after_hash": null,
      "after_bytes": null
    }
  ],
  "working_tree_hash": "sha256:..."
}
```

For changed/created files, store full after-bytes under
`checkpoints/<id>/files/<relative-path>`. For deleted files, store only the
manifest row. Periodic anchors may store a full tree copy under
`checkpoints/<id>/anchor/`; anchors trade disk for faster materialization.

### `rewind-events.jsonl`

Record preview and apply attempts without rewriting older logs:

```json
{
  "version": 1,
  "timestamp": "<RFC3339>",
  "run_id": "dr-...",
  "target": {"kind": "provider-event", "id": "17"},
  "mode": "preview|apply",
  "status": "ok|refused",
  "files": ["src/setup.rs"],
  "reason": null
}
```

## Checkpointing algorithm

Implement a `ProviderFlightRecorder` in a small module, likely
`crates/deadreckon-runtime/src/flight.rs` plus CLI-facing render helpers in
`crates/deadreckon/src/main.rs` or a new `flight_view.rs`.

Runtime placement:

1. In `run_turn_loop`, when `config.provider` is a CLI provider, start the
   recorder immediately before `router.complete(&request).await`.
2. Pass it the immutable run identity, working directory, run root, provider id,
   DeadReckon turn, descriptor ingest metadata, and optional event sender.
3. Stop/finalize it immediately after the provider call returns or errors.
4. Do not change the `Provider` trait in the first slice. The recorder observes
   provider logs and the filesystem from the runtime side.
5. Create one `flight_session_id` per provider invocation, not per run. A run may
   have multiple CLI turns if implementation notes or acceptance causes a
   follow-up turn. A resumed run may also create a new attempt for the same
   DeadReckon turn.

Recorder loop:

1. Build an initial file index from the working directory:
   `relative_path -> {hash, size, modified_at}`. Include missing/deleted paths
   by comparing against the prior index, not only the current inventory.
2. Resolve descriptor ingest roots the same way attach/import do. Candidate
   provider files must match the run working dir/cwd and freshness window.
3. Capture the starting path+line/byte offsets before spawning the provider so
   old rows from prior sessions in the same cwd are not imported into this
   flight session.
4. Tail candidate provider files by remembered path+line/byte offsets. Parse
   rows through shared normalized provider-event code, not one-off string
   parsing.
5. Append each new normalized row to `flight-events.jsonl` with a monotonically
   increasing `seq`.
6. Treat events as mutation-like when they have non-empty `files`, tool category
   `write`, `edit`, `shell`, `bash`, `apply_patch`, or provider-specific tool
   names mapped by taxonomy.
7. Poll the working tree on a small interval, default 500 ms. When the file
   index changes and stays quiet for 750 ms, capture a checkpoint. Also capture
   immediately after mutation-like provider events when file changes are already
   visible.
8. Correlate a checkpoint to the nearest previous provider event within the
   current turn. If none exists, write a `checkpoint` event with
   `trigger=file_quiet`.
9. On provider exit, capture a final checkpoint if the file index differs from
   the previous checkpoint, then let the existing turn snapshot still capture
   the official after state.

Resume/retry boundaries:

1. `resume --from-turn N` must mark flight sessions with
   `deadreckon_turn > N` as `superseded` in `flight-manifest.json` before the
   next provider invocation starts.
2. Normal `show --flight`, rewind target resolution, and attach should use only
   non-superseded sessions unless a future explicit `--all-attempts` flag is
   added.
3. Superseded checkpoints remain audit evidence but are inspect-only in this
   goal. Rewind apply to a superseded checkpoint must refuse.
4. Existing trace/spend truncation semantics may stay as they are; this rider
   does not require making older artifacts append-only.

Checkpoint storage:

1. Compare the new file index against the last checkpoint index, not only
   against the pre-turn snapshot. This preserves intermediate states.
2. For created/modified files, copy full after-bytes into the checkpoint.
3. For deleted files, write a manifest entry with `change=deleted`. This closes
   the current deleted-file blind spot where changed-file detection only
   iterates files still present after the turn.
4. Every N checkpoints, default 20, write a full anchor directory. Materializing
   a target may start from the nearest prior anchor or from `snapshots/turn-N`.
5. Keep checkpoint writes atomic: write under `.tmp-<id>`, fsync files where the
   existing helpers do so, then rename to `checkpoints/<id>`.

Materialization:

1. To preview/apply a target checkpoint, materialize the target state into a temp
   dir under `<run_root>/rewind-preview/<target>/`.
2. Start from the nearest base snapshot or checkpoint anchor.
3. Replay delta checkpoints through the target id in sequence.
4. Diff current working dir against the materialized target.
5. Refuse apply when any file that would be changed has a current hash different
   from the latest recorder-known hash, unless an explicit future override flag
   is implemented in a separate goal.
6. Apply only files in the target diff. Do not delete or overwrite unrelated
   files.
7. `--apply` must acquire the same run/task lock family used by run/resume, and
   must refuse while the run is actively executing. Read-only preview may run
   during execution, but it must label the preview as live/unstable.

## Operation mode compatibility

The goal is seamless in the sense that every mode has a clear behavior, not
that every mode gets identical powers.

| Mode/surface | Flight recording | Checkpoint rewind apply | Notes |
|---|---:|---:|---|
| Worktree run | yes for CLI providers | yes while the worktree exists | After `apply --cleanup`, preview remains possible from run-root artifacts but apply refuses because the temporary worktree is gone. |
| Copy mode | yes for CLI providers | yes against the run working copy | Exported destinations are not tracked; rewind applies to the run working dir, not arbitrary export dirs. |
| Fresh mode | yes for CLI providers | yes against the run working dir | Same mechanics as copy mode, with no source repo assumptions. |
| In-place mode | yes for CLI providers | yes, guarded, highest caution | Apply targets the user's source tree. Hash guards are mandatory; unrelated edits must be untouched or cause refusal for touched paths. |
| HTTP/JSON-action provider | no provider flight | turn snapshot only | Existing DeadReckon turn traces remain the source of truth. |
| `extend` | child-local | child-local | Extended runs get their own recorder; the parent is not rewritten. |
| `resume` | new flight session per rerun | active sessions only | Superseded sessions are inspect-only. |
| Chain step | run-local | run-local | A chain step is a normal run; chain events do not copy flight rows. |
| Plan child | child-local | child-local | `plan-id:task-id` resolves to the child run. Plan events only reference child ids. |
| Merged plan result | no new flight unless repair child runs | result-run turn snapshot only | Child flight records stay under child run roots. |
| Imported run | normalized import traces only | no checkpoint rewind | There was no live filesystem checkpointing during the imported session. |
| Cleaned/abandoned worktree | inspect/preview only | refuse | Run-root artifacts may remain; removed working dirs cannot be safely applied to. |

Mode-specific tests belong in P8/P9, not only docs.

Performance guardrails:

- Ignore `.git`, `target`, `.deadreckon`, `node_modules`, and common large
  dependency/build directories unless the run working tree explicitly includes
  them in a future config.
- Cap captured individual files at a conservative size, e.g. 5 MiB, and mark
  oversized files as `skipped_oversize` with a refusal if rewind would require
  exact bytes.
- Keep default checkpoint polling lightweight. If `notify` or FSEvents becomes
  desirable, log the dependency choice first; polling is acceptable for alpha.
- Keep all captured bytes local under the run root. This is not a cloud sync
  feature. Captured file bytes have the same sensitivity as existing snapshots,
  so docs must warn users before sharing run roots.

## Verb signatures

Add `show` flags:

```text
deadreckon show <run-id> --flight
deadreckon show <run-id> --file <path>
deadreckon show <plan-id>:<task-id> --flight
```

Add a new top-level verb:

```text
deadreckon rewind <run-id>
    --to-turn <n>
    --to-provider-event <seq>
    --to-checkpoint <id>
    --preview
    --apply
    --plain
    --json
```

Rules:

- Exactly one target flag is required.
- `--preview` or `--apply` is required; default to preview only if existing CLI
  command style already has that convention. Otherwise refuse with a `try:`.
- `--apply` requires the hash guard to pass.
- `--to-turn` may delegate to existing snapshots, but the output must use the
  same preview/apply/report shape as checkpoint rewind.
- `--to-provider-event` resolves to that event's checkpoint id. If no checkpoint
  exists, refuse and point at the nearest checkpoint before/after.

## Output contracts

`show --flight`:

```text
flight recorder
  provider        cli:codex
  turn            1
  events          42
  checkpoints     9
  source          /Users/gdc/.codex/sessions/...

17  14:32:09  tool     write       src/setup.rs       cp-000017
18  14:32:14  result   tests fail  cargo test setup   -
19  14:33:02  tool     write       src/setup.rs       cp-000019
```

`rewind --preview`:

```text
rewind preview
  run             dr-abc123
  target          provider-event 19
  checkpoint      cp-000019
  files           2 changed, 1 deleted
  guarded         passed

will modify:
  src/setup.rs     current sha256:... -> target sha256:...
will delete:
  src/old.rs

try: deadreckon rewind dr-abc123 --to-provider-event 19 --apply
```

Refusal lines must include exactly one actionable `try:` line when possible.

## Phases (eleven)

Each phase: write the named depth test(s) first and watch them fail; implement;
run focused verification for touched crates; conventional local commit; add a
CHANGELOG bullet. Do not run full-suite verification by default.

### P1 - Flight event schema and append helpers

- Add core structs for `FlightManifest`, `FlightEvent`, `CheckpointManifest`,
  `CheckpointFileChange`, and `RewindEvent`.
- Add path helpers for `flight-events.jsonl`, `flight-manifest.json`,
  `checkpoints/`, and `rewind-events.jsonl`.
- Add append/read helpers using existing JSONL conventions.

Depth tests:

- `flight_event_round_trips_with_provider_source_and_checkpoint`
- `flight_manifest_records_source_paths_without_raw_payload_copy`
- `flight_manifest_tracks_multiple_sessions_and_superseded_attempts`
- `rewind_event_append_is_jsonl_and_append_only`

### P2 - Shared provider transcript normalization

- Extract import/activity parsing into reusable normalized provider-event
  functions. Avoid duplicating Codex/Claude/Copilot/Pi/Gemini/OpenCode parsing
  logic in the recorder.
- Preserve existing attach/import behavior.
- Normalize tool category through `deadreckon_providers::taxonomy`.

Depth tests:

- `codex_log_row_normalizes_to_flight_tool_event`
- `claude_tool_use_normalizes_to_flight_tool_event`
- `copilot_and_pi_usage_rows_normalize_context_tokens`
- `unknown_provider_row_records_raw_hash_and_warning_event`

### P3 - Working-tree index and delta checkpoint engine

- Implement file indexing with hashes, size, mtime, and deleted-file detection.
- Implement delta checkpoint capture with copied after-bytes for
  created/modified files and deletion manifest rows.
- Implement periodic full anchors.
- Exclude build/dependency directories by default.

Depth tests:

- `checkpoint_delta_captures_created_modified_and_deleted_files`
- `checkpoint_materializes_from_turn_snapshot_plus_deltas`
- `checkpoint_anchor_materialization_skips_prior_deltas`
- `checkpoint_refuses_oversize_file_when_exact_rewind_needed`

### P4 - Live recorder sidecar around CLI provider execution

- Start the recorder only for CLI providers.
- Keep the provider trait unchanged unless impossible.
- Tail descriptor-discovered provider logs while `router.complete` is running.
- Finalize on provider success, provider error, cancellation, or kill.

Depth tests:

- `cli_turn_starts_and_finalizes_flight_manifest`
- `cli_second_turn_gets_distinct_flight_session`
- `flight_recorder_tails_fake_provider_log_during_cli_run`
- `flight_recorder_ignores_provider_rows_before_start_offset`
- `flight_recorder_finalizes_after_cancel_without_corrupt_jsonl`
- `http_provider_does_not_start_flight_recorder`

### P5 - Event-to-checkpoint correlation

- Trigger checkpoints for mutation-like provider events and quiet file changes.
- Attach `checkpoint_id` to the nearest provider event when correlation is
  defensible.
- Emit standalone checkpoint events for file changes without provider rows.

Depth tests:

- `mutation_like_provider_event_gets_correlated_checkpoint`
- `quiet_file_change_without_log_row_emits_checkpoint_event`
- `provider_event_without_file_state_does_not_claim_exact_rewind`

### P6 - Show flight and file provenance views

- Add `show --flight`.
- Add `show --file <path>` or extend existing show output so users can ask why a
  file changed.
- Include DeadReckon turn, provider event seq, checkpoint id, source row, and
  summary.
- JSON output must include no ANSI and stable `kind`, `id`, `events`, `paths`,
  and `try_lines`.

Depth tests:

- `show_flight_lists_provider_events_and_checkpoints`
- `show_file_links_path_to_provider_event_and_checkpoint`
- `show_flight_json_has_no_ansi_and_stable_paths`

### P7 - Rewind preview

- Add `rewind` target resolution for turn, provider event, and checkpoint.
- Materialize target state into a preview temp dir.
- Produce a changed/created/deleted file report.
- Refuse provider-event targets with no checkpoint and suggest nearest
  checkpoint.

Depth tests:

- `rewind_preview_to_checkpoint_reports_modified_created_deleted`
- `rewind_preview_to_provider_event_resolves_checkpoint`
- `rewind_preview_refuses_provider_event_without_checkpoint`
- `rewind_requires_exactly_one_target`

### P8 - Rewind apply with hash guard

- Apply only previewed file changes.
- Refuse when current files do not match latest recorder-known hashes.
- Record `rewind-events.jsonl`.
- Preserve provider logs and append-only trace/flight history.

Depth tests:

- `rewind_apply_restores_guarded_files_only`
- `rewind_apply_refuses_user_modified_file`
- `rewind_apply_refuses_while_run_is_executing`
- `rewind_apply_refuses_removed_worktree_but_preview_still_materializes`
- `rewind_apply_in_place_requires_hash_guard_for_source_tree`
- `rewind_apply_refuses_superseded_checkpoint`
- `rewind_apply_records_rewind_event_without_rewriting_flight`
- `rewind_to_turn_uses_same_preview_and_apply_contract`

### P9 - Attach/TUI and plan child integration

- Surface `flight-events.jsonl` in attach when present.
- Keep existing provider activity lines as live fallback before flight events
  are written.
- For `plan-id:task-id`, route to the child run flight view.
- Do not copy child flight events into `plan-events.jsonl`.

Depth tests:

- `attach_activity_prefers_flight_events_after_recorder_flush`
- `attach_keeps_provider_activity_fallback_during_first_rows`
- `plan_child_show_flight_resolves_child_run`
- `chain_step_flight_stays_run_local`
- `imported_run_has_no_checkpoint_rewind_try_line`

### P10 - Friendliness, refusals, and help

- Add help text for `rewind`, `show --flight`, and `show --file`.
- Error footers must distinguish "no flight recorder", "no checkpoint for
  provider event", "hash guard failed", and "oversize file skipped".
- Plain/JSON/quiet behavior must follow the current output policy.

Depth tests:

- `rewind_help_documents_preview_before_apply`
- `rewind_no_flight_recorder_refusal_has_try_line`
- `rewind_hash_guard_refusal_names_user_modified_files`
- `flight_surfaces_respect_plain_and_json_modes`

### P11 - Architecture doc, V1 candidates, and CHANGELOG

- Update `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md`:
  - section 14 telemetry: add `flight-events.jsonl`, `flight-manifest.json`,
    checkpoint manifests, and rewind events.
  - section 16 import: explain shared normalization with live flight recording.
  - section 18 TUI data source: explain flight events vs provider activity fallback.
  - section 22/what-is-built: add provider flight recorder and checkpoint rewind.
- Update `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` with remaining larger
  ideas: provider-controlled step execution, patch-level exact rewind when
  provider logs lack file bytes, web/desktop flight dashboard, and remote/team
  sharing of flight records.
- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:
  ```
  ## Provider flight recorder and checkpoint rewind (alpha) - 2026-05-25

  - ...
  ```

No depth test for P11 unless docs tooling already has a focused docs check.

## Integration matrix

| Surface | Flight events | Checkpoints | Rewind |
|---|---:|---:|---:|
| HTTP JSON-action provider | no | existing turn snapshots only | turn snapshot only |
| CLI provider live run | yes | yes | provider-event/checkpoint/turn |
| Imported provider session | normalized import traces only | no | no, unless future replay creates checkpoints |
| Plan child run | child-local | child-local | through child run id or `plan-id:task-id` |
| Plan event stream | references child runs only | no copy | no copy |
| Worktree/copy/fresh/in-place | run-local | mode-specific | see operation mode compatibility |

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| No `flight-events.jsonl` for run | `try: deadreckon show <run-id>` |
| Provider event has no checkpoint | `try: deadreckon rewind <run-id> --to-checkpoint <nearest> --preview` |
| Hash guard failed | `try: review the listed files, then rerun preview after saving or discarding your edits` |
| Oversize file skipped | `try: deadreckon rewind <run-id> --to-turn <n> --preview` |
| Ambiguous target flags | `try: choose exactly one of --to-turn, --to-provider-event, or --to-checkpoint` |
| Run still executing during apply | `try: deadreckon attach <run-id>` |
| Worktree was cleaned or abandoned | `try: deadreckon rewind <run-id> --to-checkpoint <id> --preview` |
| Superseded checkpoint target | `try: deadreckon show <run-id> --flight` |

## Config additions

Avoid durable config in the first slice. Use internal defaults:

```text
poll_ms = 500
quiet_ms = 750
anchor_every = 20
max_file_bytes = 5242880
```

If user-tunable config becomes necessary, log the proposed keys in
`docs/V1-CANDIDATES.md` before adding durable config.

## Out of scope

- Forcing CLI providers into DeadReckon's one-action JSON protocol.
- Rewriting provider-owned transcripts or adding provider transcript undo.
- Exact subturn rewind for events with no captured checkpoint.
- Web/desktop dashboard.
- Remote/team sharing of flight events.
- Rewinding exported directories after copy/fresh export.
- Applying superseded resume-attempt checkpoints.
- New provider descriptors beyond fixtures needed for tests.
- Full AST-aware patch rewind or semantic undo.
- Running full verification suites by default.

## Dependencies

Tier 1 expected:

- Existing `walkdir`, JSON/serde, sha/hash helpers already present in the repo.
- Standard library polling and filesystem metadata.

Tier 2 only if justified:

- `notify` or platform-specific file watchers. Prefer polling for alpha unless
  focused tests prove polling is too expensive or unreliable.

Tier 3 blocked:

- Background daemons, kernel extensions, provider SDK rewrites, or dependencies
  that require network services for local rewind.

## Engineering invariants

- No `PipelineState` schema changes.
- Append-only logs stay append-only. Rewind adds new events; it does not edit
  old traces, provenance, flight events, or provider logs.
- Flight events are provider-native subturns, not DeadReckon turns.
- Checkpoint rewind is exact only when the checkpoint captured file bytes.
- Deleted files must be represented explicitly.
- Hash guards are mandatory for apply.
- Keep descriptor ingest and import normalization shared where practical.
- No silent expansion. Anything beyond P1-P11 goes into `V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with focused tests passing and a CHANGELOG bullet.
- Do not run `make verify`, release builds, stress tests, smoke suites, or
  full-workspace tests by default.
- If a phase reveals a V1 architecture decision, log it in
  `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` and continue without expanding
  scope.
