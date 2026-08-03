# Changelog

## Unreleased

- Release trust generation now discovers signing records in the nested CI
  artifact layout, and public flat-download verification independently checks
  manifest signing claims against checksummed trust evidence. Conflicting,
  missing, or inaccurate signing metadata fails closed.
## 0.8.0 — The watch keeps — 2026-08-02

Seventy-nine commits since 0.7.0 turn DeadReckon from a foreground harness that
could preserve a run into a durable local executor that owns the whole approved
goal. A Job can now outlive its launching terminal, recover bounded work, prove
both the checks and the meaning of the result, and leave one operator-readable
receipt.

**Goals become durable before work starts.** Guided and ordinary Single, Graph,
Campaign, supported chain, stored-plan and continuation launches converge on
one Job scheduler. A fenced supervisor leases queued Jobs, recovers abandoned
attempts without changing their identity, enforces cumulative time, spend and
retry limits, and records distinct blocked, cancelled, budget-exhausted and
failed outcomes. `start` returns the Job ID and lifecycle commands instead of
making the launching terminal the source of truth. (Watchkeeper)

**Completion needs two independent keys.** Deterministic compiled checks must
pass before a contained, read-only semantic judge can assess the approved goal,
contract, result and evidence. The judge can achieve, revise or request review;
it cannot waive a failed check. HMAC-bound receipts cover policy, source,
result, evaluator identity and judgment, while protected helper paths and
adversarial tests fail closed against marker forgery or control-state tampering.

**Workspace evidence is bounded instead of copied blindly.** Snapshot,
checkpoint and Git staging share a frozen ignore-aware capture policy. Tracked
files always win; Cargo, SwiftPM and other configured build roots become compact
manifests; suspicious generated subtrees and oversized files are reported but
not copied; hard file, byte and traversal budgets guarantee a partial result
instead of a hang. Content-addressed materialization deduplicates repeated
evidence, and Git filter input/output is drained concurrently, closing the
SwiftPM `.build` deadlock that first exposed this class of failure.

**The front door uses the source it names.** `start` resolves one canonical
source before provider discovery, done-contract authoring or writes. Guided
`review` and `full-plan --from <dir>` freeze tracked and untracked deliverables
into a digest-checked Job-owned approved copy, and every Graph child works from
that copy rather than the mutable operator path. Contract files remain in the
launch project while a capped, redacted dossier describes the real source.
(Soundings)

**Done authoring is structured and time-bounded.** Draft, independent critic and
optional redraft use exact schemas and a structured-text-only provider posture
under one 120-second default wall budget. Cancellation reaps the whole provider
process tree and removes partial files. Redraft receives the complete previous
YAML, Markdown, helpers and findings; deterministic lint remains the floor; a
valid written contract is reused safely on retry.

**Execution teams are one coherent choice.** Guided setup resolves provider and
model per orchestration role, can apply one provider/model uniformly, and
freezes planner, implementor, reviewer and child choices for recovery. Provider
catalogs remain scoped to the selected CLI so a model from one provider cannot
leak into another.

**Plans express the work they will actually run.** The classifier emits nodes,
dependencies, apply mode and optional subplans instead of naming one of several
overlapping orchestration products. Failed nodes retry from their prior working
tree with the gate complaint and bounded remaining budget. Ordered nodes can
land as they pass, nested nodes can own subgraphs, chains appear in the common
inventory, and `undo` accepts the same positional identity vocabulary.

**Installation conflicts are repairable.** Current macOS launchd enablement
values are accepted, Linux bubblewrap preserves approved toolchain and loader
paths, and `doctor` inventories the running binary, PATH selection, managed
service, receipt and checkpoint. `doctor --repair` can reconcile the active
shell installation and a DeadReckon-managed supervisor without claiming an
unmanaged binary or service.

### Behavior and compatibility

- Durable supported launches detach under one Job ID; explicit in-place and
  uncontained execution, historical conductor-only policies and preview remain
  labelled compatibility paths and cannot earn trusted Job receipts.
- Job status reports the immutable approved source as execution truth and keeps
  the original external path only as launch provenance.
- Existing persisted runs remain readable. New `plan.json` execution-policy
  fields are defaulted, and Soundings adds no Job, launch-plan, pipeline-state
  or acceptance-file schema migration.
- Strict unattended completion now becomes `NEEDS REVIEW` when the semantic
  judge is unavailable; it is never silently accepted from deterministic checks
  alone.
- `start` done-contract authoring follows the launch's selected planner or lead
  execution provider/model and shows that same route in previews.
- Claude Code done-contract authoring receives a valid isolated empty MCP
  configuration, and direct authoring failures identify the effective
  `defaults.doc_provider` setting.

### Release trust

- The initial `0.8.0` stable publication is deliberately narrowed to GitHub and
  Homebrew while npm trusted publishing and Windows Authenticode credentials
  are deferred. The Windows archive remains covered by checksums and GitHub
  attestations; the checked-in fail-closed npm and signing paths can be enabled
  for a later release once their trust material exists.

## Soundings (stable) — 2026-08-02

Implementation, compatibility hardening and depth-test span:
`c00e4ba..00fa040`.

- `start` now resolves and validates one canonical source before provider
  discovery, done-contract authoring, writes or final confirmation. Preview,
  plain/card/JSON output, acceptance, Job authority and dispatch consume that
  same decision, so unsupported input refuses before spend or mutation.
- Guided `review` and `full-plan` accept `--from`. Job admission freezes tracked
  and untracked deliverables into a digest-checked controller-owned approved
  copy before queueing. Graph children work from that copy; the operator source
  is unchanged and can later move or disappear without redirecting execution.
- Guided contract authoring separates the launch-project writer root from the
  resolved-source inspection root. A deterministic capped/redacted dossier
  supplies real manifest, source and test facts while excluding Git, secrets,
  SpecStory history, symlinks, runtime state and rebuildable output. Generated
  checks remain portable through `{working_dir}`.
- Draft, critic and optional redraft use exact output schemas and a
  structured-text-only provider posture. Codex authoring is ephemeral with
  tool/web/MCP/user-config surfaces disabled; Claude and API routes use their
  equivalent strict posture; unsupported adapters fail closed. Capability
  probes are cached per binary/version.
- Authoring now has one cumulative 120-second default wall budget (configurable
  from 30 to 600 seconds): draft gets at most 60 seconds, critic 20, and redraft
  only the remainder up to 60. Timeout/cancellation reaps the provider's whole
  process group and removes temporary/partial files. A weak timed-out candidate
  cannot be approved; a valid written contract is reused on retry.
- Redraft receives the complete prior YAML, Markdown, helpers, dossier, lint and
  critic verdict. `reject` normalizes to `redraft` without losing findings, and
  the deterministic lint floor cannot be overruled.

The reproduced failure spent about 14 minutes authoring against the wrong empty
destination before a late `--from` refusal. Soundings moves that refusal ahead
of provider work and caps any real authoring sequence at 120 seconds by default.
The hermetic dirty/untracked Cloudwing preview-to-Graph reproduction returned a
Job ID in 2.76 seconds on the development machine; this is correctness evidence,
not a live-provider performance claim.

## Watchkeeper one durable Job and two-key completion 2026-07-28

Initial implementation and clean-evidence commit span: `fdf7601..761b001`.
The later claim-complete dogfood harness is bound to clean source `e87c70f` by
evidence commit `e7b9bb2`.

Watchkeeper makes guided `start` durable before the first agent turn. It
freezes the approved goal, definition of done, requested sandbox and tool
policy, launch plan, deliverable source tree and revision under one Job ID. It
then detaches a lease-fenced supervisor.
`attach`, `status`, `list`, `kill`, `finish` and `report` resolve that Job
without duplicating its backing run, plan or campaign.

### Ordinary execution shares the Job scheduler

Ordinary direct `run` and `orchestrate`, new supported chains, stored-plan
`fork`, direct campaigns, and public or guided follow-ups now create durable
Jobs and detach through the same supervisor. Preview, explicit
in-place/uncontained execution, historical `chain run|resume`, unsupported
conductor-only chain policies, and chain extension remain labelled
compatibility paths and cannot produce a trusted Job receipt.

### Continuation is parent-bound and durable

Public `deadreckon extend` and the follow-up selected by guided `start` queue a
Single Job from the promoted parent artifact. Approval freezes the completed
parent identity, state digest, promoted deliverable-tree digest and, for a
verified parent, its receipt digest. The child revalidates those facts before
writing continuation history. A launch-time destination is refused; the
operator uses `finish` only after the follow-up earns its own two-key receipt.

### Durable Jobs have 2 completion keys

A contained native gate must pass the frozen deterministic checks. The trusted
controller materializes the approved `acceptance.yaml`, then runs keyless
`dr-gate evaluate` under the backend that the sandbox resolver actually chose.
The evaluator receives no `GATE_*` inputs and cannot write proof or Job control
files. The sandbox runner scrubs inherited gate inputs and reaps the evaluator's
whole process group, including delayed descendants.

Strict evaluation is now crash-guarded as well as sandboxed. A private release
pipe prevents the helper from creating its evaluator process group or executing
repository-controlled checks until a unique per-attempt record has been
atomically written and synced with the Job attempt, outer launch, boot ID and
process-start identity. Losing the worker before that point closes the pipe and
the command never starts. After release, cancellation and supervisor recovery
reconcile the identity-checked evaluator group before recording `Cancelled` or
starting a retry; corrupt, reused or unverifiable identities stop
`LostContainment` instead.

A strict Job refuses a resolved backend of `none` before reading signing
material. Only after the evaluator has stopped does childless `dr-gate sign`
receive the HMAC key. It strictly revalidates the evaluation, approved contract
and tamper facts, reconstructs progress and tamper evidence, and signs the
observed backend. A fresh read-only semantic judge must then return `achieved`.
The supervisor seals an HMAC-SHA-256 receipt that binds the authority, approved
policy digest, launch plan, optional source and result revisions, deliverable
source and result tree digests, gate marker, judgment, confinement, and the
sandbox backend that actually ran.

A deterministic failure cannot be overruled. A Single Job can use semantic
`revise` for another bounded worker turn. A Graph or Campaign parent uses
`revise` for a new bounded, fenced parent-only repair attempt without rerunning
successful leaves. Uncertainty, judge failure, missing containment or an
invalid receipt still stop `NEEDS_REVIEW`.

The supervisor now renews its fenced lease during source hashing and parent
verification, not only while polling a child. Fresh Jobs bind authority to an
empty Job-local source while preserving the launch workspace's lifecycle
scope; Copy sources are normalized before the immutable plan is written.
Concurrent `start --json` calls return the exact Job ID created by that call.

Semantic `achieved` now requires non-empty, all-met, evidence-backed goal
coverage both when model output is parsed and again when the signed receipt is
sealed. Semantic judging is bounded by the Job's remaining wall time, and
Single, Graph and Campaign completion fails closed with typed spend/wall
reasons when no judging budget remains or a provider response exceeds policy.

### Graph and Campaign verify the parent result

Durable review and full-plan work always uses at-end delivery. The supervisor
copies the merged result into a same-ID parent run, runs its native gate, asks
the semantic judge, validates the receipt and then promotes the parent.
`finish` exports that receipt-bound parent, not a mutable child artifact.

A durable Campaign can recover an exactly linked persisted sub-plan. Before
parent verification, it rebuilds the worst-of roll-up from current leaf
evidence and compares it with the stored and merged copies. A refused or
changed roll-up fails before the semantic judge. A clean campaign parent then
uses the same gate, judgment, receipt and promotion sequence as a Graph.

Graph and Campaign semantic repair now records an intent before launch and a
fenced manifest/candidate before adopting the changed parent tree. Repeated
rounds are linked by attempt, launch, lease and tree identity. Recovery can
adopt a candidate-ready round without starting a duplicate worker. The marker
HMAC binds the active repair evidence, while receipt sealing and `finish`
validate the complete archived lineage from stable regular-file snapshots and
refuse mutation, removal, identity drift or byte-identical symlink
substitution. Cancellation and all Job budget limits remain authoritative
during both repair and the following semantic judgment.

### The worker cannot mint its proof

Gate keys live outside the agent-visible workspace and are not read until the
sandboxed evaluator and its residual process group are gone. Public macOS fault
tests cancel a held-open gate and SIGKILL its outer launcher; both prove the old
evaluator is gone before terminal cancellation or the next attempt. Authority,
lifecycle, gate, proof, snapshot, provenance and receipt paths are denied or
read-only across the supported sandbox and provider routes. Version-2 markers
and receipts fail closed on missing key material, synthetic proof,
`sandbox_backend = none` or `contained = false`. Legacy-v1 runs retain their
historical nonce marker validator.

### Result and delivery identity are fail-closed

DeadReckon now separates deliverable files from provider evidence, lifecycle
metadata and disposable build output. Trusted result copying preserves
executable files and symlinks without following them, rejects special files,
and keeps raw Unix path identity. Provider-created Git commits and index state
are discarded before DeadReckon creates its own hook-free result commit through
trusted Git control paths. Documentation providers run read-only and their
parsed output crosses the same trusted commit boundary.

Sandbox wrappers resolve to canonical trusted system executables rather than
ambient `PATH`. Before any reset, restore or staging operation can refresh the
worktree, DeadReckon inventories the current workspace and approved base tree
and refuses external clean, smudge or process filters. A no-refresh reset is a
second guard against racy timestamp-triggered filters.

Strict receipt fields bind the approved policy digest, optional source and
result revisions, deliverable source and result tree digests, and the resolved
backend and containment result. Worktree sealing and validation separately
enforce the approved base, merge-aware path history, filesystem and Git
identity, dirty and masked index checks, active filters, and gitlinks. Sealing
also creates a deterministic result-retention ref; the ref is not a receipt
field. For a verified worktree apply, `finish` validates every introduced
delivery-history path and restores the pre-delivery revision when final
identity checks fail. Promotion publishes only validated candidates, records
durable state before removing owned working data, and can recover each covered
crash window idempotently.

### Service support is conditional

`deadreckon supervisor install|start|status|stop` manages an owned per-user
launchd or systemd definition. It pins the current binary, `DEADRECKON_HOME`
and `PATH`, and refuses an unmanaged same-name unit. Repository tests cover the
definitions and state classification. They do not prove an active service or a
live reboot.

The operator dogfood kit contains 24 tasks across 2 repository and provider
slots. It also includes a metrics collector, human-review schema,
credential-free adversarial runner, and a passive operator-gated recorder for
the 9 current live fault claims. The ninth separates the hostile live Docker
worker, independent judge and valid-receipt claim from the narrower
credential-free Docker lifecycle proof. The committed historical
credential-free result is 13 passed, 0 failed, and 8 explicitly unproven
live/host claims. The sanitized live result records 2 attempted tasks, 22 not
run, and 0 verified. This release does not claim live task rates,
cross-provider results, machine-restart results, or false-accept and
false-reject rates.

Pass-capable live recording now uses a protected `dr-capture` helper outside
the Job workspace. It authenticates an immutable Job/trial binding, an
append-only exact-evidence history, the deterministic evaluation, and an HMAC
publication receipt. Manual operator-selected files remain useful
documentation but can only produce an inconclusive result.
Protected preparation now also refuses a Job whose actual Single, Graph or
Campaign shape falls outside the selected trial's closed shape declaration.
Each trial also signs a non-empty list of exact allowed terminal
outcome/stop-reason pairs before the intervention. A `verified/verified` pass
still requires the unchanged valid `CompletionReceipt`. An explicitly allowed
non-Verified result can pass only through separate signed terminal lineage
that binds the Job authority, complete history, final terminal event and public
report; it must not have a completion receipt.
The live network-loss trial now derives and signs one non-loopback HTTP worker
endpoint from the provider registry. Its protected evidence records an exact
reachable -> unreachable -> reachable transition for one current, durably
linked supervised attempt and accepts only that attempt's later stop followed
by its retry or an approved terminal result. Missing restoration, substituted
route/endpoint/launch identities, stale stops and arbitrary probe URLs fail
closed. The evidence deliberately claims an observed outage, not that a
particular host firewall command caused it.

Real approved guided starts now require the managed per-user supervisor to be
current, active and represented by a live schema-version-2 boot/PID/process
identity checkpoint. `deadreckon setup --supervisor` performs the explicit
install-and-start transition; read-only preview remains side-effect free. All
public durable Job creation routes also accept one RFC3339 absolute deadline.
The supervisor enforces it while children are live and reconciles outer,
evaluator, Campaign and merge-repair process authorities before recording the
typed terminal result with no retry. Invalid or fractional wall caps are
refused rather than silently widened.

The Campaign interruption oracle is no longer structurally inconclusive. It
proves the narrower claim DeadReckon actually persists: one prepared, released
and linked sub-Plan launch is adopted exactly once by the replacement fenced
owner, then that same Plan is recovered without a second persisted launch or
reopened completed task. Foreign, stale, duplicate or substituted identities
fail closed. This is implementation and recorder evidence; the real live
Campaign interruption trial remains outstanding.

A real macOS public-command end-to-end test proves the contained two-phase
Seatbelt gate. It checks protected-path denial, inherited gate-input scrubbing,
residual process-group cleanup and the signed observed backend. A separate
opt-in real Docker test proves the common key, environment, network and
control-path boundary without pulling an image. Three public strict Docker Job
tests with a statically linked Linux `dr-gate` prove deterministic completion
followed by `NEEDS_REVIEW`, cancellation without retry or receipt, and worker
`SIGKILL` cleanup before one bounded retry. Live Linux/bubblewrap evidence and
a real service-backed reboot remain outstanding.

The collector no longer treats raw receipt fields as verified evidence. Its
verified count requires a valid public `report --json` receipt assessment and a
successful public `finish`; forged raw receipts and validation-without-finish
are regression-tested to remain unverified.

## 0.7.0 — Sea trials — 2026-07-25

Six slices since 0.6.0. The harness stopped guessing at its agents and started
reading them, gained a rudder, put one keel under its ledgers, and had the
contradictions shaken out of its front door.

**The front door agrees with itself.** Every id-taking verb used to resolve
references its own way, so `deadreckon status` could report `not found: run
<id>` for an id `deadreckon list` had just printed. One resolver now answers for
all 18 of them, `latest` means one thing, and a refusal always names a command
that accepts the id it was given. `list` shows one row per plan with children
folded beneath, and surfaces cap their supporting actions at three. (Shakedown)

**The CLI agents are read, not scraped.** Codex and Claude Code publish
structured event streams; DeadReckon now consumes them for real token counts,
answers extracted from the result event, and per-run session resume, instead of
treating raw stdout as the whole response. (Semaphore)

**New provider contracts are configuration.** A compatible agent CLI can declare
its wire contract in descriptor TOML — structured-output arguments, JSON
pointers, resume arguments, capability probes — rather than needing a bespoke
Rust driver. Pi and Copilot onboard this way. (Pennant)

**You can steer a running child.** `deadreckon steer` and Helm's `:steer` inject
operator input into a live Codex turn over the app-server protocol, with a
durable at-least-once inbox, interrupt-before-kill, and approvals answered from
the run's own capability posture. Server loss degrades to the exec route rather
than failing the turn. (Rudder)

**One protocol crate under the ledgers.** Every persisted ledger line type moved
into `deadreckon-protocol` as one tagged vocabulary with generated, drift-tested
JSON Schemas and a single persistence policy. Bytes on disk are unchanged.
(Keel)

### Behavior changes

- `deadreckon verdict latest` is now scope-bound like every other verb. It
  previously searched every project on the machine. `verdict --all` already
  means "compare several recent runs", so there is no widening flag for it yet;
  use `deadreckon list --all` to find the id and pass it explicitly. Tracked in
  `docs/V1-CANDIDATES.md`.
- Refusals for a wrong-kind reference now name a verb that accepts it
  (`deadreckon show <id>`) instead of `deadreckon list`.
- `deadreckon doctor` prints at most three secondary actions plus a `help-all`
  pointer, down from ten.

## Shakedown (stable) — one reference resolver — 2026-07-24

Every id-taking verb hand-rolled its own resolution cascade, and no two covered
the same kinds in the same order, so the five commands the README calls "the
whole tool" contradicted each other. `deadreckon status` refused and pointed at
`deadreckon list`; `list` printed plan ids and recommended `status latest`; and
`status <that id>` answered `not found: run 0c11f68e` — false, because the id
existed and was simply a plan. One `resolve_ref` now answers "what does this
reference name?" for all 18 id-taking verbs, `latest` means one thing, and a
cross-verb journey test pins the invariant a per-verb audit structurally cannot
express.

Every implementation phase passed `make verify`:

- P1 (`e402b12`): added `commands/reference.rs` with `ResolvedRef`, the probe
  order and both ambiguity refusals; moved `resolve_plan_id` and
  `resolve_plan_child_ref` out of `main.rs` without changing a call site.
  Revised the rider's first-match-wins probe order — it would have resolved a
  prefix matching both a run and a plan silently to the run.
- P2 (`292cd68`): collapsed `latest_run` (scope-bound) and
  `resolve_latest_run` (all-scopes) into one rule that resolves to whatever
  `list` puts at the top. `verdict latest` became scope-bound as a result.
- P3 (`7fcc4c6`): added the refusal table and `VERB_REF_SPECS`. Every kind is
  now probed regardless of `accepts`, so a plan id refuses as a plan instead of
  being reported as a missing run.
- P4 (`e006eb1`): rewired `status` across every kind, reusing `show`'s
  renderers. Closes the reproduction.
- P5 (`84655e2`): `verdict` and `report` refuse by kind and name `show`; fixed
  a regression where routing through `probe_run` dropped the `try:` footer from
  ambiguity refusals.
- P6 (`5bc6c0d`): `show` and `attach` moved onto the resolver with the
  characterization goldens unchanged; `show` gained chains.
- P7 (`16068ad`): `kill` gained plan-child refs; `steer`, `resume`, `undo`,
  `rewind` and `merge` refuse by kind. `finish`/`doc` kept their plan-to-result-
  run mapping, which is a feature rather than a guessing cascade.
- P8 (`da7be1a`): deleted `load_cli_run`, `load_cli_run_with_scope`,
  `latest_run`, `resolve_verdict_run` and `resolve_latest_run`. Removed
  `RefQuery::accepts` so the acceptance table is load-bearing rather than
  test-only; the honesty test for it immediately found four verbs silently
  defaulting.
- P9 (`b0e38eb`): `list` shows one row per plan with children folded beneath,
  showing the task subject instead of the launch prompt they were handed.
- P10 (`b23d593`): capped secondary actions at three in
  `VerdictSurface::try_new`, with a `help-all` pointer when any were dropped.
  `doctor` went from ten to three. The cap then exposed two surfaces that had
  never had to prioritise: a paused chain listed its recovery actions last and
  lost `undo`, and `detect` was carrying per-provider install commands as
  actions rather than as evidence.
- P11: documented AS-BUILT §56, corrected the per-verb blind spot note in
  `FRIENDLINESS-AUDIT.md`, and recorded V1 boundaries.

## Keel (stable) — one protocol crate under the ledgers — 2026-07-20

- `deadreckon-protocol` owns every persisted ledger line type as one tagged
  `LedgerItem` vocabulary with generated, drift-tested JSON Schemas and a
  single persistence policy module; writers and readers were rewired without
  changing behavior — file layout and bytes are unchanged.

Every implementation phase passed `make verify`:

- P1 (`41e5b40`): added the pure protocol crate, transparent ID newtypes and
  a dependency-law guard that rejects internal crate dependencies.
- P2 (`70dbe64`): moved `RunEvent` and recorded the five pre-Keel fixture
  ledgers that pin the existing wire bytes.
- P3 (`e96c491`): moved `SpendRecord` and `TraceRecord` with legacy-field and
  unknown-field tolerance intact.
- P4 (`4cd0dac`): moved flight event types and added the pointer-only
  `NarrativeSnapshotRef` vocabulary.
- P5 (`ae857dc`): added the tagged `LedgerItem` union, transparent per-file
  line wrappers, `LedgerFile` mapping and unknown-kind tolerance.
- P6 (`50f2f9e`): centralized persistence routing and gate-nonce redaction in
  the pure protocol policy.
- P7 (`6884bf8`): generated and checked in the union and per-kind JSON Schemas
  with exact-set and drift-failing tests.
- P8 (`9e60d35`): rewired RunView, attach and history readers to import the
  protocol vocabulary directly with characterization goldens unchanged.
- P9 (`31893bf`): routed all five writer paths through policy, removed the
  temporary core re-exports and proved fixture output byte-identical.
- P10 (`fdb2da2`): documented schema paths in CLI help, added regeneration
  guidance and checked the `report --json` projection against its generated
  schema.
- P11: documented AS-BUILT §52, updated the shipped/RunView architecture, and
  recorded only the rider's V1 layout and export boundaries.

## Rudder (stable) — steer the running child — 2026-07-16

- `cli:codex-server` drives codex over its app-server: operator steering
  (`deadreckon steer` / `:steer`) with a durable at-least-once inbox,
  interrupt-before-kill, and capability-answered approvals replacing the
  danger-full-access inversion; server loss degrades to the exec route.

Every implementation phase passed `make verify`:

- P1 (`731c907`): added the JSONL JSON-RPC client, initialize handshake and
  explicit opt-in `cli:codex-server` route with exec fallback.
- P2 (`e4df6b9`): supervised the app-server child, registered its PID and
  guaranteed cleanup on provider drop.
- P3 (`0a5bc0e`): persisted app-server thread identity, route and PID in the
  existing per-run `provider-session.json` sidecar.
- P4 (`be439d2`): completed real app-server turns with answer and token-usage
  extraction, active-turn tracking and non-fatal unknown notifications.
- P5 (`ab8d0b3`): added the durable append-only `steer-inbox.jsonl` ledger and
  effective pending/delivered fold.
- P6 (`f86fdff`): added `deadreckon steer` with live-run and route validation,
  actionable refusals and pending-entry append.
- P7 (`14f6402`): delivered pending and mid-turn steers with
  `expectedTurnId`, marking them delivered only after a matching reply.
- P8 (`a8319d1`): answered command and file approvals from the run's existing
  capability posture and appended one audit trace for every decision.
- P9 (`0ca9328`): mapped normal kill to interrupt, grace and process kill;
  app-server loss now degrades to exec and leaves undelivered steers visible.
- P10 (`efc22c3`): added run-only Helm `:steer`, run help/footer affordances,
  plain-attach inbox state and pending-steer spine attention.
- P11: documented AS-BUILT §51 and the remaining V1 boundaries.

## Pennant (stable) — contracts as descriptor data — 2026-07-16

CLI provider wire contracts can now live in descriptor TOML. `[contract]`
declares structured-output arguments, JSON pointers, resume arguments and
capability probes. Pi and Copilot now report real available usage, extract
answers, resume sessions and stream live flight events. Gemini and OpenCode
remain explicit evidence-backed gaps rather than guessed contracts. A new
compatible agent CLI needs a descriptor edit plus recorded fixtures. Every
phase passed `make verify`:

- P1 (`592fec3`): added the validated `[contract]` schema. A malformed section
  warns, drops only the contract and keeps the provider usable.
- P2 (`4417505`): added tolerant JSON Lines and JSON document pointer
  extraction, with first-session and latest-terminal resolution rules.
- P3 (`7fdd0e4`): bridged descriptor contracts into Semaphore's shared
  machinery and added cached `--help` capability probes.
- P4 (`619af8a`): made the generic CLI driver honour stream arguments, answers,
  usage, cost, failures, flight rows and degraded raw-output fallback.
- P5 (`32de8a5`): added provider-scoped descriptor sessions and resume argument
  substitution, including the inherited one-time fresh retry.
- P6 (`a9913af`): onboarded Pi 0.79.1 from real fixtures, including input/output
  tokens, reported cost, answer extraction, session resume and the recorded
  tool-event shape.
- P7 (`720a112`): onboarded GitHub Copilot CLI 1.0.45 from real fixtures. The
  installed binary emits JSON Lines, output tokens, answers and session IDs.
- P8 (`570ddb7`): recorded the Gemini CLI 0.42.0 gap. Its installed credentials
  fail before structured output, so the descriptor remains contract-less.
- P9 (`844c455`): recorded the OpenCode CLI 0.15.5 event-model gap and removed
  its rejected `--dangerously-skip-permissions` argument.
- P10 (`a7cd2bf`): added nested tool-event extraction, live-flight dedupe,
  descriptor token rendering, `contract=yes|no` listings, warning output and
  the actionable `providers check` recovery command.
- P11: documented AS-BUILT §55 and the remaining V1 boundaries.

## Semaphore (stable) — read the CLI agents' signal flags — 2026-07-15

cli:codex and cli:claude-code read their structured contracts instead of
scraping raw stdout: real token usage, per-run conversation resume, live
flight ingestion, answers from the structured result, and schema-constrained
output where the binary supports it; unparseable contracts degrade with a
caveat instead of failing the turn. Landed in phases, each `make verify` green:

- P1 (2526f27): per-binary capability probes for both wire contracts, cached
  and parsed from `--help`; absent flags disable features instead of erroring.
- P2 (b0a88f0): codex `exec --json` event mirror over a provider-neutral `CliStreamEvent`
  vocabulary; unknown `type` tags degrade to Unknown, non-JSON lines skip as
  garbage, tool items lift into flight rows.
- P3 (e8d7e7c): claude `-p --output-format stream-json` event mirror, grounded on
  fixtures recorded from the real binary; the `result` line yields session id,
  usage, reported cost, and answer at once; `rate_limit_event`/hook noise
  degrades to Unknown.
- P4 (d152363): per-run `provider-session.json` (schema 1, provider-scoped, atomic) with
  a resume-failure counter that forces a fresh conversation; `ProviderRequest`
  gains `session_dir`/`output_schema`; the id is a file, not a PipelineState
  field.
- P5 (81f79f4): the codex driver reads its contract — `--json` stream folded tolerantly,
  real token usage from `turn.completed`, the answer from `--output-last-message`
  (raw stdout only in degraded mode with a `provider.contract.degraded` caveat);
  parsed tool rows ride the response trace for live flight ingestion.
- P6 (1c44745): the claude driver reads `--output-format stream-json --verbose` — real
  token usage and the answer from the `result` event, `is_error` maps to a
  provider error, and the reported `total_cost_usd` lands in the trace detail
  only (spend stays subscription/$0). Shared degraded fallback with codex.
- P7 (c1fdf9d): per-run conversation resume for both — `exec resume <thread>` /
  `--resume <session>` after turn 1 persists the id; distinct runs never share
  a conversation; a vanished conversation retries fresh once and records a
  `provider.session.reset` caveat.
- P8 (f537587): tool rows parsed from the stream ingest into the flight ledger live; a
  descriptor `[ingest] live_contract` flag makes the post-hoc file scraper
  yield for that provider, so the two never double-count.
- P9 (3542d18): `ProviderRequest.output_schema` becomes codex `--output-schema` where the
  binary is probed capable (a caveat elsewhere); the turn loop threads the run
  root as `session_dir`, and the spend ledger records real CLI tokens per turn
  for both providers.
- P10 (0c5f41f): `show`/`report` render CLI token usage on the subscription surface; a
  degraded contract raises an attention notice instead of being silently
  swallowed; `show <run> --raw provider-session` dumps the conversation record.
- P11: architecture (AS-BUILT §50) and deferrals (V1-CANDIDATES) documented.

## Contract review interstitial + launch surface dedupe — 2026-07-09

Closes the gap a real slack-clone run exposed: the accept / re-prompt / edit
review never fired for a contract drafted mid-start.

- a freshly drafted done contract now pauses for review before anything
  launches: after `start`'s def-done flow compiles the contract, the operator
  gets keep / view / check / update / cancel — "update" re-drafts through the
  compiler and re-reviews (the re-prompt loop), "cancel" stops the launch.
  Zero-questions is preserved for previously accepted contracts and for
  `--yes`/`--quiet`/replay paths, which still surface divergence.
- divergence lines always carry a remedy: `divergence: uncovered goal
  clause(s): ...` is followed by `try: deadreckon def-done refine "also
  verify: ..."` so the gap names the command that closes it.
- the orchestrate launch surface says each fact once: the preflight no longer
  duplicates provider/workspace/done-contract rows under two label
  vocabularies, and `started orchestration` prints only the launch delta
  (children, plan path, events path, watch handle) instead of repeating the
  ~40-line preflight block.

## 0.6.0 — Mission control — 2026-07-08

Three milestones ship together, plus completion polish found by real use:

- **Helm** — attach is mission control: a uniform five-question status spine
  on every surface (alive? doing what? on track? anything wrong? what next?),
  a flattened campaign -> plan -> run voyage tree with zoom-free comprehension,
  an event-driven input loop with pinned (and now config-tunable) latency
  budgets, k9s-style `:` command mode, in-frame modals, `w`-for-why cited
  evidence, a scrubable turn timeline, chain narrative parity, and a
  motion-policy effects layer. Attach now round-trips: Enter on a zoomed run
  node opens the full run surface and b/Esc pops back to the plan.
- **Contract** — a definition of done you can trust before spending: the done
  contract is compiled from the run goal, forced toward behavioral checks
  (every check falsifiable; keyword-only scans and `--if-present`-only gates
  rejected), linted deterministically, critiqued by one clamped provider
  pass, and shown for accept / re-prompt / edit on the Course card
  (`--review-done`, `[start] confirm_contract`).
- **Logbook** — run inspection that agrees with itself: one `RunView` read
  model behind `show`, `verdict`, `doc`, the new static `deadreckon report`,
  and attach's projections; snapshot-backed full-run and per-turn diffs
  (`show --diff/--turn/--raw`); every run artifact has a reader; parity
  depth tests and characterization goldens pin the projections together.
- **Orchestration completion polish** — the merged run carries the operator's
  goal verbatim (no more `./merge-orchestration-plan` folders), the guided
  start footer recommends `finish` when the work is already done, `status` on
  a merged run rolls up the children's real spend/wall/turns, and export dirs
  trim to word boundaries.

No `PipelineState` schema changes. See the milestone sections below for
per-phase detail and AS-BUILT §47 (Helm), §48 (Contract), §49 (Logbook).

## Attach plan-run navigation + audit remediation — 2026-07-08

Closes the gaps a post-hoc audit of the Helm/Contract/Logbook slices found,
and makes attach navigation bidirectional:

- attach now round-trips between the plan surface and full run surfaces:
  Enter on a zoomed run node in the voyage tree promotes to the complete run
  frame (narrative, visual map, live files, timeline, spine), and
  b/Backspace/q/Esc there pops back to the plan surface instead of detaching
  to the shell — the footer's "back to plan" is now literally true, from
  plan attach, `attach <plan>:<task>`, and campaign drill-in alike. Detach
  from the plan surface as before.
- Logbook P9 landed for real: the run surface's narrative cold path builds
  from the shared `RunView` (the loop already did), and new parity depth
  tests pin attach's timeline turn count and why-panel evidence to
  `RunView.turns` / `RunView.proof`, plus the attach characterization golden
  under its rider name.
- the five missing Logbook golden-parity guards now exist: `show`, `verdict`,
  and `doc` default outputs are characterization-pinned (with normalization
  for build-dir noise, ISO timestamps, and latencies), `doc` provably sources
  narrative/decisions paths from `RunView.why`, and `report`'s footer names
  exactly one primary action.
- `[ui] input_latency_budget_ms` exists as the Helm rider prescribed: the
  attach input-to-frame budget (default 32ms) is operator-tunable, clamped
  to 8..=1000ms, honored by every attach loop (run/plan/campaign/chain).
- CHANGELOG sections reordered newest-first (Helm/Contract/Logbook had landed
  below the 0.5.0 release section they postdate).

## Orchestration completion polish — 2026-07-07

Fixes four wonky moments in the full-plan orchestration completion experience,
found by a real run:

- The synthetic run that lands an orchestration's result no longer carries an
  internal label as its goal. It was created with `goal = "merge orchestration
  plan <goal>"`, which surfaced as a confusing goal line in `status` and — via
  `task_key` — as a suggested export dir named `./merge-orchestration-plan`. The
  merged run now carries the operator's goal verbatim.
- The guided `start` completion footer recommends the right next action. A
  synchronous full-plan returns already complete, but the footer always said
  "attach to observe the launched work"; it now recommends `finish` when the
  work is done and keeps `attach` only while it is still in flight.
- `deadreckon status` on a merged run no longer reads as broken. The merge run
  does no provider work, so its own spend/wall/turns are ~0; status now adds a
  `merge:` line naming the source plan and a `rollup:` line with the child runs'
  real spend, wall time, and turns (the orchestration's true cost).
- Suggested and default export directories trim to a word boundary
  (`./build-a-comprehensive`) instead of a mid-word cut
  (`./build-a-comprehensive-fi`).

## Logbook (stable) — run inspection that agrees with itself — 2026-07-04

- added the core `RunView` read model so `show`, `verdict`, `doc`, `report`,
  and attach narrative projections assemble run identity, verdict/signature,
  sandbox facts, spend, changed files, turn records, proof files, and missing
  artifacts from one picture.
- added snapshot-backed full-run and per-turn diffs (`show --diff`,
  `show --turn <n>`) that exclude build output and deadreckon internals, plus
  `show --raw <artifact>` for stable artifact reads with gate-secret refusal.
- added `deadreckon report <run-id>` as a static Markdown/HTML/JSON run
  report over verdict, changed files, why/docs, turns, and proof, refusing
  live runs with an attach hint.
- extended `history grep` with `--kind events`, and routed verdict signature /
  changed-file facts plus doc path resolution through `RunView`.

## Contract (stable) — a definition of done you can trust — 2026-07-04

- the done contract is compiled from the run goal, forced to test behavior
  (build/start/drive/assert; known input -> known output; every check
  falsifiable; keyword-only scans and --if-present-only gates rejected),
  checked by a deterministic falsifiability lint plus one clamped critic
  pass, and shown for accept / re-prompt / edit before launch on the Course
  card. Closes the acceptance scope-drift and stub-passable-gate gaps.

## Helm (stable) — mission-control attach — 2026-07-03

- attach is mission control: uniform five-question status spine, flattened
  campaign -> plan -> run tree with optional zoom, event-driven input loop with
  pinned latency budgets, `:` command mode, in-frame modals, cited `w` why
  evidence, scrubable turn timeline, chain narrative parity, and a
  motion-policy effects layer.
- H-P1 (`b2a6a48`): render decomposition moves attach surface entry points into
  `tui/surfaces/{run,plan,chain,campaign}.rs` and routes shared chrome through
  `tui/panes/{header,activity,narrative,docs,footer}.rs`; characterization
  goldens and the public surface remain unchanged.
- H-P2 (`912d311`): `tui::spine` adds the `SpineSnapshot` read model, 5x4
  contract table, run/plan/chain/campaign builders over durable files, and
  depth tests for event-age aliveness, single primary action, launch-plan budget
  ceilings, and inert reshape proposals.
- H-P3 (`9238a65`): attach renders the shared status spine band across run,
  plan, chain, and campaign surfaces; plain and off-TTY attach summaries print
  the same five spine answers, including paused-run attention and next action.
- H-P4 (`8c39e10`): `tui::tree` adds the pure TreeModel read model for run,
  plan, chain, and campaign roots, builds campaign/sub-goal/task/run trees from
  durable state files, and folds existing run/plan/chain/campaign feed events
  without rebuilding the tree.
- H-P5 (`473c3b0`): attach renders the Helm voyage pane with status glyphs,
  gate counts, spend, display-width-safe labels, and selection state; plan and
  campaign surfaces gain the left tree column while single-run attach collapses
  the one-node tree into the header band.
- H-P6 (`7464091`): Helm selection drives the detail pane for task and run
  leaves, Enter zooms in-frame with a breadcrumb, Esc backs out before detach,
  and campaign leaf runs expose status, activity, gate, and spend without any
  drill-in.
- H-P7 (`9366f44`): attach loops use crossterm `EventStream` with adaptive idle
  backoff instead of fixed 200/250ms polling, record input-to-frame latency
  against a dedicated budget, and keep run, plan, chain, and campaign input
  responsive without blocking redraw cadence.
- H-P8 (`e219abe`): a headless JSONL event-storm replay now proves frame
  coalescing, input-to-frame latency stays inside budget, and attach JSONL
  tails retain a bounded row window; the macOS release-size guard is
  rebaselined to the verified EventStream-enabled binary.
- H-P9 (`916d7ed`): chain attach now keeps destructive confirms and step-goal
  input inside the ratatui frame, with modal key swallowing, Esc cancel,
  quiet command dispatch, and the ratatui-0.29-compatible `tui-textarea`
  widget ready for command-mode input.
- H-P10 (`9dca498`): chain attach now opens command mode with `:`, dispatches
  fixed existing verbs through in-frame input and confirm flows, surfaces
  unknown commands with nearest `try:` guidance, and routes `:reshape` through
  the Course reshape command path.
- H-P11 (`cddec12`): run attach gains the cited `w` why panel and `attach
  --why` plain parity, with deterministic causes from state, gate progress,
  tamper proofs, and provider traces; every rendered cause carries an artifact
  path and bounded excerpt.
- H-P12 (`a3c7b3e`): run attach gains a Helm timeline band from provider
  checkpoints, spend rows, gate/tamper proof files, and reshape traces; `t`
  focuses the band, Left/Right scrubs turns, and the detail pane shows the
  selected turn story with checkpoint IDs and diff counts.
- H-P13 (`db1e9e6`): chain attach gains a supported narrative view and
  `attach --view narrative` plain/json parity; the chain step pane now
  participates as the Helm voyage view while preserving step status, activity,
  spine, and in-frame command flows.
- H-P14 (`fbc50b5`): Helm motion policy lands with `[ui] motion`,
  `:motion full|reduced|off`, and a bounded tachyonfx-backed registry for the
  three supported triggers: gate pass shimmer, verdict/completion flash, and
  node state glyph pulse. Reduced motion keeps only completion effects for
  non-TTY/replay defaults; off renders zero effect frames.
- H-P15 (`6ce444e`): Helm discoverability is polished with sectioned `?`
  overlays, a command-mode verb reference, focused-pane footer hints, and a
  first-session footer cue for panes, why, and commands.
- H-P16 (`5bf9dfb`): AS-BUILT §47 documents Helm mission-control attach,
  updates cross-references and V1 deferrals, logs widget/effect dependencies,
  and adds the Helm demo cast.

## 0.5.0 — The harness plots the course — 2026-07-03

Makes `deadreckon start` the only launch decision a user needs. One milestone
ships:

- **Course launch planning** — the operator no longer picks the execution shape
  (run / orchestrate / campaign) and hopes `start` softens it. A deterministic
  SignalBundle (goal structure, the DETECTED done contract, workspace shape,
  task history, budget fit) plus one clamped provider call resolves a typed,
  durable `launch-plan.json` that lands in every dispatched root; a golden-pinned
  course card previews WHAT/WHO/COST/DONE/WHY/ESCAPE; the only question `start`
  may ask is "How will you know it worked?" (and only when the contract is
  undetected). Campaign is never auto-chosen and never auto-accepted above the
  confirm line; plans replay (`--plan`), state-changing launches gain JSON parity
  (`--json --yes`), one-piece decompositions collapse to runs, and workers can
  propose reshapes that stay inert until `deadreckon reshape` accepts them.

No `PipelineState` schema changes — the plan is a file, not fields. See the
milestone section below for the per-phase detail (C-P1…C-P14) and AS-BUILT §46.

## Course (stable) — the harness plots the course — 2026-07-02

`deadreckon start` becomes the only launch decision a user needs. A
deterministic SignalBundle (goal structure, the DETECTED done contract,
workspace shape, task history, budget fit) plus one clamped provider call
resolves a typed, durable `launch-plan.json` that lands in every dispatched
root; a golden-pinned course card previews WHAT/WHO/COST/DONE/WHY/ESCAPE;
the only question `start` may ask is "How will you know it worked?" (and
only when contract detection is Unknown); campaign is never auto-chosen and
never auto-accepted above the confirm line; plans replay (`--plan`),
launches gain JSON parity (`--json --yes`), one-piece decompositions
collapse to runs, workers can propose reshapes that stay inert until
`deadreckon reshape` accepts them, and `[defaults] start_attach` makes
start-then-watch one motion. Closes the campaign/chain auto-detect
friendliness cells. See AS-BUILT §46.

- Fix: the launch planner's timeout is now route-aware — 30s for `cli:*`
  providers (a cold `claude -p` needs ~10-15s; the old 5s HTTP-tuned ceiling
  guaranteed a silent ladder fallback on every CLI-routed launch), 5s for
  HTTP routes. Found by a real launch whose planner returned an excellent
  3-piece plan 8 seconds after the harness had stopped listening.
- C-P14: friendliness closure (campaign/chain auto-detect cells flip to
  pass with the planner-seeded `--n` and ladder continuation), AS-BUILT §46
  (Course: Launch Planning and Reshaping) + §22 shipped entry, and Course
  follow-ups logged in V1-CANDIDATES.

- C-P13: start-then-watch — `[defaults] start_attach = true` drops an
  interactive launch straight into attach after the lifecycle footer; JSON,
  quiet, preview, and non-TTY sessions never auto-attach, and a failed
  attach can never turn a successful launch into an error.
- C-P12: reshape proposals — a worker can emit a `reshape` action proposing
  2-6 independent pieces; the loop records an INERT `reshape-proposal.json`
  (launch-plan schema, parent set, no acceptance) plus a `reshape.proposed`
  trace and keeps working — non-terminal, never self-executing. The new
  `deadreckon reshape <id>` verb previews the proposal on the course card
  and, only on explicit acceptance (card sail or `--yes`; non-TTY refuses
  with `try:`), dispatches a full-plan orchestration with the parent run
  recorded in the dispatched plan's launch-plan.json.
- C-P11: de-escalation — a decomposition of exactly one piece collapses to a
  single run instead of inflating to n=2 or refusing; the collapse is
  recorded in the plan's clamp trail, which rides `launch-plan.json` into
  the dispatched root as the durable audit record.
- C-P10: replay + launch JSON parity — `start --plan <file>` validates,
  re-clamps against `--max-spend` (a plan whose budget exceeds the cap
  refuses naming both numbers), stamps the resolution as `replay`, and
  dispatches the identical shape; `start --json --yes` launches quietly and
  emits one machine envelope (kind:launch, the plan, dispatched ids, next
  actions) — state-changing launches finally have JSON parity; `start` gains
  `--max-spend`, threaded into every dispatch arm.
- C-P9: dispatch reads the plan — start builds `launch-plan.json` from the
  resolved decision before anything runs and the accepted plan lands in the
  dispatched root (run root at creation, plan/campaign dir on launch, extend
  target on success); direct `deadreckon run` records a trivial operator
  plan so every root carries the decision record however the launch began;
  `accepted_by` distinguishes operator confirmation from `--yes` guardrail
  acceptance.
- C-P8: the one-question flow — a Polyglot-detected contract answers "done"
  with zero questions (new `detected` source, interactive or not); an unknown
  tree asks exactly one question, "How will you know it worked?" (one line
  compiled through the existing def-done flow as the `asked` source; Enter
  accepts the default gate); `--yes` skips the question and proceeds with the
  caveat on the label; non-TTY without `--yes` keeps the def-done refusal,
  aligned with the accept matrix. The old four-choice done menu (and its
  generate-from-goal variant) is gone — the ask subsumes it.
- C-P7: the course card — one calm launch surface rendering
  goal/shape/pieces/who/cost/done/why/escape through the shared Card
  primitives, golden-pinned (whitespace is spec), with the sail/edit/
  force-single/abort interaction driven through the existing prompter seam;
  forcing single records the operator override in the plan's clamp trail.
- C-P6: the launch accept matrix — campaign above the confirm line ALWAYS
  confirms interactively or refuses in non-TTY (no flag overrides the
  guardrail; an unbounded ceiling counts as above the line); `--yes`
  auto-accepts only when confidence clears the floor and the ceiling is
  under the auto-spend line; a non-TTY launch without `--yes` refuses with
  `try:` instead of hanging.
- C-P5: the provider planner supersedes the text-only goal-shape classifier —
  one bounded call whose prompt embeds the SignalBundle (contract, workspace,
  history, budget, text analysis), whose typed draft (pieces + confidence +
  rationale) is clamped against the ladder (confidence-floor downgrade,
  budget-fit downgrade, n and piece truncation, every clamp recorded), and
  whose failure silently falls back to the ladder — a planner can never fail
  a launch. Deterministic campaign classification is gone by doctrine; the
  proven parallel-workstreams keyword heuristic survives as ladder rule 2.5,
  owned by course so the auto-mode heuristic and the floor cannot drift.
- C-P4: the deterministic ladder — seven ordered rules resolve a shape with
  zero provider calls (continuation on verified history, budget-floor forcing
  single, decomposition+workspace or decomposition-alone yielding a clamped
  plan, default single). Campaign is structurally unreachable from the ladder
  (swept by a grid depth test); every decision records which rule fired.
- C-P3: the SignalBundle completes — `contract_signal` (the Polyglot detector
  as a launch signal, so `start` and the gate agree on "done" pre-spend),
  `history_signal` (prior runs by task key; verified history = continuation),
  `budget_signal` + feasibility floors (a shape the money cannot fund is
  never proposed), and `collect_signal_bundle` tying all five together.
- C-P2: the first half of the SignalBundle — `analyze_goal_structure`
  (enumerations, conjunction clauses, imperative verbs → a `strong`
  decomposability verdict) and `scan_workspace` (Cargo/pnpm/npm/go.work
  member detection + a capped tree-size bucket). Pure, total, provider-free.
- C-P1: `commands::course` module — the `LaunchPlan` schema-1 artifact
  (shape/pieces/providers/budget/contract/signals/resolution/escape, additive-
  tolerant serde), `launch-plan.json` load/save with schema check, and
  refusals with `try:` footers for missing/invalid/unsupported plan files.

## 0.4.0 — Honest verification — 2026-06-27

Makes "did it actually work?" answerable honestly, end to end. Two milestones
ship together:

- **Polyglot done-contract detection** — the acceptance gate is no longer hollow
  on non-Rust projects. A run with no operator `acceptance.yaml` now gets a real,
  detected test command for Node/Deno/Python/Go/Elixir/.NET/JVM/Ruby/PHP and
  Make/just/Task script-runners (with an optional approval-gated `--infer-contract`
  for unknown trees), so a `VERIFIED` on a non-Rust tree means something was
  actually checked.
- **`deadreckon verdict`** — a read-only "did it actually work?" report for ANY
  run, native or imported from Claude Code/Codex/aider. It re-runs the run's
  acceptance checks now and reports one of three honest states — `VERIFIED`,
  `REGRESSED`, `UNVERIFIED` — so a forged/tampered marker or a silently-broken
  build reads `REGRESSED` instead of a false `VERIFIED`. `--all` compares recent
  runs; `--json` is at parity.

No `PipelineState`/`AcceptanceMarker` schema changes. See the milestone sections
below for the per-phase detail.

## Verdict (stable) — did-it-actually-work report — 2026-06-27

Adds `deadreckon verdict`, a read-only verb that answers "did it actually work?"
for ANY run — native or imported from Claude Code/Codex/aider — by re-running its
acceptance checks now, reading (never overwriting) the signed marker, and
reporting one of three honest states: `VERIFIED`, `REGRESSED`, or `UNVERIFIED`. A
forged/tampered marker or a now-failing must-pass check reads `REGRESSED`, never a
false `VERIFIED`. `--all` compares recent runs, `--json` is at parity, and the
only write is an additive `proofs/verdict-<ts>.json` audit sidecar never read back
as authority. No `PipelineState`/`AcceptanceMarker` schema change; no run-state
mutation or promotion. See AS-BUILT §13.8 and §37.11.

- V-P2: `verdict <id|latest>` resolves a run by id/prefix or the most recent run
  across scopes; an unknown id or ambiguous prefix refuses with a `try:
  deadreckon list` footer, no runs at all with `try: deadreckon start`.
- V-P3: `verdict` re-runs the run's acceptance checks NOW through the gate's
  write-free `evaluate_acceptance_checks` (no spec, no progress, no state writes);
  a missing/cleaned working dir yields no checks and reads as "working dir
  unavailable" rather than a false pass.
- V-P4: `build_verdict_report` reads (never overwrites) the signed marker via
  validate_acceptance_marker and combines had_marker/marker_valid with the
  re-run through compute_verdict: a valid marker whose checks still pass is
  VERIFIED, a marker that no longer validates (forged signature) or whose checks
  now fail is REGRESSED, no marker is UNVERIFIED.
- V-P5: the report carries added/modified/deleted counts since the run's
  earliest snapshot, computed via the same `tamper::touched_files` diff the gate
  uses (empty when there is no snapshot).
- V-P6: a single-run `verdict` renders through `VerdictSurface` — one label
  (Verified→pass, Regressed→fail, Unverified→noop), an Explanation/Evidence
  panel (per-check pass/fail, changed-file summary, provenance line), and the
  one mapped next action (finish for Verified/Unverified-pass, resume otherwise).
- V-P7: `verdict --json` emits the inspection envelope (kind:verdict, id,
  status, checks, changed_files, source, had_signed_marker/marker_valid,
  next_actions, paths) — stable shape, per-check results included, non-TTY safe.
- V-P10: `verdict` caches each report to `<run_root>/proofs/verdict-<ts>.json` (an
  additive audit sidecar, never read back as authority — a stale cache can never
  mask a live regression), honors `--quiet`/`--plain`/`--json`, carries inspect
  and compare secondary actions that `--quiet` suppresses, and is registered in
  the friendliness contract as a read-only verb that never prompts or mutates.
- V-P9: imported runs (those with `import.json` in their run root) flow through
  `verdict` as `Unverified` with `source:"imported"` and fresh check results;
  `deadreckon import` completion now cross-links `deadreckon verdict <id>`.
- V-P8: `verdict --all [--limit N]` re-verifies the most recent runs into a
  compact one-screen table (run, verdict, checks pass/total, spend, goal);
  `--all --json` emits a stable array of per-run summaries.
- V-P1: new read-only `deadreckon verdict` verb (registered in the CLI) plus the
  `commands::verdict` module — `VerdictState` (Verified/Regressed/Unverified),
  `VerdictReport`/`VerdictSource`/`ChangedFiles` schema, and the pure
  `compute_verdict(had_marker, marker_valid, rerun_all_must_pass)` decision (a
  marker that no longer validates or whose checks now fail is Regressed, never
  silently Verified). Run resolution, live re-evaluation, rendering, `--json`,
  `--all`, and the sidecar cache land in later phases.

## Polyglot — Default done-contract detection — 2026-06-25

Makes the acceptance gate non-hollow for non-Rust projects. A run with no
operator `acceptance.yaml` in a Node/Deno/Python/Go/Elixir/.NET/JVM/Ruby/PHP or
script-runner (Make/just/Task) tree now detects the project, compiles a real
test contract, writes the generated spec for audit, and keeps tamper coverage
honest for shell test commands (deleting a JS/Py/Go test refuses like a deleted
Rust test). Optional `deadreckon run --infer-contract` proposes a contract for
an unknown tree that the operator must approve before it arms the gate — the
deterministic floor stays the only unattended marker-signer. No `AcceptanceCheck`
schema change. See AS-BUILT §13.1 / §35.9.

- P10: friendliness — `deadreckon run` previews the resolved done-contract and
  its source (detected/operator/inferred), `--preview` prints a full
  project-kind + contract report, an Unknown tree surfaces a "no test contract
  detected" caveat (no silent green), and a detected-but-unrunnable tree
  (package.json without a test script, or pyproject with no visible tests)
  refuses with a `try: … --acceptance … (or --infer-contract)` footer instead
  of running a hollow gate. (A standalone `detect` report command is deferred —
  the `detect` verb is already the provider probe; see V1-CANDIDATES.)
- P9: optional `deadreckon run --infer-contract` — for an Unknown project tree
  with no operator `acceptance.yaml`, a cheap model PROPOSES a test command the
  operator must approve before it arms the gate. The approved spec is written
  with a `# proposed by deadreckon --infer-contract (approved …): <model>`
  header. It is a no-op under `--yes`/`--quiet`/non-TTY (a model proposal can
  never define "done" unattended); no provider / low confidence falls back to
  the deterministic caveat. The deterministic floor stays the only unattended
  marker-signer.
- P1: new `deadreckon-core::acceptance_defaults` module — `ProjectKind` /
  `PackageManager` / `BuildTool` / `RubyRunner` / `PhpRunner` / `Runner` /
  `ContractSource` enums and the pure, total `detect_project_kind` (Rust +
  Unknown for now), `default_checks_for`, and `detection_caveat`. No call-site
  change yet; later phases fill the detection table and wire it into the gate.
- P8: deleting a covered non-Rust test now refuses like a deleted Rust test —
  `evaluate` scans the earliest snapshot for conventional test files when a
  shell test-runner check is present (the deleted file is gone from the
  post-run tree); suppression lint gains `--passWithNoTests` and a trailing
  `exit 0`. `classify` was already language-uniform.
- P7: tamper `check_coverage` now recognizes cross-language test-runner shell
  commands (npm/pnpm/yarn/bun test, pytest, go test, deno/mix/dotnet test,
  mvn/gradle/gradlew test, rspec/rake, phpunit/composer test, make/just/task
  test, jest/vitest) and maps the ecosystem's conventional test files
  (`tests/`/`test/`/`spec/`, `*_test.go`, `*.test.*`, `*.spec.*`, `*Test.cs`,
  `test_*.py`, …) to `Test` coverage.
- P6: the dr-gate no-spec path (`evaluate_default_acceptance`) routes through
  `default_checks_for` too, so the standalone binary and the in-process compile
  agree byte-for-byte; the Rust-only special case is gone (Node now attempts
  the real test command, not a hollow FileExists).
- P5: `compiled_acceptance_checks` detects the project kind when there is no
  operator spec, compiles the real default, and persists it to the run's
  `acceptance.yaml` with a `# generated by deadreckon detect: <kind>` provenance
  header (auditable). An operator spec wins verbatim and is never overwritten;
  the generated spec round-trips through `parse_acceptance_checks`.
- P4: `default_checks_for` compiles each non-Rust kind to a real `Shell` test
  check (cwd set to the working dir); `Unknown` keeps `FileExists`.
- P3: `detect_project_kind` resolves the ambiguous kinds — Python (sentinel AND
  visible tests; a bare `pyproject.toml` degrades to Unknown), JVM (Maven over
  Gradle, Gradle preferring `./gradlew`), Ruby (rspec when `spec/`+rspec in
  `Gemfile.lock`, else rake), PHP (composer script vs phpunit) — and the
  precedence rules (native rows beat script-runners; lower row wins).
- P2: `detect_project_kind` resolves the single-canonical-command kinds — Node
  (`scripts.test` present; package manager from `bun.lockb`/`pnpm-lock.yaml`/
  `yarn.lock`/npm), Deno, Go, Elixir, .NET (`*.csproj`/`*.fsproj`/`*.sln`) — and
  the deterministic `test`-target scan for Make/just/Task script-runners. A
  `package.json` without a test script degrades to Unknown+caveat.

## 0.3.1 — Narration actually narrates — 2026-06-22

Found by the first real end-to-end campaign run (every prior check was a unit
test): the live narrator wrote zero beats in real runs, and campaign
sub-orchestrators crashed on `--narrate`. Three fixes, each with a regression
test that reproduces the failure:

- fix(narrator): the live narrator never wrote a beat in real runs. `emit()`
  awaited a model call inline in the engine loop; for short runs that call was
  still in flight at shutdown, the 5s grace timed out, and the runtime aborted
  the task before any beat was committed. The engine's cancel token is now
  threaded into the `ProviderRequest`, so shutdown interrupts the in-flight call
  and `emit()` falls through to a floor beat. This affected all narration,
  including `dr run --narrate` from 0.2.0.
- fix(narrator): on shutdown, buffered `DocsCheckpoint`/`RunCompleted` events
  are drained (window-fill only, no blocking model call) before the final floor
  flush — `cancel` and a non-empty `recv()` race in the `select!`, and if cancel
  won, a fast run produced zero beats.
- fix(cli): `orchestrate full-plan` and `orchestrate review` now accept
  `--narrate/--no-narrate/--narrator-model`. They were only on the bare
  `orchestrate` command, so every campaign sub-orchestrator (`orchestrate
  full-plan … --narrate`) exited with "unexpected argument '--narrate'". The
  propagation test now parses the real sub-orchestrator argv through clap.
- feat(campaign): campaign-parent live aggregate. With `dr campaign --narrate`
  the parent publishes each sub-orchestrator's plan id early, tails that sub's
  grandchild leaf runs, and prints one framed line per active sub-goal to
  stderr (`campaign <id> · sub-i (i/N) <status> · <freshest descendant beat>`).
  The sub-orchestrator's own per-task aggregate is suppressed under a campaign
  so the campaign parent owns the live surface.
- fix(runtime): truncate flight summaries on char boundaries (thanks
  @jakelevirne, #1). `truncate_summary` used `MAX_SUMMARY_CHARS` (a character
  count) as a byte index, so provider output where byte 240 landed mid-character
  (em dash, smart quote, emoji, box-drawing) tripped `String::truncate`'s
  `is_char_boundary` assertion and crashed the flight-recorder task — which under
  orchestration paused the whole plan. Now truncates via `char_indices().nth(N)`.

## 0.3.0 — Orchestrated Narration — 2026-06-17

Extends the Live Narrator (0.2.0, AS-BUILT §44) to every orchestrated and
campaign child. Children are subprocesses, so they got zero live beats; now
each narrates file-only to its own `snapshots.jsonl`, the plan attach surfaces
each child's live headline, `dr orchestrate --narrate` prints a one-line-per-
child stderr aggregate, and `dr attach <campaign> --view narrative` renders a
campaign projection at plan parity. See AS-BUILT §45.


- P1: shared `build_run_narration` helper (extracted from run.rs) + a
  `resolve_narrator_config_for_child` that narrates orchestrate/campaign
  children FILE-ONLY (foreground=false, headless_append=false) so beats hit
  `snapshots.jsonl` but never a child's stdout/stderr. Children default to the
  deterministic floor unless a `--narrator-model` is pinned; the `dr run`
  TTY contract is unchanged. The child path activates via the
  `DEADRECKON_NARRATE_CHILD` env the parent sets.
- P2/P3: both `extend` paths (in-place + worktree) now wire the narrator —
  reviewer children re-entering `extend` narrate file-only via
  `build_run_narration` (previously `event_sender:None`). `extend`/and the
  shared `resolve_narration` thread `--narrate/--no-narrate/--narrator-model`;
  shutdown is bounded and runs before lock release. The `dr run` TTY contract is
  unchanged.
- P4/P5: `dr orchestrate` gains `--narrate/--no-narrate/--narrator-model`,
  threaded through `fork_command` to each spawned child's argv (`run_plan_child`
  appends `--narrate` + sets `DEADRECKON_NARRATE_CHILD=1` and
  `DEADRECKON_AUTH_PROBE=0`), so coder/full-plan children narrate file-only to
  their own `snapshots.jsonl`. Child argv building is extracted into a pure
  `child_argv` so propagation is unit-tested without spawning.
- P7: `spend_summary` now counts only `kind:"loop"` rows — a latent leak where
  `kind:"narrator"` rows (written by any narrating run) inflated tokens/turns/
  wall and could overwrite the run's `total_usd`. The total is taken from the
  last loop row.
- P6: `dr campaign` gains `--narrate/--no-narrate/--narrator-model`, appended to
  the `orchestrate full-plan` sub-orchestrator argv (`build_sub_orchestrator_command`),
  so the campaign → orchestrate → run/extend chain narrates end to end.
- P10: campaign Narrative view (Option D2) — `dr attach <campaign-id> --view
  narrative` (and `--json`) now renders a campaign-scoped projection built by
  `build_campaign_projection`, aggregating each sub-goal's freshest child
  narration (its merged run's live beat, else its sub-plan's snapshot) into an
  agent table. It flows through the same `narrative_plain_lines` renderer as a
  plan, so the campaign narrative has full section parity. Adds a `Campaign`
  `NarrativeScope` variant (additive).
- P11: docs — AS-BUILT §45 (Orchestrated Narration) added; §44.5/§44.6
  corrected (`effective_plain` is unwired; the spend-math guarantee was only
  made real in §45.7); §22 shipped list updated; V1-CANDIDATES gains an
  Orchestrated Narration follow-ups section.
- P9: parent aggregate stderr line (Option D1) — when `dr orchestrate --narrate`
  is active the parent tails each running child's `snapshots.jsonl` (reusing
  `JsonlTail`) and prints one capped line per active child to STDERR every ~2s,
  preferring each child's latest Live beat. The aggregate never touches stdout
  (the parent scrapes children's run-ids off its own stdout), enforced by a
  test-threaded sink.
- P8: plan-attach surfacing reliability — the per-child agent table caps at
  `PLAN_AGENT_TABLE_MAX` active children with a `+N more` overflow line, and
  `latest_child_narrative_snapshot` now reads via `read_latest_live_snapshot`,
  which prefers the latest Live beat over a later attach-time Deterministic
  projection so an on-demand refresh can never mask a child's live headline.

## 0.2.0 — Live Narrator

A `dr run` now narrates itself in plain English as it works — a
continuity-carrying, subscription-first, model-driven sidecar with a
deterministic floor — so an operator can glance at progress instead of reading
tool calls, edits, and JSONL. Phase detail follows.


- P1: narration snapshot schema 2 — additive `live` beat field (beat_seq,
  covers_turn, source, rolling_summary) that legacy schema-1 snapshots parse
  as absent; `SpendRecord.kind` (defaulted "loop", narrator rows write
  "narrator"); additive `RunLoopConfig.narrate: Option<NarratorConfig>` (None
  preserves every existing constructor).
- P2: narrator backend selection — subscription-first preference
  (claude-code/haiku → codex/gpt-5.1-codex-mini → anthropic/claude-haiku-4-5 →
  openai/gpt-4o-mini → deterministic floor). Pure `select_narrator_route` over
  an availability predicate, plus a registry-backed predicate that gates CLIs
  on binary presence + login state and HTTP on a non-empty API-key env var.
  `--narrator-model` overrides the model without changing provider order.
- P3: the run process now wires a `RunEventBus` into the turn loop and spawns an
  in-process narrator sidecar that drains run events and stops cleanly when the
  run finishes or is cancelled. On a TTY narration is on by default; off-TTY
  without `--narrate` the run is wired exactly as before (no bus, no task).
- P4: continuity — `build_live_narrator_prompt` feeds the model the prior
  narrative + only the windowed new turns + a rolling summary and asks it to
  amend/extend; `apply_live_narrator_response` merges the reply into a NEW
  appended beat (never overwriting the prior beat) and validates claims against
  prior-citation + new-turn (`turn:N`) evidence, so beats may add genuine new
  claims but never cite a turn outside the window. New `skills/live-narrator`
  prompt skill carries the voice.
- P5: `NarratorWindow` accumulates only the turns since the last beat (never the
  full trace) and folds them into a rolling summary bounded to 1200 chars by
  eliding older content — so each beat's model input is a constant ceiling and
  total narration cost is O(turns), not O(turns²). `turn_record_to_input` maps a
  persisted `TurnRecord` into the per-turn narrator input.
- P6: cadence — `cadence_decision` emits a model beat only when there is new
  work and either the min gap elapsed or a turn burst accumulated, coalescing
  faster bursts and capping total beats per run; a long single turn escalates
  to a beat via the quiet timer. Between model beats a deterministic $0 ticker
  (`turn N · tool (elapsed)`) keeps a long turn from looking frozen, with no
  provider call.
- P7: narrator spend isolation — `NarratorLedger` tracks the narrator's own
  spend against its per-run budget cap, fully separate from the run loop's
  totals; `narrator_should_use_model` degrades to the deterministic floor once
  the cap is hit (or the backend is the floor); `narrator_spend_record` writes
  `kind: "narrator"` rows so the run's spend math (which filters `kind: "loop"`)
  never counts narration. Subscription backends record $0.
- P8: foreground calm block — `live_block_lines` renders the headline plus the
  top current_work claims bounded to `narrate_lines` (a few lines max, never a
  stream); `ForegroundBlock` redraws in place (clearing the prior block) so the
  block updates rather than scrolls. Foreground narration is on by default on a
  TTY and off under `--no-narrate`.
- P9: headless narration — `dr run --narrate` streams append-only, turn-stamped
  beats (`[turn N] …`) to stderr, keeping stdout clean for piped consumers;
  `--no-narrate` disables narration and `--narrator-model` pins the model
  (validated against the catalog). Raw cursor-control ANSI moved to the `ui`
  module to honor the source coherence contract. The silent-piped-run progress
  decision (`effective_plain`) is unit-tested but deliberately not wired to the
  run surface — this project keeps rich rendering when piped (opting out only
  via `NO_COLOR`/`--plain`), so `--narrate` is the piped-progress path and the
  auto-plain wiring is a V1 candidate.
- P10: attach + post-hoc convergence — the attach Narrative view renders the
  live beats the run already wrote to `snapshots.jsonl` with no provider call;
  the post-hoc `RUN-NARRATIVE.md` seeds `current_narrative` from the full
  accumulated live narration (digest of every beat + the latest rolling
  summary), consolidating the live story rather than re-deriving from the raw
  trace. `--narrator-model` is validated against the catalog and refused with a
  `try: deadreckon models` hint; conflicting `--narrate`/`--no-narrate` is
  refused.

## 0.1.1

- Subscription CLI runs default to a ten-hour wall cap (was one hour):
  the fallback in run/extend resolution and the `cli_max_wall_seconds`
  value written by `init` are now 36000 seconds. Explicit
  `--max-wall-seconds` and configured `defaults.cli_max_wall_seconds`
  still win.
- Failure surfacing: plan- and campaign-level refusals name the
  underlying child failure reason (session limits, wall caps) instead of
  only their own layer's status; provider quota errors surface as
  resumable with the provider's stated reset time; refused campaign
  roll-ups recommend resuming the interrupted children, and
  `campaign repair` refuses honestly when subs never merged. Error
  footers interpolate real ids and drop the generic doctor hint when a
  specific try line exists.
- Homebrew publish job pulls the formula from the release assets (the
  v0.1.0 cut proved the artifact-bundle path wrong).

## 0.1.0 — Stable

Release highlights distilled from the 0.1.0-rc.2 through 0.1.0-rc.11
candidates; the sections below carry the full per-change record.

- First-class model selection: populated per-provider model catalogs, a
  `deadreckon models` verb, an interactive model picker in `start`, and
  per-role model flags (`--model`, `--planner-model`, `--coder-model`,
  `--reviewer-model`, `--child-model IDX=MODEL`) across run, chain,
  orchestrate, and campaign — echoed on previews and provider-role tables.
- Never-dead-end launches: an unusable resolved provider on a TTY drops into
  the probe-before-ask provider picker instead of refusing; off-TTY refusals
  are unchanged.
- Durability: history.json corruption falls back to traces.jsonl
  reconstruction with atomic re-save; lock reclaim never usurps an alive
  holder pid regardless of heartbeat age.
- One prompt engine (inquire) behind every interactive surface, a gradient
  wordmark banner, smart bare invocation, and a visually informative
  installer with SHA256SUMS verification.
- Release trust end to end: signed + notarized macOS archives, fail-closed
  rc/stable lanes, self-update re-homed and proven live, CI on every push
  with the full 54-binary suite green.
- Consciously narrowed for this cut: npm publishing (no npmjs token yet)
  and Windows Authenticode signing (no certificate yet) are deferred via
  explicit policy flags; the Windows zip ships unsigned, and Homebrew +
  curl-installer + GitHub release are the supported channels.

## Stable Readiness - 2026-06-10

- Populated model catalogs for every built-in provider descriptor, each with
  exactly one recommended entry; custom descriptors fail closed on multiple
  recommendations.
- `deadreckon models [PROVIDER] [--all] [--json]` — the catalog surface for
  choosing a model, marking the recommended entry and the configured default.
- Per-role model flags: `--model` on start/run/chain, `--planner-model` /
  `--model` / `--child-model IDX=MODEL` on orchestrate full-plan,
  `--coder-model` / `--reviewer-model` on orchestrate review, campaign
  equivalents — additive serde-default fields on plan.json, echoed in
  previews and the provider-roles table; "provider default" sends no model
  argv.
- Interactive `start` gains a model picker after the provider choice.
- Never-dead-end launches: unusable resolved routes on a TTY drop into the
  provider picker (keep/cancel reproduce the original refusal); off-TTY
  refusals byte-identical.
- history.json corruption falls back to traces.jsonl reconstruction with an
  atomic re-save; save_history writes via tempfile + rename.
- Lock reclaim never usurps an alive holder pid; LockHeld names the
  heartbeat age and the kill --force escape hatch.
- Stable-lane gates: `## 0.1.0` CHANGELOG section, lane-asymmetry depth
  tests (changelog + npm wrapper pins are stable-only), explicit
  checksum = "sha256", embedded-checksum upgrade path recorded.
- `release/preflight-real.sh` real-provider proof harness +
  `release/known-good-providers.json` (schema_version 1); stable v0.1.0
  operator checklist and Windows smoke checklist in docs/RELEASE.md;
  models/picker/rescue documented in HOWTO.md.

## Self-update that actually updates - 2026-06-10

- The axoupdater-backed `deadreckon update` pointed at the pre-re-home
  `gdc/deadreckon` repository in six places (release source, API URLs, brew
  tap hint), so every real update would 404. All update surfaces now point at
  `gregce/deadreckon`, and the portability guard gained a repo-slug list so
  `gdc/` references cannot return — which immediately caught the Homebrew tap
  still reading `gdc/homebrew-tap` in dist-workspace.toml, the release
  workflow, the formula patcher, and the manifest.
- Latest-release resolution is RC-era aware, mirroring the installer:
  `releases/latest` (stable) first, newest release of any kind as fallback —
  no more silent "up to date" while newer release candidates exist. When the
  resolved latest is itself a prerelease, the updater installs it without
  requiring `--pre`.
- Proven live end to end: a sandboxed shell install with an rc.7 receipt
  resolved rc.10 via `update --check`, swapped binaries with `update --yes`
  (rollback backup retained), and the cached startup hint
  ("deadreckon X is available...") fires on the next TTY command. The
  evidence panel's installer-asset URL is also fixed
  (releases/tag -> releases/download).

## Visual interaction overhaul: banner, smart bare invocation, one prompt engine - 2026-06-09

- Help surfaces gain a figlet wordmark with a per-character 256-color
  gradient (twelve palettes, picked per invocation) and a version tagline —
  TTY-only, so pipes and every output contract stay byte-clean.
- Bare `deadreckon` reads the room: no config on the machine → a first-run
  welcome listing detected agent CLIs with an on-TTY offer to run guided
  setup; configured but no runs in this directory → orientation (source mode
  start would use here, the production flow, where other runs live); runs
  present → status, as a returning operator expects.
- One prompt engine (inquire) powers every interactive surface: arrow-key
  selects with detection hints, styled confirms, validated number input, and
  text prompts, themed to the shared Tone palette and colorless under
  --plain/NO_COLOR. Off-TTY and DEADRECKON_PROMPT_LINE_MODE keep the
  original numbered line prompts byte-stable for scripts and tests. Existing
  pickers (start, campaign, orchestrate, config) inherit the upgrade through
  the shared API.
- The init provider prompt is a probe-before-ask menu: detected subscription
  CLIs lead with live login-state hints, API routes show whether their key is
  already exported, and a typed route stays reachable. The legacy hand-rolled
  stderr non-git menu is unified into the same engine.

## Installable macOS archives and user-verifiable SHA256SUMS - 2026-06-09

- rc.7's macOS `curl | sh` failed at the final `mv`: the signing step
  repacked archives with `tar -C dir .`, prefixing every member with `./`,
  which breaks the cargo-dist shell installer's layout resolution. The repack
  now packs explicit top-level names, and `verify-manifest` fails closed on
  any `./`-prefixed archive member so this cannot ship again.
- `SHA256SUMS` now records flat basenames (one entry per published asset,
  identical nested CI duplicates collapse, divergent content fails closed),
  so the runbook's documented `shasum -a 256 -c SHA256SUMS` works next to
  downloaded files — and the install wrapper's integrity check of
  `deadreckon-installer.sh` actually engages instead of always warning.

## Sleep-preview test race fix - 2026-06-09

- `prevent_sleep_linux_falls_back_when_systemd_inhibit_missing` read
  `DEADRECKON_SLEEP_INHIBITED`-dependent state without holding the test
  binary's `ENV_LOCK` while a sibling test mutates that variable under it —
  parallel scheduling decided the verdict (green in two CI runs, red in
  rc.6's verify). Both env-sensitive preview tests now hold the lock.

## CI on every push; platform-scoped hygiene baselines - 2026-06-09

- New `ci.yml` runs `cargo fmt --check` and the full workspace suite
  (`--no-fail-fast`, with `expect` installed) on ubuntu for every push and
  pull request — completing the release-trust-closure item so host couplings
  surface in branch feedback instead of release-candidate tags. The release
  verify step also uses `--no-fail-fast` so one red binary cannot hide the
  rest.
- The release-binary size baseline is per-OS (`tests/.size-baseline-macos`,
  `tests/.size-baseline-linux` — Mach-O and ELF sizes are not comparable),
  and the rustfmt-commit archaeology test skips on shallow clones, which
  cannot see repo history. Both failed rc.5's verify gate.

## Host-coupling sweep for the test suite - 2026-06-09

- Swept the suite for works-on-the-author-machine couplings after rc.4's
  verify gate caught three more: every test `git init` now pins
  `--initial-branch=main` (branch-name assertions no longer depend on the
  host's `init.defaultBranch`); the config provider/model shortcut test
  resolves a stub `codex` from a prepended PATH instead of requiring a real
  install; and the interactive prompt tests probe for `expect(1)` — skipping
  with a notice on dev machines, failing loudly when CI lacks it. The release
  verify job installs `expect` so the interactive coverage actually runs on
  the Linux gate.

## Platform-stable characterization goldens - 2026-06-09

- The characterization goldens embedded environment noise: raw temp-path
  *length* decided kv wrap points, path truncation points, and the smoke
  provider's prompt-length-derived token counts, so two goldens generated on
  macOS failed on the Linux release runner (the v0.1.0-rc.3 verify gate).
  Characterization workspaces now live at one fixed canonical path length on
  every platform and the goldens are regenerated to match.
- `DEADRECKON_UPDATE_GOLDENS=1` regenerates the characterization goldens
  instead of asserting, for the next time the pinned surface intentionally
  changes.

## Chain hook EPIPE fix - 2026-06-09

- A chain hook that exits (or closes stdin) without reading its advisory JSON
  payload no longer turns into `apply_refused_json_error__broken_pipe` — the
  payload write tolerates EPIPE and the hook's exit code stays the contract.
  This raced reliably on Linux CI runners (it broke the v0.1.0-rc.2 release
  verify gate) and is now pinned by a deterministic closed-pipe unit test plus
  a stdin-closing hook integration test.

## Workspace suite green; releases gated on it - 2026-06-09

- `cargo test --workspace` passes end to end (53 binaries) and the release
  workflow's verification step now runs the full suite —
  `cargo fmt --check && cargo test --workspace --locked` plus the release
  build — completing the release-trust-closure contract that
  `release_workflow_verification_matches_release_trust_contract` pins. No
  release ships on a suite that never ran.
- Fixed the three standing failures: the chain TTY test strips ANSI (the
  PTY-attached binary colorizes now) and invokes `script(1)` portably (BSD
  positional args on macOS, `-qec` on util-linux); the README first-screen
  coherence window covers the command table that moved below the install
  instructions; and two test fixtures in `prompt.rs`/`main.rs` build their
  ANSI escapes at runtime so `raw_ansi_escapes_stay_in_ui_module` holds.

## Self-healing turn loop - 2026-06-09

- The retryability taxonomy is finally load-bearing: transient provider
  errors (408/429/5xx, transport blips, CLI rate-limit phrasings) get one
  bounded retry with a 2s backoff inside the turn loop. The retry is audited —
  events.jsonl records "turn N hit a transient provider error; retrying once"
  and "retry succeeded; continuing" — so recovery is visible in attach, never
  silent. `ProviderError::Http` carries an explicit `retryable` flag set at
  construction; `is_fatal()` is now its exact complement.
- The router preserves the typed error when exactly one route was attempted,
  so retryability survives instead of being flattened into an opaque
  `NoRoute` string; multi-route fallthrough still aggregates.
- The HTTP client has real timeouts (30s connect / 600s request) — a stalled
  API connection can no longer hang an unattended run forever. HTTP error
  bodies are trimmed on a char boundary (the old byte slice could panic on
  multibyte error text exactly while reporting a failure).
- A provider error that survives the retry now persists `Failed` plus a
  `failure_reason` and emits the run-completed event before surfacing — a
  dead run shows as failed in `list`/`status` immediately instead of
  lingering as a zombie `Executing` until pid liveness is probed.

## Attach TUI help overlay and one abandon key - 2026-06-09

- Every attach surface (run, plan, campaign, chain) gains a `?` help overlay:
  a centered popup with the complete key reference for that surface; any key
  closes it. The footer shows the load-bearing subset and truncates on narrow
  terminals — the overlay is the full reference, one keystroke away, and every
  footer now advertises `? help`.
- One shared help-key rule (`handle_help_key`): `?` opens, any key while open
  closes, everything else flows to the surface's normal handling — a key
  pressed with the overlay open can never fire an action underneath it.
- One abandon key: the CLI completion prompt now advertises `x abandon`
  matching the attach TUI (where `b` is "back"); `b` stays accepted for muscle
  memory but is no longer documented. HOWTO's TUI key table caught up
  (`x abandon`, confirm keystrokes, the `?` overlay).

## Honest spend and a wall cap that binds mid-turn - 2026-06-09

- Subscription run spend now reads as the budget it really is —
  `not metered (subscription) · 23m of cli:claude-code · 7 turns` — instead of
  a raw seconds count; `status` adds a `billing` row ("subscription: cost is
  not metered, time is the budget") and the JSON gains an additive `billing`
  field.
- `--max-wall-seconds` now binds DURING a turn, for every provider kind: the
  provider call is bounded by the remaining wall budget, the in-flight
  subprocess is cancelled (not orphaned) with a bounded grace period, the cut
  turn's elapsed time is recorded honestly in `spend.jsonl`, and the run
  pauses at cap exactly like the spend cap. Previously the cap was checked
  only between turns and only for subscription-billed turns, so an API-billed
  run had no wall cap at all and a single hung turn was uncapped for everyone.
- Direct-API turns that report no wall time now accrue measured elapsed wall
  time, so wall accounting (and the cap) is universal.

## Provider login preflight - 2026-06-09

- Subscription CLI descriptors may declare an `[auth_probe]` (a local status
  subcommand, e.g. `claude auth status` / `codex login status`) with
  logged-in/logged-out markers and `login_try_lines`. Matching strips
  whitespace so JSON pretty-printing differences don't matter, and is
  fail-open: unsupported subcommands, stubs, and unexpected output classify as
  Unknown and behave exactly as binary presence did before.
- `deadreckon doctor` now distinguishes "CLI binary found; logged in" from
  "installed but not logged in (<detail>)", with the provider's own login
  command as the action.
- The shared provider-setup resolver probes login state on the launch path
  (`require_usable_route`) and refuses up front — `try: claude login` —
  instead of failing mid-run with raw subprocess stderr. Previews stay
  presence-only and never spawn the probe.

## Portability: no developer-machine paths in the shipped surface - 2026-06-09

- `DEADRECKON_HOME` now defaults to `~/.deadreckon` derived from the running
  user's home (`default_deadreckon_home()`), and the provider config default
  follows it (`default_config_path()`). The compiled-in `/Users/gdc/...`
  constants (`DEFAULT_DEADRECKON_HOME`, `SOURCE_ROOT`, `DEFAULT_CONFIG_PATH`)
  are gone; installed binaries work on any machine without env setup.
- Source-tree fallbacks (run/doc skills, chain hooks, self-improvement targets,
  learning redaction) resolve through `source_root()` — `$DEADRECKON_SOURCE_ROOT`
  override first, then the compile-time workspace — and degrade cleanly to the
  user tier when no checkout is visible.
- `release/install.sh` defaults to the latest GitHub release instead of a
  pinned RC tag; `DEADRECKON_TAG` still pins. The Makefile derives `ROOT` from
  `CURDIR` and `alias-zsh` edits `$HOME/.zshrc`.
- HOWTO.md is written for any machine (`~/.deadreckon`, `~/.zshrc`,
  `/tmp/try-deadreckon`) instead of the author's.
- New guard test (`tests/portability.rs`) fails the build if a
  developer-machine path reappears anywhere in crates, release scripts, the npm
  wrapper, the Makefile, or user-facing docs. Import goldens normalize the
  workspace as `<SOURCE_ROOT>`.

## Attach TUI Uniformity: narrative panels - 2026-06-08

- The plan narrative panel now windows its fixed-height view and shows a
  `plan narrative first-last/total` scroll indicator (`plan_narrative_title`),
  matching the run narrative panel — an overflowing plan narrative scrolls
  instead of silently clipping. In plan narrative view the shared nav keys drive
  a `NarrativeScrollNav` (clamped to `total - visible_rows`) that scrolls the
  prose rather than moving the task cursor. Closes the one "every list panel"
  gap left by the uniformity slice.

## Attach TUI Uniformity - 2026-06-05

- One shared key dispatcher (`tui::navigation::NavigableSurface` +
  `dispatch_navigation`) drives run, plan, campaign, and chain: arrows/jk,
  Tab/BackTab, PgUp/PgDn, Home/End/g/G behave identically everywhere, with each
  surface supplying only a mode hook. Plan and campaign gained the paging keys
  they lacked.
- One selection cursor (`selection_glyph()` -> `>`), one footer builder
  (`footer(items)`, replacing four divergent styles and deleting the
  parent-plan string-`replace()` hack), and one scroll-position indicator
  (`scroll_indicator()`) on every list panel.
- Apply and Abandon now require a two-step in-TUI confirm; a single mistyped key
  can no longer fire them. Abandon moved off `b` (now unambiguously "back") to
  `x`, and the dead `d`->Docs overload was removed.
- Uniform exit/return: the "press Enter to return" prompts accept
  Enter/q/Esc/Backspace, and Enter on an unloadable child shows an "unavailable"
  notice instead of a silent no-op.
- Friendly empty states (no leaked `*-events.jsonl` filenames), one
  `NARRATIVE_SPLIT_WIDTH` breakpoint shared by run and plan, and an ASCII
  fallback + legend for the chain step glyphs.

## Uniform Surface - 2026-06-05

- Added one `display_width()` (strip ANSI, then Unicode display width) and routed
  the line and card truncation/padding helpers through it so wide (CJK) and
  zero-width glyphs no longer miscount terminal columns.
- Collapsed the two divergent `Tone` enums into one shared enum with a single
  tone->ANSI table and a derived tone->ratatui::Color table, so a status renders
  the same color on a line and in a frame; replaced the silent status fallback
  with an explicit `Status` classifier where an unknown status stays visible
  rather than being dimmed into the background.
- Fixed the column-alignment bug where a colored id cell padded with `{:<N}` was
  short by its ANSI escape length: added a shared `pad_visible` (display-width
  padding) and routed the provider/library id columns through it, aligned the
  provider id/symbol column order across full and summary modes, and measured the
  run-list pad helpers by display width.
- Honored `--no-hints` / `DEADRECKON_HINTS=0` everywhere: fixed the campaign
  completion surface that bypassed the hint toggle, and routed the
  inspection/doc/chain completion surfaces through `completion_hints_enabled`
  so the toggle is respected uniformly.
- Added a shared `kv_block` primitive (auto-sized `key: value` on display width)
  and migrated the status report's run-health, library, and disk sections onto
  it, fixing the misaligned `gate:` and `scope artifacts:` lines so every colon
  lines up under the widest key.
- Added a shared `columns` table primitive (lowercase headers, display-width
  padding so colored cells align like plain ones) and migrated the library table
  onto it; lowercased the run-list header. Provider/plan/chain tables retain
  their display-width-correct renderers and can adopt `columns` incrementally.
- Hardened the selectable prompt menu: multi-digit number entry (menus with 10+
  choices are now reachable by number), `Esc` always cancels even without an
  explicit cancel choice, out-of-range digits show a `choose 1-N` notice, tall
  menus fall back to line mode instead of corrupting the screen, and the footer
  advertises the available keys. Key dispatch is factored into a pure, unit-tested
  `menu_step`.
- Added `prompt::ask_number(range)` that re-prompts on empty, non-numeric, or
  out-of-range input, and routed the campaign and orchestrate child-count prompts
  through it so a typo re-prompts instead of aborting the whole command.
- `deadreckon start` with no goal now prompts for one interactively on a TTY
  (and prints a one-line notice when prompts are suppressed) instead of erroring
  out. Confirm-vs-select modality is standardized incrementally (binary
  decisions use `confirm`, multi-way use `select_one`).
- Added one `wrap_words` engine (display-width-aware) and collapsed the kv-value,
  run-list, and campaign-facts wrappers onto it; gave the chain step glyphs an
  ASCII fallback under `--plain`/non-VT terminals (the Windows weak spot); and
  replaced the bare `println!("cancelled")` run paths with a verdict surface that
  carries a Recommended next step.
- Colorized the verdict surface (doctor, status hints, and every run/finish/
  campaign/chain/import/learning outcome): the verdict label is tone-coded by
  kind, section headers are bold, evidence keys are dimmed with their
  `passed`/`warning`/`failed` status words colored, and Recommended/Secondary
  commands are styled. Dimmed the status report's kv keys. Color is gated on a
  TTY, so `--plain`/`NO_COLOR`/piped output stays byte-identical.
- Swept the per-command raw output (chain, inspection, lifecycle, acceptance,
  plan, providers, campaign, doc, attach): 85 additional colorizations — section
  headings, ids/hashes, runnable commands, status words, and dimmed labels — all
  through the TTY-gated helpers. Fixed-width/padded table columns were left plain
  to preserve alignment (the ANSI-padding bug class).

## Release Trust - 2026-06-02

- Added a lane-aware release policy gate for branch/PR, RC, stable, and invalid
  tags so official RC/stable releases share one publish/signing/provenance
  contract while forks and PRs remain secret-free dry-runs.
- Hardened official releases to fail closed when macOS signing/notarization,
  Homebrew, npm provenance, attestation, manifest, checksum, or Windows signing
  policy requirements are missing.
- Moved macOS signing proof to the packaged cargo-dist artifact: CI now signs,
  verifies, notarizes, and repacks the archive contents before upload.
- Added release trust artifacts: `SHA256SUMS`, `release-manifest.json`,
  `release.spdx.json`, GitHub artifact attestations, Homebrew checksum
  verification, and npm `--provenance` publishing.
- Updated the release runbook with the Apple Developer ID checklist, npm
  trusted-publishing/token fallback, Windows Authenticode signing secrets, and
  artifact verification commands.

## Verdict Surface - 2026-06-02

- Added a shared Verdict Surface contract for terminal outcomes: one verdict,
  one `Recommended` command, one `Explanation`/`Evidence` panel, additive
  `verdict`/`primary_action` JSON, and subordinate secondary actions.
- Normalized run/lifecycle, plan/orchestrate/fork/merge, campaign, chain,
  recovery, setup/diagnostic, import, learning, and doc outcome surfaces through
  the shared contract while preserving command names, quiet/plain/json/no-hints
  behavior, and durable state schemas.
- Demoted competing TUI/help/preflight action hints from primary-looking
  `recommended:` rows to compact `next`/inspection guidance.
- Burned down the FRIENDLINESS-AUDIT one-primary-action failures and added a
  regression test that rejects new in-scope audit failures.

## Seam conformance kit - 2026-05-31

- Added `examples/seams/` with fixture JSON, a sample `[seams]` config, and
  POSIX shell workers for policy allow/deny, catalog override, hooks JSONL, and
  event-sink JSONL.
- Added `deadreckon seams validate <kind> --config <path> [--fixture <path>]
  [--json] [--sandbox <backend>]` so workers can be checked against the same
  sandboxed dispatch primitive used by runtime.
- Documented the seam protocol, fail policies, sandbox expectations,
  `--no-seams`, and the non-swappable gate boundary in `docs/SEAMS.md` and
  AS-BUILT §39.

## Composable seams (production release) - 2026-05-31

- P1: Added the runtime `SeamCommand` primitive, `[seams]` config parser with a
  hard non-swappable gate guard, sandboxed JSON-over-stdio dispatch with fixed
  per-kind fail policies, stdin/denylist support in the sandbox runner, and
  per-run `seams.json` audit writing.
- P2: Wired the policy seam into bash/write_file dispatch after the
  `sandbox.toml` floor, reusing the existing tool-refusal provenance path for
  denials while preserving builtin behavior when no policy seam is configured.
- P3: Added the model-catalog seam path: catalog responses can override route
  context windows and pricing at router construction, while malformed or absent
  catalog seams fall open to the built-in model list.
- P4: Added hook fanout for tool start/result events with fail-safe dispatch;
  hook outputs are observe-only, non-fatal, and covered by proof-subtree sandbox
  denial.
- P5: Added the event-sink seam as an additive `RunEvent` broadcast mirror while
  keeping `events.jsonl` as the source of truth for attach and failure recovery.
- P6: Added deterministic direct-API history compaction with `[compaction]`
  config, `compaction.jsonl` audit records, catalog/seam/fallback context-window
  sources, and full `history.json` retention.
- P7: Added `--no-seams` run/start controls, seam resolution in preview and
  doctor output, and policy-seam refusal footers with recovery commands.
- P8: Added adversarial trust-boundary tests for the then-current legacy-v1
  gate, proving seam workers could not write markers/proofs, read `gate/nonce`,
  or affect gate signatures.
- P9: Added explicit seam config validation tests for unknown kinds, empty
  commands, and bad timeouts, plus an all-seams smoke run that writes
  `seams.json` and validates a gate marker.
- P10: Added resume-sweep coverage for seam re-resolution, deterministic
  compaction replay, and survival of `seams.json`/`compaction.jsonl`.
- P11: Documented composable seams in AS-BUILT §39, updated the shipped/thin
  accounting, and logged V1 seam follow-ups.
- Release summary: one uniform seam contract (sandboxed JSON-over-stdio
  subprocess, fixed per-kind fail policy) makes policy, model-catalog,
  hook-fanout, and event-sink swappable via `[seams]`; unconfigured seams keep
  built-in behavior and `--no-seams` forces all built-ins.
- Release summary: the acceptance gate stayed deliberately non-swappable. In
  that legacy-v1 path, no seam could write or redirect the marker, read
  `gate/nonce`, or alter the signature; seam workers ran sandboxed. Strict
  durable Jobs now use the keyless-evaluate/HMAC-sign boundary described above.
- Release summary: deterministic, resume-safe context-window compaction closes
  the direct-API history gap in `compaction.jsonl`; CLI-provider paths are
  untouched.

## Navigable campaign attach (production release) - 2026-05-31

- Added campaign attach state/feed plumbing so `attach <campaign-id>` can refresh
  campaign snapshots, roll-up, aggregate spend, sub-plan spend, and a bounded
  campaign feed from `campaign-events.jsonl` plus each discovered sub-plan's
  `plan-events.jsonl`.
- Added a live ratatui campaign attach surface for TTYs: campaign header,
  selectable sub-plan cards, campaign feed, and footer controls for select,
  drill-in, back, refresh, and detach. Off-TTY/`--plain` keeps the read-only
  summary, while `--json` emits a structured campaign attach object.
- Wired campaign -> sub-plan -> child-run drill-in by suspending the campaign
  frame and reusing the existing plan/run attach loops unchanged, with campaign
  breadcrumbs threaded into plan and run attach views.
- Covered the feature with focused navigable attach tests for campaign event
  tailing, render text, key handling, nested suspend/resume depth, breadcrumbs,
  JSON/plain fallbacks, latest campaign resolution, and campaign tick timing.
- Updated AS-BUILT and V1 deferrals: campaign attach is now navigated production
  behavior; the remaining V1 work is a flattened recursive event tree.
- Rebaselined the release binary-size guard to the verified post-feature binary
  after adding the campaign TUI/feed code path.

## Decompose (maintainability refactor) - 2026-05-30

- P1 (`7ef2d5c`): Added a full-binary CLI characterization net for plan creation,
  quiet plan creation, start full-plan preview JSON, chain status, off-TTY attach,
  and canonical `try:` refusal footers, with normalized goldens under
  `crates/deadreckon/tests/goldens/characterization/`.
- P2 (`a6f8d57`): Added shared integration-test helpers under
  `crates/deadreckon/tests/common/` and migrated duplicated tempdir,
  command-construction, stdout/stderr, and success-assertion helpers without
  changing test assertions.
- P3a (`a601ae3`): Lifted `acceptance_integrity_tests` out of `main.rs` into a
  sibling `src` test module without changing test names or widening runtime
  visibility.
- P3b (`9a9892d`): Lifted `acceptance_render_tests` out of `main.rs` into a
  sibling `src` test module while preserving its four render-focused unit test
  names and private-helper access.
- P3c (`098b1cf`): Lifted `campaign_spawn_tests` out of `main.rs` into a
  sibling `src` test module while preserving its campaign orchestration helper
  coverage and private-helper access.
- P3d (`e15eb86`): Lifted `effortless_consistency_tests` out of `main.rs` into a
  sibling `src` test module while preserving its cross-surface consistency
  assertions and private-helper access.
- P3e (`02d8396`): Lifted `flight_cli_tests` out of `main.rs` into a sibling
  `src` test module while preserving CLI flight/log fixture coverage and
  private-helper access.
- P3f (`8e0f276`): Lifted `self_improve_pr_tests` out of `main.rs` into a
  sibling `src` test module while preserving self-improvement PR adapter
  coverage and private-helper access.
- P3g (`bf64b50`): Lifted `tui_tests` out of `main.rs` into a sibling `src`
  test module while preserving attach, plan, narrative, provider-log, and
  guided-start TUI coverage plus private-helper access.
- P4a (`1768d17`): Created the private `commands/` facade, moved the chain
  command family into `src/commands/chain/`, and routed the `main_inner` chain
  branch through `commands::chain` while keeping shared attach infrastructure in
  the crate root.
- P5a (`58cfbd4`): Moved the acceptance and def-done command family into
  `src/commands/acceptance.rs`, preserving the existing `main_inner` dispatch
  and keeping acceptance render helpers in the crate root for the later TUI
  split.
- P5b (`eb55274`): Moved the supervised `run` command body into
  `src/commands/run.rs`, with `main_inner`, `start`, and `try` now calling the
  private command module while shared preview/render helpers remain in the crate
  root.
- P5c (`d72bc9d`): Moved the `init` command body into `src/commands/init.rs`,
  keeping shared completion, config rendering, and provider-detection helpers in
  the crate root for later cleanup phases.
- P5d (`05e96ba`): Moved the campaign command family into
  `src/commands/campaign.rs`, keeping root/start/orchestrate/attach/show/kill
  call sites routed through the private command module.
- P5e (`d1e66f2`): Moved the attach command dispatch and terminal event loops
  into `src/commands/attach.rs`, leaving pure render/state helpers in the crate
  root for the P6 TUI extraction.
- P5f (`5ec29f0`): Moved the merge command entrypoint and CLI repair-strategy
  parsing into `src/commands/merge.rs`, keeping shared merge/repair helpers in
  the crate root for plan dependency composition and the later plan split.
- P5g (`7416c0b`): Moved the orchestrate front-door and interactive
  mode/provider selection helpers into `src/commands/orchestrate.rs`, keeping
  plan creation, fork, and shared render helpers in the crate root for the
  remaining plan split.
- P5h (`9c2a8cf`): Moved the plan/fork command family and child-launch
  orchestration helpers into `src/commands/plan.rs`, leaving plan result docs
  and shared TUI render helpers in the crate root for later phases.
- P6a (`8d94316`): Created the private `src/tui/` module and moved the
  run-attach TUI state, key handling, post-action notice, and panel layout
  helpers into `src/tui/attach_state.rs`.
- P6b (`e213718`): Moved the pure Markdown-to-ratatui line renderer into
  `src/tui/render.rs`, leaving the run-doc file lookup wrapper in the crate
  root while the TUI render module takes over presentation-only parsing.
- P6c (`f4dc268`): Moved pure run-attach activity, live-file, process, narrative
  item, panel title, and context-count render helpers into `src/tui/render.rs`
  while keeping the terminal draw loop and file/doc lookup wrappers in place.
- P6d (`e2481ae`): Moved the run-attach widget shell, header/footer/status,
  spend/context/acceptance panels, and live-files/process panels into
  `src/tui/render.rs`, leaving provider refresh, narrative projection caching,
  and docs file lookup wrappers in the crate root.
- P6e (`3bc3971`): Moved the plan-attach widget shell, narrative panel, footer,
  task-pane layout, activity feed formatting, and task detail rendering into
  `src/tui/render.rs`, while preserving shared plan summary/event helpers behind
  the private crate facade for existing command output.
- P6f (`5fd1063`): Moved the chain-attach TUI state, renderer, event-read
  hinting, header/footer text, timeline rows, and activity rows into
  `src/tui/render.rs`, leaving the chain command event loop and actions in
  `src/commands/chain/`.
- P6g (`c4fe1e7`): Moved the run narrative/docs widget rendering,
  `RunNarrativeRenderInput`, deterministic run narrative projection helpers,
  markdown-doc line rendering, and narrative line/count helpers into
  `src/tui/render.rs`, leaving provider refresh jobs and attach event loops in
  the command/root orchestration layer.
- P6h (`efda723`): Added the pure-render unit snapshots for run attach and
  chain attach frames, using the extracted `src/tui` render seams to lock the
  terminal frame shape before leaving the TUI extraction phase.
- P7 (`76600a8`): Added merge-conflict-path characterization and extracted the
  shared merge composition loop used by campaign result composition, final plan
  merge working trees, and full-plan dependency source assembly without changing
  conflict semantics or repair behavior.
- P8 (`3390ad8`): Added command-existence characterization for bare PATH lookup
  and explicit-path handling, then unified the start/setup/doctor command
  lookup call sites through one private helper without changing provider
  detection behavior.
- P9 (`242bfa3`): Added cross-crate retryable-I/O characterization, promoted
  `deadreckon_core::error::is_retryable_io_kind` as the single shared helper,
  reused it from providers and sandbox, and recorded the one justified public
  surface rebaseline for that new core path.
- P10 (`8b80969`): Pruned unused `tracing`/`chrono` dependencies, deleted
  confirmed dead helpers, hardened docs regex initialization with BUG-tagged
  `expect` calls plus compile coverage, and applied targeted allocation nits
  while keeping characterization goldens unchanged.
- P11: Documented the post-decompose binary layout in AS-BUILT §38, updated the
  built-vs-thin accounting, and logged the rejected command/API/test reshaping
  work in `docs/V1-CANDIDATES.md`.
- Post-P11a: Moved descriptor-backed `deadreckon import` handling into
  `src/commands/import.rs`, keeping `main_inner` as the only call boundary and
  reducing `main.rs` to roughly 20.2k lines.
- Post-P11b: Moved `deadreckon learn` and `deadreckon improve` command handling
  into `src/commands/learning.rs`, preserving the crate-private self-improvement
  PR adapter seam and reducing `main.rs` to roughly 19.4k lines.
- Post-P11c: Moved the guided `deadreckon start` flow into
  `src/commands/start.rs`, keeping shared launch-preview and command-existence
  helpers at the root and reducing `main.rs` to roughly 16.7k lines.
- Post-P11d: Moved `deadreckon detect`, `deadreckon providers list`, and
  `deadreckon update` handling into `src/commands/providers.rs`, leaving shared
  provider-id helpers at the root and reducing `main.rs` to roughly 16.0k lines.
- Post-P11e: Moved shell completion and `deadreckon doctor` handling into
  `src/commands/completion.rs` and `src/commands/doctor.rs`, leaving shared
  command-existence lookup at the root and reducing `main.rs` to roughly 15.4k
  lines.
- Post-P11f: Moved `deadreckon list`, `deadreckon history grep`, and
  `deadreckon library` handling into `src/commands/inspection.rs`, keeping only
  crate-private plan/library list seams for start/status and reducing `main.rs`
  to roughly 14.3k lines.
- Post-P11g: Moved `deadreckon doc` run/plan dispatch and doc-polish preview
  helpers into `src/commands/doc.rs`, leaving narrative attach provider
  selection at the root and reducing `main.rs` to roughly 14.0k lines.
- Post-P11h: Moved `finish`, `export`/`materialize`, `apply`, `abandon`,
  `cleanup`, `extend`, parent markers, and lifecycle notification firing into
  `src/commands/lifecycle.rs`, leaving status/resume/control helpers at the
  root and reducing `main.rs` to roughly 12.4k lines.
- Post-P11i: Moved attach-loop tick timing and async narrative-refresh job
  plumbing into `src/commands/attach_runtime.rs`, keeping attach/chain event
  loops as callers and reducing `main.rs` to roughly 11.9k lines.

## Effortless (production release) - 2026-05-28

- P1 (`c81b617`): Added the whole-surface friendliness contract table and `docs/FRIENDLINESS-AUDIT.md`, with depth tests proving every canonical top-level verb has one row per six-clause contract item.
- P2 (`bacf76f`): Added `deadreckon try`, a keyless local smoke run that uses the real turn loop and signed `dr-gate` proof, then prints the proof/story/lineage block and one next command.
- P3 (`bbf1e73`): Factored the proof-block renderer and surfaced the signed proof/story/lineage block on completed run exit cards.
- P4 (`e20cf54`): Made `deadreckon start` adopt a single detected subscription CLI inline, keep the provider picker for multiple detected CLIs, and refuse with `deadreckon try`/provider setup recovery when none are available.
- P5 (`663843f`): Added a shared primary-action slot to cards and made exit cards, status, and finish lead with one primary action while demoting secondary lifecycle actions.
- P6 (`85f1d31`): Swept spend and gate verdict rendering so exit cards, status, finish, plan child details, and campaign child summaries show honest subscription spend and per-check gate results.
- P7 (`0bef0f4`): Added opt-in `[notify]` parsing, bounded native/command/webhook channels, redacted notification context, and `notify.jsonl` attempt records.
- P8 (`823945b`): Fired enabled notifications on accepted, paused-at-cap, and failed lifecycle outcomes while disabled configs stay silent.
- P9 (`10dd47b`): Added bounded provider-backed goal-shape recommendations for `start`, preview-scoped classifier records, optional campaign `--n`, and editable campaign preflight controls.
- P10 (`7425883`): Unified the verified-run glossary, changed completed exit cards to the `VERIFIED` verdict, expanded refusal `try:` footer coverage, and added command-notification failure recovery hints.
- P11 (`c37ca2b`): Documented AS-BUILT §37 for the Effortless contract, updated shipped-vs-thin accounting, and logged the palette/localization/template/notifier/classification/onboarding deferrals in V1-CANDIDATES.

## Campaign Orchestration (production release) - 2026-05-28

- P1: Added `deadreckon-core::campaign` module with the nesting `Lineage` record, the `CAMPAIGN_MAX_DEPTH = 2` hard cap, and a `guard` that refuses a campaign at depth >= 1 or a sub-goal that cycles to an ancestor `task_key`/scope.
- P2: Added the file-backed `Campaign`/`SubGoal` model (`campaign.json`) with `build_sub_goals` decomposition validation (exactly-N planner output, non-empty, distinct sub-goals) and `Campaign::new` reusing `validate_task_count` (2..=6).
- P3: Added the sub-orchestrator spawn (`build_sub_orchestrator_command`, lineage env transport + `sub-result.json` sidecar) reusing the plan-child isolation idiom, and wired `orchestrate full-plan` to report its merged result when launched by a campaign.
- P4: Added `run_campaign_fork`, a sequential sub-orchestrator driver that records `campaign-events.jsonl` (`campaign_started`/`sub_launched`/`sub_merged`/`sub_failed`) and marks a failed sub without aborting its siblings.
- P5: Added the tree-budget allocator (`allocate_budget`, even split with remainder-to-first), aggregate-spend exhaustion enforcement that refuses the next sub launch (`tree_budget_exhausted` + `budget_exhausted` event), and the unbounded-budget warning.
- P6: Extracted the shared `mergeable_run_files` enumeration (used by plan merge unchanged) and added `compose_roots`/`compose_result_runs` for independent sub-results; a cross-sub file conflict is reported so the campaign fails rather than silently overwriting.
- P7: Added the gate-verdict roll-up (`CampaignRollup`, `worst_of`, `rollup_permits_completion`, `build_rollup`): any refused or unmerged leaf makes the whole campaign refused (the no-laundering invariant). The roll-up is bound into the result run's marker signature, so editing `campaign-rollup.json` after signing invalidates the marker.
- P8: Added `campaign_can_complete`: a campaign reaches completion only when every sub merged and the roll-up permits it; a refused sub never reaches a clean completed state.
- P9: Added the top-level `deadreckon campaign <goal> --n <2..=6>` verb (peer to run/orchestrate/chain): decomposes via the planner, guards depth/cycles, previews (`--preview`), forks N sub-orchestrators, rolls up verdicts, composes one promoted result run with a `deadreckon-campaign-manifest.json`, and refuses to promote on any refused leaf or cross-sub conflict.
- P10: Surfaced campaigns in `attach <campaign-id>` (sub rows + roll-up + breadcrumb), `show <campaign-id> --why-failed` (refused/caveat subs), and `kill <campaign-id>` (cascades into each sub-plan, then marks the campaign killed) via `resolve_campaign`.
- P11: Documented campaign orchestration in AS-BUILT §36 and logged depth>2, cross-level dependencies/merge-repair, recursive attach, planner-chosen breadth, and richer tree-budget strategies in V1-CANDIDATES.

## Tamper-Evident Gate (production release) - 2026-05-28

- Refuse to sign when a run edits `acceptance.yaml` or a compiled check carries a suppression pattern; downgrade to a surfaced caveat when a run modifies a check-covered test/target file; bind the tamper record into the marker signature.
- Surface per-check verdicts and a tests-modified flag on the exit card, status, and `--why-failed`.
- Render honest subscription spend with `not metered (subscription)` for subscription-only routes and a subscription note for mixed routes.

## Production release posture - 2026-05-28

- Replaced current product docs and generated run-doc front matter that still labeled DeadReckon as alpha with production-release posture language.
- Kept dated alpha changelog entries and old goal briefs as historical records while moving new user-facing wording to compatibility-release terminology.
- Removed live CLI and narrative fallback messages that described current behavior as an alpha slice.

## Plan Doc Consolidation (production release) - 2026-05-28

- Added consolidated orchestration plan docs: `PLAN-NARRATIVE.md`, `PLAN-AS-BUILT.md`, `PLAN-DECISIONS.md`, `PLAN-CHILDREN.md`, and `PLAN-DOCS-MANIFEST.json`.
- Built a plan-doc input collector that reads child run docs, task summaries, worker specs, merge repair evidence, and final result inventory in task-graph order.
- Added provider-backed plan-doc consolidation with bounded input bundles, citation validation, invented-path checks, and deterministic fallback when provider output is unavailable or invalid.
- Materialized plan docs into merged libraries, plan apply worktrees, and exported artifacts without copying child internal logs.
- Rewrote synthetic plan-result apply `RUN-*` docs as wrappers that point to consolidated `PLAN-*` docs instead of showing empty zero-turn run docs.
- Extended `deadreckon doc`/`docs` and `show` so plan ids and plan-result wrapper runs resolve to plan documentation.

## Production command model (alpha) - 2026-05-27

- Reframed default help around the production flow: `start`, `attach`, `status`, `list`, `finish`, setup, and control commands.
- Kept power-user and advanced verbs callable and discoverable through `deadreckon help-all`, per-command help, and completions without crowding the first screen.
- Made `deadreckon start` history-aware for repos with completed promoted runs: TTY users can choose a follow-up, while preview/JSON output shows exact extend, review, and full-plan commands.
- Added done-criteria transparency to interactive `start` when project criteria already exist, with keep/view/check/update/cancel choices before launch.
- Updated README, HOWTO, AS-BUILT, the user-facing matrix, and focused tests without adding runtime schema or durable config.

## Start picker (alpha) - 2026-05-27

- Added selection-first TTY prompts to `deadreckon start` for launch mode, detected/configured provider routes, missing done-criteria action, non-git and dirty source handling, and final launch confirmation.
- Kept scripted surfaces deterministic: non-TTY, `--json`, `--plain`, `--quiet`, and `--yes` never enter the picker and continue to emit structured output or `try:` recovery lines.
- Let interactive users choose a detected CLI provider ephemerally for a launch without writing provider config.
- Routed selected provider routes into existing run/review/full-plan dispatch, with previews showing alpha role reuse for review and full-plan orchestration.
- Documented the picker behavior and remaining V1 deferrals without adding durable launch profiles or runtime state schemas.

## Guided first use (alpha) - 2026-05-26

- Reframed README/HOWTO first-run examples around provider-neutral `deadreckon start`, while keeping direct `run` and `orchestrate` paths documented for power users.
- Added a `start lifecycle` footer after successful guided launches so the new front door ends with exact attach, status, kill, and finish commands for the created run or plan.
- Locked `deadreckon start` JSON, plain, and quiet output behavior with focused coverage for structured recovery, ANSI-free previews, and quiet successful launches.
- Connected confirmed `deadreckon start` launches to the existing run and orchestrate handlers while keeping start previews state-free.
- Added source-mode recovery to `deadreckon start`, including `--fresh`, `--worktree`, `--from`, and `--allow-dirty` parsing plus non-git and dirty-worktree recovery that points to valid guided commands.
- Wired `deadreckon start` into shared provider setup and done-criteria recovery so missing providers, detected-but-unconfigured CLIs, and absent done criteria end with concrete `try:` lines instead of the placeholder launcher error.
- Shared launch preview rows for start, run, and orchestrate so previews name path, provider, done criteria, workspace, watch, stop, and finish actions.
- Added deterministic `start --mode auto` launch-decision heuristics for run, review, and full-plan paths.
- Added the visible `deadreckon start` parser and help surface for the guided front door.
- Clarified DeadReckon's audience as the harness around agent CLIs for unattended, sandboxed, auditable work, and pointed first-use help/docs at `deadreckon start`.
- Documented the guided first-use architecture and V1 deferrals in AS-BUILT and V1 candidates without adding durable launch state.

## TUI Responsiveness (alpha) - 2026-05-26

- Added in-memory attach tick budgets and loop-stage timing for run, plan, and chain attach surfaces, with provider narrative refresh classified as background work for the responsive attach scheduler.
- Moved run narrative attach refresh onto a coalesced background job so manual `r` redraws without awaiting the provider and detach cancels in-flight provider work.
- Routed run attach event and quiet-threshold narrative refreshes through the same background job, preserving failure notices until a later refresh replaces them.
- Moved plan narrative attach refresh onto a plan-keyed background job so manual, event, and quiet-threshold refreshes coalesce while child drill-in and detach cancel in-flight provider work.
- Replaced run attach live-file collection with an attach-specific inventory walker that prunes heavy cache/profile directories before descent and caps displayed files without losing total counts.
- Added attach-owned JSONL tail caches for run spend, trace, and flight activity streams so live run attach parses appended complete rows instead of rereading whole files each tick.
- Added live attach provider-log scan throttling so current flight rows delay fallback root scans, fallback matches are cached by freshness, and matched provider logs invalidate on mtime changes.
- Added run and plan narrative projection caches for attach rendering so redraws reuse covered projections, preserve stale provider snapshots, and avoid appending narrative snapshots from render paths.
- Added incremental chain activity tailing for chain attach, including partial-line tolerance and status hints when event reads fall behind.
- Added focused responsiveness smokes for slow narrative refreshes, large worktrees, and max-size chain timelines without invoking full release or stress suites.
- Documented the TUI responsiveness alpha contract and known limits: no attach daemon, no shared cross-surface broadcaster, and no diagnostic dashboard yet.

## Narrative Attach (alpha) - 2026-05-26

- Added `deadreckon attach --view narrative` for cited run and plan overviews, with `n` to return to raw activity and `v` to cycle architecture, agents, files, evidence, and no-visual modes.
- Added the `Narrated` operator heading for narrative attach projections so the calmer view has a clear product label.
- Defaulted provider-backed narrative refresh to local Claude Code on `sonnet`, while keeping `--narrative-provider` as an explicit route override.
- Added `--no-narrative-provider` for deterministic-only narrative attach when provider refresh is not desired.
- Added file-backed run/plan narrative projections under `narrative/state.json`, `narrative/snapshots.jsonl`, and `narrative/architecture-graph.json` without changing `PipelineState`.
- Added evidence-backed ASCII map rendering for run architecture, plan agents, touched files, and evidence chains, including plain/JSON attach output.
- Added redacted provider refresh on manual `r`: attach builds bounded prompts, validates structured claims and graph labels against known evidence, enforces budget/cadence guards, and falls back to deterministic stale facts when refresh is unavailable or rejected.
- Added TTY narrative-view refresh triggers for meaningful run and plan evidence, including errors, completions, tool milestones, docs checkpoints, child-run discovery, task terminal states, and merge-repair milestones.
- Added quiet-threshold TTY refresh attempts for running runs/plans when no meaningful narrative event arrives for the configured quiet window.
- Added narrative refresh triggers for acceptance running/pass/fail transitions so test evidence can update the operator summary without requiring raw-log watching.
- Added plan narrative roll-up from child run narrative snapshots so plan agent rows can cite the child's latest operator summary before falling back to child run state.
- Added plan file-map roll-up from child narrative graphs so plan-level visuals can show cross-agent touched file evidence.
- Kept plan narrative footer controls visible even when the selected child run is not available yet, preserving the one-key path back to raw activity.
- Added focused run/plan TUI render coverage for narrative panes, citations, agent rows, and visual-map hints.
- Added focused plain/JSON narrative attach coverage, including deterministic non-TTY fallback behavior and the explicit chain unsupported response.
- Added acceptance proof/progress citations to run narrative projections so failed done criteria point at the immutable acceptance artifact instead of only generic run state.
- Added focused run TUI mode coverage for narrative/activity toggling, visual cycling, narrow-terminal footers, and completed-run docs staying separate from narrative attach.
- Added command-level narrative attach smokes for flight-backed run output, file/evidence visuals, plan child refs, two-child plan agent visuals, and completed-run docs separation.
- Added final narrative attach guards for stale provider-refresh fallbacks, attach help copy, provider-neutral examples, and visual-map privacy/no-color documentation.
- Added focused coverage for narrative schemas, malformed snapshot tolerance, redaction, claim validation, graph validation, provider refresh validation, cadence/budget decisions, deterministic fallback, and plain map rendering.

## Self-Improvement Loop (alpha) - 2026-05-26

- Added file-backed learning state under `DEADRECKON_HOME/learning/` for episodes, deterministic signals, provider-backed insights, proposals, redacted bundles, candidates, evals, PR dry-run/open events, and local policy.
- Added `deadreckon learn index`, `deadreckon learn report`, required-reflection `deadreckon learn propose`, and redacted `learn export`/`learn import-bundle` so proposal creation uses a provider only after deterministic redacted evidence exists.
- Added `deadreckon improve self <proposal-id|goal-file>` preview, isolated-worktree candidate execution, evidence scoring, high-risk path classification, PR dry-run body generation, diff redaction checks, and a fake-testable live PR adapter gated behind explicit `--open-pr`.
- Added focused core and CLI coverage for learning paths, schema versioning, episode idempotency, bundle redaction/hash checks, signal extraction, proposal reflection validation, PR risk gating, learning CLI output, public-surface stability, PR dry-run, fake PR adapter behavior, and self-improve preview.

## Provider flight recorder and checkpoint rewind (alpha) - 2026-05-25

- Added durable `flight-manifest.json`, `flight-events.jsonl`, `checkpoints/<id>/`, and `rewind-events.jsonl` files for CLI-backed provider runs, with normalized provider-native events and delta checkpoints.
- Wrapped CLI provider execution in a polling flight recorder sidecar that ingests descriptor logs, watches working-tree changes, captures tool/quiet/exit checkpoints, and marks rerun sessions as superseded.
- Added `deadreckon show <run-id> --flight`, `deadreckon show <run-id> --file <path>`, and preview-first `deadreckon rewind` target resolution with hash-guarded `--apply`.
- Routed attach/TUI provider activity through flight events while keeping descriptor provider-log lines as the live fallback during long CLI subprocesses.

## Provider and done-criteria setup unification (alpha) - 2026-05-24

- Added a shared runtime setup resolver for provider roles and done-criteria sources so `init`, `config provider`, `run`, `extend`, `resume`, `orchestrate`, and doc polish use the same source labels, unknown-provider refusals, credential/install hints, and preview vocabulary.
- Switched run/orchestrate previews from `gate` to user-facing `done criteria` rows while preserving `.deadreckon/acceptance.yaml` as the technical file name and signed `dr-gate` as the enforcement mechanism.
- Updated `--acceptance` help text to describe done-criteria files, kept hidden `acceptance` compatibility wording advanced, and added focused coverage for unknown provider refusal plus run/orchestrate done-criteria preview parity.

## Descriptor import hardening (alpha) - 2026-05-20

- Reworked `deadreckon import` around descriptor-backed provider transcript discovery, concrete session selection, import manifests, and normalized trace/provenance events while preserving Cursor SQLite import.

## Implementation notes (alpha) - 2026-05-18

- Added root `implementation-notes.html` seeding for new runs, with required Design decisions, Deviations, Tradeoffs, and Open questions sections.
- Updated the default run prompt and CLI sub-agent prompt to frame work as `Implement the SPEC` and require the live notes file to stay current while files change.
- Made `RUN-DECISIONS.md` the converged implementation decision ledger by rendering the same four notes sections plus a separate evidence-filtered multi-alternative decision details section.
- Added done-time freshness checks so JSON-action providers and CLI sub-agents cannot complete after documentable implementation changes until `implementation-notes.html` is current.
- Updated `narrator-decisions` and split polish merging so implementation notes can feed the four interpretation sections without turning every note into a multi-alternative decision.
- Pointed lifecycle/doc hints toward `deadreckon doc <run-id> --kind decisions` as the primary inspection path for implementation interpretation.

## Orchestration live UX (alpha) - 2026-05-18

- Added shared orchestration role and dependency summaries across plan creation, orchestrate preflight/start, fork completion, plan attach summaries, and merge completion.
- Added provider role tables with route/model/source/notes rows for planner, default child, child overrides, coder, reviewer, and merge repair roles.
- Added explicit parallelism/dependency summaries that show which children start now, which wait, and which tasks unblock downstream work.
- Replaced terse merge repair plan summaries with structured repair detail covering mode, attempts, provider, conflict paths, sidecar paths, repair run status, latest repair event, and next action.
- Moved plan attach onto a `PlanEventBus` feed that replays `plan-events.jsonl`, tolerates partial/malformed event rows, emits plan snapshots, and multiplexes child and repair run events into the plan activity stream.
- Standardized plan attach footer grammar around detach, focus, child-run entry, back navigation, and `try:` lines.

## Coherence closure (alpha) - 2026-05-17

- Aligned top-level `attach` and `kill` id handling so run, chain, and plan ids all resolve through the normal lifecycle commands, with shared `attaching to <kind> <prefix>` and `killed <kind> <prefix>` banner wording.
- Clarified help for `attach`, `kill`, and `show` to name run, chain, plan, and `plan-id:task-id` support where the commands already accept those ids.
- Aligned provider setup wording so `doctor`, `detect`, and `providers list` use the same `kind=cli|http|local-http|scripted|custom` tokens and normal help says provider route instead of descriptor.
- Added coherence coverage for the updated help, orchestration help vocabulary, top-level chain attach/kill dispatch, provider kind vocabulary, status key/value layout, shared stderr error rendering, raw ANSI ownership, visual identity helpers, and plan-child show help.
- Refreshed README/HOWTO examples to use canonical `run`, `--branch-name`, `--overwrite`, `--max-spend`, `--git-strategy`, `--all-scopes`, and `--escalate` wording.
- Added `docs/PLAN-NARRATIVE.md` for merged plans so orchestration has one plan-level reading path built from child summaries.
- Rendered top help and `help-all` from one command catalog, with tests that catch duplicate rows and catalog entries that drift away from clap commands.
- Clarified the `help-all` discovery policy so documented advanced commands are distinct from compatibility aliases kept inline on canonical rows.
- Standardized `--plain` help across run, orchestration, lifecycle, and inspection commands as "without TUI, spinner, or ANSI affordances."
- Standardized cross-project flag help on "all project scopes" while keeping provider `--all` scoped to provider inventory.
- Renamed visible update override help from `--force` to `--anyway`, keeping `--force` as a hidden alpha alias.
- Aligned branch target wording so `run` names worktree branches with `--branch-name`, `apply`/`finish` target branches with `--into`, and apply output says changes landed `into` the target branch.
- Scoped strategy vocabulary so `merge --strategy` means plan composition, `apply`/`finish --git-strategy` means git apply behavior, and chain help separates `--apply-mode` from per-step `--apply-strategy`.
- Added a `help-all` output/scripting policy and aligned help for `--yes`, `--no-confirm`, `--quiet`, `--plain`, `--json`, and `--no-hints`.
- Added a provider-role glossary to `help-all` and aligned orchestration/doc help around provider routes for planner, child, coder, reviewer, repair, and documentation roles.
- Clarified cleanup help so it names temporary run worktrees/branches as its target and explicitly excludes plan state, promoted library artifacts, and exported directories.
- Made plan merge/result output keep the plan primary, with result run and artifact library labeled as secondary implementation details.
- Moved the CLI style facade into `ui.rs` and added coherence coverage so status tone mapping and public style helpers have one source of truth.
- Added standard JSON envelope fields across representative machine-readable surfaces and exposed `plan --json` for scriptable plan creation.
- Split note, warning, paused, and failed/killed style intents, and routed extended-run terminal outcomes through status tones.
- Rendered run, extend, and resume start summaries through the shared key/value block instead of bespoke provider/docs/state lines.
- Added a `help-all` spend-cap glossary for run, per-child, aggregate chain, and doc polish caps.
- Closed the user-facing matrix as an alpha record and moved larger output-layout, orchestration, provider/done-criteria, and snapshot work to V1 candidates.
- Made integration-test temp roots worktree-relative so coherence verification can run from a detached worktree.

## Semantic merge repair (alpha) - 2026-05-16

- Changed orchestration merge to default to DAG-aware composition, so descendant child artifacts can supersede ancestor file edits without a manual `prefer-child` retry.
- Added automatic bounded merge repair for true parallel conflicts: `merge` writes conflict/request/plan/run sidecars under `merge-proofs/`, invokes a repair provider by default, and can prefer a child file, synthesize conflict paths, or run a normal repair child from `merge-working`.
- Added repair controls for advanced/debug flows: `--no-repair`, `--repair-provider`, `--repair-mode auto|prefer|synthesize|child`, `--repair-attempts`, and `--strategy fail-on-conflict|dag-aware|prefer-child`.
- Added plan events for repair planning, repair start, repair child discovery, repaired merges, and repair failure; `show --why-failed`, plain plan summaries, and `history grep --plan` surface the new repair evidence.
- Updated `orchestrate` started/preflight output to say merge repair is automatic and to carry repair through the one-command flow, with `orchestrate ... --no-repair` kept as a debug-only raw conflict path.
- Added orchestration integration coverage for conflict bundles, repair requests, DAG merge precedence, planner prefer/synthesize/child repair, refusal validation, and headless `orchestrate --yes` auto-repair.
- Updated `docs/AS-BUILT-ARCHITECTURE.md` with the semantic merge repair model and sidecar layout.

## Plan observability (alpha) - 2026-05-15

- Added `plan-events.jsonl` as the orchestration-level event timeline for plan, task, child discovery, merge, failure, completion, and kill lifecycle edges.
- Added plan-event surfacing to `attach <plan-id>`, plain plan summaries, `history grep --plan`, and `show <plan-id> --why-failed`.
- Added plan attach drill-down/back context so a user can enter a selected child run's normal attach view and return to the parent plan/task.
- Hardened plan kill bookkeeping so discovered child run ids are preserved even if a child reaches a terminal state before the kill sweep inspects it.
- Hardened plan attach and kill recovery for partial `plan-events.jsonl` lines, missing child run roots, explicit `b`/Backspace back navigation, terminal failed-plan events, and sidecar-recovered child run ids.
- Hardened full-plan planning so build goals ask for implementation/verification child slices instead of research-only packets, and multiplayer/live/networked goals preview network capability correctly.
- Improved interactive `orchestrate` setup with goal-based mode and child-count recommendations, configured-provider guidance, optional child provider overrides, preflight warnings for research-only build plans, and a run-like started banner with attach/show/plan paths.
- Updated `docs/AS-BUILT-ARCHITECTURE.md` with `§32 Plan Observability` and amended `§22`/`§30` to reflect the file-backed plan event stream and remaining broadcast-bus limit.

## Distribution & self-update (alpha) - 2026-05-15

- Added install receipt and update-check cache files under `~/.deadreckon/` with channel detection for npm, Homebrew, shell, cargo, and source installs.
- Added `deadreckon update --check` plus npm/Homebrew/cargo/source channel routing; source installs refuse with a `try: cargo install --path crates/deadreckon` hint.
- Added shell-channel update backups and in-place swap plumbing through axoupdater, with deterministic backup/failure tests and pruning to the latest three backups.
- Added the cached startup stale-version hint, disabled for non-TTYs, source installs, `doctor`, `update`, and `DEADRECKON_UPDATE_CHECK=0`.
- Added cargo-dist release scaffolding for five OS/arch targets, shell/PowerShell installers, glibc 2.28 Linux metadata, and a push-time `dist plan` workflow check.
- Added guarded macOS Developer ID codesign/notarization steps and public release setup docs for the required Apple secrets.
- Added the npm wrapper package, five per-platform optional dependency templates, no-network receipt postinstall, and npm publish workflow wiring.
- Added Homebrew tap publishing for `gdc/homebrew-tap`, including a formula patch that writes the brew install receipt.
- Added first-run update receipt persistence plus shell-update previews, non-TTY `--yes` enforcement, and post-update doctor hints.
- Updated the as-built architecture docs with the distribution/self-update model and remaining operator release steps.

## Overnight UX (alpha) - 2026-05-14

- Added a shared `ui_card` renderer for run preview, run exit summaries, and completed attach footers with `--plain` / `NO_COLOR` behavior.
- Kept read-only inspection surfaces (`list`, `show`, and `status`) as quieter table/report output so they do not repeat the same run metadata inside card wrappers.
- Added `run --prevent-sleep <auto|on|off>` with macOS `caffeinate`, Linux `systemd-inhibit` re-exec/ready-file handling, run-local sleep metadata, and doctor sleep checks.
- Hardened production git invocations behind `deadreckon-core::git` with `GIT_TERMINAL_PROMPT=0` and commit-family GPG signing disabled.
- Added `spend_summary` replay so subscription or estimated turns render approximate spend with `~` without changing the numeric total.

## Orchestration prompt polish (alpha) - 2026-05-14

- Mined Claude Code's coordinator guidance into deadreckon worker specs: self-contained briefs, no sibling transcript peeking, concrete dependency summaries, correction vs fresh-review guidance, and skeptical reviewer posture.
- Planner prompts now ask for execution-order child DAGs with enough context for each child to run without the parent conversation.
- Plan children now run with `--no-docs`; plan-level summaries remain responsible for orchestration docs, avoiding accidental provider-backed narrator work in child runs.
- The coordinator now records each child run id under `plans/<plan-id>/launch/<task-id>/run-id`, so plan kill can map live child PIDs back to run state before marking children killed.
- Added/kept exact orchestration depth coverage for review-mode extension, child PID snapshots, kill cascade, prompt hygiene, and plan lifecycle friendliness.

## Coherence pass (alpha) - 2026-05-14

- Added one glossary for status words; `running` replaces `executing` in user-visible run and phase surfaces while stored enum variants stay unchanged.
- Added one style module and prompt builder; raw ANSI escapes now live in `ui.rs`, and every confirmation prompt uses the same `? question [Y/n]: ` or `? question [y/N]: ` shape.
- Added one key/value block for run and plan summaries, with lowercase keys and aligned colons.
- Standardized alpha flag names with hidden aliases: `--escalate`, `--overwrite`, `--anyway`, `--all-scopes`, `--global`, `--branch-name`, `--into`, `--max-spend`, and `--git-strategy`.
- Preserved the cyan `deadreckoning` banner, course strip, magenta IDs, spend gauge colors, and chain glyphs, with applied steps now using `◉`.
- Aligned attach and kill banners across runs, chains, and plans.
- Aligned `show --why-failed` and `chain show --why-failed` through one failure-summary layout, and added JSON output for list/status/show/doctor/provider/library inspection surfaces.
- Made `export` the visible copy-out word in help, completion prompts, docs, and refusal text while keeping `materialize` as an alpha compatibility alias/internal marker.

## Orchestration milestone (alpha) — 2026-05-13

- Renamed the multi-child orchestration mode from `split` to `full-plan`, added `deadreckon orchestrate review` and `deadreckon orchestrate full-plan` mode subcommands, and require `--yes` after the preflight in headless execution.
- Added file-backed orchestration plans with task DAG validation, provider roles, worker specs, coordinator messages, child summaries, and plan child markers without changing `PipelineState`.
- Added `deadreckon plan`, `fork`, `merge`, and review-mode `orchestrate` so a common coder -> reviewer -> merge flow can complete end to end.
- Added explicit planner/default-child/per-child/coder/reviewer provider resolution and persisted overrides into `plan.json`.
- Added merge conflict detection with `--strategy prefer-child --prefer-child <idx>` and promoted merge artifacts with `deadreckon-plan-manifest.json`.
- Added plan-aware `attach`, `show`, and `kill` so plan IDs participate in the normal lifecycle, including a basic multi-pane plan TUI with child drill-in.
- Added `deadreckon history grep <pattern>` for plan-aware trace/provenance search and `deadreckon show <id> --why-failed` for run or plan failure summaries.
- Review-mode orchestration now launches the reviewer lane as an `extend` of the coder run, preserving parent context and `extended_from_parent` trace lineage.
- Independent full-plan children now start as ready batches, with coordinator PID snapshots for every live child in the batch.
- Plan attach now surfaces child turn/status, spend or token accounting, latest trace activity, acceptance/gate state, capability preview, and final merged gate status in both the TUI and non-TTY summary.
- Headless orchestration flags now apply consistently: `run --plain --quiet` is accepted, `run --quiet` emits no success stdout, `attach --plain` bypasses the TUI, and `plan`/`fork`/`merge` preserve plain output.
- Added provider-backed planning depth coverage: planner prompts are asserted read-only, `--n` outside `2..=6` refuses before saving, one-task provider decompositions are rejected, and explicit planner/default-child/per-child providers are persisted.
- Coordinator launches now refresh each child worker spec with completed dependency summaries, so dependent child prompts include concrete predecessor context instead of only a plan-time dependency id.
- Merge manifests now include an explicit task graph, child summary paths, provider roles, and coordinator message counts for audit without replaying child transcripts.
- Added `show --why-failed` depth coverage for completed runs, failed run RCA traces, and plan blocker messages.
- Added P10 friendliness coverage for `try:` footers, quiet/plain headless output, review-mode provider hints, and plan ready/blocked task counts.
- Verified with focused orchestration tests plus core plan round-trips, clippy on the orchestration target, and `cargo fmt --check`; a broadcast-backed plan event stream remains a future slice.

## Copilot and Pi providers (alpha) - 2026-05-13

- Added built-in descriptor-backed `cli:copilot` and `cli:pi` providers with subscription auth, detection/install hints, model flags, sandbox read/write roots, and generic CLI routing coverage.
- Added Copilot session-state and Pi session JSONL TUI ingest, including cwd matching, tool/result/thinking rows, and context token telemetry without rewriting provider-owned logs.
- Kept verification focused on provider registry, CLI routing, detect/list UX, provider JSONL parsing, fmt, and crate-local clippy; the long full-suite commands remain out of this goal's default loop.

## Provider CLI ingest (alpha) — 2026-05-13

- Added optional descriptor `[ingest]` metadata and backfilled Codex/Claude Code so TUI provider activity is resolved by registry descriptors instead of provider-id conditionals.
- Added canonical tool-category normalization and schema-keyed provider activity parsers for Codex, Claude Code, Gemini JSON/JSONL, and OpenCode file-mode logs.
- Added descriptor-backed generic CLI launch through `exec_template`, including model flags, prompt delimiters, sandbox placeholders, descriptor sandbox writes, and subscription wall-time spend.
- Added built-in `cli:gemini` and `cli:opencode` descriptors with detection/install hints, `providers list` coverage, registry-order `init --no-confirm`, and stable `cli:` output filenames.
- Kept verification focused on provider/CLI/TUI surfaces; `make verify`, release builds, smoke, stress, and full-workspace tests remain out of this goal's default loop.

## Provider registry (alpha) — 2026-05-13

- P1: Added descriptor TOML, `ProviderDescriptor`, `ProviderRegistry`, override loading from `providers.d`, and shell-like custom command parsing; existing built-in providers now have compiled-in descriptors.
- P2: Existing provider defaults now come from descriptor TOML, `ProviderKind` supports generic descriptor IDs, and CLI sandbox write allowlists are descriptor-backed while preserving current adapter behavior.
- P3: Added descriptor-backed provider probes and `deadreckon detect [<id>]`, including PATH/version checks, credential checks, JSON output, and install `try:` hints.
- P4: Added `deadreckon providers list` with configured-only/default and `--all`, `--models`, and `--full` views backed by the registry.

## Workspace hygiene (alpha) — 2026-05-12

- P1: Captured smoke and public-surface baselines, added invariant tests, and made `make smoke` run fresh/non-interactive for deterministic verification.
- P2: Added warn-only `[workspace.lints]`, `clippy.toml`, per-crate lint inheritance, and a clippy warning snapshot for the P3 cleanup pass.
- P3: Promoted core workspace clippy rules to deny, removed the temporary warning snapshot, and added deny-level lint tests plus a `-D warnings` clippy guard.
- P4: Added `rustfmt.toml` and guard tests for the dedicated format commit and clean `cargo fmt --check`.
- P5: Tuned release/dev profiles and captured a release binary size baseline with slack guard.
- P6: Routed internal crates through `[workspace.dependencies]` and guarded the internal cargo metadata DAG.
- P7: Added library-crate print refusal while keeping the binary crate exempt.
- P8: Added registry-shape guard tests for `deadreckon-core`'s library root; no public surface changed.
- P9: Regrouped provider/runtime/sandbox library roots into registry shape and preserved the public re-export set.
- P10: Added exhaustive retryable/fatal taxonomy methods to core, provider, and sandbox errors while keeping runtime errors on the core taxonomy.
- P11: Updated `docs/AS-BUILT-ARCHITECTURE.md` with §29 Workspace Hygiene and amended §22 to mark the hygiene rider as structural, not a prior thin-item closure.

## Doc depth (alpha) — 2026-05-12

- Per-turn capture extended: full provider response (50 KB cap), per-file diff samples with largest-hunk excerpts, and bash stdout/stderr (10 KB cap each).
- Turn-end documentation is now an explicit run event for both CLI sub-agent turns and JSON-action provider turns; `_incremental.jsonl` is checkpointed before completion polish/acceptance/promotion.
- Templated narrative no longer truncates the title at 40 chars; per-turn outcomes no longer cut at 200 chars; phase prose synthesizes per-turn summaries instead of "deadreckon progressed through turn N".
- Component-table inference uses path rules (`crates/`, `skills/`, `docs/`, manifests, tests, routes, migrations, CI); generic "Project files" rows are not emitted.
- Process topology ASCII is generated only when at least three top-level directories changed.
- Provider-backed doc polish now defaults to four repo skills: `narrator-overview`, `narrator-phases`, `narrator-as-built`, and `narrator-decisions`, each with a 16K output budget and per-subcall status/cost recorded in `polish.json` schema v2.
- `deadreckon run` and `deadreckon doc --polish` expose doc-provider selection (`--doc-provider`) with flag/config/subscription/run-provider resolution, preview output, preflight `--budget-cap` refusal, and post-polish subcall summaries.

## Lifecycle help polish — 2026-05-12

- Added `deadreckon finish` / `done` as a completion intent command that routes completed worktree runs to `apply`, fresh/copy runs to `export`, and in-place runs to review guidance.
- Added lifecycle-oriented `--help` text to every top-level verb, including real `chain` subcommand examples and focused `deadreckon chain help <topic>` output.
- Expanded friendly aliases across the lifecycle: `setup`, `settings`, `check`, `runs`, `artifacts`, `keep`, `clean`, `follow-up`, `docs`, `watch`, `stop`, `continue`, `restore`, and `inspect`.

## Autonomous chaining (alpha) — 2026-05-11

- Added the chain data model foundation: `chain.json`, `chain-events.jsonl`, chain path helpers, chain lock task-key convention, and `RunPromoted` events after promotion.
- Added the first user-facing chain flow: `chain "..."`, `--from-file`, `--from-stdin`, `--draft`, preview/confirm, `chain run`, `chain list/status/show/attach`, and a foreground conductor that runs sequential steps through existing run/apply paths.
- Added provider-backed `chain plan` / `chain expand`, including JSON-array validation, duplicate/single-step refusal, and planner spend recording under the chain directory.
- Added chain policy depth: branch-policy stack/base behavior, aggregate per-step spend allocation, and chain hooks for `pre-step`, `post-step`, `on-promote`, and `on-chain-end` with hook events.
- Added chain-step context markers to inner runs and surfaced them in single-run `show` / non-TTY attach summaries.
- Added lifecycle depth for `latest`/`last`, `resume`, `extend`, `redo`, `undo`, pause refusals, and cascade `chain kill` that terminates the live inner run and conductor.
- Added the multi-step `chain attach` TUI with policy header, step timeline, chain activity stream, pause/kill/redo/extend controls, and single-run `attach` chain drill-out via `c`.
- Added policy gate coverage for allowlist refusal, manual apply pause, merge branch policy, on-fail stop/skip, and configurable circuit breaker thresholds.
- Completed the rider depth-test matrix under exact test names and tightened resume-after-manual-pause, quiet auto-apply, bounded undo, TTY auto-attach, preview diff, and aggregate wall-clock behavior.
- Updated `docs/AS-BUILT-ARCHITECTURE.md` with §28 Chains and refreshed §17/§22 chain accounting.

## Hardening v2 (alpha) — 2026-05-11

- Added `docs/AUDIT-2026-05-11.md` mapping the original 25 unmet needs to current evidence and the P2-P10 closure plan.
- Replaced TUI polling-only attach with event-backed attach: same-process broadcast plus cross-process `events.jsonl` replay.
- Hardened cross-process cancellation with durable cancel markers, provider abort coverage, and kill-storm tests.
- Hardened partial-trace resume and `resume --from-turn` so trace, spend, and snapshot tails are truncated together.
- Added durable `sandbox.toml` per run, per-tool sandbox policy, and refusal provenance for disallowed filesystem/network actions.
- Expanded `acceptance.yaml` support with required/optional checks, file/content/build/shell checks, and signed per-check proof results.
- Made `doctor` more actionable across providers, sandboxes, OS, permissions, disk, and opt-in provider pings.
- Added `deadreckon library list|search|show` for promoted artifacts, including goal/date filters and promoted-doc grep.
- Hardened Claude Code/Codex/Cursor import normalization with source metadata, deterministic imported run IDs, stable Cursor ordering, malformed JSONL errors, and committed show-output golden tests.
- Polished CLI help/status/completion UX, including command groups, run health/library/disk status blocks, and `DEADRECKON_HINTS=0`.
- Updated `docs/AS-BUILT-ARCHITECTURE.md` and `docs/AUDIT-2026-05-11.md` with the Hardening v2 closure evidence.

## UX consolidation — 2026-05-11

- Added an in-TUI Markdown docs view for completed runs. Press `d` in `attach` to toggle a styled `RUN-NARRATIVE.md` rendering instead of dropping to plain terminal output.
- Made `deadreckon apply` idempotent when a run branch has already landed on the target branch; it now reports `already applied` and can still perform `--cleanup` instead of failing on an empty commit.
- Added explicit provider/model affordances: `run --model`, `extend --model`, model-aware run previews, and `deadreckon config provider|model` shortcuts.
- Made `deadreckon list` project-scoped by default, with `--all` for global history and `--full` for script-friendly full values.
- Added `latest` / `last` run-id aliases for user-facing run commands, resolved to the latest run in the current project.
- Added `deadreckon status` with `next` as an alias; running `deadreckon` with no subcommand now shows the current project's latest run and next action.
- Added `deadreckon cleanup` with `prune` as an alias for cleaned, stale, or completed worktree cleanup.
- Added friendlier command aliases: `export` for `materialize` and `discard` for `abandon`.
- Improved root and subcommand help text, terminal output formatting, TUI layout, completion action footer, and scoped workflow hints.

## Apply/list usability — 2026-05-11

- Made run-id arguments accept unique prefixes so compact `deadreckon list` IDs can be reused directly.
- Made `deadreckon list` compact by default with `--full` for scripts and exact full values.
- Added `deadreckon apply --autostash` for dirty checkouts and `--cleanup` to remove the run worktree/branch after a successful apply.

## Self-documenting runs (alpha) — 2026-05-11

- Added run-start doc scaffolding under `working/.deadreckon/docs/` with stoa-shaped `RUN-NARRATIVE.md`, `RUN-AS-BUILT.md`, `RUN-DECISIONS.md`, `_incremental.jsonl`, and `polish.json`.
- Added deterministic per-turn narrative chunks, phase coalescing, decision detection, trace/snapshot citations, worktree commit SHA capture, and optional `AS-BUILT-DELTA.md`.
- Added the `run-narrator` skill, provider-backed end-of-run polish with JSON retry, SHA-256 idempotency, diff coverage retry, and nonfatal polish failure statuses.
- Added `deadreckon doc <run-id>`, `list` DOCS status, doc-aware completion actions, extend-parent narrative updates, and generated `apply` commit bodies from run docs.
- Added 48 rider-named depth tests in `crates/deadreckon/tests/self_documenting.rs`.

## Codebase modes (alpha) — 2026-05-11

- P1: Added codebase mode records, fresh-mode metadata, and deterministic mode resolution plumbing without changing `PipelineState`.
- Added codebase-aware `run` defaults: clean git repos now run in an isolated `git worktree` on a `dr/...` branch, while the old empty-working-dir behavior remains behind `--fresh`.
- Added explicit copy (`--from`), worktree (`--worktree`, `--base`, `--branch`, `--allow-dirty`), and in-place (`--in-place --i-know-its-a-lot`) modes with single-screen preview and `--preview` / `--yes` scripting paths.
- Added worktree lifecycle verbs: `deadreckon apply <run-id>` with squash/merge/cherry-pick strategies and `deadreckon abandon <run-id>` with branch/worktree cleanup.
- Integrated codebase modes into `list`, `show`, `materialize`, `extend`, `undo`, run completion prompts, and TUI completion actions. Worktree runs now hint apply/abandon; copy/fresh runs continue to hint materialize/extend.
- Added worktree-aware `extend`: child worktree runs branch from the parent `dr/...` branch and record `parent_branch` in `codebase.json`; in-place parents refuse with a `run --in-place` hint.
- Added depth coverage for every rider-named codebase test, including dirty/refusal preflight, preview and non-git prompt UX, worktree/copy/in-place modes, apply conflict handling, abandon force cleanup, lifecycle hints, and extend integration.

## Lifecycle ergonomics

Phase commits: `4481617`, `556897d`, `91ab9a6`.

- Added `deadreckon materialize <run-id> [--dest <path>] [--force] [--include-manifest]` to copy completed library artifacts to user-owned paths with `.deadreckon/parent.json` provenance and library `.materialized-to` reverse markers.
- Added `deadreckon extend <run-id> "<new-goal>"` to create a fresh run from a completed parent artifact, seed the working tree, prepend a parent summary into `history.json`, and record lineage through marker files plus a synthetic trace.
- Added lifecycle hints after completed `run`/`attach`, `--no-hints` suppression, `list` materialization status, and `show` parent-lineage output.
- Kept `PipelineState` unchanged; lifecycle lineage lives in marker files.

## 0.1.0 - Robustness Milestone (alpha)

Implementation commit: `cec49f3`.

- Hardened the run loop with broadcast/file-backed events, per-turn timers, cancellation tokens, wall-clock CLI spend accounting, partial-trace resume, and `resume --from-turn`.
- Hardened sandbox execution with generated Seatbelt/bwrap policy inputs, tmp `$HOME`, network denial, persisted profiles, and adversarial path/network tests.
- Hardened the original legacy-v1 acceptance path by moving `dr-gate` to
  `acceptance.yaml`, signing markers with a run-local nonce, and refusing forged
  self-attestation. Strict durable Jobs now use HMAC-SHA-256 and the contained
  two-phase gate described above.
- Hardened import normalization for Claude Code, Codex, and Cursor histories into deadreckon traces/provenance.
- Hardened multi-run coordination with scope-qualified lock files and same-scope refusal tests.
- Hardened library promotion with post-gate atomic move, manifest writing, and crash recovery.

Still thin: provider pings in `doctor` are intentionally conservative unless explicitly enabled, and the TUI uses durable event replay for cross-process attach because Tokio broadcast is in-process.
