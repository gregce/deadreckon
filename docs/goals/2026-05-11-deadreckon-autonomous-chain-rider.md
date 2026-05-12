# deadreckon — Autonomous Chain Rider (serial goal chaining + auto-apply)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-05-11-deadreckon-autonomous-chain-goal.md`.
It supersedes nothing in prior riders
(`2026-05-10-deadreckon-build-rider.md`,
`2026-05-11-deadreckon-primary-flow-rider.md`,
`2026-05-11-deadreckon-robust-rider.md`,
`2026-05-11-deadreckon-usability-rider.md`,
`2026-05-11-deadreckon-orchestrate-rider.md`,
`2026-05-11-deadreckon-codebase-rider.md`,
`2026-05-11-deadreckon-self-documenting-rider.md`,
`2026-05-11-deadreckon-audit-harden-rider.md`,
`2026-05-11-deadreckon-doc-depth-rider.md`,
`2026-05-11-deadreckon-provider-registry-rider.md`) — their
invariants, sandbox defaults, files-not-fields posture, error-footer
convention, existing verbs, codebase-mode resolution, doc polish,
provider registry, and acceptance gate still apply. This rider adds:
a serial `chain` mechanism with provider-decomposable plans, a
foreground conductor process, green-policy auto-apply between steps,
chain-level spend caps, multi-step attach TUI, chain-level hooks at
`~/.deadreckon/hooks/chain/`, and `pause`/`resume`/`undo`/`kill`
semantics that compose with single-run semantics.

**All paths absolute.** Source `/Users/gdc/deadreckon/`, runtime
`/Users/gdc/.deadreckon/`.

## Posture (decided — do not redesign)

- **Maturity stays `alpha`.** Workspace stays `version = "0.1.0"`.
  Chains are feature work that extends the alpha substrate, not a V1.
- **No `PipelineState` schema changes.** Chain state, conductor pid,
  per-step lineage all live in files under
  `~/.deadreckon/chains/<chain-id>/` and `working/.deadreckon/chain-step.json`.
- **The conductor is a CLI verb, not a new binary, not a daemon.**
  `chain run` foregrounds; on Ctrl-C it cascades and exits. The
  precedent is `extend_command` (`main.rs:2458`) — a CLI verb that
  calls `run_turn_loop()` internally; the conductor calls `run`/
  `apply` in sequence.
- **The orchestrate-rider's `plan`/`fork`/`merge` is parallel; this
  rider's `chain` is serial.** They are sibling concepts. The
  orchestrate-rider's verbs (if/when shipped) coexist; this rider does
  not preempt them. If both ship, a chain *may* contain a fork step
  (V1 invention; logged in `docs/V1-CANDIDATES.md`).
- **No `git push`.** Phased local commits only.
- **No V1 invention.** Mid-chain provider-driven *replanning* (Codex
  Goal Mode shape), conductor-side parallel-step execution (DAG with
  edges), and cross-machine chain handoff all go to
  `docs/V1-CANDIDATES.md`. If a phase reveals a major architectural
  decision, log it and continue.
- **Edits stay inside `/Users/gdc/deadreckon/`.** No edits to stoa or
  any other repo.

## What we mined from external harnesses (the deltas worth porting)

| Pattern | Source | What we adopt |
|---|---|---|
| **Plan-as-on-disk-markdown the user can edit between steps** | Claude Code plan mode + Armin Ronacher's critique | `chain.json` is editable while `chain.status = paused`; the conductor rereads it on `chain resume` |
| **Atomic-commit-per-step / `git log` as the chain UI** | Aider's `--auto-commits` + OpenHands PR-shaped output | Each step lands one squash commit on its `dr/...` branch via `apply --squash --no-confirm` |
| **Tiered auto-apply, not a single toggle** | Cline / Continue per-category whitelist | `--apply-mode = auto | preview | manual`; auto only with green policy |
| **Hook lifecycle as policy layer** | Claude Code PreToolUse / PostToolUse / Stop | Chain hooks at `~/.deadreckon/hooks/chain/{pre-step,post-step,on-promote,on-chain-end}` |
| **Budget-as-stop-condition** | Codex Goal Mode (`/goal "..." --budget N`) | Chain-level `--max-spend` is aggregate; per-step caps default to remaining/remaining_steps |
| **Bounded fix-loop with rollback** | Printing Press `fixloop.go:39-96` | Circuit breaker: N consecutive failed steps (default 2) → chain pauses |
| **Anti-self-attestation** | Printing Press `phase5_gate.go:144-195` + deadreckon `gate.rs:88-118` | Auto-apply refuses without a valid `AcceptanceMarker`; conductor validates marker even if the inner run claims `Completed` |
| **Append-only event log + replayable resume** | SWE-agent's append-only history | `chain-events.jsonl` per chain; resume reads it like `traces.jsonl` for runs |

We do **not** port: Codex `set_goal` self-mutation (the agent must not
rewrite its own goal list — only the user, via editing `chain.json`
while paused). Devin's "managed Devins" (cross-VM parallel) — that's
the orchestrate-rider's territory.

## Data model (files-not-fields)

### `chain.json` — the chain's source of truth

Path: `~/.deadreckon/chains/<chain-id>/chain.json`.

```json
{
  "schema_version": 1,
  "chain_id": "<uuid simple form>",
  "root_goal": "<original user input or 'manual: 3 steps'>",
  "steps": [
    {
      "index": 0,
      "goal": "<step goal text>",
      "status": "pending",
      "run_id": null,
      "applied_at": null,
      "applied_sha": null,
      "fail_reason": null,
      "max_spend_usd": null,
      "spend_usd": 0.0
    }
  ],
  "branch_policy": "stack",
  "apply_mode": "auto",
  "apply_strategy": "squash",
  "apply_allowlist": [],
  "on_fail": "stop",
  "circuit_breaker_threshold": 2,
  "circuit_breaker_consecutive_failures": 0,
  "max_spend_usd": null,
  "max_wall_seconds": null,
  "total_spend_usd": 0.0,
  "total_wall_seconds": 0.0,
  "scope": "<canonical scope at chain-create time>",
  "base_branch": "<resolved base, e.g. main>",
  "base_sha": "<resolved sha at chain-create time>",
  "cwd": "<absolute cwd>",
  "provider": null,
  "model": null,
  "sandbox": "auto",
  "status": "pending",
  "paused_reason": null,
  "failure_reason": null,
  "conductor_pid": null,
  "created_at": "<RFC3339>",
  "started_at": null,
  "completed_at": null,
  "deadreckon_version": "<crate version>"
}
```

`steps[i].status` transitions: `pending → running → completed | failed | skipped | applied`.
`status` transitions: `pending → running → (paused|completed|failed)`.
`pending` is a draft (allowed to be edited); `running` is locked.

### `working/.deadreckon/chain-step.json` (per inner run)

Each step's inner run gets a marker analogous to `parent.json`:

```json
{
  "schema_version": 1,
  "kind": "chain_step",
  "chain_id": "<chain-uuid>",
  "step_index": 0,
  "chain_root_goal": "<root>",
  "step_goal": "<this step's goal>",
  "prior_applied_sha": null,
  "created_at": "<RFC3339>",
  "deadreckon_version": "<crate version>"
}
```

`deadreckon show <inner-run-id>` reads this and reports
`Step <index> of chain <chain-id>` in the header.

### `chain-events.jsonl` (append-only)

Path: `~/.deadreckon/chains/<chain-id>/chain-events.jsonl`.
Append after every transition. One JSON object per line. Schema:

```json
{
  "timestamp": "<RFC3339>",
  "chain_id": "<uuid>",
  "event": "chain.created" | "chain.step_started" | "chain.run_completed"
          | "chain.apply_started" | "chain.applied" | "chain.apply_refused"
          | "chain.step_failed" | "chain.paused" | "chain.resumed"
          | "chain.killed" | "chain.completed" | "chain.undo_started"
          | "chain.undone_step" | "chain.hook_invoked",
  "step_index": 0,
  "detail": { "run_id": "...", "reason": "..." }
}
```

### `conductor.json` (present only while a conductor runs)

Path: `~/.deadreckon/chains/<chain-id>/conductor.json`.
Written at conductor start, deleted on clean exit; reclaimed via PID
liveness like the orchestrate-rider's `coordinator.json`.

```json
{
  "schema_version": 1,
  "chain_id": "<uuid>",
  "conductor_pid": 12345,
  "started_at": "<RFC3339>",
  "live_step": 0,
  "live_run_id": "<inner run uuid or null>"
}
```

### Chain-level lock

Path: `~/.deadreckon/locks/chain--<chain-id>.lock`.
Same `LockState` format as task locks (`lock.rs:15-24`); `task_key`
is set to `chain--<chain-id>`. Reuses
`acquire_lock` / `heartbeat` / `release_lock`.

### `DeadreckonPaths` additions (new methods, no field changes)

```rust
impl DeadreckonPaths {
    pub fn chains_dir(&self) -> PathBuf { self.home().join("chains") }
    pub fn chain_dir(&self, chain_id: &str) -> PathBuf
        { self.chains_dir().join(chain_id) }
    pub fn chain_json(&self, chain_id: &str) -> PathBuf
        { self.chain_dir(chain_id).join("chain.json") }
    pub fn chain_events(&self, chain_id: &str) -> PathBuf
        { self.chain_dir(chain_id).join("chain-events.jsonl") }
    pub fn conductor_json(&self, chain_id: &str) -> PathBuf
        { self.chain_dir(chain_id).join("conductor.json") }
}
```

### `RunEventBus` addition

A new `RunEventKind::RunPromoted { library_dir: PathBuf }` is emitted
inside `promote_completed_run` (`promotion.rs:62-68`) after the
atomic rename succeeds. The conductor subscribes to it; the existing
`RunCompleted { status }` event is also kept (the conductor falls back
to it when the broadcast is unavailable across processes).

## Conductor lifecycle

```
chain run <chain-id>
 ├─ load chain.json; refuse if status ∈ {running, completed}
 ├─ acquire chain lock; refuse if held by another live pid
 ├─ write conductor.json (pid, started_at)
 ├─ heartbeat every 30s on the chain lock
 ├─ for step in steps:
 │   if step.status == completed|applied|skipped: continue
 │   invoke pre-step hook (may skip or modify step.goal)
 │   compute base_ref (per branch_policy)
 │   spawn `deadreckon run "<step.goal>" --worktree --base <base_ref>
 │         --max-spend <step.max_spend> --no-hints --quiet`
 │   record step.run_id = inner.run_id; chain.json save
 │   poll inner state.json + chain-events.jsonl + bus subscription
 │   on RunCompleted{status=completed} AND valid acceptance marker:
 │     invoke post-step hook (may refuse to advance)
 │     if apply_mode == auto:
 │       if dirty target | conflict | not in allowlist:
 │         fall back to preview (paused, print try:)
 │       else: invoke on-promote hook; apply --no-confirm; record applied_sha
 │     elif apply_mode == preview: pause, print diff + try:
 │     elif apply_mode == manual: pause unconditionally
 │   on Failed | Killed:
 │     record fail_reason; circuit breaker +1
 │     per on_fail: stop|skip|continue
 │ ├─ on chain end: invoke on-chain-end hook; chain.status = completed
 │ └─ on any exit: release chain lock; delete conductor.json
```

## Branch policy (rider §"core idea")

- **`stack`** (default for refactor chains): step N+1's `dr/...`
  branch is based on step N's freshly-applied HEAD on the user's
  base_branch (after `apply --squash`). Stacked PRs are natural.
- **`base`** (default for experiment chains): every step's `dr/...`
  branch is based on `chain.base_sha`. Independent diffs; no
  sequencing. The conductor still runs them in declared order so
  hooks can sequence side effects.
- **`merge`**: like `stack`, but `apply --strategy merge --no-ff`
  produces a merge commit between steps. Useful when each step is a
  PR boundary.

Refusal cases:

| Combo | Error | `try:` |
|---|---|---|
| `branch_policy=merge` + any step `--in-place` | `merge requires worktree mode` | `--branch-policy stack` or remove `--in-place` |
| `branch_policy=stack` + any step `apply_mode=skip` | `stack policy needs applied steps` | `--branch-policy base` |
| `--in-place` on any chain step | `chains never run in-place` | `chain --steps ... (drop --in-place)` |

## Apply-mode green policy

Auto-apply lands a step iff **all** of:

1. Inner run reached `RunStatus::Completed` (gate-checked).
2. `validate_acceptance_marker` succeeds (anti-self-attestation —
   `gate.rs:88-118`); marker `run_id` matches the inner run.
3. `git diff --stat <chain.base>..<branch>` is non-empty.
4. `git rebase --onto <prev-applied-sha> <prev-base> <branch>` simulated
   in-memory via `git rebase --no-commit --no-update-refs` (or
   equivalent dry-run) returns no conflicts.
5. Every file in the diff matches `chain.apply_allowlist` (default
   `[]` meaning "anything inside `<cwd>`"; explicit globs supported).
6. The pre-step `Skip` hook did not abstain; the on-promote hook did
   not refuse.

Failing any check: the conductor sets `chain.status = paused`,
`paused_reason = apply_refused_<reason>`, writes a `chain.apply_refused`
event, and prints the paused-footer (rider §"Friendliness").

## Hook contract (`~/.deadreckon/hooks/chain/`)

Four hooks, each an executable script (any shebang). Invoked via
`stdin = JSON event payload`, exit code controls:

| Hook | When | Payload | Exit code semantics |
|---|---|---|---|
| `pre-step` | Before each step's inner run starts | `{chain_id, step_index, step_goal, base_ref}` | 0 = proceed; 1 = skip step (records `skipped`); 2 = pause chain |
| `post-step` | After inner run terminates, before apply decision | `{chain_id, step_index, run_id, status, library_dir?}` | 0 = continue to apply decision; 1 = pause chain; 2 = refuse advance (records `failed`) |
| `on-promote` | After auto-apply preflight passes, before `git merge --squash` | `{chain_id, step_index, run_id, diff_stat, files_changed[]}` | 0 = apply; 1 = pause; 2 = refuse (records `apply_refused_hook`) |
| `on-chain-end` | After last step, before deleting conductor.json | `{chain_id, status, steps_completed, total_spend_usd}` | exit code ignored; output appended to last `chain.completed` event |

Hooks see env vars `DEADRECKON_CHAIN_ID`, `DEADRECKON_HOME`, `DEADRECKON_STEP_INDEX`.
Stdout from a hook is captured to `chain-events.jsonl` as a
`chain.hook_invoked` event with `detail.stdout` truncated at 4 KB.
A hook missing on disk is treated as exit 0 (proceed).

A `chain hooks list` sub-command (P10) prints which hooks are
resolved + their resolution tier (project `./.deadreckon/hooks/chain/`,
user `~/.deadreckon/hooks/chain/`, repo `/Users/gdc/deadreckon/hooks/chain/`).

## Verb signatures

```
deadreckon chain plan <goal>
    [--n <2..=12>]                # default 4
    [--provider <id>]             # decomposition provider (defaults to default)
    [--model <id>]                # decomposition model
    [--out <path>]                # default ~/.deadreckon/chains/<chain-id>/chain.json
    [--no-hints]

deadreckon chain <goal>...        # explicit, positional steps
    [--from-file <path>]          # alternative: newline-separated goals
    [--branch-policy stack|base|merge]
    [--apply-mode auto|preview|manual]
    [--apply-strategy squash|merge|cherry-pick]
    [--apply-allowlist <glob>...]
    [--on-fail stop|skip|continue]
    [--max-spend <USD>]           # chain aggregate
    [--max-wall-seconds <N>]      # chain aggregate
    [--provider <id>] [--model <id>]
    [--sandbox auto|sandbox-exec|bwrap|docker|none]
    [--base <ref>]                # base branch / commit; default current HEAD
    [--preview]                   # render plan + caps + DAG, do not run
    [--yes]                       # skip preview confirm in interactive
    [--no-hints]

deadreckon chain run <chain-id>
    [--detach]                    # background; conductor prints chain id and exits 0
    [--quiet] [--plain]

deadreckon chain attach <chain-id>
    [--quiet] [--plain]

deadreckon chain pause <chain-id>
    [--reason <text>]
deadreckon chain resume <chain-id>
    [--from-step <N>]             # default: first non-completed
    [--max-spend-add <USD>]       # top up chain cap
deadreckon chain kill <chain-id>
    [--force]                     # skip 2 s SIGTERM grace
deadreckon chain undo <chain-id>
    [--through step<N>]           # bounded
    [--no-confirm]

deadreckon chain status [<chain-id>]
deadreckon chain show <chain-id>
    [--why-failed]
deadreckon chain list
    [--all]                       # default = current scope
    [--full]                      # exact IDs/paths
deadreckon chain hooks list
```

Refusal cases (parameterized over P10 depth tests):

| Condition | Error | `try:` |
|---|---|---|
| Chain id unknown | `no chain '<id>'` | `deadreckon chain list` |
| `chain run` when status == running | `chain '<id>' is already running (pid <p>)` | `deadreckon chain attach <id>` or `kill <id>` |
| `chain run` when status == completed | `chain '<id>' is completed` | `deadreckon chain show <id>` |
| `chain run` when status == paused | `chain '<id>' is paused (<reason>)` | `deadreckon chain resume <id>` |
| `chain "..."` with one step | `chain must have ≥ 2 steps` | `deadreckon run "<the only step>"` |
| `chain "..."` with > 12 steps | `chain capped at 12 steps; got N` | `--from-file` and use multiple chains |
| `chain plan` returns one sub-goal | `decomposition produced <N> goals; need ≥ 2` | `--n 3` or run as single `run` |
| `chain` cwd outside git | `chains require a git repo` | `cd into a repo or chain ... --base-policy base --cwd <repo>` |
| `chain undo` with no applied steps | `nothing to undo` | `deadreckon chain abandon <id>` |
| `chain pause` when status != running | `cannot pause '<status>' chain` | (status-specific verb) |
| Conductor lock held by live pid | `chain '<id>' lock held by pid <p>` | `kill <p>` or `chain attach <id>` |
| Conductor lock held by dead pid | (auto-reclaim, info-line only) | — |
| `apply_mode=auto` + dirty target | `step '<n>' refused auto-apply (dirty target)` | `chain pause <id>; git stash; chain resume <id>` |
| `apply_mode=auto` + rebase conflict | `step '<n>' refused auto-apply (conflict at <path>)` | `chain resume <id> --apply-mode preview` |
| `apply_mode=auto` + file outside allowlist | `step '<n>' refused auto-apply (<path> outside allowlist)` | `--apply-allowlist <glob>` |
| Marker validation fails | `step '<n>' acceptance marker invalid` | `deadreckon show <inner-run-id>` |
| Hook exit 2 | `step '<n>' refused by hook <name>` | (hook-defined stderr message) |
| Pre-step circuit breaker open | `circuit breaker open after <N> failures` | `chain resume <id> --reset-breaker` |
| `--max-spend` reached mid-chain | `chain aggregate spend cap hit` | `chain resume <id> --max-spend-add 5` |

## Phases (eleven)

Each phase: (1) write the named depth test(s) **first** and watch
them fail; (2) implement; (3) run `cargo build --release && cargo test
--workspace && cargo clippy --workspace -- -D warnings && cargo fmt
--check` green; (4) conventional-commit local commit; (5) one-line
CHANGELOG entry.

### P1 — `chain.rs` data model + paths + RunPromoted event

- New module `crates/deadreckon-core/src/chain.rs` with the `Chain`,
  `ChainStep`, `ChainEvent` structs + JSON round-trip helpers.
- Extend `DeadreckonPaths` with the four new methods above.
- Add `RunEventKind::RunPromoted { library_dir: PathBuf }` to
  `events.rs:11-47`. Emit from `promote_completed_run`
  (`promotion.rs:62-68`) after `state.promoted_library_dir = Some(...)`
  and before `save_state`.
- Extend `lock.rs` to accept the special task-key prefix `chain--<id>`
  without modification (it already does; document the convention in a
  doc comment on `LockState::task_key`).

Depth tests (in `crates/deadreckon-core/src/chain.rs` inline tests and
`crates/deadreckon/tests/chain.rs`):
- `chain_json_serializes_roundtrip`
- `chain_step_status_transitions_pending_running_completed`
- `chain_paths_match_locks_pattern`
- `run_promoted_event_emitted_after_atomic_swap`
- `run_promoted_event_includes_library_dir`
- `chain_lock_task_key_prefix_chain_double_dash`

### P2 — `chain` creation (explicit and `--from-file`)

- New CLI verb `chain "g1" "g2" "g3"` (positional, repeating). Implemented
  in `crates/deadreckon/src/main.rs:chain_create_command`.
- `--from-file <path>` reads a UTF-8 file, splits on newlines, trims,
  drops blanks and lines starting with `#`.
- Writes `chain.json` with `status = pending`.
- Resolves `base_branch`/`base_sha` from current git HEAD; refuses if
  cwd is not a git repo (when `branch_policy != base+--cwd <repo>`).
- `--preview` prints rider §"Preview format" without writing.
- `--yes` writes without an interactive confirm.
- Post-action hint after write:
  ```
  chain: <chain-id> with <N> steps
  edit:  vim ~/.deadreckon/chains/<chain-id>/chain.json
  run:   deadreckon chain run <chain-id>
  ```

Depth tests (`crates/deadreckon/tests/chain.rs`):
- `chain_explicit_writes_chain_json_with_n_steps`
- `chain_from_file_parses_newline_separated_goals`
- `chain_refuses_one_step_with_try_run_hint`
- `chain_refuses_more_than_12_steps`
- `chain_refuses_non_git_cwd_with_try_hint`
- `chain_preview_lists_per_step_provider_mode_branch_base_caps`
- `chain_create_writes_post_action_hint`

### P3 — `chain plan` (provider-decomposed)

- New sub-verb `chain plan <goal>` that prompts the configured provider
  for an ordered JSON array of `<= --n` sub-goals. Prompt template:
  ```
  You are decomposing a coding goal into an ordered serial chain.
  Output a JSON array of <= N strings, each <= 160 chars, each a
  concrete next step. Each step builds on the previous step's result.
  No prose, no commentary. Goal: "<G>".
  ```
- Validate: ≥ 2 sub-goals; no duplicates after `lowercase+whitespace-collapse`;
  each non-empty after trim; each ≤ 160 chars.
- Writes `chain.json` with `status = pending`.
- Falls back: on provider error, exits non-zero with `try: chain --steps ...`.

Depth tests:
- `chain_plan_writes_chain_json_with_n_steps`
- `chain_plan_refuses_single_step_response`
- `chain_plan_refuses_duplicate_steps`
- `chain_plan_clamps_n_to_2_through_12`
- `chain_plan_falls_back_with_try_hint_on_provider_error`

### P4 — Foreground conductor (`chain run`) — sequential, no auto-apply yet

- New sub-verb `chain run <chain-id>` (`chain_run_command`).
- Acquires chain lock; refuses if held by a live pid.
- Writes `conductor.json` (pid).
- Subscribes to a per-step `RunEventBus`; falls back to polling inner
  `state.json` + `events.jsonl` if the bus is unavailable cross-process.
- For each pending step: spawns `deadreckon run` as a child process
  with `--no-hints --quiet --max-spend <step-cap>` and the computed
  `--base <ref>`. Writes `working/.deadreckon/chain-step.json` into
  the child's working dir (post-init hook, not pre-spawn).
- On inner termination: records `chain.run_completed` or
  `chain.step_failed` events. **No auto-apply yet (P5).**
- On Ctrl-C: cascades — SIGTERM the inner child, wait 2 s, SIGKILL,
  exit 130.
- On clean end: releases lock, deletes `conductor.json`, prints
  post-action hint.

Depth tests:
- `chain_run_advances_through_steps_sequentially_no_apply_in_p4`
- `chain_run_writes_chain_events_jsonl`
- `chain_run_holds_chain_lock_releases_on_exit`
- `chain_run_idempotent_on_replay_skips_completed`
- `chain_run_refuses_when_lock_held_by_live_pid`
- `chain_run_reclaims_lock_from_dead_pid_with_info_line`
- `chain_run_ctrl_c_cascades_terminate_in_under_5s`
- `chain_run_writes_chain_step_json_into_child_working_dir`

### P5 — Auto-apply with green policy

- After each step's inner run reaches `RunStatus::Completed` and the
  acceptance marker validates, the conductor evaluates the green
  policy (rider §"Apply-mode green policy"). On green: invoke `apply
  --strategy <strategy> --no-confirm --autostash` against the inner
  run id.
- On any check failure: pause the chain, set `paused_reason`, print
  the paused-footer (P10), exit non-zero only when run in `--detach`
  mode (foreground waits for user to `resume`).
- Record `chain.apply_started` / `chain.applied` / `chain.apply_refused`
  events with the refusal reason.

Depth tests:
- `apply_mode_auto_lands_step_when_gate_passes_and_clean_rebase`
- `apply_mode_auto_falls_back_to_preview_on_dirty_target`
- `apply_mode_auto_falls_back_to_preview_on_rebase_conflict`
- `apply_mode_auto_refuses_when_file_outside_allowlist`
- `apply_mode_auto_refuses_when_marker_invalid`
- `apply_mode_preview_writes_diff_summary_before_landing`
- `apply_mode_manual_pauses_chain_after_inner_completion`

### P6 — Branch policy (`stack` / `base` / `merge`)

- Implement the three policies (rider §"Branch policy").
- For `stack`: after step N applies, the conductor reads the new
  HEAD sha and passes it as `--base <sha>` to step N+1's `run`.
- For `base`: every step gets `--base <chain.base_sha>`.
- For `merge`: same as `stack` but `--apply-strategy merge`.
- Refuse the combos in rider §"Branch policy".

Depth tests:
- `branch_policy_stack_chains_branches_off_prior_head`
- `branch_policy_base_each_step_off_chain_base`
- `branch_policy_merge_writes_merge_commit_between_steps`
- `branch_policy_refuses_in_place_on_any_step`
- `branch_policy_stack_refuses_apply_mode_skip`

### P7 — Stop policy + circuit breaker

- Implement `--on-fail stop|skip|continue` and the consecutive-failure
  circuit breaker (default threshold 2).
- `stop` pauses the chain at the first red.
- `skip` records the step as `failed`, marks `applied_sha = None`,
  re-bases step N+1 on the prior applied sha (for `stack`) or the
  chain base (for `base`).
- `continue` is like `skip` but does not increment the breaker.
- `chain resume --reset-breaker` clears the consecutive counter.

Depth tests:
- `on_fail_stop_pauses_at_first_red`
- `on_fail_skip_advances_past_failed_step`
- `on_fail_continue_does_not_increment_breaker`
- `circuit_breaker_pauses_after_n_consecutive_failures`
- `circuit_breaker_threshold_configurable_via_flag`
- `chain_resume_reset_breaker_clears_counter`

### P8 — Aggregate spend cap + per-step budget allocation

- `chain --max-spend $X` is a chain-level ceiling. Per-step caps
  default to `(X - chain.total_spend_usd) / pending_steps_count`.
- Each step's inner `deadreckon run` is invoked with the computed
  per-step cap. The conductor updates `chain.total_spend_usd` after
  each inner run terminates by reading `spend.jsonl`.
- On aggregate cap reached: pause with `paused_reason = "cap"`.
- `chain resume --max-spend-add $Y` adds $Y to the ceiling and
  recomputes per-step caps for remaining steps.
- Wall-clock cap is symmetric (`--max-wall-seconds`).

Depth tests:
- `chain_max_spend_is_aggregate_not_per_step`
- `chain_per_step_cap_is_remaining_over_remaining_steps`
- `chain_resume_inherits_remaining_budget_no_reset`
- `chain_pause_on_cap_with_try_hint`
- `chain_resume_max_spend_add_recomputes_per_step`
- `chain_wall_clock_cap_pauses_chain`

### P9 — `chain attach` multi-step TUI

- `deadreckon chain attach <chain-id>` opens a vertical step-timeline
  TUI in ratatui. Layout:
  ```
  ┌─ chain <id-prefix>  status: running  steps: 2/5  spend: $1.20/$10.00
  ├─ step 0  applied   "scaffold cargo workspace"      $0.45
  ├─ step 1  applied   "add hello binary"              $0.55
  ├─ step 2  running   "wire CI"           turn 3      $0.20   ◀ focus
  │     │ tool: cargo test          0.8 s
  │     │ tool: edit .github/workflows/ci.yml
  │     │ ...
  ├─ step 3  pending   "publish to crates.io"
  └─ step 4  pending   "tag release"
  ```
- Subscribes to each step's `RunEventBus` while focused; for completed
  steps reads `events.jsonl` for replay.
- Keys: `Tab`/`Shift+Tab` page focus; `Enter` drills into focused
  step's single-run TUI (existing `attach <run-id>`); `Esc` returns;
  `Ctrl-D` detaches without killing; `q` quits TUI (no kill).
- Off-TTY: prints a plain text snapshot every 2 s (`--plain`).
- Footer when paused includes the four `try:` lines (P10).

Depth tests:
- `chain_attach_renders_step_timeline_with_dots`
- `chain_attach_tab_pages_focus_between_steps`
- `chain_attach_enter_drills_to_single_run_tui_esc_returns`
- `chain_attach_ctrl_d_detaches_does_not_kill_conductor`
- `chain_attach_shows_aggregate_spend_in_header`
- `chain_attach_plain_emits_periodic_snapshot`

### P10 — Friendliness pass (10 contracts + hooks + footers)

- **Hook lifecycle.** Implement `pre-step`/`post-step`/`on-promote`/
  `on-chain-end` resolution and invocation per rider §"Hook contract".
  Add `chain hooks list` sub-verb.
- **Error-footer convention.** Every chain user-facing error routes
  through `deadreckon_core::user_error` (the helper introduced in the
  orchestrate-rider §P10; if not yet landed, this rider lands it
  here). Canonical pairs per rider §"Refusal cases".
- **`--quiet`/`--plain`** added on `chain run`/`attach`/`resume`/`pause`/
  `kill`/`undo`. Periodic plain-text progress on stderr at 2 s:
  `[chain:<id-prefix>] step=<i> status=<s> spend=$X.YZ`.
- **Post-action hints**:
  ```
  After `chain plan` / `chain "..."`:
    chain: <chain-id> with <N> steps
    edit:  vim ~/.deadreckon/chains/<id>/chain.json
    run:   deadreckon chain run <id>

  After `chain run` (clean exit, all steps completed+applied):
    chained: <id> done <N>/<N>
    show:    deadreckon chain show <id>
    list:    deadreckon chain list

  After paused (any reason):
    chain <id> paused at step <n>/<N> (<reason>)
      try: deadreckon chain show <id> --why-failed
      try: deadreckon chain resume <id>
      try: deadreckon chain resume <id> --apply-mode preview
      try: deadreckon chain undo <id>
  ```
- **`status` extension.** `deadreckon status` (existing) and
  `deadreckon list` learn to surface "chain context" when the latest
  run carries `chain-step.json`:
  `step 2/5 of chain <id-prefix>`.
- **`chain pause`/`resume`/`kill`/`undo`/`status`/`show`/`list`** —
  the remaining lifecycle verbs in rider §"Verb signatures" land
  here. `undo` reads `chain-events.jsonl`, walks `applied` steps in
  reverse, runs `git revert --no-edit <sha>` on each `applied_sha`,
  and records `chain.undone_step` events.

Depth tests (`crates/deadreckon/tests/chain_friendliness.rs`):
- `chain_post_action_hints_print_next_verbs`
- `chain_error_messages_end_with_try_footer` (parameterized over the
  refusal-cases table — one assertion per pair)
- `chain_paused_footer_lists_four_try_lines`
- `chain_quiet_emits_no_stdout_on_success`
- `chain_plain_emits_periodic_progress_no_ansi`
- `chain_status_surfaces_chain_context_when_step_json_present`
- `chain_pre_step_hook_can_skip_step`
- `chain_post_step_hook_pause_pauses_chain`
- `chain_on_promote_hook_refuse_blocks_apply`
- `chain_on_chain_end_hook_runs_and_records_stdout`
- `chain_hooks_list_emits_resolution_tiers`
- `chain_undo_through_step_n_bounded_and_reverts_in_reverse`
- `chain_undo_records_undone_step_events`
- `chain_kill_cascade_terminates_inner_run_and_conductor_under_5s`
- `chain_pause_then_resume_preserves_step_progress`
- `chain_pause_refuses_when_status_not_running_with_try`
- `chain_resume_inherits_remaining_budget`

### P11 — AS-BUILT update + CHANGELOG (doc only; no depth test)

- Insert a new top-level section in
  `/Users/gdc/deadreckon/docs/AS-BUILT-ARCHITECTURE.md` after the
  current §27 "Provider Registry" (created by the provider-registry
  rider; if not yet landed, append after the highest existing
  numbered section):

  ```
  ## 28. Chains & Autonomous Goal Chaining

  28.1 Mental model (one chain → N steps → conductor advances)
  28.2 chain.json schema (verbatim quote from chain.rs)
  28.3 Branch policy (stack / base / merge)
  28.4 Apply-mode green policy
  28.5 Conductor lifecycle + chain lock
  28.6 Hook contract (pre-step, post-step, on-promote, on-chain-end)
  28.7 chain-events.jsonl + RunPromoted event
  28.8 Pause / resume / undo / kill semantics
  28.9 Aggregate spend cap + per-step budget allocation
  28.10 What's not yet built (mid-chain provider replanning,
        parallel-step DAG within a chain, cross-machine handoff —
        all V1 candidates).
  ```

- Update §10 Provider Model: no changes (chains reuse the registry
  as-is).
- Update §17 CLI Surface: add the new `chain` family verbs to the
  table.
- Update §22 (Built vs Scaffolding-Thin):
  - **Add to "Built and reliable":** chain plan/run/attach/show/
    status/list/pause/resume/kill/undo verbs; chain-level hooks;
    aggregate spend cap; auto-apply with green policy; RunPromoted
    event.
  - **Partially close in thin list:** #9 multi-run coordination —
    note the sequential half is now built; parallel half remains in
    the orchestrate-rider's scope.
  - **Leave thin (explicitly out of scope):** #1 TUI streaming
    (event-streamed single-run TUI still uses the orchestrate-
    rider's deferred work; chain TUI subscribes through the bus on
    same-process and replays from `events.jsonl` cross-process —
    consistent with current single-run attach), #2 partial-trace
    resume, #3 cross-process cancellation cleanliness, #4 wall-clock
    spend richness, #5 sandbox profiles depth, #6 doctor
    exhaustiveness, #7 import normalization round-trip, #8
    acceptance YAML spec.

- Append to `/Users/gdc/deadreckon/CHANGELOG.md`:

  ```
  ## Autonomous chaining (alpha) — 2026-05-11

  - chain plan/chain "..."/chain run/chain attach/chain status/chain
    show/chain list/chain pause/chain resume/chain kill/chain undo
    landed (P2–P10).
  - Conductor is a foreground CLI verb that advances a chain through
    sequential `deadreckon run` invocations and `apply --no-confirm`
    with a green policy (gate-pass + clean rebase + allowlist match).
  - branch_policy = stack | base | merge; on_fail = stop | skip |
    continue; aggregate --max-spend; per-step cap = remaining /
    remaining_steps.
  - Hooks at ~/.deadreckon/hooks/chain/{pre-step,post-step,on-promote,
    on-chain-end} (Claude Code shape).
  - RunPromoted event emitted from promotion.rs; chain-events.jsonl
    is the chain's append-only audit log.
  - AS-BUILT-ARCHITECTURE.md gains §28; §22 thin-list updated (P11).
  - Still thin (deferred): #1 #2 #3 #4 #5 #6 #7 #8.
  ```

(P11 is doc-only; no depth test.)

## Integration matrix

| Surface | What changes |
|---|---|
| `crates/deadreckon-core/src/chain.rs` | New module |
| `crates/deadreckon-core/src/paths.rs:9-65` | Four new `chains_*` methods |
| `crates/deadreckon-core/src/events.rs:11-47` | `RunPromoted` variant |
| `crates/deadreckon-core/src/promotion.rs:62-68` | Emit `RunPromoted` after atomic swap |
| `crates/deadreckon-core/src/lock.rs:15-24` | Doc comment for `chain--<id>` task-key prefix |
| `crates/deadreckon/src/main.rs` | New `chain` family verbs (handlers + clap groups) |
| `crates/deadreckon/src/main.rs:apply_command (1993)` | Reused as-is via subprocess; no internal API change |
| `crates/deadreckon/src/main.rs:status_command (3875)` | Surface chain context when latest run has `chain-step.json` |
| `crates/deadreckon/src/main.rs:list_command (3137)` | New `chain:<id-prefix>` column for chained runs |
| `~/.deadreckon/chains/<id>/` | New runtime location |
| `~/.deadreckon/locks/chain--<id>.lock` | New lock namespace (existing format) |
| `~/.deadreckon/hooks/chain/` | New runtime location |
| Frontmatter / `Doc-writer:` line (self-doc rider) | Chain step inner runs continue to write their own docs; chain-level docs are V1 |
| TUI `attach <run-id>` | Unchanged; chain attach is a separate verb |

## Error-footer canonical pairs

(Parameterized over `chain_error_messages_end_with_try_footer`; see
rider §"Verb signatures" → "Refusal cases" for the full table —
each row is one assertion.)

## Config additions (`config.toml`)

```toml
[defaults]
chain_branch_policy = "stack"          # default for chain creation
chain_apply_mode    = "auto"           # default apply mode
chain_on_fail       = "stop"
chain_circuit_breaker_threshold = 2

[chain_hooks]
# resolution order is project → user → repo; this section is
# optional; absent = no chain hooks.
search_paths = [
  ".deadreckon/hooks/chain",
  "~/.deadreckon/hooks/chain",
]
```

## Out of scope (explicitly not in this milestone — V1 candidates)

- **Mid-chain provider replanning** (Codex Goal Mode shape). The
  conductor never re-prompts the provider to refine step N+1's goal
  based on step N's narrative. The chain is committed at create-time;
  the user is the only one who can edit `chain.json` between steps,
  and only while paused.
- **Parallel-step DAG within a chain.** A chain step might *contain*
  a `fork` (orchestrate-rider's parallel decomposition) — that
  composition is a V1 question, not this rider's.
- **Cross-machine chain handoff.** Conductor pid + lock are local;
  no remote pickup.
- **Chain-level docs polish** (one `RUN-NARRATIVE.md` over the whole
  chain). Each step's inner run polishes its own; an aggregate is V1.
- **`chain bisect`** for "find the first regressing step" — a future
  utility that re-runs steps from snapshot pairs.
- **`chain export`** to publish a chain as a reusable template.

## Dependencies (Tier 1 / 2 / 3 policy)

Tier 1 (utility, free):
- `uuid` (already in workspace): chain ID generation.
- `serde` / `serde_json` (already in workspace): chain.json + events.

Tier 2 (architectural, log to `DEPENDENCIES.md`): none expected.
The conductor is built from existing primitives (`tokio::process`,
`RunEventBus`, `apply_command`, `lock.rs`).

Tier 3 (blocked): same blocks as prior riders (no new daemons, no
Lima/bollard, no new sandbox runtimes).

## Engineering invariants (do not violate)

- **No `PipelineState` schema changes.** Chain state in files; per-
  step lineage in `chain-step.json`.
- **One depth test before each phase implementation.** A phase whose
  tests were never red is suspect.
- **The conductor never writes the acceptance marker.** Only `dr-gate`
  does; the conductor only *reads* the marker via
  `validate_acceptance_marker` (`gate.rs:88-118`).
- **The user owns the goal list.** The agent (decomposition provider
  in `chain plan`) writes the *initial* goals; only the user edits
  them after, and only while paused. The conductor never mutates
  `steps[*].goal`.
- **Auto-apply is never silent.** Every applied step writes a
  `chain.applied` event with the post-apply sha; every refusal writes
  a `chain.apply_refused` event with the reason.
- **Hooks are policy, not prompt.** Hooks see structured JSON, return
  exit codes; they do not see or modify the agent's prompt.
- **No silent expansion.** Anything beyond P1–P11 goes into
  `docs/V1-CANDIDATES.md`.
- **`apply_mode = auto` has a green-policy contract.** Changing any
  of the six conditions in §"Apply-mode green policy" requires a new
  rider — the policy is depth-tested.

## Process invariants

- Phased local commits only. No `git push`.
- Each phase ends with the relevant depth tests passing and a
  CHANGELOG entry naming the SHA.
- After P11, optionally capture a demo (asciicast or short video) of
  a 3-step chain running end-to-end under
  `/Users/gdc/deadreckon/demo-chain.cast`. Skip if the change is not
  user-visible (it is, so capture is preferred but not required).
- If a phase reveals a V1-architecture decision, stop and log it in
  `docs/V1-CANDIDATES.md`; do not silently expand scope.
