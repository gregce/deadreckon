# deadreckon — Orchestration Rider (multi-agent on a goal-driven harness)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-11-1444-deadreckon-orchestrate-goal.md`.
It supersedes nothing in prior riders
(`2026-05-10-1400-deadreckon-build-rider.md`,
`2026-05-11-1400-deadreckon-primary-flow-rider.md`,
`2026-05-11-1400-deadreckon-robust-rider.md`,
`2026-05-11-1400-deadreckon-usability-rider.md`) — their invariants, dependency
policy, sandbox defaults, CLI surface, lifecycle hints, and existing
verbs still apply. This rider adds **plans**, **task graph children**, a
**coordinator**, self-contained worker specs, coordinator-only typed
messages, compact child summaries, explicit provider assignment, a
coder/reviewer orchestration lane, multi-pane TUI, and the ergonomic
conventions the multi-agent view requires.

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
  "mode": "split|review",
  "n": 3,
  "providers": {
    "planner": "cli:codex",
    "default_child": "cli:claude-code",
    "coder": null,
    "reviewer": null,
    "children": { "0": "cli:claude-code", "1": "cli:codex" }
  },
  "capability_preview": {
    "network": "deny|allowlist|full",
    "deploy": false,
    "global_install": false,
    "filesystem": ["<relative or absolute allowed path>"],
    "notes": ["<human-readable reason>"]
  },
  "tasks": [
    {
      "index": 0,
      "task_id": "task-0",
      "subject": "<imperative 5-10 word task label>",
      "goal": "<self-contained task goal text>",
      "active_form": "<present-progress text for the TUI>",
      "provider": "cli:claude-code",
      "role": "child|coder|reviewer",
      "depends_on": [],
      "worker_spec": "worker-specs/task-0.md",
      "summary_path": null,
      "review_status": null,
      "child_run_id": null,
      "child_scope": null,
      "status": "pending"
    }
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

`mode = "split"` uses a provider-decomposed task graph. `mode = "review"` uses
two logical tasks without provider planning: a coder run followed by a reviewer
extend/review run. `tasks[i].status` transitions:
`pending → running → completed | failed | killed`.
`plan.status` transitions: `pending → forked → merged | failed`.

Provider entries are resolved at plan creation and copied into the plan file so
that a later `fork` is reproducible. CLI flags may override them at fork time,
but the override is recorded back into `plan.json` before any child starts.

The `tasks` array intentionally follows Claude Code's task-list shape more than
a bare list of task slices: each task has an owner/provider, a short subject, an
`active_form` string for progress displays, dependencies, and a pointer to the
durable worker spec. Dependencies are allowed but must form a DAG.

### Child `.deadreckon/parent.json` (extends usability-rider schema)

A child's working dir contains:

```json
{
  "schema_version": 1,
  "kind": "plan_child",
  "parent_plan_id": "<plan-uuid>",
  "parent_scope": "<parent scope>",
  "parent_goal": "<root goal>",
  "task_id": "task-0",
  "child_index": 0,
  "task_goal": "<this child's task goal>",
  "worker_spec": "/Users/gdc/.deadreckon/plans/<plan-id>/worker-specs/task-0.md",
  "provider": "cli:claude-code",
  "role": "child|coder|reviewer",
  "created_at": "<RFC3339>",
  "deadreckon_version": "<crate version>"
}
```

The `kind` field distinguishes from `materialized` and `extended` per
`usability-rider`. `deadreckon show <run-id>` reads this and reports
`Child <index> of plan <plan-id>` in the header, including provider and role.

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
      "scope": "<scope>", "provider": "cli:claude-code",
      "role": "child|coder|reviewer", "status": "running" }
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
worker-specs/
  task-0.md             (self-contained prompt/spec for one child)
summaries/
  task-0.md             (compact child result summary for merge/review)
messages.jsonl          (typed coordinator mailbox; append-only)
merge-working/          (created by `merge`; promoted to library/ on gate pass)
merge-proofs/           (gate output for the merge run)
```

Children themselves live under the normal
`~/.deadreckon/runstate/<sub-scope>/runs/<child-run-id>/`. The plan
directory contains pointers, never copies of child state.

### `messages.jsonl` (coordinator-only mailbox)

Path: `/Users/gdc/.deadreckon/plans/<plan-id>/messages.jsonl`.

```json
{
  "schema_version": 1,
  "ts": "<RFC3339>",
  "request_id": "<uuid-or-null>",
  "from": "coordinator|task-0|task-1",
  "to": "coordinator|task-0|task-1",
  "type": "progress|blocker|review_request|review_response|capability_request|shutdown_request|shutdown_response",
  "summary": "<5-12 word display summary>",
  "body": {}
}
```

This borrows Claude Code's typed message idea without adopting arbitrary live
child-to-child chat. Children never broadcast to each other directly. The
coordinator writes prompts/follow-ups and reads child reports; the TUI can show
these messages as a plan activity stream. `request_id` is required for
`review_request`, `review_response`, `shutdown_request`, and
`shutdown_response`.

### Worker specs

Path: `/Users/gdc/.deadreckon/plans/<plan-id>/worker-specs/<task-id>.md`.

Each child receives this file path and an inline copy of the spec in its prompt.
The spec must be self-contained:

- root goal and this task's exact scope;
- provider and role;
- files/directories owned by the task, if known;
- dependencies already satisfied;
- acceptance checks relevant to the task;
- capability constraints (network, deploy, filesystem, install);
- done criteria;
- required report shape: scope, result, key files, files changed, issues.

Worker specs must include the child hygiene rules: do not spawn subagents, stay
within scope, do not editorialize between tool calls, verify before reporting,
and record changed files/commit hash when the provider supports commits.

### Child summaries

Path: `/Users/gdc/.deadreckon/plans/<plan-id>/summaries/<task-id>.md`.

At child completion the coordinator writes a compact summary from
`state.json`, traces, provenance, changed files, acceptance output, and the
child's final provider message. Merge/review phases consume summaries rather
than full transcripts unless `--verbose` is requested.

## Provider assignment model

Provider choice is a first-class part of orchestration. The user must be able to
see and override each role before work starts.

Resolution order:

1. Explicit role flags on `orchestrate`, `plan`, or `fork`.
2. Per-child flags in the form `--child-provider <idx=id>`.
3. `plan.json.providers.children[idx]`.
4. `plan.json.providers.default_child`.
5. Existing deadreckon provider config default.

Roles:

- `planner` - provider used only to decompose a split-mode goal.
- `default_child` - provider used for split children without an explicit
  override.
- `child:<idx>` - provider for one split child.
- `coder` - provider used for the implementation run in review mode.
- `reviewer` - provider used for the review/fix run in review mode.

The preview must print the resolved provider table:

```text
providers
  planner:       cli:codex
  default child: cli:claude-code
  child 0:       cli:claude-code
  child 1:       cli:codex
```

Review mode preview:

```text
providers
  coder:    cli:claude-code
  reviewer: cli:codex
```

Review mode is intentionally conservative: the coder run produces the initial
artifact, then the reviewer provider is launched through the existing `extend`
path with a prompt that asks it to inspect, write `.deadreckon/REVIEW.md`, and
apply only review-fix changes needed to satisfy acceptance. The final reviewed
run is the promoted output. If the reviewer only writes findings and no code
changes, the coordinator still gates the coder artifact and records the review.

## Planning and worker prompt contracts

These contracts are mined from Claude Code's coordinator, plan-agent, fork, and
review prompt surfaces, adapted to deadreckon's file-backed model.

### Read-only planner

The split-mode planner is a read-only agent. Its prompt forbids file writes,
temporary files, installs, commits, and destructive shell commands. It may only
inspect the repository and return JSON. The planner output is a task DAG, not a
free-form essay. Required planner fields per task: `subject`, `goal`,
`active_form`, `depends_on`, `role`, optional `owned_paths`, optional
`acceptance_notes`, and optional capability requests. The binary validates the
DAG and writes the canonical `plan.json`; the planner never writes it.

### Coordinator synthesis

The coordinator never tells a child "based on the previous worker's findings".
Before a child starts or is continued, the coordinator writes a synthesized
worker spec with concrete context: file paths, error snippets, acceptance
result, scope boundaries, and done criteria. This is what keeps provider CLIs
with different memory models deterministic.

### Continue versus spawn

- Continue the same child when correcting its own failed acceptance check or
  extending the exact files it just edited.
- Spawn a fresh reviewer for verification so it starts without the coder's
  implementation assumptions.
- Spawn fresh for unrelated retries after a wrong approach; stale context is a
  liability.

### Reviewer prompt

Review mode uses a skeptical reviewer prompt derived from Claude Code's review
surface: correctness, regressions, tests, security/permissions, acceptance
mismatch, and user-goal fit. The reviewer must write
`.deadreckon/REVIEW.md` with findings first, then may apply only fixes that are
directly tied to those findings and acceptance. No multi-round debate loop in
this milestone.

## Phases

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
- Model a task DAG, not a flat task-slice list: task ids, subjects,
  `active_form`, providers, roles, `depends_on`, worker-spec path, summary
  path, review status, child run pointer, and status.
- Add `PlanMessage` and append-only `messages.jsonl` helpers.
- Add deterministic worker-spec and child-summary path helpers.
- Extend `DeadreckonPaths` to expose `plan_dir(plan_id)`,
  `plan_json(plan_id)`, `coordinator_json(plan_id)`,
  `plan_messages(plan_id)`, `worker_spec(plan_id, task_id)`,
  `child_summary(plan_id, task_id)`, `merge_working(plan_id)`.
- Extend the child-side `parent.json` writer to support `kind:
  "plan_child"` per the schema above, including provider and role.

Depth tests:
- `plan_json_serializes_roundtrip` — write/read a fully-populated
  `Plan`; equality on round-trip.
- `plan_task_dag_rejects_cycles` — fixture with `task-0 -> task-1 -> task-0`
  is refused with a `try:` line.
- `child_parent_json_plan_kind` — write/read with `kind: "plan_child"`;
  required fields present.
- `plan_json_preserves_provider_role_assignments` — round-trip split and review
  plans with planner/default-child/per-child/coder/reviewer providers intact.
- `plan_messages_jsonl_roundtrips_typed_requests` — append/read progress,
  review request/response, and shutdown request/response with `request_id`
  validation.
- `worker_spec_paths_are_plan_local` — path helpers never escape
  `~/.deadreckon/plans/<plan-id>/`.

### P3 — `plan` verb (provider-driven decomposition + role preview)

- New verb. Provider prompt template (in
  `crates/deadreckon-core/src/plan.rs`) is read-only and asks for JSON tasks,
  not prose: each task has `subject`, `goal`, `active_form`, dependencies,
  role, optional owned paths, optional acceptance notes, and optional capability
  requests.
- `--planner-provider` selects the provider used to decompose the goal.
- `--provider` is an alias for `--default-child-provider` in split mode.
- `--child-provider <idx=id>` records per-child provider overrides after the
  provider returns tasks and before writing `plan.json`.
- `--mode review` skips provider decomposition and writes a two-role plan:
  coder then reviewer, with reviewer depending on coder.
- Validate provider output: ≥ 2 tasks, each non-empty after trim, no duplicate
  subjects after lowercase/whitespace-normalize, dependencies reference known
  tasks, dependency graph is acyclic.
- Write `worker-specs/<task-id>.md` for every task before `plan.json`.
- Write `capability_preview` and render it in the preview; do not grant new
  capabilities here.
- Write `plan.json` with status `pending`.
- Print post-action hints (see "Hints" below).

Depth tests:
- `plan_writes_plan_json_with_n_tasks` — mock provider returns 3
  tasks; assert file present, schema valid.
- `plan_writes_worker_specs_for_each_task` — every task has a durable spec
  containing root goal, task scope, provider, role, done criteria, and
  capability constraints.
- `planner_prompt_is_read_only` — captured mock provider prompt contains the
  no-write/no-install/no-commit constraints.
- `plan_refuses_one_task_response` — mock returns 1 task;
  assert exit non-zero, error text + `try:` line, no plan.json
  written.
- `plan_n_flag_clamped_to_2_through_6` — `--n 1` and `--n 7` rejected.
- `plan_records_explicit_planner_and_child_providers` — mock plan with
  `--planner-provider cli:codex --provider cli:claude-code --child-provider 1=cli:codex`
  records the provider table and child override.
- `plan_review_mode_writes_coder_reviewer_plan_without_decomposition` —
  `--mode review` does not call the planner provider and records coder/reviewer
  roles.
- `plan_preview_prints_capabilities_and_provider_table` — preview includes
  providers, tasks, dependencies, working dirs, and requested capabilities.

### P4 — `fork` verb (spawn split children + review lane)

- New verb. Loads `plan.json`, refuses if status != `pending`.
- Runs tasks whose dependencies are satisfied. Independent split tasks may run
  concurrently; dependent tasks wait for predecessor completion and summaries.
- For each ready task, spawns a child `deadreckon run` subprocess with:
  - `--goal "<task goal>"`
  - `--scope <parent-scope>-c<index>`
  - the inherited resource flags (`--max-spend`, `--max-wall-seconds`,
    `--sandbox`, resolved `--provider`)
  - `--parent-plan-id <plan-uuid>` (new internal flag; rider §"Verb
    signatures" lists)
- The child prompt includes the inline worker spec and the absolute
  `worker-spec.md` path. The coordinator appends `progress` messages when
  tasks start/finish and `blocker` messages when a task cannot start.
- In split mode, children with no explicit provider use
  `providers.default_child`; children with `tasks[i].provider` use that
  provider.
- In review mode, child 0 runs with `providers.coder`; child 1 is launched only
  after child 0 completes and passes acceptance. Child 1 is an `extend` of the
  coder artifact using `providers.reviewer`; its prompt is review-focused and
  includes the coder run id, artifact path, acceptance summary, and request to
  write `.deadreckon/REVIEW.md`.
- On child completion, write `summaries/<task-id>.md` from state, traces,
  provenance, changed files, acceptance output, and final provider message.
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
- `fork_launches_each_child_with_resolved_provider` — fixture plan with child
  provider overrides; fake binaries assert each child used the expected route.
- `fork_respects_task_dependencies` — task 1 depends on task 0; assert task 1
  does not start until task 0 completed and summary exists.
- `fork_passes_worker_spec_to_child_prompt` — fake provider captures prompt and
  sees the exact worker spec, not only a bare task sentence.
- `fork_writes_progress_messages_jsonl` — start/completion/blocker messages are
  appended with typed schema and display summaries.
- `fork_refuses_when_plan_already_forked` — re-fork yields the
  expected error + `try:` line.
- `fork_writes_coordinator_json_with_child_pids` — assert each PID
  alive at write time.
- `review_mode_runs_coder_then_reviewer_extend` — fake coder completes,
  reviewer starts only after coder acceptance, reviewer parent points at coder
  run, and final plan status references the reviewed run.
- `review_mode_stops_before_reviewer_when_coder_fails_gate` — coder failure
  pauses the plan with a `try: deadreckon show <coder-run-id> --why-failed`.

### P5 — Multi-pane attach TUI

- `deadreckon attach <plan-id>` opens a plan TUI: grid of N panes
  (auto-layout up to 6 children; ≥4 children → 2-row grid).
- Each pane shows: task subject, dependency state, child index, goal
  (truncated), status, current turn, provider, role, spend/context, `active_form`
  progress text, latest trace line, and latest coordinator message summary.
- A side or footer strip shows plan-level capability preview, review/gate state,
  and counts: ready/running/completed/failed/blocked.
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
- `attach_plan_shows_provider_and_role_per_pane` — split and review fixtures
  render child/coder/reviewer labels and provider ids.
- `attach_plan_shows_task_dependency_and_message_summary` — fixture with one
  blocked task renders dependency state and the last `messages.jsonl` summary.
- `attach_plan_shows_capability_preview` — fixture plan with network/deploy
  requests renders the capability strip.
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
  `manifest.json` with the plan_id + child run_ids + provider roles, mark
  `plan.status = merged`, write `plan.merged_run_id`.
- Merge manifest also records task ids, dependency edges, worker spec paths,
  child summary paths, coordinator message counts, and capability preview so the
  merged artifact can be audited without replaying provider transcripts.
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
- `merge_manifest_records_child_provider_roles` — manifest includes the provider
  used by each child and, for review mode, identifies the reviewed run.
- `merge_manifest_records_task_graph_and_summaries` — manifest includes task
  ids, dependency edges, worker specs, summaries, and message counts.
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
    Include the latest relevant `messages.jsonl` blocker/review summary and the
    child summary path when present.

Depth tests:
- `show_why_failed_completed_says_no_failures` — completed fixture
  → expected exact string.
- `show_why_failed_failed_emits_rca` — fixture run that errored at
  turn 3; output contains turn 3, exit code, stderr snippet.
- `show_why_failed_plan_names_blocking_child` — 3-child plan where
  child 1 failed → output names child 1 + its child-run-id +
  try-line.
- `show_why_failed_plan_includes_blocker_message` — fixture messages.jsonl with
  a `blocker` entry appears in the plan RCA.

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
  | `provider role missing` | `deadreckon orchestrate "goal" --mode review --coder-provider cli:claude-code --reviewer-provider cli:codex` |
  | `child provider index out of range` | `--child-provider 1=cli:codex` |
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
  plan: <plan-id> with <N> tasks
  providers: planner=<id> default-child=<id>
  capabilities: network=<mode> deploy=<yes/no> install=<yes/no>
  tasks: <ready>/<blocked> ready now
  edit: vim /Users/gdc/.deadreckon/plans/<plan-id>/plan.json
  fork: deadreckon fork <plan-id>
  ```

  After review-mode plan:
  ```
  plan: <plan-id> review mode
  providers: coder=<id> reviewer=<id>
  flow: coder -> fresh reviewer -> final gate
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
  - Lifecycle: `init`, `orchestrate`, `plan`, `fork`, `run`, `attach`, `merge`,
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
- `review_mode_post_action_hints_name_coder_and_reviewer` — hints show both
  providers and the next attach/merge command.
- `plan_hints_name_capabilities_and_ready_tasks` — split-mode hints include
  capability preview and ready/blocked task count.

### P11 — AS-BUILT update + CHANGELOG (doc only; no depth test)

- Insert a new top-level section in
  `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` after current
  §17 (CLI Surface):

  ```
  ## 18. Plans & Multi-Agent Orchestration

  18.1 Mental model (one plan → task DAG → children → one merge)
  18.2 plan.json schema (verbatim quote from plan.rs)
  18.3 Provider roles and assignment precedence
  18.4 Task graph, dependencies, worker specs, and child summaries
  18.5 Coordinator mailbox (typed messages, no arbitrary child chat)
  18.6 Child lineage (parent.json with kind: plan_child)
  18.7 Coordinator process (coordinator.json, supervision lifecycle)
  18.8 Review mode (coder provider → fresh reviewer provider → final gate)
  18.9 Sub-scope naming
  18.10 Multi-pane TUI (layout sketch)
  18.11 Merge strategy & conflict resolution
  18.12 Cancellation cascade
  18.13 Plan-aware history grep
  18.14 What's not yet built (e.g., team-context WriteToTeam action,
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
  - explicit planner/default-child/per-child/coder/reviewer provider roles
  - task DAG, worker specs, typed coordinator messages, and child summaries
  - review mode runs one provider as coder and a second as reviewer/fixer
  - history grep + show --why-failed landed (P8–P9)
  - TUI event streaming via RunEventBus (P1)
  - Cross-process cancellation cascade (P7)
  - Error-footer convention; --quiet/--plain flags (P10)
  - AS-BUILT-ARCHITECTURE.md gains §18; §22 thin-list updated (P11)
  - Still thin (deferred): #2 #4 #5 #6 #7 #8
  ```

## Verb signatures

```
deadreckon orchestrate <goal>
    [--mode split|review]                 # default review for normal one-goal review lane
    [--n <2..=6>]                         # split mode only; default 3
    [--planner-provider <id>]             # split-mode decomposition provider
    [--provider <id>]                     # default split child provider
    [--child-provider <idx=id>]...        # split child override
    [--coder-provider <id>]               # review mode
    [--reviewer-provider <id>]            # review mode
    [--max-spend <USD>]
    [--max-wall-seconds <N>]
    [--sandbox <auto|sandbox-exec|bwrap|docker|none>]
    [--plain]
    [--quiet]
    [--no-hints]
```

```
deadreckon plan <goal>
    [--n <2..=6>]              # default 3
    [--mode split|review]      # default split for explicit plan
    [--planner-provider <id>]
    [--provider <id>]          # default child provider in split mode
    [--child-provider <idx=id>]...
    [--coder-provider <id>]    # review mode
    [--reviewer-provider <id>] # review mode
    [--out <path>]             # default ~/.deadreckon/plans/<plan-id>/plan.json
    [--no-hints]
    [--quiet]
```

```
deadreckon fork <plan-id>
    [--max-spend <USD>]        # per-child
    [--max-wall-seconds <N>]   # per-child (CLI providers)
    [--sandbox <auto|sandbox-exec|bwrap|docker|none>]
    [--provider <id>]          # overrides split-mode default for all children
    [--child-provider <idx=id>]...
    [--coder-provider <id>]    # review mode override
    [--reviewer-provider <id>] # review mode override
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
    --plan-task-id <task-id>          # hidden from --help; same
    --child-index <N>                 # hidden from --help; same
    --plan-role <child|coder|reviewer> # hidden from --help; same
    --worker-spec <path>              # hidden from --help; included in prompt
    [...existing flags...]
```

## Refusal-case table (consolidates the error-footer cases)

| Verb | Condition | Error | Try |
|---|---|---|---|
| `plan` | empty goal | `--goal must be non-empty` | `deadreckon plan "your goal"` |
| `plan` | N out of range | `--n must be 2..=6` | `deadreckon plan ... --n 3` |
| `plan` | < 2 valid tasks | `provider returned <K> tasks; need >=2` | `deadreckon plan ... --provider <other>` |
| `plan`/`fork` | provider role missing | `provider role <role> is not configured` | `deadreckon orchestrate "goal" --mode review --coder-provider cli:claude-code --reviewer-provider cli:codex` |
| `plan`/`fork` | child provider index out of range | `child provider index <idx> outside 0..<N>` | `--child-provider 1=cli:codex` |
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

- `deadreckon orchestrate` is the one-command wrapper around `plan` + `fork` +
  `merge` for the selected mode. It uses the same files and coordinator model;
  there is no separate orchestration state.
- `deadreckon fork` is a foreground process that becomes the
  coordinator. It writes `coordinator.json`, appends typed activity to
  `messages.jsonl`, spawns ready child subprocesses, and waits.
- Each child is launched with `Command::new(<self>)` arguments
  reconstructed from the parent invocation, with the parent flag
  set, including task id, worker spec, resolved provider, and role. The child
  writes its `state.json` and runs the normal turn loop.
- Task dependencies are scheduled conservatively: only tasks whose
  `depends_on` tasks completed and have summaries are eligible to start.
- In review mode, the coordinator runs the coder child first, waits for gate
  success, then launches the reviewer through the existing extend path with the
  reviewer provider. The reviewed run becomes the merge/promote candidate.
- Supervision: every 500 ms the coordinator polls each child's
  `state.json` status field and `kill(pid, 0)` liveness; updates
  `coordinator.json.children[i].status`. On child completion it writes the
  compact child summary before scheduling dependents.
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
│ ┌── task-0 code ui (cli:claude) ───┐ ┌── task-1 review (cli:codex) ───┐ │
│ │ running · Coding game board      │ │ blocked: waits for task-0       │ │
│ │ turn: 4   context: 42%           │ │ reviewer · fresh context        │ │
│ │ cap: network=deny deploy=no      │ │ message: waiting on summary     │ │
│ │ > turn 4 tool=Bash exit=0        │ │ > acceptance: pending           │ │
│ └──────────────────────────────────┘ └─────────────────────────────────┘ │
│                                                                          │
│ ┌── task-2 docs (cli:claude) ─────────────────────────────────────────┐  │
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
- A general-purpose `WriteToTeam` action or arbitrary live child-to-child chat.
  This milestone only includes typed coordinator-mediated messages in
  `messages.jsonl` and child summaries. Rich shared team memory is a
  V1-CANDIDATE.
- Auto-decompose inside a running turn loop (`Fork` as a tool action).
  V1-CANDIDATE.
- N > 6 children. The current TUI layout caps useful display at 6;
  larger N stays a future concern.
- Automatic provider selection by cost/benchmark/latency. Providers are
  explicit or config-derived; the coordinator does not guess "best" providers.
- Multi-round reviewer debate loops. Review mode is coder -> reviewer/fixer ->
  final gate. If more loops are needed, use chain/extend after the reviewed run.
- Distributed coordination across machines. All children + coordinator
  live on one host.

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.** If you find a reason, stop
  and write the case in `V1-CANDIDATES.md`.
- **One depth test before each phase implementation.** A phase whose
  tests all started green never failed; that's a smell.
- **Child runs are deadreckon runs.** Do not introduce a "child mode"
  in the turn loop. The only difference is the `parent.json` content
  the worker spec included in the prompt, and the scope naming.
- **Children do not spawn children.** Worker specs explicitly forbid recursive
  orchestration; the coordinator is the only process that creates child runs.
- **Coordinator messages are typed.** Do not add free-form cross-child chat in
  this milestone. Add a new message type only with a depth test and a display
  rule.
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
