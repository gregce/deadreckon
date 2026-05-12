# deadreckon — Orchestration Rider (multi-agent on a goal-driven harness)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-11-1444-deadreckon-orchestrate-goal.md`.
It supersedes nothing in prior riders
(`2026-05-10-1400-deadreckon-build-rider.md`,
`2026-05-11-1400-deadreckon-primary-flow-rider.md`,
`2026-05-11-1400-deadreckon-robust-rider.md`,
`2026-05-11-1400-deadreckon-usability-rider.md`) — their invariants, dependency
policy, sandbox defaults, CLI surface, lifecycle hints, and existing
verbs still apply. This rider adds **plans**, **children**, a
**coordinator**, four new top-level verbs, multi-pane TUI, and the
ergonomic conventions the multi-agent view requires.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided — do not redesign)

- **Maturity stays `alpha`.** Workspace stays `version = "0.1.0"`. This
  is feature work that extends the alpha substrate, not a V1.
- **No `PipelineState` schema changes.** Plan + child lineage live in
  **files** (this rider's `plan.json`, child-side
  `.deadreckon/parent.json`, coordinator-side `coordinator.json`).
- **No new architectural axes.** A child run uses the existing turn
  loop, gate, promotion, locks, scopes — unchanged. The coordinator is
  a supervisor process, not a new state machine inside `state.json`.
- **No `git push`.** Phased local commits only.
- **No V1 invention.** If a phase reveals a V1-architecture decision,
  log it in `/Users/gdc/deadreckon/docs/V1-CANDIDATES.md` and continue.
- **Edits stay inside `/Users/gdc/deadreckon/`.** No edits to stoa or
  any other repo.

## Data model (files, not fields)

### `plan.json`

Path: `/Users/gdc/.deadreckon/plans/<plan-id>/plan.json`.

```json
{
  "schema_version": 1,
  "plan_id": "<uuid>",
  "root_goal": "<original user goal>",
  "n": 3,
  "sub_goals": [
    { "index": 0, "goal": "<sub-goal text>",
      "child_run_id": null, "child_scope": null, "status": "pending" }
  ],
  "parent_scope": "<canonical parent-scope or null>",
  "status": "pending|forked|merged|failed",
  "created_at": "<RFC3339>",
  "forked_at": null,
  "merged_at": null,
  "merged_run_id": null,
  "deadreckon_version": "<crate version>"
}
```

`sub_goals[i].status` transitions: `pending → running → completed | failed | killed`.
`plan.status` transitions: `pending → forked → merged | failed`.

### Child `.deadreckon/parent.json` (extends usability-rider schema)

A child's working dir contains:

```json
{
  "schema_version": 1,
  "kind": "plan_child",
  "parent_plan_id": "<plan-uuid>",
  "parent_scope": "<parent scope>",
  "parent_goal": "<root goal>",
  "child_index": 0,
  "sub_goal": "<this child's sub-goal>",
  "created_at": "<RFC3339>",
  "deadreckon_version": "<crate version>"
}
```

The `kind` field distinguishes from `materialized` and `extended` per
`usability-rider`. `deadreckon show <run-id>` reads this and reports
`Child <index> of plan <plan-id>` in the header.

### `coordinator.json`

Path: `/Users/gdc/.deadreckon/plans/<plan-id>/coordinator.json`.
Written by `deadreckon fork` while running; deleted on clean exit.

```json
{
  "schema_version": 1,
  "plan_id": "<plan-uuid>",
  "coordinator_pid": 12345,
  "started_at": "<RFC3339>",
  "children": [
    { "child_index": 0, "run_id": "<uuid>", "pid": 12346,
      "scope": "<scope>", "status": "running" }
  ]
}
```

### Sub-scope naming

`<parent-scope>-c<index>`. Example: parent scope `hello-rust-ab12cd34`
yields children `hello-rust-ab12cd34-c0`, `…-c1`, `…-c2`. The parent
scope is the canonical run-root scope at `plan` time; if `--cwd` is
absent at fork time, the coordinator's cwd is used.

### File layout under `~/.deadreckon/plans/<plan-id>/`

```
plan.json
coordinator.json        (present only while a fork is supervised)
merge-working/          (created by `merge`; promoted to library/ on gate pass)
merge-proofs/           (gate output for the merge run)
```

Children themselves live under the normal
`~/.deadreckon/runstate/<sub-scope>/runs/<child-run-id>/`. The plan
directory contains pointers, never copies of child state.

## Eleven phases

Each phase: (1) write the named depth test(s) **first** and watch them
fail; (2) implement; (3) run
`cargo build --release && cargo test --workspace && cargo clippy --workspace -- -D warnings && cargo fmt --check`
green; (4) conventional-commit local commit; (5) one-line CHANGELOG
entry.

### P1 — TUI streaming (foundation; closes thin #1)

- Switch the single-run `attach` TUI from disk-polling to a
  `tokio::sync::broadcast` subscription on `RunEventBus`
  (`crates/deadreckon-core/src/events.rs`). The bus exists; wire it.
- Emit events from `turn_loop`: `TurnStart`, `ToolCallStart`,
  `ToolCallResult`, `TokenUsageDelta`, `SpendDelta`, `Error`, `TurnEnd`.
- Status bar gains: per-turn timer (seconds since `TurnStart`),
  context-burn estimate (input tokens / model context limit), budget
  callout (`78% of $10 cap`) when `spend / max_spend > 0.6`.
- Detach (`Ctrl-D`) cleanly without killing the run; status-bar
  reminder always visible.

Depth tests (in `crates/deadreckon/tests/orchestrate.rs`):
- `tui_streams_tool_call_within_250ms` — fixture run; assert receiver
  gets the event before the previous 500 ms polling interval.
- `tui_detach_does_not_kill_run` — start, detach, verify run continues,
  reattach, verify state preserved.
- `tui_budget_callout_appears_above_60_percent` — fixture spend.jsonl;
  TUI renders the callout.

### P2 — Plan data model + paths

- New module `crates/deadreckon-core/src/plan.rs` with the `Plan`
  struct + JSON round-trip. No CLI yet.
- Extend `DeadreckonPaths` to expose `plan_dir(plan_id)`,
  `plan_json(plan_id)`, `coordinator_json(plan_id)`,
  `merge_working(plan_id)`.
- Extend the child-side `parent.json` writer to support `kind:
  "plan_child"` per the schema above.

Depth tests:
- `plan_json_serializes_roundtrip` — write/read a fully-populated
  `Plan`; equality on round-trip.
- `child_parent_json_plan_kind` — write/read with `kind: "plan_child"`;
  required fields present.

### P3 — `plan` verb (provider-driven decomposition)

- New verb. Provider prompt template (in
  `crates/deadreckon-core/src/plan.rs`): asks for a JSON array of N
  short sub-goals, each ≤ 120 chars, distinct, parallelizable.
- Validate provider output: ≥ 2 sub-goals, each non-empty after trim,
  no duplicates after lowercase/whitespace-normalize.
- Write `plan.json` with status `pending`.
- Print post-action hints (see "Hints" below).

Depth tests:
- `plan_writes_plan_json_with_n_subgoals` — mock provider returns 3
  sub-goals; assert file present, schema valid.
- `plan_refuses_one_subgoal_response` — mock returns 1 sub-goal;
  assert exit non-zero, error text + `try:` line, no plan.json
  written.
- `plan_n_flag_clamped_to_2_through_6` — `--n 1` and `--n 7` rejected.

### P4 — `fork` verb (spawn children)

- New verb. Loads `plan.json`, refuses if status != `pending`.
- For each sub-goal, spawns a child `deadreckon run` subprocess with:
  - `--goal "<sub-goal>"`
  - `--scope <parent-scope>-c<index>`
  - the inherited resource flags (`--max-spend`, `--max-wall-seconds`,
    `--sandbox`, `--provider`)
  - `--parent-plan-id <plan-uuid>` (new internal flag; rider §"Verb
    signatures" lists)
- Acquires each child's task lock per existing `lock.rs` semantics.
- Writes `coordinator.json` with all child PIDs.
- Foregrounds as the coordinator until all children terminate.
- On Ctrl-C: SIGTERM all children, wait 2 s, SIGKILL stragglers,
  exit 130.
- On all children completing: set `plan.status` based on aggregate
  (all `Completed` → `forked` stays until `merge`; any failed/killed
  → record but plan remains `forked` so user can merge partials).

Depth tests:
- `fork_spawns_n_children_with_distinct_scopes` — fixture plan;
  fork; assert N child runstate dirs exist, distinct scopes.
- `fork_refuses_when_plan_already_forked` — re-fork yields the
  expected error + `try:` line.
- `fork_writes_coordinator_json_with_child_pids` — assert each PID
  alive at write time.

### P5 — Multi-pane attach TUI

- `deadreckon attach <plan-id>` opens a plan TUI: grid of N panes
  (auto-layout up to 6 children; ≥4 children → 2-row grid).
- Each pane shows: child index, sub-goal (truncated), status, current
  turn, spend, progress bar (turns / estimated total ≈ unknown so use
  spend / cap; if `--max-spend` absent, render activity dots), latest
  trace line.
- Keys: `Enter` drills into the focused child (existing single-run
  TUI); `Esc` returns to plan view; `Ctrl-D` detaches without killing;
  `q` quits the TUI (does not kill); arrow keys move focus.
- Plan TUI subscribes to each child's `RunEventBus` via the child's
  events file mirror (each child run writes `events.jsonl`; the plan
  TUI tails them via the broadcast wired in P1 when the child is
  running on this host, else falls back to the file tail).
- `deadreckon attach <run-id>` (non-plan arg) is unchanged.

Depth tests:
- `attach_plan_shows_n_panes` — TUI snapshot test using crossterm's
  test backend; assert N panes rendered.
- `attach_plan_enter_drills_then_esc_returns` — synthesize input
  events; assert view changes.
- `attach_plan_ctrl_d_detaches_does_not_kill` — children survive
  detach.

### P6 — `merge` verb (compose, gate, promote)

- New verb. Refuses if plan status != `forked` or if any child is
  still `Running`.
- For each child in index order, copies child's `library/<scope>/<run-id>/`
  into `~/.deadreckon/plans/<plan-id>/merge-working/`.
- **Conflict detection.** If two children wrote the same path with
  different SHA-256, behavior depends on `--strategy`:
  - `fail-on-conflict` (default): exits with the conflict error +
    `try: --strategy prefer-child <idx>`.
  - `prefer-child <idx>`: that child's content wins; conflict logged
    to `merge-proofs/conflicts.json`.
- Once merge-working is composed, the existing gate runs against it
  (acceptance YAML if present, else `cargo test` heuristic, else
  pass-by-default). Failure aborts before promotion.
- On gate success: atomic rename `merge-working/` →
  `library/<merged-scope>/<merged-run-id>/`, write
  `manifest.json` with the plan_id + child run_ids, mark
  `plan.status = merged`, write `plan.merged_run_id`.
- Print materialize hint.

Depth tests:
- `merge_composes_disjoint_children` — 2 fixture children writing
  different files; merge succeeds; both files present in library
  entry.
- `merge_fails_on_conflict_default` — 2 children write same path
  different content; merge fails with conflict error + `try:` line.
- `merge_prefer_child_resolves_conflict` — `--strategy prefer-child 1`
  picks child 1's content; conflicts.json recorded.
- `merge_promotes_to_new_library_entry` — assert
  `library/<scope>/<merged-id>/` exists with manifest.json.
- `merge_refuses_running_child` — child still running → error with
  `try: kill or wait`.

### P7 — Cross-process cancellation + plan cascade (closes thin #3)

- `deadreckon kill <run-id>` upgrade: implement true cross-process
  termination using PID liveness probe + SIGTERM/SIGKILL (existing
  primitives in `lock.rs`).
- `deadreckon kill <plan-id>`: reads `coordinator.json`, SIGTERMs each
  child PID, waits 2 s, SIGKILLs stragglers, then SIGTERMs the
  coordinator. With `--force`, skips the 2 s wait.
- The HTTP/CLI request paths honor `tokio_util::sync::CancellationToken`
  hierarchy (`run_token → turn_token → tool_token`) per the robust
  rider's §3 plan if not already landed.

Depth tests:
- `kill_plan_cascade_cleans_all_children_in_under_5s` — fork 3-child
  plan, kill plan-id, assert all child PIDs gone, all locks released,
  all child `state.json` move to `Killed`.
- `kill_run_across_processes_terminates_in_5s` — start run in
  subprocess A, `kill` from subprocess B, assert termination.
- `kill_during_http_streaming_aborts_request` — long mock-streaming
  response; kill mid-stream; assert reqwest task aborted, no orphan
  socket (check with `lsof`-style probe or process FD count).

### P8 — `history grep` (closes thin #10 + lands need #5)

- New sub-command `deadreckon history grep <pattern>`.
- Walks `~/.deadreckon/runstate/**/library/**/traces.jsonl` (and
  `provenance.jsonl` when `--kind provenance`), substring-matches
  each line, prints
  `<run-id-prefix> <iso-ts> <scope> | <matched-line-trimmed>`.
- Flags:
  - `--plan <plan-id>` — restrict to children of that plan.
  - `--scope <scope>` — restrict to that scope.
  - `--since <duration>` — accepts `7d`, `24h`, `30m`; filters by file
    mtime.
  - `--kind trace|provenance` — default `trace`.
  - `--limit <N>` — default 100; output is truncated with a `…
    (N more)` line.
  - `--regex` — flip to regex match (validate pattern up-front; reject
    invalid regex with `try: re-quote or escape`).

Depth tests:
- `history_grep_substring_finds_pattern_across_library` — seed 2
  runs with known traces; assert both matches found.
- `history_grep_plan_scope_excludes_others` — plan with 2 children
  + 1 unrelated run; `--plan <id>` excludes the unrelated.
- `history_grep_regex_invalid_pattern_errors` — `--regex '['` exits
  non-zero with try-footer.
- `history_grep_limit_respected` — seed 200 matches; assert ≤ limit
  output + truncation line.

### P9 — `show --why-failed` (lands need #8)

- Extend `deadreckon show <id>`:
  - `--why-failed` for a run-id: if `status == Completed`, print
    `no failures detected`; else scan last 10 trace entries for
    `exit_code != 0`, `level == "error"`, or panic strings; print
    the 3 most recent: `turn N tool=<T> exit=<code> tool_call_id=<id>`
    + a 200-char stderr snippet.
  - `--why-failed` for a plan-id: aggregate over children. Report
    each child's status; for any non-completed child, print its
    one-line RCA + `try: deadreckon show <child-run-id> --why-failed`.
    If merge failed: report the gate failure or conflict that blocked
    promotion.

Depth tests:
- `show_why_failed_completed_says_no_failures` — completed fixture
  → expected exact string.
- `show_why_failed_failed_emits_rca` — fixture run that errored at
  turn 3; output contains turn 3, exit code, stderr snippet.
- `show_why_failed_plan_names_blocking_child` — 3-child plan where
  child 1 failed → output names child 1 + its child-run-id +
  try-line.

### P10 — Friendliness pass

- **Error footer convention.** Every user-facing error printed by the
  binary has the form:

  ```
  error: <terse description, no trailing period>
    try: <one specific command or fix>
  ```

  Implementation: a `pub fn user_error(msg: &str, try_hint: &str) ->
  ! ` helper in `crates/deadreckon-core/src/error.rs`; every existing
  `.unwrap()` / `.expect()` user-facing call-site that surfaces to the
  user is routed through it.

  Canonical pairs (this rider enforces these via a parameterized test):

  | Error | `try:` line |
  |---|---|
  | `no plan <id>` | `deadreckon plan list` |
  | `no run <id>` | `deadreckon list` |
  | `plan <id> is <status>` | status-specific (see verb section) |
  | `child <idx> is <status>` | `deadreckon show <child-run-id> --why-failed` |
  | `conflict at <path>` | `--strategy prefer-child <idx>` |
  | `merge promotion blocked by gate` | `deadreckon show <plan-id> --why-failed` |
  | `no provider configured` | `deadreckon init` |
  | `lock held by pid <P>` | `kill <P> or wait` |
  | `--n must be 2..=6` | `deadreckon plan ... --n 3` |
  | `--goal must be non-empty` | `deadreckon plan "your goal"` |
  | `dest <path> is not empty` | `--force or choose a fresh path` (already in usability-rider) |

- **`--quiet`.** Available on `run`, `fork`, `merge`, `resume`,
  `extend`, `materialize`. Suppresses ALL stdout on success (exit 0,
  no text). Errors and warnings still go to stderr. Post-action hints
  suppressed. Implies `--no-hints` (existing flag).
- **`--plain`.** Available on `run`, `fork`, `attach`, `merge`. Forces
  non-TTY behavior; no ratatui, no ANSI, no spinners. Periodic
  plain-text progress on stderr (interval 2 s):
  `[<id-prefix>] turn=N tool=<T> spend=$X.YZ status=<S>`. Final line
  on completion: `[<id-prefix>] completed turns=N spend=$X.YZ`. On
  failure: `[<id-prefix>] failed reason=<short> turn=N`.
  `--quiet` + `--plain` together: only the final-on-completion or
  final-on-failure line.
- **Post-action hints.** Stdout, suppressed by `--no-hints` or
  `--quiet`:

  After `plan`:
  ```
  plan: <plan-id> with <N> sub-goals
  edit: vim /Users/gdc/.deadreckon/plans/<plan-id>/plan.json
  fork: deadreckon fork <plan-id>
  ```

  After `fork` (clean exit, all children completed):
  ```
  forked: <plan-id> done with <N>/<N> completed
  attach: deadreckon attach <plan-id>
  merge:  deadreckon merge <plan-id>
  ```

  After `fork` (some children failed):
  ```
  forked: <plan-id> done with <M>/<N> completed
  why:    deadreckon show <plan-id> --why-failed
  merge:  deadreckon merge <plan-id>  (proceeds with completed children only — confirm with --strategy)
  ```

  After `merge` (success):
  ```
  merged: <merged-run-id>
  materialize: deadreckon materialize <merged-run-id> --dest ./<task-prefix>
  ```

- **Task-first `--help` grouping** (do not rewrite; add clap
  group attributes):
  - Lifecycle: `init`, `plan`, `fork`, `run`, `attach`, `merge`,
    `list`, `kill`, `resume`, `materialize`, `extend`
  - Inspection: `show`, `history`, `doctor`
  - Recovery: `undo`, `import`
  - Config: `config get/set`

Depth tests:
- `error_messages_end_with_try_footer` — parameterized over the
  canonical pair table; capture stderr; assert each line present.
- `quiet_emits_no_stdout_on_success` — `run --smoke --quiet`; assert
  stdout byte count == 0; exit 0.
- `plain_mode_progress_works_without_tty` — set
  `TERM=dumb DEADRECKON_FORCE_PLAIN=1`; assert periodic lines emitted,
  no ANSI escapes in output.
- `quiet_plain_combined_emits_only_final_line` — assert exactly one
  matching line in output.

### P11 — AS-BUILT update + CHANGELOG (doc only; no depth test)

- Insert a new top-level section in
  `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` after current
  §17 (CLI Surface):

  ```
  ## 18. Plans & Multi-Agent Orchestration

  18.1 Mental model (one plan → N children → one merge)
  18.2 plan.json schema (verbatim quote from plan.rs)
  18.3 Child lineage (parent.json with kind: plan_child)
  18.4 Coordinator process (coordinator.json, supervision lifecycle)
  18.5 Sub-scope naming
  18.6 Multi-pane TUI (layout sketch)
  18.7 Merge strategy & conflict resolution
  18.8 Cancellation cascade
  18.9 Plan-aware history grep
  18.10 What's not yet built (e.g., team-context WriteToTeam action,
        auto-decompose-during-run)
  ```

- Renumber existing §18–§23 → §19–§24. Update the TOC. Update any
  internal cross-references (search for `§18`, `§19`, …, `§23` in the
  doc and adjust).
- Update §22 (now §23) "What's Built vs Scaffolding-Thin":
  - **Move to "Built and reliable":** multi-pane TUI, plan/fork/merge,
    kill cascade, `history grep`, `show --why-failed`, event-streamed
    TUI, cross-process cancellation, error-footer convention,
    `--quiet`/`--plain` flags.
  - **Remove from thin list:** #1 TUI streaming, #3 cancellation
    model, #9 multi-run coordination, #10 promotion / library
    workflow.
  - **Leave in thin list, explicitly out of scope for this milestone:**
    #2 partial-trace resume, #4 wall-clock spend richness, #5 sandbox
    profiles depth, #6 doctor exhaustiveness, #7 import normalization
    round-trip, #8 acceptance YAML spec.
- Append to `/Users/gdc/deadreckon/docs/CHANGELOG.md`:

  ```
  ## Orchestration milestone (alpha) — <YYYY-MM-DD>

  - plan/fork/merge/kill/attach<plan-id> verbs landed (P3–P7)
  - history grep + show --why-failed landed (P8–P9)
  - TUI event streaming via RunEventBus (P1)
  - Cross-process cancellation cascade (P7)
  - Error-footer convention; --quiet/--plain flags (P10)
  - AS-BUILT-ARCHITECTURE.md gains §18; §22 thin-list updated (P11)
  - Still thin (deferred): #2 #4 #5 #6 #7 #8
  ```

## Verb signatures

```
deadreckon plan <goal>
    [--n <2..=6>]              # default 3
    [--provider <id>]
    [--out <path>]             # default ~/.deadreckon/plans/<plan-id>/plan.json
    [--no-hints]
    [--quiet]
```

```
deadreckon fork <plan-id>
    [--max-spend <USD>]        # per-child
    [--max-wall-seconds <N>]   # per-child (CLI providers)
    [--sandbox <auto|sandbox-exec|bwrap|docker|none>]
    [--provider <id>]          # overrides plan default for all children
    [--no-hints]
    [--quiet]
    [--plain]
```

```
deadreckon attach <plan-id-or-run-id>
    [--plain]
    [--no-hints]
```

```
deadreckon merge <plan-id>
    [--strategy fail-on-conflict|prefer-child <idx>]   # default fail-on-conflict
    [--no-gate]                # debug only; emit a stderr warning
    [--no-hints]
    [--quiet]
    [--plain]
```

```
deadreckon kill <plan-id-or-run-id>
    [--force]                  # skip SIGTERM grace
```

```
deadreckon history grep <pattern>
    [--plan <plan-id>]
    [--scope <scope>]
    [--since <duration>]       # 7d / 24h / 30m
    [--kind trace|provenance]  # default trace
    [--limit <N>]              # default 100
    [--regex]
```

```
deadreckon show <id>
    [--why-failed]
    [--diff]                   # nice-to-have; rider does not require depth test
    [--turn <N>]               # nice-to-have
```

Internal-only flag added to `deadreckon run` for use by `fork`:

```
deadreckon run <goal>
    --parent-plan-id <plan-uuid>      # hidden from --help; sets parent.json kind
    --child-index <N>                 # hidden from --help; same
    [...existing flags...]
```

## Refusal-case table (consolidates the error-footer cases)

| Verb | Condition | Error | Try |
|---|---|---|---|
| `plan` | empty goal | `--goal must be non-empty` | `deadreckon plan "your goal"` |
| `plan` | N out of range | `--n must be 2..=6` | `deadreckon plan ... --n 3` |
| `plan` | < 2 valid sub-goals | `provider returned <K> sub-goals; need >=2` | `deadreckon plan ... --provider <other>` |
| `fork` | plan not found | `no plan <id>` | `deadreckon plan list` (or `ls ~/.deadreckon/plans/`) |
| `fork` | plan status != pending | `plan <id> is <status>` | status-specific (`merge`, `attach`, `kill`) |
| `fork` | child lock held | `lock held by pid <P>` | `kill <P> or wait` |
| `attach` | id not found | `no plan or run <id>` | `deadreckon list && ls ~/.deadreckon/plans/` |
| `merge` | plan status != forked | `plan <id> is <status>` | status-specific |
| `merge` | child running | `child <idx> still running` | `deadreckon kill or wait` |
| `merge` | child failed | `child <idx> is <status>` | `deadreckon show <child-run-id> --why-failed` |
| `merge` | conflict | `conflict at <path> between child <a> and child <b>` | `--strategy prefer-child <idx>` |
| `merge` | gate failure | `merge promotion blocked by gate` | `deadreckon show <plan-id> --why-failed` |
| `kill` | id not found | `no plan or run <id>` | (same as attach) |
| `history grep` | invalid regex | `invalid regex: <error>` | `re-quote or escape` |

## Coordinator process model

- `deadreckon fork` is a foreground process that becomes the
  coordinator. It writes `coordinator.json`, spawns N child
  subprocesses, and waits.
- Each child is launched with `Command::new(<self>)` arguments
  reconstructed from the parent invocation, with the parent flag
  set. The child writes its `state.json` and runs the normal turn
  loop.
- Supervision: every 500 ms the coordinator polls each child's
  `state.json` status field and `kill(pid, 0)` liveness; updates
  `coordinator.json.children[i].status`.
- Plan TUI subscribes to a coordinator broadcast channel that
  multiplexes child events (the TUI process is separate from the
  coordinator — it tails the children's `events.jsonl` files when not
  the coordinator-host).
- On clean exit: coordinator removes `coordinator.json`.
- On Ctrl-C: coordinator broadcasts SIGTERM to children, waits 2 s,
  SIGKILL stragglers, exits 130.

## TUI multi-pane layout

```
┌── Plan: <plan-id-prefix> "<root-goal-trimmed>" (<status>) ──────────────┐
│                                                                          │
│ ┌── Child 0: "<sub-goal-trimmed>" ─┐ ┌── Child 1: "<sub-goal>" ────────┐ │
│ │ status: running                  │ │ status: completed               │ │
│ │ turn: 4   spend: $0.12           │ │ turn: 6   spend: $0.18          │ │
│ │ ▓▓▓░░░░░ 35% of $0.50            │ │ ▓▓▓▓▓▓▓▓ 100%                   │ │
│ │ > turn 4 tool=Bash exit=0        │ │ > acceptance: pass              │ │
│ └──────────────────────────────────┘ └─────────────────────────────────┘ │
│                                                                          │
│ ┌── Child 2: "<sub-goal>" ────────────────────────────────────────────┐  │
│ │ status: pending                                                      │  │
│ └──────────────────────────────────────────────────────────────────────┘  │
│                                                                          │
└─ ↑↓: focus │ Enter: drill │ Esc: back │ Ctrl-D: detach │ q: quit ────────┘
```

## Dependencies (per Tier 1/2/3 policy)

Tier 1 (utility):
- `regex` — `history grep --regex`. Check workspace `Cargo.lock`
  first; if already a transitive dep, surface it; else add direct.
- `tokio-stream` — broadcast helpers (allowed already by robust rider).

Tier 2 (architectural, log to `DEPENDENCIES.md`):
- None expected. If you reach for a process supervisor crate, stop and
  justify before adding.

Tier 3 (blocked):
- Same as prior riders.

## Out of scope (explicitly not in this milestone)

Items in §22 of AS-BUILT that this milestone does **not** close:
- #2 partial-trace resume
- #4 wall-clock spend richness for CLI providers
- #5 sandbox profiles depth (per-tool policy)
- #6 doctor exhaustiveness (provider ping, etc.)
- #7 import normalization round-trip parity
- #8 acceptance YAML spec

Also out of scope:
- A `WriteToTeam` action / shared team-context file for live
  cross-child communication. Children get the root goal and their
  sub-goal at fork time and run independently. Cross-child memory is
  a V1-CANDIDATE.
- Auto-decompose inside a running turn loop (`Fork` as a tool action).
  V1-CANDIDATE.
- N > 6 children. The current TUI layout caps useful display at 6;
  larger N stays a future concern.
- Heterogeneous providers per child (one provider per plan for now;
  same `--provider` applies to all children).
- Distributed coordination across machines. All children + coordinator
  live on one host.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.** If you find a reason, stop
  and write the case in `V1-CANDIDATES.md`.
- **One depth test before each phase implementation.** A phase whose
  tests all started green never failed; that's a smell.
- **Child runs are deadreckon runs.** Do not introduce a "child mode"
  in the turn loop. The only difference is the `parent.json` content
  and the scope naming.
- **Locks remain task-scoped.** Each child takes its own task lock.
  The plan does not take a lock.
- **The coordinator is not a state machine.** If you find yourself
  encoding states for `coordinator.json` beyond mirroring child
  statuses, you're overbuilding.
- **No silent expansion.** Anything beyond the eleven phases above
  goes into `V1-CANDIDATES.md`.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with the relevant depth tests passing and a
  CHANGELOG entry naming the SHA.
- After P11, run the smoke flow from the goal end-to-end and capture
  the asciinema cast at
  `/Users/gdc/deadreckon/demo-orchestrate.cast`. (Reuse the existing
  `demo.cast` machinery — see `Makefile`.)
- If a phase reveals a V1-architecture decision, stop and log it in
  `V1-CANDIDATES.md`; do not silently expand scope.
