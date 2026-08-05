# Research Brief: DeadReckon, the Operator's Burden, and What Harnesses Actually Provide

Assembled from 8 research packets. Every claim below is traceable to a packet finding; file citations are preserved verbatim. **Nothing in this brief was observed at runtime** — see §6.

---

## 1. What DeadReckon's machinery actually is

Grouped by mechanism rather than by crate. All paths are absolute.

### 1.1 The durable Job: immutable identity, append-only truth

- A `Job` and its `JobPolicy` / `JobExecutionPolicy` / `JobToolPolicy` / `JobShape` / `SemanticJudgeMode` are written once. `write_job` refuses any replacement whose bytes differ — `history_corrupt(... "job.json is immutable and already contains different bytes")`.
  `/Users/gdc/deadreckon/crates/deadreckon-protocol/src/job.rs#L85`, `#L99`, `#L113`; `/Users/gdc/deadreckon/crates/deadreckon-core/src/job.rs#L318-332`
- Control truth is `job-events.jsonl`, an append-only log with 30 `JobEventKind` variants (Created, LeaseAcquired, AttemptStarted/Stopped, DeterministicGatePassed/Failed, SemanticJudgeAchieved/Revise/Uncertain, Verified, ResultApplied, UndoStarted/Completed/Failed…). `projection.json` is only a rebuildable checkpoint.
  `/Users/gdc/deadreckon/crates/deadreckon-protocol/src/job.rs#L357`, `#L334`, `#L373-405`; `/Users/gdc/deadreckon/crates/deadreckon-core/src/job.rs#L349`, `#L403`, `#L448`, `#L45`
- The reducer fails closed rather than guessing: sequence gaps, wrong-job events, duplicate ids with different bytes, non-increasing lease epoch, any event after a terminal outcome, and an outcome/stop-reason pair outside the allowed vocabulary all raise `job history corrupt for <id>`. A torn final JSONL row is ignored with a caveat; appending after a torn row is refused.
  `/Users/gdc/deadreckon/crates/deadreckon-core/src/job.rs#L411-444`, `#L564-578`, `#L583-587`, `#L777-805`, `#L360-398`, `#L466-471`
- Cancellation is sticky: once `CancelRequested` is in the projection, a racing worker's stop reason is ignored and any terminal outcome other than Cancelled/Blocked/LostContainment is corruption.
  `/Users/gdc/deadreckon/crates/deadreckon-core/src/job.rs#L602-611`, `#L792-800`; tests `#L1205`, `#L1227`
- Failure is typed, not prose: 8 `JobOutcome` variants and exactly 18 `StopReason` variants, with `JobOutcome::accepts_stop_reason` as the const cross-product used by both the reducer and every status surface.
  `/Users/gdc/deadreckon/crates/deadreckon-protocol/src/job.rs#L234-243`, `#L249-291`, `#L297-327`

### 1.2 Single ownership: leases and fencing

- `JobLease` (owner_id, epoch, boot_id, pid, process_start_identity, process_group, child_pid). `claim_job_lease` serializes on `control.lock` and writes the LeaseAcquired/LeaseReclaimed **event before** the lease checkpoint — "The event is lifecycle truth. The lease file is only its checkpoint." Reclaim only on `Expired`, `BootIdentityChanged`, or `MissingCheckpoint`; a same-boot live process whose `process_start_identity` still matches cannot be stolen.
  `/Users/gdc/deadreckon/crates/deadreckon-protocol/src/job.rs#L409-425`; `/Users/gdc/deadreckon/crates/deadreckon-core/src/job_lease.rs#L117-220`, `#L215-217`, `#L77-97`, `#L152-168`
- `append_fenced_job_event` / `append_next_fenced_job_event` re-validate the token under the control lock and reject any event whose `lease_epoch` differs. Epoch zero is reserved for trusted pre-lease controller events.
  `/Users/gdc/deadreckon/crates/deadreckon-core/src/job_lease.rs#L243-283`, `#L293-336`; `/Users/gdc/deadreckon/crates/deadreckon-protocol/src/job.rs#L364-366`
- TTL 60s, heartbeat 2s, 10s renewal safety margin, up to 4 Jobs recovered concurrently.
  `/Users/gdc/deadreckon/crates/deadreckon/src/commands/supervisor.rs#L39-53`

### 1.3 The two-key completion receipt

- `seal_completion_receipt` refuses unless (a) the deterministic proof is a validated native `dr-gate` v2 marker (`is_native_gate_proof`: schema ≥ 2, proof_kind NativeGate, issuer "dr-gate"), (b) `marker.contained` is true with backend ≠ "none", and (c) the semantic judgment decision is `Achieved` for the same job + run.
  `/Users/gdc/deadreckon/crates/deadreckon-core/src/completion.rs#L171-269`, esp. `#L195-206`, `#L207-216`; `/Users/gdc/deadreckon/crates/deadreckon-core/src/gate.rs#L124-129`
- `CompletionReceipt` binds nine digests plus attempt identity: authority, goal, contract, effective_policy, launch_plan, source_tree, result_tree, deterministic_marker, semantic_judgment, sandbox_boundary_observation, optional `CompletionExecutionEvidence`, attempt/outer_launch_id, signature. Validation re-checks every digest.
  `/Users/gdc/deadreckon/crates/deadreckon-protocol/src/job.rs#L510-540`, `#L493-497`; `/Users/gdc/deadreckon/crates/deadreckon-core/src/completion.rs#L301-422`
- Signing is length-prefixed and magic-tagged so field boundaries cannot be confused: receipts use `b"deadreckon.completion-receipt.v1\0" || be_u64(len) || json(receipt with empty signature)`; markers use `b"deadreckon.acceptance-marker.v2\0"` with per-field 4-byte BE length prefixes; boundary observations use a third magic.
  `/Users/gdc/deadreckon/crates/deadreckon-core/src/completion.rs#L35`, `#L1690-1736`; `/Users/gdc/deadreckon/crates/deadreckon-core/src/gate.rs#L33`, `#L1693-1799`, `#L1857-1869`; `/Users/gdc/deadreckon/crates/deadreckon-core/src/sandbox_observation.rs#L24`
- When the terminal Verified fact exists but its proof was deleted or tampered with, `JobView::verified_receipt_error` says so — "terminal Verified fact is not bound to the current signed receipt".
  `/Users/gdc/deadreckon/crates/deadreckon-core/src/job.rs#L90-96`, `#L235-288`

### 1.4 Key 1 — the deterministic gate

- Per-run 32-byte key derived from three UUIDv4s hashed with SHA-256, stored **outside the workspace** in a 0700 keyring with 0600 files, created with `create_new` (never replaced), refused if group/other-readable, a symlink, or not a regular file.
  `/Users/gdc/deadreckon/crates/deadreckon-core/src/gate.rs#L252-257`, `#L274-293`, `#L295-325`, `#L327-354`, `#L381-463`
- Evaluator and signer are split so repository-controlled check commands never share a process with the key. `dr-gate evaluate` refuses to run if `DEADRECKON_GATE_KEY` / `_CONTAINED` / `_SANDBOX_BACKEND` are present and emits an unsigned `GateEvaluation`; `dr-gate sign` rejects legacy containment env and rejects an evaluator-identity mismatch *before* the key is read.
  `/Users/gdc/deadreckon/crates/deadreckon/src/bin/dr-gate.rs#L163-204`, `#L491-533`, test `#L659`; `/Users/gdc/deadreckon/crates/deadreckon-core/src/gate.rs#L206-222`, `#L724-746`
- Five check kinds compiled from `acceptance.yaml`: CargoTest | FileExists | ContentMatch | BuildSuccess | Shell, each with `must_pass`, plus an `AcceptanceTamperVerdict` = Clean | Caveat | Refuse recording spec_modified, lint_findings, covered_files_touched.
  `/Users/gdc/deadreckon/crates/deadreckon-core/src/gate.rs#L140-170`, `#L807-819`; `/Users/gdc/deadreckon/crates/deadreckon-core/src/tamper.rs#L45-49`, `#L75-85`
- Toolchain identity is frozen before the first turn: `GateEvaluatorIdentity { schema_version, protocol_version, controller: GateBinaryIdentity{sha256,os,arch}, evaluator, docker: Option<DockerGateIdentity{image_id, platform, guest_path}> }`, with a compiled-in marker `deadreckon-gate-evaluator-protocol-v1` retained through LTO.
  `/Users/gdc/deadreckon/crates/deadreckon-protocol/src/job.rs#L153-189`, `#L21-31`; `/Users/gdc/deadreckon/crates/deadreckon/src/bin/dr-gate.rs#L22-24`; `/Users/gdc/deadreckon/crates/deadreckon/src/commands/job.rs#L263-267`
- Containment is proven separately: `dr-gate probe-boundary` runs inside the resolved sandbox and produces a `SandboxBoundaryObservation` recording gate_key_read_denied, proof_write_denied, control_write_denied, operator_capture_read/write_denied, signing_env_scrubbed — as a `deny_unknown_fields` wire shape so an agent cannot add self-attesting fields.
  `/Users/gdc/deadreckon/crates/deadreckon-protocol/src/job.rs#L622-650`, test `#L696`; `/Users/gdc/deadreckon/crates/deadreckon/src/bin/dr-gate.rs#L89-121`; `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/turn_loop.rs#L4351`

### 1.5 Key 2 — the semantic judge

- A fresh, read-only, no-session provider call with three verdicts only: `SemanticDecision::{Achieved,Revise,Uncertain}`, mapped to `SemanticJudgeResult::{Achieved, Revise, NeedsReview, Unavailable, LostContainment}`. Output constrained by a JSON schema requiring decision/summary/goal_coverage/blocking_missing with `additionalProperties: false`. "CLI routes receive an empty, temporary workspace under an enforceable read-only sandbox… No worker session is ever supplied."
  `/Users/gdc/deadreckon/crates/deadreckon-protocol/src/job.rs#L475-479`, `#L458-471`; `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/semantic_judge.rs#L68-74`, `#L898-917`, `#L1091-1130`, `#L291-311`
- The prompt is exact and auditable ("You are an independent completion judge. Assess meaning only; deterministic checks have already passed and you may not override them… Every evidence citation must be one of approved-goal, approved-contract, source-diff, deterministic-gate, authority, implementation-notes."), max_output_tokens 2048, `WorkspaceAccess::ReadOnly`.
  `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/semantic_judge.rs#L865-896`
- The judge cannot be fed a different result than the one that passed the gate: `SemanticEvidencePack` has exactly six ids with byte caps (contract 64KiB, diff 256KiB, notes 64KiB, summary 4000 chars, 64 findings); `validate_semantic_judgment_input` recomputes `input_sha256` and refuses with "semantic judgment does not bind the current result, deterministic marker and approved evidence".
  `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/semantic_judge.rs#L30-43`, `#L46-58`, `#L171-187`
- An `achieved` verdict is structurally policed: responses citing evidence ids outside the six are rejected, as are `achieved` verdicts carrying blocking_missing entries or non-`met` coverage — with a server-side re-check.
  `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/semantic_judge.rs#L919-946`; `/Users/gdc/deadreckon/crates/deadreckon-core/src/completion.rs#L216`

### 1.6 Budgets, clocks and boundaries

- Four durable dimensions frozen at start: `max_spend_usd: f64`, `max_wall_seconds: u64`, `max_attempts: u32`, `deadline: Option<DateTime<Utc>>`, alongside `semantic_judge: Required`, hashed into `effective_policy_sha256`.
  `/Users/gdc/deadreckon/crates/deadreckon-protocol/src/job.rs#L99-110`; `/Users/gdc/deadreckon/crates/deadreckon/src/commands/job.rs#L268-282`
- The wall cap survives restarts because it is folded from the event log, not a process timer: `active_attempt_wall` sums every AttemptStarted/Stopped interval and adds `now - started` for a live attempt; double-start, orphan stop, backwards timestamps and overflow all fail closed.
  `/Users/gdc/deadreckon/crates/deadreckon/src/commands/supervisor.rs#L5855-5901`, `#L5903-5915`, `#L5567-5574`, test `#L8321`
- Deadline and wall cap compete in one `JobWorkAllowance { remaining, cutoff, boundary }` with `ActivePolicyBoundary::{WallCap, Deadline}`; cancellation beats both, deadline beats wall cap on a tie.
  `/Users/gdc/deadreckon/crates/deadreckon/src/commands/supervisor.rs#L5822-5853`, `#L380`, `#L395`, `#L5771-5794`
- A wall-cap terminal event is only written after every supervised process is proven stopped; unprovable cleanup becomes Blocked/LostContainment, not a quiet BudgetExhausted.
  `/Users/gdc/deadreckon/crates/deadreckon/src/commands/supervisor.rs#L5590-5619`, `#L5714-5735`, `#L5688-5712`
- Spend is enforced twice: the run loop pauses at `PausedAtCap`, the supervisor classifies the failed child as BudgetExhausted/SpendCap; the judge is given `remaining_spend_usd` and refused outright at the cap.
  `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/turn_loop.rs#L925-937`, `#L5167-5184`, `#L5192-5198`; `/Users/gdc/deadreckon/crates/deadreckon/src/commands/supervisor.rs#L5177-5195`
- Attempt caps produce `StopReason::AttemptLimit` → `JobOutcome::RetryExhausted`, not generic Failed; plan default is 3.
  `/Users/gdc/deadreckon/crates/deadreckon/src/commands/supervisor.rs#L1140`, `#L1520`, `#L1687-1704`; `/Users/gdc/deadreckon/crates/deadreckon-core/src/job.rs#L658-666`; `/Users/gdc/deadreckon/crates/deadreckon-core/src/plan.rs#L141`
- `RunWorkClock` resumes from `state.total_wall_seconds` and `sync` only moves the persisted total forward (`.max(...)`).
  `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/turn_loop.rs#L216-265`, `#L214-215`

### 1.7 Recovery, containment of the child, and machine durability

- "The append-only job history is control truth. Process exit is only a wakeup to inspect persisted run evidence; it is never accepted as completion." On relaunch, `supervise_one_job` reloads the JobView, claims/reclaims the lease, re-checks cancel/deadline/wall, recovers crash-partial driver state, and distinguishes a pre-release crash (relaunch same attempt) from a post-release crash (adopt, do not duplicate).
  `/Users/gdc/deadreckon/crates/deadreckon/src/commands/supervisor.rs#L1-4`, `#L1039-1218`, `#L1142-1150`, `#L2756`, `#L2889`
- `dr-gate guarded-exec` reads a ≤512-byte release token from stdin, checks its SHA-256 against the durable launch, and requires pid, launch_id, attempt, release digest, phase == Prepared and `SupervisedProcessIdentity::Current` to match before `setpgid` + `exec`.
  `/Users/gdc/deadreckon/crates/deadreckon/src/bin/dr-gate.rs#L289-364`; constant `GUARDED_LAUNCH_PROTOCOL = "stdin_release_v1"` at `/Users/gdc/deadreckon/crates/deadreckon/src/commands/supervisor.rs#L59`; test `/Users/gdc/deadreckon/crates/deadreckon/tests/watchkeeper_trust_boundary.rs#L37`
- Machine-restart durability uses launchd/systemd with a preflight that refuses to claim durability unless the service is loaded/active **and** enabled with a live checkpoint; other platforms are honestly reported unsupported.
  `/Users/gdc/deadreckon/crates/deadreckon/src/commands/supervisor_service.rs#L430-433`, `#L600-612`, `#L284`, `#L419`

### 1.8 Rewind, provenance, undo

- Per-turn snapshots written atomically via staging dir + rename; `restore_snapshot` errors "refusing an unproven restore" without a capture manifest.
  `/Users/gdc/deadreckon/crates/deadreckon-core/src/artifacts.rs#L91-138`, `#L148-175`
- Sub-turn flight recording: `FlightManifest`/`FlightSession`, `CheckpointManifest` (quiet_ms 750, poll_ms 500, anchor_every 20), `CheckpointTrigger::{ProviderTool, FileQuiet, ProviderExit, Manual}`, `CheckpointChangeKind::{Created,Modified,Deleted,SkippedOversize}`, and a `RewindEvent` ledger with `RewindTargetKind::{Turn,ProviderEvent,Checkpoint}`, `RewindMode::{Preview,Apply}`, `RewindStatus::{Ok,Refused}`.
  `/Users/gdc/deadreckon/crates/deadreckon-core/src/flight.rs#L26-29`, `#L84-101`, `#L103-164`, `#L166-206`, `#L525`, `#L633`
- `ProvenanceRecord { timestamp, prompt_id, model, tool_call_id, session_id, files }` appended to `<run_root>/provenance.jsonl`, alongside `traces.jsonl` and `spend.jsonl`.
  `/Users/gdc/deadreckon/crates/deadreckon-core/src/artifacts.rs#L29-36`, `#L80-84`; `/Users/gdc/deadreckon/crates/deadreckon-core/src/run_view.rs#L67-68`
- Undo is authorized by controller-signed artifacts, not mutable events: "Mutable events never decide what repository, ref, or revision to change." `job_undo_command` revalidates the receipt *after* taking `operation.lock`, rebuilds `AppliedDeliveryAuthority` from the signed `AppliedGitDeliveryReceipt`/`GitDeliveryIntent`, refuses if that authority changed, and verifies an exact revert commit.
  `/Users/gdc/deadreckon/crates/deadreckon/src/commands/undo.rs#L1-8`, `#L88-105`, `#L433-439`; `/Users/gdc/deadreckon/crates/deadreckon-core/src/job.rs#L709-775`; `/Users/gdc/deadreckon/crates/deadreckon-protocol/src/job.rs#L565-602`

### 1.9 The operator surface

- `start` is an admission pipeline, not a launcher: TTY/config defaults → source-mode resolution *first* → goal-shape classification → mode prompt → provider/model + done criteria → contract materialization → final confirmation → service install → launch plan → durable Job + detached supervisor. "Soundings: source policy is an admission decision, not a dispatch afterthought."
  `/Users/gdc/deadreckon/crates/deadreckon/src/commands/start.rs#L4002-4247`, comment at `#L4021`
- Goal shape (Single/Orchestrate/Campaign) comes from a deterministic ladder that always answers, optionally overridden by a provider "course planner"; the recommendation carries a rationale that becomes the operator-visible suggestion.
  `start.rs#L523-565`, `#L727-749`, `#L759-787`, `#L4036`
- Prompt time is excluded from the admission budget: "Prompt time between admission phases is not provider work."
  `start.rs#L4063-4067`, `#L4112-4118`
- Service install — the first durable machine mutation — happens only after contract authoring and the operator's confirmation both succeed.
  `start.rs#L4125-4133`
- The decision becomes a durable artifact before anything runs: `launch_plan_from_decision` serializes goal, shape, pieces, per-role providers/models, budget, contract source, the verbatim signal bundle, rationale and `accepted_by` into launch-plan.json, then SHA-256s it.
  `start.rs#L4193`, `#L4204`; `/Users/gdc/deadreckon/crates/deadreckon/src/commands/course.rs#L234-259`; `/Users/gdc/deadreckon/crates/deadreckon/src/commands/job.rs#L253`
- `create_job` is the freeze point: refuses `sandbox = none` ("durable Jobs require containment"), validates the contract as strict, freezes gate-evaluator identity, computes six digests, writes an immutable authority.json, appends Created / ContractApproved / Queued — the last with detail `"approved inputs frozen before first agent turn"`. The absolute deadline is rechecked at this last boundary.
  `job.rs#L139-368`, `#L171-176`, `#L255-260`, `#L284-303`, `#L325-345`, `#L304-307`
- Detachment re-execs the same binary as `supervisor serve --once <job-id>` into its own process group with stdin nulled.
  `job.rs#L383-407`
- `attach` is read-only (`q`/Esc/Ctrl-D detach without killing work), with an activity view and `--view narrative`.
  `cli.rs#L456-471`; `attach.rs#L28-60`; `/Users/gdc/deadreckon/crates/deadreckon/src/tui/render.rs#L802-872`
- `status` (alias `next`) prints one orientation block and exactly one computed next action: non-terminal → attach; receipt error → status; verified+delivered → report; verified+undelivered → finish.
  `main.rs#L13739-13802`; `job.rs#L748-850`, `#L859-876`
- `finish` validates the two-key receipt before touching anything (refuses unless outcome==Verified && stop_reason==Verified, requires sealed receipt.json, refuses "job {id} receipt does not prove contained execution"), then routes by codebase mode: Worktree→apply, Copy/Fresh→export, InPlace→review guidance.
  `/Users/gdc/deadreckon/crates/deadreckon/src/commands/lifecycle.rs#L102-258`, `#L278-319`; `cli.rs#L368-384`
- `verdict` is explicitly non-authoritative: it re-runs acceptance checks NOW through the contained keyless evaluator in a disposable workspace, *reads* the original marker, and reports VERIFIED / REGRESSED / UNVERIFIED. "Checks pass now — verified now, not gated at build time." / "The signed marker no longer validates (forged or tampered)."
  `/Users/gdc/deadreckon/crates/deadreckon/src/commands/verdict.rs#L1-7`, `#L28-54`, `#L184-284`, `#L210-223`, `#L225-236`
- `report` emits a fixed evidence document (md/html/json) with lifecycle, work clock, approved-vs-current contract digest, deterministic results, semantic judgment, per-attempt records, resources, revisions, receipt, and an explicit `missing_evidence` list.
  `/Users/gdc/deadreckon/crates/deadreckon/src/commands/report.rs#L12-83`, `#L86-172`, `#L198-242`, `#L264-270`

### 1.10 The def-done contract compiler

- **Half one, deterministic:** `acceptance_defaults.rs` is pure, total, no-network, no-subprocess. A 17-row first-match sentinel ladder maps to a `ProjectKind`; Rust → `CargoTest`, every other kind → a single `Shell` check running that ecosystem's canonical test command (`deno test -A`, `go test ./...`, `python -m pytest -q`, `mix test`, `dotnet test`, `mvn -q test`, `./gradlew test`, `bundle exec rspec`, `composer test`, `make/just/task test`), Unknown → `FileExists` + caveat. Purpose: "so \"VERIFIED\" means a real test set ran — in any language."
  `/Users/gdc/deadreckon/crates/deadreckon-core/src/acceptance_defaults.rs#L1-11`, `#L82-138`, `#L306-324`, `#L336-362`, `#L394-399`
- Generated specs carry a provenance header `# generated by deadreckon detect: {kind_label}`.
  `/Users/gdc/deadreckon/crates/deadreckon-core/src/gate.rs#L1140-1162`; on-disk instance `/Users/gdc/deadreckon/.deadreckon-smoke/runstate/deadreckon-6283b242/runs/ed1b80f6326d48879223cb1b589b6cc9/acceptance.yaml`
- **Half two, natural language:** draft → deterministic lint → critic → single redraft → validate → write. If the deterministic floor still says Redraft, the command hard-fails with a remedy: `deadreckon def-done refine "add behavioral checks for every goal clause"`. If the critic is unavailable in a strict launch: "done contract critic is unavailable for a strict launch".
  `/Users/gdc/deadreckon/crates/deadreckon/src/commands/acceptance.rs#L858-916`, `#L1067-1228`, `#L1197-1202`, `#L1147-1165`; `cli.rs#L679-714`, `#L251-266`
- Falsifiability is enforced by classifiers, not prompt hope: behavioral-vs-inspection and can-fail-vs-weak labels; grep/rg/ag → source scan; `true`/`:`/`pwd`/`ls`/`echo ok`/`test -d .`/`cat …` → trivial; `--if-present` build/test → unfalsifiable. `lint_contract` emits NoBehavioralCheck, IfPresentOnlyBuildOrTest, OnlySourceScanIsSubstantive, UnfalsifiableCheck.
  `acceptance.rs#L583-622`, `#L624-635`, `#L637-645`, `#L647-657`, `#L659-662`, `#L664-698`
- `reconcile(goal, contract)` reports goal clauses whose tokens appear nowhere in the compiled contract, and always names the one command that closes the gap — "Divergence without a remedy is just an accusation."
  `acceptance.rs#L717-733`, `#L2228-2255`
- Generated contracts are validated before write: every path under `{working_dir}`, absolute source-root spellings rejected, bare `mktemp`/`mktemp -t` refused.
  `acceptance.rs#L1704-1724`, `#L1768`, `#L1853`, `#L1677`

### 1.11 Execution teams

- A provider+model **pair** is chosen as one unit. `start_execution_selections` expands each available route across its model catalog, one row per pair, e.g. "cli:codex · registry · context 200k · configured".
  `start.rs#L1059-1064`, `#L1116-1200`; `cli.rs#L172-175`
- One selection can apply uniformly to all roles; per-role customization is opt-in. Role labels are mode-derived (Review → "the implementor and reviewer"; FullPlan → "the planner and N implementors"; Campaign → "the planner and N sub-orchestrators").
  `start.rs#L1398-1409`, `#L1411-1474`, `#L1476-1496`
- Selections are frozen twice — into `CourseProviders` and into an immutable `DriverSpec` embedded in the launch plan, which is then SHA-256'd into the authority. At execution the supervisor resolves each role with an explicit precedence chain (driver per-role → durable orchestration spec → legacy single model).
  `course.rs#L137-158`, `#L1576-1650+`; `/Users/gdc/deadreckon/crates/deadreckon/src/commands/graph_job.rs#L67-91`, `#L762-777`, `#L7343`, `#L7368-7387`; `job.rs#L193-194`, `#L253`
- Flags for scripted determinism: `--planner-provider/--planner-model`, `--coder-*`, `--reviewer-*`, `--child-provider IDX=PROVIDER`, `--child-model IDX=MODEL`.
  `cli.rs#L748-777`, `#L180-182`

### 1.12 Providers: how inner harnesses are driven and observed

- 11 compiled-in descriptors: anthropic, openai, openai-compatible, smoke, cli:claude-code, cli:codex, cli:codex-server, cli:gemini, cli:opencode, cli:copilot, cli:pi.
  `/Users/gdc/deadreckon/crates/deadreckon-providers/src/registry/mod.rs#L20-56`; one TOML each in `/Users/gdc/deadreckon/crates/deadreckon-providers/descriptors/`
- Four have hand-written adapters (CliClaudeCode, CliCodex, ScriptedSmoke, HTTP `ProviderAdapter`), one is special-cased (`cli:codex-server` → JSON-RPC `CliCodexServerProvider`), and everything else is `GenericCliProvider` built from descriptor TOML — cli:gemini, cli:opencode, cli:copilot, cli:pi.
  `router.rs#L306-341`, `#L322-339`; `cli_generic.rs#L38-43`
- Operators can add/override without recompiling via `<home>/providers.d/*.toml` deep-merged over builtins.
  `registry/mod.rs#L455-530`
- Default route order is CLI-first: cli:claude-code, cli:codex, anthropic, openai, openai-compatible. `cli:codex-server` is registered but never a default.
  `router.rs#L294-303`; test at `/Users/gdc/deadreckon/crates/deadreckon-providers/src/lib.rs#L97-119`
- **Two contracts.** Direct-API providers return exactly one structured action per turn (`bash`, `write_file`, `reshape`, `done`) and DeadReckon executes it. CLI-harness providers receive a goal prompt and mutate the tree themselves.
  `turn_loop.rs#L2804`, `#L395`, `#L2917`; `smoke.rs#L18-34`; `turn_loop.rs#L2926`, `#L2865-2867`, `#L611-617`, `#L953`
- A CLI harness turn is observed through five independent channels, none of them the model's own claim: (1) git/filesystem diff since the prior snapshot, (2) fresh snapshot, (3) a trusted commit labelled `cli_subagent`, (4) per-file provenance, (5) the flight ledger. An empty deliverable set fails the turn outright.
  `turn_loop.rs#L983-1055`, `#L646-652`, `#L841`, `#L2872-2877`; `/Users/gdc/deadreckon/crates/deadreckon-runtime/src/flight.rs`
- Three tiers of structured parsing: bespoke typed mirrors (Claude `-p --output-format stream-json`; Codex `exec --json`; Codex app-server JSON-RPC with deterministic approval answering driven by `CapabilityPosture`), a declarative RFC-6901 `[contract]` tier (cli:copilot, cli:pi), and genuine substring scraping as the floor (`contains("error"|"tool"|"token")`, recursive hunt for `summary|message|content|text|delta` and `input_tokens|prompt_tokens`).
  `claude_events.rs#L66-96`, `#L101-143`, `#L154-192`; `codex_events.rs#L146-200`, `#L60-137`; `codex_app_server.rs#L34-41`, `#L1062-1078`, `#L409-412`, `#L972`; `registry/mod.rs#L150-165`, `#L173-232`; `cli_contract.rs#L423-591`; `flight.rs#L918-947`, `#L957-966`, `#L969-990`, `#L992-1005`
- `live_contract = true` (claude-code, codex, codex-server, copilot, pi) stands the post-hoc file scraper down so paths never double-count; gemini and opencode lack it and are scraped, with dated checked-in probe notes explaining why and a test asserting the gap stays documented.
  descriptors; `flight.rs#L291-298`; `/Users/gdc/deadreckon/crates/deadreckon-providers/tests/registry.rs#L39-60`
- Degradation is a caveat, never a turn failure: `provider.contract.degraded`, `provider.contract.capability_missing`, `provider.contract.unavailable`.
  `cli_contract.rs#L347-354`, `#L470-520`; `cli_generic.rs#L268-299`

### 1.13 Sandbox containment

- Four backends — SandboxExec (macOS Seatbelt), Bwrap (Linux), Docker, None — plus Auto, which falls back to None with an explicit warning off macOS/Linux.
  `/Users/gdc/deadreckon/crates/deadreckon-sandbox/src/backend.rs#L56-64`, `#L94-131`
- The wrapper is resolved from a hard-coded absolute path list, never PATH, and must be an absolute canonicalized executable regular file.
  `backend.rs#L133-164`, `#L166-186`
- One chokepoint, `with_protected_boundary`, strips the three gate env vars and merges `ProtectedPathPolicy::discover()` + `protect_workspace_git_control(&spec.cwd)`. Callers can narrow but not omit.
  `/Users/gdc/deadreckon/crates/deadreckon-sandbox/src/commands.rs#L20-38`, `#L62-75`
- Read-denial and write-denial are split deliberately: unreadable+unwritable = `<home>/gate-keys` and operator captures; readable-but-unwritable = jobs dir, acceptance.yaml, trusted-codebase record, sandbox.toml, gate/, provider-evidence/, proofs/, snapshots/, provenance.jsonl — so an independent judge and the operator can still inspect them.
  `/Users/gdc/deadreckon/crates/deadreckon-sandbox/src/policy.rs#L43-90`, test `#L240-282`
- Git *routing* is protected, not just the git dir: the `.git` entry (a file containing `gitdir:` for linked worktrees) is denied and followed to protect the real git dir and commondir.
  `policy.rs#L92-108`, `#L140-164`, `#L166-185`
- Denies are emitted under both literal and subpath rules with canonicalized variants, so a symlinked HOME cannot walk around the key store.
  `commands.rs#L639-648`; `policy.rs#L209-224`; test `#L319-335`
- ReadOnly is enforced per backend (Seatbelt `(deny file-write* (subpath <cwd>))`, bwrap `--ro-bind`, docker `:ro`) and fails closed: `build_command` rejects non-ReadWrite with backend None, and the runner refuses a read-only spawn with no backend unless the inner CLI enforces it (only Codex sets `inner_read_only_enforced`).
  `commands.rs#L482-495`, `#L164-172`, `#L237-243`, `#L34-38`; `cli_common.rs#L239-248`, `#L226-229`; `cli_codex.rs#L207`
- `--sandbox none` cannot weaken a judge: `ProviderRequest::enforceably_read_only_with_backend` promotes None → Auto so the runner fails closed.
  `/Users/gdc/deadreckon/crates/deadreckon-providers/src/types.rs#L165-171`, test `#L277-286`
- Container backends actively mask protected paths (bwrap `--tmpfs` / `--ro-bind`; Docker tmpfs, /dev/null readonly bind, readonly self-bind), and Docker re-scrubs gate env at argv serialization because guest env is encoded into argv.
  `commands.rs#L369-422`, test `#L1267-1362`; `#L311-321`, test `#L1153-1194`
- Trusted Docker gate evaluation pins identity: `--pull=never`, explicit `--platform`, sha256 image id, `--entrypoint` to a read-only bind-mounted sidecar, `--cidfile`, io.deadreckon.* labels, with the sidecar re-stat'ed against symlink/non-regular substitution.
  `commands.rs#L247-263`, `#L323-333`, `#L349-367`
- Cancellation is proven: SIGTERM→SIGKILL across the process group with `SandboxError::CleanupIncomplete` if the group cannot be proven gone (fatal, never retryable), plus an independent provider-layer wait on a confirmed-absent PID authority file.
  `/Users/gdc/deadreckon/crates/deadreckon-sandbox/src/process.rs#L568-601`; `backend.rs#L20-24`, `#L41-51`; `cli_common.rs#L183-215`

### 1.14 Explicitly not done, inside DeadReckon

These are the packets' own "NOT DONE" findings and must be stated as such:

- **No per-host network filtering on any backend.** Seatbelt only distinguishes allowlist `["*"]` → `(allow network*)` from anything else → `(deny network*)`, so `["api.openai.com"]` collapses to a full deny. bwrap and Docker ignore `network_allowlist` entirely.
  `commands.rs#L440-448`, `#L195-197`, `#L307-310`; test `/Users/gdc/deadreckon/crates/deadreckon-sandbox/src/lib.rs#L530-537`
- **The inner coding agent has unrestricted outbound network under every backend**, because `ToolSandboxPolicy::cli_provider` sets `allow_network: true, network_allowlist: ["*"]`. Network containment applies to DeadReckon's own bash tool, not the inner harness.
  `policy.rs#L24-40`; `cli_common.rs#L287`
- **Filesystem parity across backends is not achieved.** macOS ordinary work is `(allow default)` with denies layered on and blanket `/tmp` + `/private/tmp` writes; bwrap and Docker are structurally deny-by-default.
  `commands.rs#L496-506`, `#L100-209`
- **Windows has effectively no sandbox** (docker.exe only; Auto → None; process-group containment and signalling compiled out off unix).
  `backend.rs#L188-197`, `#L117-123`; `process.rs#L565-566`, `#L604-606`, `#L743-745`
- **`SandboxBackend::None` runs unwrapped** with only a warning string; doctor calls it "available but unsafe".
  `commands.rs#L45-57`; `/Users/gdc/deadreckon/crates/deadreckon-sandbox/src/doctor.rs#L35-41`
- **The generic descriptor driver cannot serve schema-constrained structured output** and refuses rather than degrading; only Codex, Claude Code and the HTTP adapters can.
  `cli_generic.rs#L72-78`; `cli_codex.rs#L355-359`; `cli_claude_code.rs#L86-97`
- **`JobShape::LegacyChain` is a reserved discriminator only** — the durable scheduler refuses it; the supervisor accepts Single | Graph | LegacyCampaign.
  `/Users/gdc/deadreckon/crates/deadreckon-protocol/src/job.rs#L204-208`; `supervisor.rs#L1049-1057`
- **There is no turn cap in the durable `JobPolicy`.** `max_turns` lives only in `RunLoopConfig`, hard-coded to 12 at every CLI call site, not frozen into the authority, not hashed, with no `StopReason`.
  `turn_loop.rs#L78`, `#L540`; `run.rs#L1241`, `main.rs#L12269`, `lifecycle.rs#L4264`, `#L4773`; absent from `job.rs#L99-110`
- **Legacy Run/Plan/Chain/Campaign artifacts get a degraded read-only projection**, `precision: "legacy_snapshot"` + `StopReason::LegacyUnknown`, with a test asserting no `jobs/` directory is created.
  `/Users/gdc/deadreckon/crates/deadreckon-core/src/job.rs#L110-143`, test `#L1345`
- **Strict behavior is conditional on `jobs/<id>/job.json` existing.** Runs without it are compatibility runs on the old deterministic-only completion path and "never presented as a new two-key Job receipt".
  `turn_loop.rs#L5150-5155`, `#L4225`
- Several protocol fields are Option-typed purely for backward compatibility (`JobPolicy.execution`, `JobExecutionPolicy.gate_evaluator`, `JobAuthority.gate_evaluator_sha256`, `SandboxBoundaryObservation.gate_evaluator_sha256`, `JobLease.process_start_identity`); a new strict Job always populates them.
  `job.rs#L106-124`, `#L443-445`, `#L420-423`, test `#L715`; `commands/job.rs#L263-303`

### 1.15 The fault matrix

Claims above are backed by a dedicated fault matrix and restart suite (source read, not run): torn-append fail-closed, lease reclaim/fencing, receipt tamper blocking promotion, two racing supervisors executing each job once, every pre-release crash relaunching the same attempt, verified-projection failpoints recovering exactly once.
`/Users/gdc/deadreckon/crates/deadreckon-core/tests/watchkeeper_fault_matrix.rs` (L240, L277, L322, L363, L390); `/Users/gdc/deadreckon/crates/deadreckon/tests/watchkeeper_restart_boundary.rs` L26, L40; `/Users/gdc/deadreckon/crates/deadreckon/tests/watchkeeper_verified_projection_restart.rs` L41

---

## 2. The original unmet needs and the founding thesis

### 2.1 Provenance of the research

- The research is **not** in the DeadReckon repo. It lives at `/Users/gdc/stoa/docs/research/2026-05-10-unmet-needs/REPORT.md` ("Unmet Needs Adjacent to SpecStory — 2026-05-10", 47,703 bytes), with `seed-queries.json` and `iterations/iter-{1,2,3}/{ranked.json,dead-ends.json,raw/*.json}`. The build goal cites it as the scope source: `/Users/gdc/deadreckon/docs/goals/2026-05-10-1400-deadreckon-build-goal.md#L10`.
- **Method:** an automated social/web market scan, not interviews. A Codex agent spawned Claude sub-agents running the `last30days:last30days` skill over Reddit, X, YouTube, TikTok, Hacker News, Polymarket, GitHub and the open web, in a 3-iteration converging loop. Scored Frequency + Pain + WTP + Tangency, Confidence tracked separately; `ranked.json` shows `swaps_vs_prev=0`, `converged=true`. (REPORT.md §Methodology)
- **Commissioned for SpecStory**, not DeadReckon: stoa-web, stoa-cli, specstory-cli. `/Users/gdc/stoa/docs/goals/2026-05-10-unmet-needs-research-goal.md`
- The founding human request, verbatim, at `/Users/gdc/stoa/.specstory/history/2026-05-10_20-37-41Z-command-name-effort-command.md#L19486` (2026-05-10 23:40:52Z) asked for a verifiable agentic loop to find "the biggest unmet needs that people will pay for."
- The pivot to building is a single turn at `#L20027` (2026-05-11 03:05:01Z): "a harness that solves all the painpoints (without being everythin + the kitchen sink, it should be USERFRIENDLY) and 2. can tackle long running tasks as its PRIMARY goal with ease + support BYOK, similar to opencode."
- The name arrives 9 minutes later at `#L21274`: "ok i want the thing to be called deadreckon".

### 2.2 The 25 needs

**Top 10, all tied at composite 11 (freq 3, pain 3, WTP 2, tangency 3, confidence 3):**

| # | Need | One-line description (verbatim) |
|---|---|---|
| U-001 | Live Context And Spend Meter For Coding Agents | see context burn, compaction boundaries, token spend, and likely limit exhaustion while an agent works |
| U-002 | Multi-Agent Worktree Coordination Layer | task claiming, shared context, branch isolation, and merge sequencing |
| U-003 | Infinite Undo For Agent Edits | per-turn file snapshots, code-aware rewind, safe rollback that restores files, not only chat |
| U-004 | Prompt-To-Code Provenance Audit Trail | which prompt, session, tool call, model, and acceptance criteria produced each code change |
| U-005 | Searchable Team Memory For AI Coding Conversations | histories saved locally but hard to search, share, connect, reuse |
| U-006 | Cross-Tool State Sharing (Cursor / Claude Code / Codex / review bots) | tools do not share state or hand off context cleanly |
| U-007 | Terminal-Agent Desktop UI | visual diffs, file tree, live PTY, workspace dashboard |
| U-008 | Agent Observability With Traces, Evals, Failure RCA | "logs told us what happened but never why" |
| U-009 | Disposable Sandboxes For YOLO Agent Work | bypass-permission speed without trusting the host |
| U-010 | Billing Guardrails For Agent Runs | agent loops can silently burn quota or bill unexpectedly |

Source: REPORT.md L17, L40, L64, L87, L111, L135, L158, L182, L206, L230 + `iterations/iter-3/ranked.json`.

**#11–19 (composite 10):** Permission Boundaries For Unattended Agents; MCP Setup And Tooling Debugger; Team Onboarding From Real Agent Sessions; Structural Verification Before Agent Claims; Hooks And Test Gates Made Discoverable; Provider Routing Without Losing Workflow; Shared Session Handoff Between Humans And Agents; Port And Environment Isolation For Parallel Agents; Agent Security And Governance Receipts. (REPORT.md L256-465)

**#20–25:** AI Review Limits And Paid Review Continuity (8); Local-First Agent Data Control (10); Meeting Action Items Connected To Code Work (8); Prompt Libraries That Encode Team Standards (10); Execution-Efficiency Evals For Agent Runs (10); Agent Workspace Inventory And Run Queue (10). (REPORT.md L466-605)

**Ranking mechanic:** the top 10 are *not* individually ordered — all tie at 11. The cut line is driven almost entirely by Frequency (3 = 31+ mentions in 30 days vs 2 = 11–30). Pain and Tangency were near-universal 3; **WTP was 2 for every single one of the 25**. Rubric at `/Users/gdc/stoa/docs/goals/2026-05-10-unmet-needs-research-rider.md#L39-54`.

### 2.3 The thesis, and where each version actually appears

Two distinct statements exist and must not be conflated:

- **The research's own highest-level claim** (REPORT.md §Executive Summary, verbatim): "The highest-value opportunities cluster around making agentic coding predictable enough to delegate… The main product question is whether to package that substrate as operational infrastructure for safe delegation rather than as only history capture or local versioning."
- **DeadReckon's founding thesis as stated today** (`/Users/gdc/deadreckon/docs/MAP-OF-DEADRECKON.md#L31`, verbatim): "the gap between *an agent saying it is done* and *an operator being able to trust, inspect, recover, and accept the result*. This matches the original unmet-needs research: predictable delegation needed operational infrastructure more than another transcript or chat surface." Tagline at `/Users/gdc/deadreckon/README.md#L5`: "Run your coding agent unattended, and trust the result."

The MAP's version is a **retrospective articulation** (doc refreshed 2026-08-02, ~3 months after the build began); it is not in the original research or the 2026-05-10 goal/rider. See §6.

### 2.4 How the needs became scope

- The top 10 were mapped 1:1 to modules/commands in the build rider, each requiring an in-source comment (`// REPORT.md: Live Context & Spend Meter`), enforced by a grep loop: `for need in "Live Context" "Multi-Agent" … grep -rq "REPORT.md: $need" "$REPO/crates" || echo "FAIL_MISSING_NEED: $need"`.
  `/Users/gdc/deadreckon/docs/goals/2026-05-10-1400-deadreckon-build-rider.md#L59-73`, `#L396-402`
- V0 silently swapped research #16 (Provider Routing) into slot 9 and #25 (Workspace Inventory) into slot 10, dropping #5 (Searchable Team Memory) and #7 (Desktop UI).
- Re-audited one day later (`/Users/gdc/deadreckon/docs/AUDIT-2026-05-11.md`, 25-row "Need (verbatim title)" matrix) and again in `MAP-OF-DEADRECKON.md#L326-356`. The MAP is markedly more sober: #12 MCP "**Absent**", #22 meeting-to-code "**Absent / intentionally out of scope so far**", #20 "**Mostly absent / not prioritized**" — where the 2026-05-11 audit had marked needs 1, 5, 6, 9, 11, 13, 14, 15, 19 "Resolved".

---

## 3. What pi is and why it matters to the story

- **What it is:** an open-source (MIT) agent-harness monorepo at github.com/earendil-works/pi (npm scope `@earendil-works`, website pi.dev), authored by Mario Zechner (badlogic). `/Users/gdc/pi/LICENSE`, `/Users/gdc/pi/README.md`
- The headline product `pi` is an interactive terminal coding agent with exactly four built-in tools — read, write, edit, bash — self-described as "a minimal terminal coding harness". `/Users/gdc/pi/packages/coding-agent/README.md:15`, `:91`
- **Philosophy is deliberate subtraction**, stated as a list of refusals: "**No MCP.** … **No sub-agents.** … **No permission popups.** … **No plan mode.** … **No built-in to-dos.** … **No background bash.** Use tmux." `/Users/gdc/pi/packages/coding-agent/README.md:492-508`
- **No permission system at all; isolation is delegated to containers** — a Gondolin micro-VM extension, plain Docker, or OpenShell. "By default, it runs with the permissions of the user and process that launched it." `/Users/gdc/pi/README.md:37-45`, `docs/containerization.md`
- 9 lockstep-versioned packages at 0.83.0: pi-ai, pi-agent-core, pi-coding-agent, pi-tui, pi-protocol, pi-client, pi-server, pi-evals, session-backends/sqlite-node. `/Users/gdc/pi/AGENTS.md:123`
- **What it ships that Claude Code and Codex CLI do not** (per the packet): hot-reloadable in-process TypeScript extensions with full tool/UI/event access (`docs/extensions.md`); an installable package ecosystem for extensions+skills+prompts+themes (`docs/packages.md`, `pi install npm:…|git:…|path`); the LLM layer, TUI layer and protocol as separately consumable libraries (`packages/ai/README.md`, `packages/tui/README.md`, `packages/protocol/README.md`); ~15 API-key providers plus three subscription OAuth paths with cross-provider mid-session handoff; a CBOR remote-session protocol; a built-in model-backed eval harness (`packages/evals/README.md`); and session branching.
- It implements the harness-neutral Agent Skills standard, reading `~/.agents/skills` and `.agents/skills` alongside its own. `docs/skills.md:5`
- Four run modes: interactive TUI, print/`--mode json`, `--mode rpc` (JSONL over stdin/stdout), in-process Node SDK. `README.md:20`, `docs/rpc.md`, `docs/sdk.md`
- A **next-generation durable runtime is designed but not adopted**: `packages/agent/docs/harness-v2.md` specifies crash-durable operations, parallel named "lanes" over a shared append-only conversation tree, deterministic manual stepping through a gated effect boundary, and pluggable storage — with an explicit non-goal that "`packages/coding-agent` remains on its current runtime".
- Supply-chain hardening is unusually explicit: exact-pinned deps, `.npmrc min-release-age=2`, generated shrinkwrap, lifecycle-script allowlist, `--ignore-scripts`, npm trusted publishing via GitHub OIDC. `/Users/gdc/pi/README.md:75-87`, `AGENTS.md:157`
- Contributor posture: new issues and PRs from new contributors are auto-closed by default. `/Users/gdc/pi/README.md:11`

**Why it matters to the story.** pi is the clean counter-example: a harness that explicitly refuses to own approvals, sandboxing, sub-agents, or plan mode, and says so in its README. It proves the boundary DeadReckon claims. And DeadReckon consumes it as a first-class provider with **no bespoke Rust at all** — `cli:pi` is a compiled-in TOML descriptor (`/Users/gdc/deadreckon/crates/deadreckon-providers/descriptors/cli-pi.toml`; `registry/mod.rs:55`), launched as `pi --mode json --print <prompt>` with a 1800s timeout, sandbox writes scoped to `~/.pi/agent`. The Pennant work added a declarative `[contract]` mapping pi's JSON stream (`/id`, `/message/usage/{input,output}`, `/message/usage/cost/total`, `/assistantMessageEvent/content`, `/toolCallId`, resume via `--session {conversation_id}`) so it is parsed as telemetry rather than dumped as a blob — the motivating complaint being "the generic driver dumps that JSON as raw response content with `usage: 0/0`" (`/Users/gdc/deadreckon/docs/goals/2026-07-15-1658-deadreckon-pennant-goal.md:1`). DeadReckon also ingests pi's on-disk sessions (`~/.pi/agent/sessions/<encoded-cwd>/*.jsonl`, gated on a `session` header row, `cwd_match = top-level`, 2-minute freshness) for live TUI activity (`/Users/gdc/deadreckon/crates/deadreckon/src/main.rs:16478`, `:16445-16476`).

That combination — a minimal harness that owns none of the operator burden, driven by a descriptor with zero adapter code — is the cleanest illustration of the layering the essay is about.

---

## 4. The 25 book patterns, mapped to burden and mechanization

### 4.0 The book's own thesis

"Once the code is cheap, the work is the intent behind it and the proof that it holds." Supporting: "Specifying clearly turned out to be necessary and not enough. Agents are fluent, confident, and wrong often enough that a clean brief is only the opening move."
`/Users/gdc/extract-agentic-engineering/book/front/intro.typ#L17-23`, `#L19`

Corpus: 6 people at SpecStory built Stoa Sept 2025–May 2026, 1,310 captured agent sessions, 4,670 commits, "almost none of it written by hand."

Six parts: Verification Is the Job (01–04), Steering Not Typing (05–09), The Brief Is the Work (10–14), Docs Are the API Between Turns (15–17), Code Is Cheap Understanding Is Dear (18–20), You Run an Org Not a Pair (21–25). Every pattern file carries a `todo:` tuple which is the most direct enumeration of operator burden in the source.

### 4.1 The mapping

Verdicts: **M** = mechanized, **P** = partly mechanized, **L** = left manual.

| # | Pattern | Operator burden (from the book) | Verdict | DeadReckon mechanism |
|---|---|---|---|---|
| 01 | Calibrated Distrust | Interrogate every confident claim; demand the artifact. "A claim with no cited artifact is unverified by default." (`01-calibrated-distrust.typ#L11`) | **M** for the completion claim | Process exit is never accepted as completion (`supervisor.rs#L1-4`); two-key receipt refuses without contained native marker + Achieved judgment (`completion.rs#L195-216`); `verified_receipt_error` when the proof is gone (`core/src/job.rs#L90-96`) |
| 02 | Source of Truth Outside the Agent's Reach | Nominate and maintain an external oracle; reject agent-generated corroboration. (`02-…typ#L11`) | **M** | Gate key in a 0700 keyring outside the workspace (`gate.rs#L252-354`); read-denied paths (`sandbox/policy.rs#L43-90`); container backends mask them (`commands.rs#L369-422`); `.git` routing protected (`policy.rs#L92-108`) |
| 03 | The Premise Auditor | Name a prime suspect, reward disconfirmation, don't pre-decide. (`03-…typ#L11`) | **P** — only for contracts | The def-done critic + deterministic floor (`acceptance.rs#L1067-1228`) and lint classifiers (`#L664-698`) adversarially audit the *contract*. Nothing audits a general investigative premise |
| 04 | The Read-Only Turn | Commission diagnosis-only turns; type "Do not edit files"; withhold the edit. (`04-…typ#L11`) | **P** | Read-only is a real enforced posture for the judge (`semantic_judge.rs#L291-311`; `sandbox/commands.rs#L482-495`, `#L164-172`, `#L237-243`) and cannot be weakened (`types.rs#L165-171`). There is no operator-facing read-only *diagnostic* turn verb |
| 05 | The Interrupt Is the Keyboard | Watch the first tool call; kill mid-stream. 614 interrupts / 184 transcripts. (`05-…typ#L11`, `#L15`) | **P** — substituted, not mechanized | Durable sticky cancellation (`core/src/job.rs#L602-611`), proven process-group teardown (`process.rs#L568-601`), budget/deadline boundaries (`supervisor.rs#L5822-5853`). But the whole point of unattended operation is that nobody is watching; mid-run corrective steering is absent |
| 06 | Assert the Ground Truth, Collapse the Branch | Supply the missing runtime fact in one line. (`06-…typ#L11`) | **L** | No mechanism injects a fact into a live turn. Ground truth is front-loaded into the frozen goal/contract instead (`commands/job.rs#L284-303`) |
| 07 | Steer by Reference, Not Spec | Carry a mental index of which subsystem already solved this. (`07-…typ#L11`) | **L** | Nothing |
| 08 | The Human Is the Runtime Sensor | Run the real app; paste the artifact. "The agent proposed; I ran the world and brought back the receipt." (`08-…typ#L11`) | **P** | The gate genuinely executes the product — CargoTest/BuildSuccess/Shell with per-ecosystem test commands (`acceptance_defaults.rs#L306-362`) — and `probe-boundary` observes containment from inside (`dr-gate.rs#L89-121`). But nothing watches a TUI paint or drives a browser unless the contract's shell check does |
| 09 | License the Agent to Ask Before It Acts | Attach a permission-to-ask clause to ambiguous openers. (`09-…typ#L11`) | **P** | `StopReason::OperatorInputRequired` and `JobOutcome::NeedsReview` exist (`protocol/job.rs#L234-291`); an Uncertain/Unavailable judge becomes NeedsReview rather than promoting (`semantic_judge.rs#L68-74`). This is asking *at the end*, not mid-turn |
| 10 | The Prompt Is an Engineered Brief | Type the whole continuity into every cold handoff. "Front-loading is the only continuity the work has." (`10-…typ#L9`) | **P** | The brief is packaged, frozen and hashed: goal, contract, effective policy, launch plan, source tree/revision, gate evaluator — six digests into an immutable authority.json (`commands/job.rs#L139-368`, `#L284-303`); launch plan embeds the verbatim signal bundle and `accepted_by` (`course.rs#L234-259`). Authoring the content is still the operator's |
| 11 | Structure Compounds, Incantation Depreciates | Layer structure; measure your own scope-fence rate. (`11-…typ#L13`, `#L21`) | **L** | Structure exists (typed events, typed stop reasons, digests), but nothing measures the operator's brief-writing skill curve |
| 12 | Pin the Work to a SHA | Paste full 40-char hashes, not prose. (`12-…typ#L14`) | **M** | Everything is content-addressed: `source_revision`, `source_tree_sha256`, `result_revision`, `result_tree_sha256`, plus contract/policy/plan/marker/judgment/observation digests (`protocol/job.rs#L510-540`); contract identity re-checked in report (`report.rs#L264-270`) |
| 13 | The Self-Grading Spec (Goal + Rider) | Author the exit condition as runnable predicates before the work exists. "a falsifiable definition of finished." (`13-…typ#L11`) | **M** — the flagship | The entire def-done compiler: detector + generated spec with provenance header (`acceptance_defaults.rs#L1-11`, `gate.rs#L1140-1162`), NL→check pipeline with lint/critic/redraft (`acceptance.rs#L1067-1228`), falsifiability classifiers (`#L583-698`), goal/contract reconciliation (`#L717-733`), and dr-gate executing it contained (`dr-gate.rs#L163-204`) |
| 14 | Commit at Phase Boundaries, Never Push | Hold the autonomy ceiling; do the promotion keystroke yourself. "The edge, I keep." (`14-…typ#L11`, `#L22`) | **M** | Completion and delivery are separate verbs. `finish` refuses without a valid two-key receipt and routes worktree→apply / copy-fresh→export / in-place→review (`lifecycle.rs#L102-258`, `#L278-319`); `status` says "finish" only for a verified undelivered Job (`job.rs#L859-876`); undo is receipt-bound (`undo.rs#L1-8`, `#L88-105`) |
| 15 | Write for a Reader Who Remembers Nothing | Insist every doc claim carries a re-openable citation. (`15-…typ#L11`) | **P** | `report` produces a fixed evidence document with approved-vs-current digests, per-attempt records, and an explicit `missing_evidence` list (`report.rs#L86-172`, `#L264-270`); `verdict` states provenance and why (`verdict.rs#L210-236`). Prose documentation for humans is still hand-written |
| 16 | The Incident Doc Programs the Next Agent | Write postmortems preserving dead ends and their rejection reasons. (`16-…typ#L11`) | **L** | Nothing. The event log records what happened, not what was rejected and why |
| 17 | The AS-BUILT Map Is a Test That Can Go Stale | Maintain dated, SHA-stamped architecture maps; treat staleness as failure. (`17-…typ#L11`, `#L13`) | **L** | DeadReckon *has* such docs (`docs/MAP-OF-DEADRECKON.md`, `docs/AS-BUILT-ARCHITECTURE.md`) but ships no drift detection. Note: the packets themselves caught this doc drifting — see §6 |
| 18 | Delete and Regenerate from a Clean Base | Throw away days of working commits; keep the learning. (`18-…typ#L9`) | **P** | Discarding is made cheap and safe: per-turn snapshots with capture manifests (`artifacts.rs#L91-175`), sub-turn checkpoints and a rewind ledger (`flight.rs#L103-206`), receipt-bound undo of a delivered result (`undo.rs`). The judgment to discard is entirely manual |
| 19 | Band-Aid Is a Verdict, Not a Default | Re-price every workaround; refuse the debt ticket. (`19-…typ#L10`, `#L18`) | **L** | Nothing |
| 20 | Fix the Generator, Defend the Shape | Classify failure as semantic vs structural; the reflexes run opposite ways. (`20-…typ#L11`, `#L15`) | **P** | "Defend the shape" is mechanized hard: `deny_unknown_fields` on the boundary observation (`protocol/job.rs#L622-650`), closed evidence-id vocabulary and structural policing of `achieved` (`semantic_judge.rs#L919-946`), length-prefixed canonical signing (`completion.rs#L1690-1736`). "Fix the generator" is `def-done refine` — operator-initiated |
| 21 | The Model Is a Swapped Dependency | Record which model co-authored what; run the census; route per task. (`21-…typ#L9`, `#L13`) | **M** for the census, **P** for routing | `ProvenanceRecord` carries `model` per turn (`artifacts.rs#L29-36`); per-role provider+model frozen twice, into `CourseProviders` and an immutable `DriverSpec` hashed into the authority, with an explicit resolution precedence at execution (`course.rs#L137-158`; `graph_job.rs#L67-91`, `#L7368-7387`). Which model to route where is the operator's pick (`start.rs#L1411-1496`) |
| 22 | The Human Is the Message Bus | Copy one agent's scrollback into another vendor's prompt. "the colleague has no inbox." (`22-…typ#L9`, `#L13`) | **M** — the clearest fill | This is the packet's one pure tooling gap, and DeadReckon closes it: `JobShape::Graph` with planner/coder/reviewer roles and per-child providers (`graph_job.rs#L67-91`); the semantic judge is a *different provider* fed a bounded structured `SemanticEvidencePack` bound by `input_sha256` (`semantic_judge.rs#L46-58`, `#L171-187`) rather than a pasted transcript |
| 23 | Agents Self-Assign; You Pick the Lane | Read the offer; name the lane; don't status-chase. (`23-…typ#L9`, `#L13`) | **P** | The two-tier shape resolver produces a recommendation with a rationale that becomes the operator-visible suggestion row; the operator confirms (`start.rs#L523-565`, `#L727-749`, `#L759-787`). `status`/`next` computes the single next action so the operator does not have to ask "what's left" (`job.rs#L859-876`) |
| 24 | Fan Out Along Non-Overlapping Seams | Cut disjoint seams, scope each lane, reconcile yourself. (`24-…typ#L11`, `#L15`) | **P** | Graph jobs with per-index `children`/`child_models` and an immutable driver spec provide the lanes (`course.rs#L137-158`; `graph_job.rs#L762-777`). Seam-cutting is delegated to a planner provider or done by the operator; note the packet's own gap — the graph/campaign *production* side was not traced |
| 25 | Make the Agent Show Its Rails | Keep destructive git in advisor mode; audit the narrated safety mechanics. "No rails, no keys." (`25-…typ#L11`, `#L13`) | **M** — rails are structural, not narrated | The agent never holds the keys: keyless evaluator / trusted signer split with pre-key identity check (`dr-gate.rs#L163-204`, `#L491-533`); guarded-exec that cannot start without a matching release token and identity (`dr-gate.rs#L289-364`); git control paths denied inside the sandbox (`policy.rs#L92-108`); undo authorized only by controller-signed artifacts (`undo.rs#L1-8`) |

### 4.2 Honest summary of the mapping

- **Fully mechanized (7):** 01, 02, 12, 13, 14, 22, 25 — plus the census half of 21. These are the load-bearing claims: the completion claim itself, the external oracle, content-addressing, the self-grading spec, the promotion boundary, the inter-agent channel, and the rails.
- **Partly mechanized (12):** 03, 04, 05, 08, 09, 10, 15, 18, 20, 21, 23, 24. In every case the *machinery* exists but the *judgment* is still the operator's — which contract to write, which model to route where, which seam to cut, when to discard.
- **Left manual (6):** 06 (assert ground truth mid-run), 07 (steer by reference), 11 (measure your own brief-writing), 16 (the incident doc), 17 (keeping the AS-BUILT map true), 19 (band-aid as a priced verdict). DeadReckon has **no mechanism at all** for any of these six.

Two structural honesties worth stating plainly in the essay:

1. **The steering patterns (05, 06) are not solved — they are traded away.** Unattended operation is precisely the condition under which the interrupt keyboard cannot be used. DeadReckon's answer is to move all of that leverage forward into admission (freeze the brief, freeze the contract, freeze the policy) and backward into verification (two keys, receipts, verdict, undo). The middle of the run is genuinely less steerable, not more.
2. **The doc patterns (15, 16, 17) are the book's own practice, and DeadReckon mechanizes almost none of them.** `report` is a structured evidence document, not a postmortem; there is no drift detection for architecture maps and no representation of a rejected approach anywhere in the event vocabulary.

---

## 5. What mainstream coding agents provide and do not provide

All of this is grounded in on-disk artifacts: `~/.claude`, DeadReckon's descriptors/adapters/fixtures, and design docs.

### 5.1 Provided (verified on disk)

- **Full harness self-description in one event.** A Claude Code `system/init` row reports 30 built-in tools (Task, Bash, Edit, Read, Write, Skill, WebFetch, WebSearch, ToolSearch, TaskCreate/Get/List/Output/Stop/Update, Monitor, LSP, NotebookEdit, EnterWorktree/ExitWorktree, CronCreate/Delete/List, PushNotification, RemoteTrigger, ScheduleWakeup, SendMessage, ReportFindings, DesignSync, Workflow), 5 MCP servers with status, model id, permissionMode, ~100 slash commands, 8 subagents, ~70 skills, 7 plugins, memory_paths, capabilities. `/Users/gdc/deadreckon/crates/deadreckon-providers/tests/fixtures/semaphore/claude-simple.jsonl`
- **Hooks, really executed, streamed into the transcript** as `system/hook_started` / `hook_progress` / `hook_response` with hook_id, hook_name ("SessionStart:startup"), stdout, stderr, exit_code, outcome; plus `stop_hook_summary`. Configured declaratively by plugins with tool matchers (`"matcher": "Edit|Write|MultiEdit|NotebookEdit"`), conditional predicates (`"if": "Bash(git commit:*)"`) and timeouts. `claude-simple.jsonl`; `/Users/gdc/.claude/projects/-Users-gdc-deadreckon/896097e6-dd5b-45f5-8922-495591638d19.jsonl`; `/Users/gdc/.claude/plugins/cache/claude-plugins-official/security-guidance/2.0.6/hooks/hooks.json`; `.../vercel/0.45.1/hooks/hooks.json`
- **Permission modes as first-class persisted state.** Global default `auto` (`/Users/gdc/.claude/settings.json:6-8`); the fixture ran `bypassPermissions`; transcripts carry a `permission-mode` record distinct from `mode`.
- **A genuine two-way approval protocol in Codex app-server:** `item/commandExecution/requestApproval` and `item/fileChange/requestApproval` requests a client must answer. `/Users/gdc/deadreckon/crates/deadreckon-providers/src/codex_app_server.rs:617-643`, `:416-420`, `:462-463`
- **An enforceable inner sandbox in Codex** with three named levels — `read-only`, `workspace-write`, `danger-full-access` — the one inner sandbox DeadReckon trusts. `descriptors/cli-codex.toml`; `cli_generic.rs:540-549`; `cli_common.rs:226-229`
- **Session resume**, in different shapes: Claude `--resume <id>`, Codex `exec resume <id>` (which omits `--sandbox` because resume inherits the original policy). `cli_claude_code.rs:100-101`, `:204-225`; `cli_codex.rs:117-152`, `:371-439`
- **Todos, plans and subagents as harness-provided state:** 7,850 todo files under `~/.claude/todos`; 199 plan markdown docs under `~/.claude/plans`; a `Plan` subagent in `agents[]`; sidechain transcripts tagged inline via `isSidechain`/`parentUuid`/`leafUuid`.
- **Skills, plugins and marketplaces as real distribution:** 21 user skills, 4 marketplaces, 6 enabled plugins, versioned plugin cache. `/Users/gdc/.claude/settings.json:9-36`
- **MCP, including its failure states surfaced but unresolved:** one server `needs-auth`, four `pending`. `claude-simple.jsonl`
- **Tool-call telemetry from every CLI** — in five mutually incompatible vocabularies: Codex `item.started/item.completed` with `type:"command_execution"`; Copilot `assistant.message.toolRequests[]` + `tool.execution_start/complete`; Pi `/toolCallId`; Claude content-block `tool_use`/`tool_result`; OpenCode `step_start`/`text`. Fixtures under `tests/fixtures/{semaphore,pennant}/`; `/Users/gdc/deadreckon/docs/design/PROVIDER-CLI-INGEST.md:139`
- **Provider-side rate-limit surfacing** (a quota window, not a user budget): `rate_limit_event` with rateLimitType "five_hour", resetsAt, overageStatus "rejected", overageDisabledReason "out_of_credits".
- **Cost display — for some CLIs, in incompatible units.** Claude `total_cost_usd`; Pi `/message/usage/cost/total`; Copilot `premiumRequests` (not dollars); Codex tokens only, no cost field at all.

### 5.2 Not provided (verified by absence on disk)

- **No durable job lifecycle that survives the terminal.** Claude Code's session registry is keyed by OS pid — `~/.claude/sessions/` holds exactly `80130.json`, `87885.json`, `92024.json`, matching the three running `claude` processes. The daemon log ends `[supervisor] idle 5s with no clients — exiting`. `~/.claude/daemon/roster.json` is stale since May 2026 with `workers: {}`. `~/.claude/jobs/pins.json` is `[]`.
- **No independent verification of completion.** The only completion signal any CLI emits is the model's self-report: Claude `"stop_reason":"end_turn"` + `is_error:false`; Codex `turn.completed`; Copilot `exitCode:0`. The `ContractSection` schema has **no verification field at all** — its 12 fields are stream_args, dialect, conversation_id_path, usage_input/output_path, cost_path, answer_path, error_flag_path, error_message_path, flight_event_paths, resume_args, probe_substring. `registry/mod.rs:152-165`
- **No signed receipt anywhere.** No `signature`, `hmac`, or receipt field appears in any of the seven CLI descriptors or five fixtures. Signing exists only on the DeadReckon side (`gate.rs:10`, `:59`, `:620`, `:1634` — "acceptance marker signature is invalid; forged self-attestation refused").
- **No cumulative wall-clock cap across restarts.** The only expressible bound is per-invocation `timeout_seconds = 1800`, identical in all seven CLI descriptors, resetting on every spawn. `max_wall_seconds` exists only in DeadReckon's crates.
- **No cross-provider control vocabulary.** Only 2 of 7 CLIs (pi, copilot) can be driven declaratively; Claude and Codex need hand-written Rust mirrors; Gemini and OpenCode are shipped contract-less with recorded reasons — Gemini "exit[s] with IneligibleTierError/UNSUPPORTED_CLIENT before any structured event is emitted"; OpenCode "emitted text(answer), error, then text(null) while exiting zero". `cli_contract.rs:56-63`
- **Non-interactive means approval disabled, not delegated:** Claude `--dangerously-skip-permissions`; Codex `--ask-for-approval never`; Copilot `--allow-all`; OpenCode 0.15.5 removed even the hidden skip flag.
- **No deliberate promotion step.** Nothing in any descriptor, adapter or fixture distinguishes "agent finished" from "work accepted and landed". Promotion vocabulary (apply, finish, export, library, undo) exists only in DeadReckon's own catalog. `/Users/gdc/deadreckon/docs/design/USER-FACING-MATRIX.md:60`, `:77-82`, `:91`
- **No self-indexing or rewind of session logs on behalf of an outer supervisor:** "Provider JSONL files are never touched… the provider's own session UUIDs do not map to deadreckon turn IDs in any persisted index." `PROVIDER-CLI-INGEST.md:156-160`
- **Session state is discoverable but never uniformly.** Four cwd-matching strategies: session-meta (codex, `payload.cwd`), claude-project-dir, json-pointer at `data.context.cwd` (copilot), top-level (pi), directory-field (opencode), and none at all (gemini — no cwd in transcript). Cursor is not a log stream at all: a SQLite DB read by shelling to `sqlite3 -json <path> "select rowid as source_rowid, * from messages order by rowid"` from `~/.cursor/chats/*.db`, with `cwd_match = None` — Cursor history cannot be attributed to a working directory. `import.rs:371-388`, `:799-830`
- **The launchable set is small.** agentsview covers 24 agents, but "Most are IDE plugins (VSCode-Copilot, Cursor, Kiro IDE, Positron) or hosted SaaS (Claude.ai, ChatGPT, Warp, Piebald, Forge) and have no launchable binary — skip them." `PROVIDER-CLI-INGEST.md:17-21`, `:251-253`
- **Capability is discovered by parsing `--help`, not declared** — evidence these surfaces are unstable. Claude: 4 booleans from substrings (`--output-format` && `stream-json`, `--resume`, `--json-schema`, plus 5 schema-only flags). Codex: 10 booleans plus a feature bitmask. Both adapters code a resume-vanished-and-retry-once path with the message "resume target vanished; retried once with a fresh conversation". `claude_events.rs:18-56`; `codex_events.rs:20-36`; `cli_claude_code.rs:294`; `cli_codex.rs:524`

### 5.3 The synthesis (and its status)

The out-of-the-box scope of these harnesses is a **single process lifetime**: tool loop, approval prompts, optional sandbox, hooks, subagents, resume-by-id, MCP, todos, plan mode, cost readout. Everything beyond one process — supervision, budget accounting across restarts, adjudication of "done", and promotion — is left to whoever wraps them.

**This synthesis is an inference, not a cited fact.** The packet flags it: no single file asserts it, and the closest on-disk statement is `/Users/gdc/deadreckon/docs/HARNESS-ENGINEERING-COMPARISON.md:8`, which is DeadReckon's own claim about its own role (it "wraps existing coding-agent CLIs and owns their execution boundary, verification and promotion"; `:14` "the human still defines or approves 'done'"). That document is competitor-authored evidence and should be quoted as such.

---

## 6. Contradictions and unverified claims the essay must not assert

### 6.1 Nothing was executed

Across all packets, **not one behavioral claim was observed at runtime.**

- No `cargo test` run. Every DeadReckon "this is real" claim is a reading of implementation + test *source*. The watchkeeper fault-matrix and restart-boundary tests exist and reference the exact cited functions, but were not confirmed to pass on the current working tree.
- The live Docker boundary test (`deadreckon-sandbox/src/lib.rs#L393 live_docker_denies_control_tampering_and_gate_inputs`) is `#[ignore]`d behind `DEADRECKON_LIVE_DOCKER_TEST=1`. Docker containment claims rest on argv construction plus unit tests over that argv, **not on a live container**.
- The macOS hostile-agent test and the Seatbelt network-block test were not executed.
- **SBPL rule precedence was not verified empirically.** The reading assumes last-matching-rule-wins so trailing denies beat `(allow default)`. If that assumption is wrong, the ordinary-posture denies are ineffective. Do not assert macOS containment strength without this check.
- No `deadreckon start` / `attach` / `def-done` end-to-end run. No provider CLI was executed.
- No course command was run — every quoted output block (green check "8 passed, 0 failed", probe footer, promote refusals) is documentation as written.

### 6.2 Internal contradictions across packets

- **Receipt schema drift.** Packet 1 and 3 describe `CompletionReceipt` as carrying `attempt`, `outer_launch_id`, `sandbox_boundary_observation_sha256` and `execution_evidence`. The only receipt on disk (`/Users/gdc/deadreckon/.test-tmp/.tmp1roeU6/home/jobs/d882ffbc43074e86a343ab665272910a/receipt.json`, written 2026-07-29) is `schema_version: 1` and lacks all four. **Do not present that sample as a current receipt.**
- **The prebuilt binary lags the source.** `/Users/gdc/deadreckon/target/release/deadreckon` reports version 0.8.1 while the source tree has since grown Campaign, work-clock and gate-evaluator-identity code. The quoted `--help` text traces to `product.rs`, but other help output from that binary must be re-verified against a fresh build before quoting.
- **The research report contradicts its own data.** REPORT.md prose says needs #6 and #9 have "frequency score 2"; `iterations/iter-3/ranked.json` records frequency 3 and composite 11 for both. Since composite drives the top-10 cut, the report body read literally would place #6 and #9 *below* the line. Which is authoritative is unknown.
- **The founding thesis is retrospective.** The "gap between an agent saying it is done and an operator being able to trust, inspect, recover, and accept the result" sentence appears first in `MAP-OF-DEADRECKON.md` (refreshed 2026-08-02, ~3 months after the build began). It is **not** in the original research, the 2026-05-10 build goal, or the rider; the researcher could not find an earlier statement in README.md, DESIGN.md, CONCEPTS.md or CHANGELOG.md. The research's nearest actual language is "making agentic coding predictable enough to delegate." Attribute accordingly.
- **The MAP and the AUDIT disagree about what shipped.** `AUDIT-2026-05-11.md` marks needs 1, 5, 6, 9, 11, 13, 14, 15, 19 "Resolved"; `MAP-OF-DEADRECKON.md#L326-356` later grades several "Partly met" and marks #12 and #22 absent. The MAP is the later assessment.

### 6.3 Claims that are weaker than they look

- **WTP was never validated.** All 25 needs scored WTP=2 ("mentions current paid workaround"); the top score of 3 ("explicit budget statement or would switch tools") was never awarded. Several needs share one recycled quote ("combining Cursor Ultra and Claude Max subscriptions" backs #2, #6, #18, #25). The report's own Process Notes call for a follow-up to "quantify packageable WTP"; no evidence that follow-up ran.
- **The top-10 ordering is arbitrary.** All ten tie at composite 11. The #1..#10 numbering the build rider, audit and MAP all treat as priority is really the U-00N id order assigned during iteration-1 clustering. No tie-break rationale is recorded.
- **Citation quality is uneven and partly circular.** Need #1's "pain quote" ("real-time context meter") is a feature name from a product launch post; #5's ("the chat is accessible in my chat history") is from a bug report. Needs #1, #7 and #10 share an identical citation block — three of the ten "distinct" top needs rest on the same six sources. **No cited URL was verified to resolve or to quote accurately.**
- **The research is pre-filtered.** Every candidate was scored for tangency to stoa-web / stoa-cli / specstory-cli, with a `tangency == 0` veto. The 25 needs are adjacency-to-SpecStory-filtered, not a neutral market read. DeadReckon inherited that filter without re-deriving it. There were no interviews or surveys; the window was 2026-04-10 to 2026-05-10.
- **V0 scope silently deviated from the top 10** (dropped #5 and #7, promoted #16 and #25) with no recorded rationale.
- **"Mainstream harnesses do not provide X" is argued from absence** in descriptors, adapters and fixtures written by a team whose product supplies X. Strong evidence no usable surface was found; not proof none exists.
- **Claude Code's feature set here comes from its own runtime output, not documentation.** No Claude Code docs exist on the machine outside plugin caches. `~/.claude/agents` is empty (all 8 agents are built-ins/plugins), and no hooks are configured outside plugins — the full supported hook-event vocabulary could not be established (only SessionStart, UserPromptSubmit, PostToolUse, SessionEnd and stop_hook_summary were observed).
- **Gemini and OpenCode structured output was never successfully recorded**, so statements about what those two provide are weaker than for the other five. All fixture versions are pinned mid-2026: Claude Code 2.1.210, Copilot 1.0.45, Pi 0.79.1, Gemini 0.42.0, OpenCode 0.15.5.
- **pi version drift is untested in effect.** DeadReckon onboarded pi 0.79.1 (also what is installed) while the source tree is 0.83.0 plus unreleased work. Whether the `[contract]` JSON pointers or the ingest schema still hold for 0.83.0's `--mode json` output was not tested. The `--help` probe substring and the flags (`--mode`, `--print`, `--session`, `--model`) do still exist on the installed binary.
- **The Claude Code / Codex side of the pi comparison is general knowledge**, not read from their sources.

### 6.4 Judgments presented as findings

- **The book packet's manual-discipline / tooling-gap / both classification is the researcher's judgment.** The `.typ` files never use those terms. Patterns 01, 05, 11, 17 and 21 are explicitly flagged as reclassifiable. The packet's own tally is internally inconsistent (it states one split, then corrects itself to manual 11 / tooling 1 / both 13). §4 of this brief uses a *different* three-way scheme (mechanized / partly / manual) against DeadReckon and should not be conflated with the book's own framing.
- **Pattern titles drift between plan and source.** `PATTERN-PLAN.md#L792` names pattern 25 "Make the Agent Show Its Rails Before You Hand Over the Irreversible"; the shipped `25-show-its-rails.typ#L5` says `name: [Make the Agent Show Its Rails]`. The plan's audit trail also uses stale numbering (#17/#18/#19/#20). **The `.typ` `name:` field is authoritative.**
- **`COURSE-EVALUATION.md` describes an earlier state of the course** (33-box PROOF dossier, pods/breakouts/rotating roles, Discord, required grader-agreement, 8-minute Demo Day). The current `course/` files contradict most of that. `ONE-LAB-PROPOSAL.md` and `PROVE-IT-SESSION-REWRITE-BRIEF.md` are forward-looking briefs; some items may not have shipped — the S2 guide still says the interrupt cannot carry a fact, suggesting `interrupt --fact` did not land on the required path.
- The course's `163-line` gate claim and the existence of its quoted refusal strings in code were not verified against `control/dr-gate.ts`.

### 6.5 Coverage gaps that limit what can be claimed

- The **graph/campaign production path** (`crates/deadreckon/src/commands/graph_job.rs`) was not fully traced — parent-repair lineage, ordered candidate manifests and `CompletionExecutionEvidence` were read on the validation side only.
- **`crates/deadreckon-sandbox` enforcement strength** was not assessed in depth: the receipt records `contained`/`sandbox_backend` and the probe asserts denials, but how strongly sandbox-exec / bwrap / docker enforce them in practice is unconfirmed.
- **Clock skew:** the cumulative-wall mechanism depends on wall-clock timestamps in job events. Beyond `nonnegative_wall_interval` refusing backwards timestamps, there is **no monotonic fallback**.
- **Operator-facing surfaces were not verified to render the data** — TUI, `verdict`, cards were confirmed only to have the model behind them (`verified_receipt_error`, stop reasons, budget boundaries).
- **`crates/deadreckon-core/src/operator_capture.rs`** (3,715 lines; `OperatorCaptureReceipt`, provenance levels TrustedSupervisor / PublicDeadreckon / AuthoritativeHost) is a substantial additional evidence subsystem that was only sampled; its relationship to the completion receipt is uncharacterized.
- **`sandbox.toml` per-run tool policy** interaction with `ToolSandboxPolicy` defaults was not traced.
- **Operator overrides** in `~/.deadreckon/providers.d/*.toml` were not enumerated, so the effective provider list on this machine may differ from the 11 builtins.
- **Cost/spend evidence in the course corpus is thin** — interrupt counts and a burned attempt budget exist, but no dollar figures or token-spend measurements; `story-bank.md:12` explicitly bans "dollar figures from internal systems", so such data may exist but is deliberately excluded.
- **External figures quoted in the course reader** (METR doubling every 4.3 months to 16–20 hour tasks, Jan 2026; Cursor's 87.1% → 73.0% after removing reward hacks, June 2026; Deloitte Australia AU$440,000 refund; FDA Elsa; ~tenfold rise in nonexistent biomedical references) are **quoted from `course/reader/why-now.md` and `story-bank.md`, not independently verified.** The story bank itself requires re-citing public sources on redaction.