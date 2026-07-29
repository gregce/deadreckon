# deadreckon — Watchkeeper Rider (one durable job, two-key completion)

This rider holds the prescriptive constraints for the goal at
`/Users/gdc/deadreckon/docs/goals/2026-07-28-2321-deadreckon-watchkeeper-goal.md`.
It supersedes the old decision that a local supervisor belongs only in V1.
It otherwise preserves prior riders and composes three proven substrates:

- Shakedown's one-reference and cross-verb journey discipline;
- Graph's `Plan` executor, retry budgets, nested subplans, and orphan
  reconciliation;
- chain's detached conductor, shared lock, heartbeat, and child-run pattern.

The intended structural match is: **chain-style durable conductor supervising
Graph-style dependency-ready work**. The one intentional difference is that
the conductor claims typed queued jobs with fenced lease epochs rather than
walking only a fixed sequential step list.

**All paths absolute.** Source `/Users/gdc/deadreckon`, runtime
`$DEADRECKON_HOME` or `~/.deadreckon`.

## Posture (decided — do not redesign)

- **Maturity stays stable.**
- **One user-visible identity.** A new launch receives one job id. Its root
  run/plan uses that id where practical; internal attempt ids stay evidence,
  not peer objects the ordinary user must manage.
- **Files, not a server database.** Durable control is versioned JSON and
  append-only JSONL under `jobs/<job-id>/`, using the same atomic-write and
  synced-append primitives as existing state.
- **Events are lifecycle truth.** `job.json` is immutable identity/policy.
  `projection.json` and the existing run/plan/chain/campaign state files are
  rebuildable checkpoints and compatibility projections.
- **Rich evidence stays where it is.** Tool calls, provider output, spend,
  traces, snapshots, provenance and docs remain leaf-run ledgers. Do not copy
  their bytes into `job-events.jsonl`.
- **`RunView` remains the leaf read model.** `JobView` composes one or more
  `RunView`s and the job lifecycle. Do not silently change checked RunView JSON.
- **`Plan` remains the graph executor.** Do not create a fifth orchestration
  language. A single run is a one-leaf optimization; ordered, parallel and
  nested work use existing Graph semantics.
- **No blind replay.** After a crash, adopt a live child or inspect durable
  state. Before retrying an interrupted mutating attempt, prove the old process
  group is dead and either continue from its isolated tree or restore the last
  completed snapshot. External/deploy effects stop `blocked`.
- **No live service installation in tests or by this implementation session.**
  Generate and verify service definitions in temporary homes. Installation is
  an explicit operator action.
- **No git push, release, or edits outside `/Users/gdc/deadreckon`.**

## Temporal extraction and intentional divergence

Extract only these patterns from Temporal:

1. one durable workflow/job id and a distinct attempt/run id;
2. append-only lifecycle history;
3. workers may disappear while execution remains open;
4. explicit open vs terminal states;
5. cancellation differs from forceful termination;
6. recovery resumes from the last recorded boundary;
7. start returns identity while execution continues elsewhere.

DeadReckon intentionally diverges by staying local-first, using source
workspaces rather than arbitrary activities, requiring an approved natural
language definition of done, independently checking meaning, and deliberately
promoting code into the operator's repository.

## Object model

```text
Job                         operator identity and lifecycle
├── immutable launch/authority
├── execution graph         Plan semantics
├── attempt(s)              isolated runs and their RunViews
├── deterministic proof     dr-gate
├── semantic judgment       fresh read-only model call
└── completion receipt      signed two-key result and delivery trail
```

Legacy runs, plans, chains and campaigns load through adapters without being
rewritten. Their histories are labelled `legacy_snapshot`; never manufacture
events that did not occur.

## Durable layout

```text
~/.deadreckon/
├── jobs/<job-id>/
│   ├── job.json
│   ├── launch-plan.json
│   ├── authority.json
│   ├── job-events.jsonl
│   ├── projection.json
│   ├── lease.json
│   ├── supervisor.out
│   ├── supervisor.err
│   └── receipt.json
├── gate-keys/<job-or-run-id>.key
├── supervisor/
│   ├── instance.json
│   ├── supervisor.lock
│   └── service-status.json
└── ... existing runstate/plans/chains/library ...
```

`launch-plan.json` remains the accepted Course artifact. The job store copies
it before enqueue and records its SHA-256 digest. Do not move the Course wire
type into core merely to avoid a copy; protocol owns persisted job vocabulary,
while Course remains the launch planner.

## Persisted protocol

Persisted types belong in `deadreckon-protocol/src/job.rs`. I/O and reduction
belong in `deadreckon-core/src/job.rs`.

### `job.json`

```json
{
  "schema_version": 1,
  "job_id": "32 hex chars",
  "scope": "project scope",
  "goal": "operator-approved goal",
  "shape": "single|graph|legacy_chain|legacy_campaign",
  "created_at": "RFC3339",
  "source_cwd": "/absolute/path",
  "launch_plan_sha256": "sha256:...",
  "authority_sha256": "sha256:...",
  "policy": {
    "max_spend_usd": 10.0,
    "max_wall_seconds": 36000,
    "max_attempts": 3,
    "deadline": null,
    "semantic_judge": "required"
  }
}
```

This file is immutable after `created`. Changes are new events or a new job.

### Job status and stop reason

Execution phase and stop reason are separate.

```rust
pub enum JobPhase {
    Queued,
    Running,
    VerifyingChecks,
    VerifyingMeaning,
    Waiting,
    Terminal,
}

pub enum JobOutcome {
    Verified,
    NeedsReview,
    Blocked,
    BudgetExhausted,
    DeadlineReached,
    RetryExhausted,
    Cancelled,
    Failed,
}

pub enum StopReason {
    Verified,
    SemanticUncertain,
    SemanticUnavailable,
    OperatorInputRequired,
    SpendCap,
    WallCap,
    Deadline,
    AttemptLimit,
    CancelRequested,
    FatalProvider,
    FatalGate,
    LostContainment,
    CorruptHistory,
}
```

Do not infer these from arbitrary `failure_reason` strings for new jobs.
Existing state may project a coarse `legacy_unknown` reason.

### Job events

Every event carries:

```json
{
  "schema_version": 1,
  "job_id": "...",
  "sequence": 17,
  "event_id": "uuid",
  "causation_id": "stable idempotency key",
  "timestamp": "RFC3339",
  "lease_epoch": 3,
  "kind": "attempt_started",
  "detail": {}
}
```

Allowed lifecycle kinds:

```text
created
contract_approved
queued
lease_acquired
lease_reclaimed
attempt_started
child_linked
attempt_stopped
retry_scheduled
deterministic_gate_passed
deterministic_gate_failed
semantic_judge_achieved
semantic_judge_revise
semantic_judge_uncertain
needs_review
blocked
budget_exhausted
deadline_reached
cancel_requested
cancelled
failed
verified
result_applied
result_exported
```

Rules:

- sequence starts at 1 and is contiguous;
- `event_id` duplicates are idempotent only when the full bytes agree;
- a duplicate id with different bytes is corruption;
- a sequence gap is visible corruption, never silently skipped;
- only the current lease epoch may append worker control events;
- partial final JSONL rows are ignored as torn writes and reported as a caveat;
- events append before a projection checkpoint is updated.

### `lease.json`

```json
{
  "schema_version": 1,
  "job_id": "...",
  "owner_id": "random supervisor instance uuid",
  "epoch": 3,
  "acquired_at": "RFC3339",
  "heartbeat_at": "RFC3339",
  "expires_at": "RFC3339",
  "boot_id": "platform boot identity or generated service-start identity",
  "pid": 1234,
  "process_group": 1234,
  "child_pid": 1240
}
```

PID is advisory. Ownership is `(owner_id, epoch)` under an OS file lock.
Claim increments the durable epoch. A stale worker may finish its process but
cannot append a verified/terminal event with an expired epoch.

Heartbeat cadence defaults to 2 seconds; expiry defaults to 15 seconds. Tests
use injected clocks and shorter values, never sleeps.

## Pre-turn authority

`authority.json` is written after operator approval and before the first agent
turn. It is the immutable statement the gate and final receipt judge against.

```json
{
  "schema_version": 1,
  "job_id": "...",
  "run_id": "...",
  "approved_at": "RFC3339",
  "accepted_by": "operator|yes-flag-guardrail",
  "goal_sha256": "sha256:...",
  "contract_sha256": "sha256:...",
  "effective_policy_sha256": "sha256:...",
  "launch_plan_sha256": "sha256:...",
  "source_tree_sha256": "sha256:...",
  "source_revision": "git sha or null",
  "sandbox_requested": "auto",
  "semantic_judge_mode": "required"
}
```

The contract must exist before this manifest. Lazy gate-time generation is not
allowed for new strict jobs. Detected defaults are materialized, shown in the
preview, and approved like operator-authored criteria.

`effective_policy_sha256` covers the resolved sandbox/tool capability policy,
not merely the requested backend string. Agent-visible `sandbox.toml` is an
execution input, not authority; any change after approval is either refused or
requires a new operator approval event and authority version.

Use `flight::build_working_file_index(...).tree_hash()` as the established
canonical tree-hash sibling. Do not use mtimes.

## Semantic judgment

### Artifact

`proofs/semantic-judgment.json`:

```json
{
  "schema_version": 1,
  "job_id": "...",
  "run_id": "...",
  "judged_at": "RFC3339",
  "provider": "...",
  "model": "...",
  "decision": "achieved|revise|uncertain",
  "summary": "bounded explanation",
  "goal_coverage": [
    {
      "claim": "goal clause",
      "status": "met|missing|unclear",
      "evidence": ["relative artifact reference"]
    }
  ],
  "missing": [],
  "input_sha256": "sha256:...",
  "spend_usd": 0.0
}
```

### Independence and read-only input

The judge is a fresh provider request with:

- no worker conversation/session id;
- no tool or write capability;
- no agent working-directory access;
- a bounded evidence pack assembled by the trusted supervisor;
- a schema-constrained JSON result where supported;
- its own provider/model/spend record.

The evidence pack contains the approved goal, compiled contract, deterministic
check results, authority digests, bounded source diff, changed-file list,
implementation notes and deterministic report references. It does not trust
the worker's “done” sentence as evidence.

CLI providers need an explicit read-only request posture in the provider
contract. Running a normal coding CLI in the working tree with a polite
read-only prompt is not sufficient.

### Decision rules

- Deterministic failure: append `deterministic_gate_failed`; return to work.
  Never call the semantic judge.
- `achieved`: persist judgment, then seal the combined receipt.
- `revise`: append the judge's bounded, supervisor-authored findings to the
  next turn history and continue within remaining budgets.
- `uncertain`: strict jobs stop `needs_review`; optional legacy jobs may retain
  a clearly labelled deterministic-only result but cannot be `verified`.
- Provider unavailable or malformed response: strict jobs stop
  `needs_review`; never fall back to self-attestation or deterministic-only
  `verified`.

The semantic judge cannot override deterministic failure.

## The final receipt and Binnacle amendments

Execute the existing Binnacle rider, amended by these reconciled requirements:

1. **Key lifetime follows evidence lifetime.** `cleanup` and `abandon` retain
   the run record, so they must retain its key. Remove the key only when the
   receipt/run evidence is actually purged. Missing v2 key means unverifiable,
   never legacy-pass.
2. **Digest, do not use mtime.** Promotion recomputes the canonical result tree
   digest and compares it with the signed receipt. Backdated mtimes cannot make
   a changed tree look unchanged.
3. **Bind authority.** The signed receipt covers goal, contract, effective
   policy, launch plan, source tree/revision and result tree digests.
4. **Issuer/proof kind is explicit.** Test helpers and synthetic plan/campaign
   result markers cannot be reported as native `dr-gate` verification.

The deterministic marker remains useful evidence. The final authority is
`receipt.json`, signed with HMAC-SHA-256 using the protected per-run key.

```json
{
  "schema_version": 1,
  "job_id": "...",
  "run_id": "...",
  "issued_at": "RFC3339",
  "issuer": "deadreckon-supervisor",
  "proof_kind": "two_key_completion",
  "outcome": "verified",
  "stop_reason": "verified",
  "authority_sha256": "sha256:...",
  "goal_sha256": "sha256:...",
  "contract_sha256": "sha256:...",
  "effective_policy_sha256": "sha256:...",
  "launch_plan_sha256": "sha256:...",
  "source_tree_sha256": "sha256:...",
  "source_revision": null,
  "result_tree_sha256": "sha256:...",
  "result_revision": null,
  "deterministic_marker_sha256": "sha256:...",
  "semantic_judgment_sha256": "sha256:...",
  "contained": true,
  "sandbox_backend": "sandbox-exec",
  "signature": "hex HMAC-SHA-256"
}
```

Promotion, plan per-node landing, chain auto-apply, campaign roll-up, `finish`,
`verdict`, `RunView`/`JobView`, show and reports use one receipt validator.
No surface may deserialize a marker and call it passed without validation.

## Supervisor algorithm

```text
loop:
  acquire supervisor singleton lock
  scan jobs with nonterminal JobView
  for each eligible job:
    acquire job lock
    if live unexpired lease: skip
    reconcile old lease/process group/root execution
    claim next epoch and append lease_acquired/reclaimed
    spawn or adopt one worker process group
    append attempt_started/child_linked
    heartbeat lease while monitoring
    on exit, inspect job + root artifacts, never exit code alone
    append a typed stop/retry/terminal event using current epoch
  wait on filesystem wakeup or bounded poll
```

Reconciliation order:

1. fold `job-events.jsonl`; refuse corrupt history;
2. read lease and compare owner/epoch/expiry/boot id;
3. inspect process-group liveness;
4. inspect linked run/plan state and its OS lock;
5. adopt a live child without spawning;
6. harvest a completed child;
7. if interrupted, prove the group is dead and classify whether continuation
   from the isolated tree is safe;
8. retry only when policy and remaining spend/wall/deadline permit;
9. otherwise append the exact terminal stop reason.

The supervisor uses Capstan process groups. Auto-relaunch without whole-tree
termination/adoption is prohibited.

## Machine restart

`deadreckon setup --supervisor` is the explicit installation action.

- macOS: generate a user LaunchAgent with `RunAtLoad`, `KeepAlive` for failure,
  `ProgramArguments = [current_exe, "supervisor", "serve"]`, and
  `DEADRECKON_HOME`.
- Linux: generate a systemd user unit with `Restart=on-failure`,
  `WantedBy=default.target`, and the same explicit arguments/environment.
- Unsupported platforms: record `unsupported`; `start` must not claim machine
  restart durability. Add the platform implementation before changing that
  label.

Tests render definitions into a temporary home and exercise a service-restart
simulation by terminating one supervisor process and launching another. They do
not call `launchctl`, `systemctl`, or Task Scheduler.

At runtime:

- TTY `start` preflights the service and offers the one-time setup.
- noninteractive durable `start` without a restart-capable supervisor refuses
  with `try: deadreckon setup --supervisor`.
- an explicit advanced foreground/legacy escape hatch remains available and is
  labelled process-bound, not durable.

## Ordinary command contract

```text
deadreckon start "<goal>"     create/approve/enqueue; return one job id
deadreckon attach [id]        observe/control JobView; detach never cancels
deadreckon status [id]        one phase, stop reason, progress and next action
deadreckon list               every job kind, newest activity first
deadreckon finish [id]        validate receipt, then apply/export deliberately
```

Advanced run/orchestrate/chain/campaign verbs either register a compatibility
job or are explicitly documented as legacy foreground execution. During the
migration they remain callable and readable.

`ResolvedRef` gains `Job` first. Existing ids adapt to JobView without bulk
migration. One id printed by `list` must work or give a runnable, non-looping
next action in each ordinary command—the Shakedown journey test grows rather
than being replaced.

## Phases (eleven)

Every phase starts with the named depth tests and watches them fail. Implement
only after the causal failure is visible. End with focused tests, format, lint
where practical, a conventional local commit, and a CHANGELOG line naming the
commit. Milestone boundaries run `make verify`.

### P1 — Characterize the journey and add the job protocol

- Pin current start/run/plan/chain/campaign identities and terminal semantics.
- Add `deadreckon-protocol/src/job.rs` with wire types and checked schemas.
- Add paths only; no scheduler yet.

Depth tests:

- `job_event_schema_is_checked`
- `job_event_sequence_starts_at_one`
- `job_stop_reasons_are_distinct`
- `current_start_shapes_have_characterized_root_ids`
- `legacy_terminal_states_project_without_invented_precision`

### P2 — Event store, reducer and `JobView`

- Add `deadreckon-core/src/job.rs`.
- Synced append under a job lock; atomic rebuildable projection.
- Compose child `RunView`s; keep rich evidence external.
- Add read-only legacy adapters without writes.

Depth tests:

- `job_event_sequence_reduces_deterministically`
- `duplicate_event_id_is_idempotent_only_when_bytes_match`
- `event_gap_is_reported_not_hidden`
- `partial_final_event_is_ignored_with_a_caveat`
- `job_projection_rebuild_matches_saved_projection`
- `legacy_run_plan_chain_campaign_keep_their_ids_without_writes`
- `job_view_run_facts_match_run_view`

### P3 — Capstan process groups and fenced leases

- Reconcile and execute the Capstan rider's process-group/capture primitives.
- Add periodic lease heartbeats with injected clock.
- Epoch-fence all worker control events.
- Put plan ownership under the same shared lock; preserve coordinator evidence
  until reconciliation completes.

Depth tests:

- `only_one_supervisor_can_claim_a_job`
- `expired_lease_reclaim_increments_epoch`
- `stale_worker_cannot_commit_with_old_epoch`
- `boot_identity_change_reclaims_even_when_pid_is_reused`
- `heartbeat_renews_without_phase_transition`
- `kill_reaps_the_entire_provider_and_gate_process_group`
- `plan_claim_is_atomic_not_pid_check_then_write`

### P4 — Immutable authority and Binnacle closure

- Materialize detected contracts before approval.
- Write `authority.json` before agent execution.
- Execute Binnacle key store, HMAC, containment, denylist and hostile-agent
  phases with the four amendments above.
- Protect contract, policy, gate, proof, snapshot and provenance authority from
  every worker/provider route.
- Replace all unchecked marker displays with validated facts.

Depth tests:

- `strict_job_has_materialized_contract_before_first_turn`
- `authority_binds_goal_contract_policy_launch_source`
- `agent_cannot_edit_authority_or_acceptance_inputs`
- `v2_receipt_uses_hmac_sha256_and_constant_time_verify`
- `cleanup_that_retains_receipt_retains_verification_key`
- `synthetic_marker_is_not_native_gate_proof`
- `backdated_result_mutation_invalidates_receipt`
- every hostile-agent test required by the Binnacle rider

### P5 — Independent semantic judge and combined receipt

- Add a schema-constrained semantic request/response protocol.
- Add explicit provider read-only posture; no worker session or working-tree
  write access.
- Integrate after deterministic pass and before final sealing/promotion.
- `revise` returns bounded judge-authored findings to the loop.
- strict `uncertain`/unavailable becomes `needs_review`.
- Seal and validate `receipt.json`.

Depth tests:

- `deterministic_failure_never_calls_semantic_judge`
- `semantic_judge_receives_goal_contract_diff_and_gate_evidence`
- `semantic_judge_has_no_worker_session_or_write_capability`
- `achieved_plus_gate_pass_seals_two_key_receipt`
- `revise_returns_bounded_findings_to_next_turn`
- `uncertain_strict_job_stops_needs_review`
- `unavailable_strict_judge_never_falls_back_to_verified`
- `semantic_judgment_mutation_invalidates_receipt`
- `promotion_requires_valid_two_key_receipt`

### P6 — Queue and detached leaf supervisor

- Split run preparation from drive.
- `start` persists job/launch/authority/queued before spawning.
- Supervisor claims and drives single-leaf jobs first.
- Use the job id for the root run id.
- Replace start's before/after filesystem scans.

Depth tests:

- `start_persists_approved_job_before_spawning_worker`
- `start_returns_job_id_before_worker_finishes`
- `closing_start_parent_does_not_stop_job`
- `failed_spawn_leaves_a_visible_typed_job`
- `root_run_uses_the_job_id`
- `attach_disconnect_does_not_cancel_execution`

### P7 — Crash adoption and machine-restart service

- Reconcile/adopt live or completed children.
- Recover expired leases without duplicate attempts.
- Add service definition generation and explicit setup/preflight.
- Restart simulation resumes same job, workspace, budgets and evidence.

Depth tests:

- `dead_supervisor_lease_is_reclaimed`
- `reclaimed_job_does_not_run_one_attempt_twice`
- `crash_before_child_link_is_reconciled_without_duplicate`
- `service_restart_resumes_same_job_workspace_budget_and_attempt`
- `macos_launch_agent_is_restart_capable_and_path_safe`
- `linux_user_unit_is_restart_capable_and_path_safe`
- `unsupported_platform_never_claims_machine_restart_durability`

### P8 — One graph scheduler for all shapes

- Use Plan as job graph; preserve single-leaf optimization.
- Route ordered, parallel and nested launch plans through it.
- Translate chain compatibility while retaining hooks, undo and apply rules.
- Translate campaign only after roll-up/no-laundering parity is proven.
- Parent/child links are job events, not PID-only control truth.

Depth tests:

- `single_ordered_parallel_nested_share_job_lifecycle`
- `ordered_graph_preserves_per_node_landing`
- `parallel_graph_preserves_at_end_apply`
- `chain_crash_mid_step_adopts_linked_child_job`
- `chain_hooks_and_undo_survive_job_adapter`
- `campaign_rollup_refusal_cannot_be_laundered_by_adapter`
- `run_plan_chain_campaign_emit_same_terminal_vocabulary`

### P9 — Five commands over one `JobView`

- Extend `ResolvedRef` and the shared acceptance matrix.
- Route list/status/show/report/attach/finish through JobView.
- Keep renderers where possible; replace their data source first.
- Require final receipt for all automatic landing and promotion paths.

Depth tests:

- `list_contains_every_job_kind_including_campaign`
- `latest_is_the_first_listed_job`
- `every_listed_job_has_a_non_looping_five_command_journey`
- `status_distinguishes_phase_from_stop_reason`
- `finish_refuses_missing_uncertain_or_uncontained_receipt`
- `report_cites_contract_checks_semantic_attempts_spend_and_revisions`
- `missing_optional_narrative_does_not_break_factual_receipt`

### P10 — Failure matrix, dogfood and measurements

- Add deterministic crash/fault injection at lease, spawn, link, gate, judge,
  seal and promotion boundaries.
- Add an operator-only dogfood harness using public commands and receipts.
- Define a 20–30 task matrix across at least two repositories/providers; real
  paid/provider execution remains operator-triggered.
- Emit a factual metrics artifact.

Metrics:

- unattended verified completion rate;
- automatic recovery rate;
- false acceptance/rejection found by human review;
- operator intervention count;
- time to understand final result;
- worker, supervisor and judge spend/time;
- retry and semantic-revision counts;
- final stop-reason distribution;
- confinement level.

Depth tests:

- `fault_matrix_covers_every_durable_boundary`
- `two_supervisors_racing_execute_each_job_once`
- `spend_wall_deadline_retry_cancel_blocked_needs_review_are_distinct`
- `dogfood_harness_uses_public_start_status_finish_and_receipt`
- `dogfood_matrix_has_at_least_twenty_tasks_and_two_provider_slots`
- `metrics_are_derived_from_job_view_not_narrative`

### P11 — AS-BUILT, MAP, claims, CHANGELOG and operator handoff

- Add `## 58. Watchkeeper: one durable job, two-key completion` to
  `docs/AS-BUILT-ARCHITECTURE.md`.
- Update `docs/MAP-OF-DEADRECKON.md` to HEAD and distinguish persisted
  durability from supervised durability.
- Update README/CONCEPTS only to claims enforced by contained vs uncontained
  receipts and installed vs process-only supervisor posture.
- Move superseded daemon/queue items out of V1-CANDIDATES; retain genuinely
  deferred cross-machine/cloud scheduling.
- Add `## Watchkeeper (stable)` to CHANGELOG with phase commit span.
- Produce the operator acceptance checklist from the scenarios below.

## Compatibility and migration rules

- No bulk migration at install time.
- Existing runs/plans/chains/campaigns remain readable.
- Legacy objects may receive an in-memory JobView adapter; no fake event log.
- Additive/defaulted protocol fields only within a schema version; incompatible
  meaning requires a new schema version and explicit loader.
- Direct advanced verbs remain callable for one compatibility window. Their
  result must say whether it is durable-supervised, process-only, two-key,
  deterministic-only, contained or uncontained.
- Campaigns currently lack scope. Legacy campaign scope is `unknown`; never
  guess it from the caller's current directory.
- Chain hooks, redo, undo, application semantics and campaign worst-of roll-up
  are behavior, not surface debt. Preserve before translating storage.

## Error-footer canonical pairs

| Error | `try:` |
|---|---|
| Supervisor service required for noninteractive durable start | `deadreckon setup --supervisor` |
| Job history corrupt or has a sequence gap | `deadreckon show <id> --json` |
| Live lease already owns the job | `deadreckon attach <id>` |
| Strict semantic judge unavailable or uncertain | `deadreckon show <id>` |
| Spend or wall budget exhausted | `deadreckon extend <id> --max-spend-add <amount>` |
| Operator input required | `deadreckon attach <id>` |
| Receipt uncontained | `deadreckon show <id>` |
| Receipt or result digest changed | `deadreckon verdict <id>` |
| Legacy process-bound execution requested | `deadreckon start "<goal>"` |

## Operator acceptance scenarios

The human should eventually be able to run these from a temporary repository:

1. Start one smoke goal and receive one id before execution ends.
2. Close the launching terminal; status shows the same job progressing.
3. Kill the coding CLI; supervisor records a typed stop/retry and preserves
   remaining budget.
4. Kill the supervisor; a replacement adopts the same job without duplicate
   work.
5. Restart the configured user service; unfinished work resumes.
6. Exhaust spend, wall, retry and deadline separately; each reports the right
   reason and next action.
7. Produce semantic `revise`; observe another turn. Produce `uncertain`; observe
   `NEEDS_REVIEW`, not verified.
8. Mutate the contract, policy, result tree and semantic proof after sealing;
   each mutation prevents finish.
9. Run single, ordered, parallel and nested work; use the same five commands on
   every id.
10. Finish a verified job; inspect one receipt citing authority, checks,
    semantic judgment, attempts, spend, confinement, source/result and delivery.
11. Open a legacy run/plan/chain/campaign id; inspection remains honest.
12. Remove optional generated narrative; factual status/report/receipt still
    render with a bounded missing-evidence caveat.

## Dependencies

Expected Tier 2 additions:

- `hmac` (RustCrypto) for Binnacle HMAC-SHA-256.

Prefer existing `sha2`, `uuid`, `chrono`, filesystem locks, process helpers and
Tokio. Do not add a workflow engine, embedded database, daemon framework, async
process supervisor, or cryptographic suite without first proving the existing
primitives cannot express the invariant and recording the decision in
`docs/V1-CANDIDATES.md` and `DEPENDENCIES.md`.

## Engineering invariants

- One depth test before each phase implementation.
- Event append precedes projection update.
- One writer lease epoch; stale epochs cannot commit control truth.
- No PID-only ownership decision.
- No automatic retry until the prior process group is dead or adopted.
- No strict job without a materialized approved contract and authority.
- No semantic call after deterministic failure.
- No semantic `uncertain`/unavailable strict result reported as verified.
- No promotion/auto-apply without the same final receipt validator.
- No mtime used as proof of unchanged source or result.
- No unchecked marker deserialization on user-visible truth surfaces.
- No ordinary command creates a second lifecycle model.
- No generated narrative is acceptance authority.
- No live service install, git push, tag, release, or external dogfood spend by
  the implementation agent.

## Process invariants

- Phased local commits only; never stage the user's `.specstory` or
  `.gitignore` changes.
- Focused tests per edit; `make verify` at milestone boundaries.
- If a real provider/backend is unavailable, report it and keep the hermetic
  proof; never count an unavailable integration as a pass.
- At each buildable phase boundary, stop and hand the operator a plain-language
  acceptance script before claiming the outcome.
